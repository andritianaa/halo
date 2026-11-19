//! Files dragged onto the notch: it opens into two drop zones — **Vault** (encrypt
//! into the unlocked vault) and **Shelf** (keep shortcuts for later) — and the
//! dashboard shows the shelf while it holds files.
//!
//! Halo never moves or deletes the dropped originals.

use std::path::PathBuf;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use halo_modules::shelf::{file_icon, file_name};
use halo_ui::hsla;
use halo_ui::tokens::{self, Palette};

use super::Overlay;
use crate::state::Section;
use crate::ui;

/// Where dropped files go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Zone {
    Vault,
    Shelf,
}

impl Zone {
    /// Same zone as reported by the drop helper.
    pub fn matches(self, z: halo_core::dropipc::DropZone) -> bool {
        matches!(
            (self, z),
            (Zone::Vault, halo_core::dropipc::DropZone::Vault) | (Zone::Shelf, halo_core::dropipc::DropZone::Shelf)
        )
    }
}

/// What the vault zone can do right now.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum VaultState {
    /// Module not enabled (opt-in).
    Off,
    /// Enabled, but no vault created yet.
    NoVault,
    Locked,
    Unlocked(String),
}


impl Overlay {
    /// "{n} files …" with a singular form (`<key>.one`) for one file.
    fn count_msg(&self, key: &str, n: usize) -> String {
        let s = self.shared.borrow();
        if n == 1 { s.tf(&format!("{key}.one"), &[("n", "1")]) } else { s.tf(key, &[("n", &n.to_string())]) }
    }

    pub(super) fn vault_state(&self) -> VaultState {
        let s = self.shared.borrow();
        if !s.module_running("vault") {
            return VaultState::Off;
        }
        let v = &s.secure.vault;
        match v.unlocked() {
            Some(name) => VaultState::Unlocked(name),
            None if v.vaults().is_empty() => VaultState::NoVault,
            None => VaultState::Locked,
        }
    }

    /// Files dropped on a zone.
    pub(super) fn drop_files(&mut self, zone: Zone, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let n = paths.len();
        match zone {
            Zone::Shelf => {
                let list: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                self.shared.borrow_mut().update_config(|c| {
                    c.settings.shelf_add(list);
                });
                let msg = self.count_msg("notch.toast.shelf", n);
                self.notify_toast("folder-down", msg);
            }
            Zone::Vault => {
                let (files, skipped): (Vec<PathBuf>, Vec<PathBuf>) = paths.into_iter().partition(|p| p.is_file());
                if files.is_empty() {
                    let msg = self.shared.borrow().t("notch.toast.folders");
                    self.notify_toast("folder-x", msg);
                    return;
                }
                match self.vault_state() {
                    VaultState::Unlocked(_) => {
                        let vault = self.shared.borrow().secure.vault.clone();
                        let task = cx.background_executor().spawn(async move {
                            files.iter().filter(|f| vault.add_file(f).is_ok()).count()
                        });
                        cx.spawn(async move |this, cx| {
                            let added = task.await;
                            let _ = this.update(cx, |this, cx| {
                                let mut msg = this.count_msg("notch.toast.vault", added);
                                if !skipped.is_empty() {
                                    msg = format!("{msg} · {}", this.shared.borrow().t("notch.toast.folders"));
                                }
                                this.notify_toast("lock", msg);
                                cx.notify();
                            });
                        })
                        .detach();
                    }
                    state => {
                        // Not ready: queue the files and take the user to the vault page.
                        let count = files.len();
                        {
                            let mut s = self.shared.borrow_mut();
                            if state != VaultState::Off {
                                s.pending_vault.extend(files);
                            }
                            s.open_hub(Section::Vault);
                        }
                        let key = if state == VaultState::Off { "notch.toast.vault_off" } else { "notch.toast.vault_pending" };
                        let msg = self.count_msg(key, count);
                        self.notify_toast("lock", msg);
                    }
                }
            }
        }
        cx.notify();
    }

    fn drop_zone(&self, zone: Zone, p: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let s = self.shared.borrow();
        let (id, icon, title, hint) = match zone {
            Zone::Shelf => ("drop-shelf", "folder-down", s.t("notch.drop.shelf"), s.t("notch.drop.shelf.hint")),
            Zone::Vault => {
                let hint = match self.vault_state() {
                    VaultState::Unlocked(name) => s.tf("notch.drop.vault.ready", &[("v", &name)]),
                    VaultState::Locked => s.t("notch.drop.vault.locked"),
                    VaultState::NoVault => s.t("notch.drop.vault.none"),
                    VaultState::Off => s.t("notch.drop.vault.off"),
                };
                ("drop-vault", "lock", s.t("notch.drop.vault"), hint)
            }
        };
        drop(s);
        let accent = hsla(tokens::ACCENT);
        let track = hsla(p.overlay_track);
        div()
            .id(id)
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(8.0))
            .px(px(20.0))
            .rounded(px(20.0))
            .border_1()
            .border_color(hsla(p.overlay_line))
            // Hover reported by the drop helper (elevated Halo), or by GPUI directly.
            .when(self.ext_zone.is_some_and(|z| zone.matches(z)), |d| d.border_color(accent).bg(track))
            .drag_over::<ExternalPaths>(move |st, _, _, _| st.border_color(accent).bg(track))
            .child(
                div()
                    .size(px(44.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(hsla(p.overlay_tile))
                    .child(ui::icon(icon, 20.0, p.overlay_ink)),
            )
            .child(div().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(hsla(p.overlay_ink)).child(title))
            .child(div().text_size(px(12.0)).text_center().text_color(hsla(p.overlay_ink_2)).child(hint))
            .on_drop(cx.listener(move |this, paths: &ExternalPaths, _, cx| {
                this.drop_files(zone, paths.paths().to_vec(), cx);
            }))
    }

    /// Expanded notch while a file is dragged over it.
    pub(super) fn drop_zones(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::dark();
        let vertical = self.anchor().orientation() == halo_core::overlay::Orientation::Vertical;
        div()
            .size_full()
            .flex()
            .when(vertical, |d| d.flex_col())
            .when(!vertical, |d| d.flex_row())
            .gap(px(12.0))
            .px(px(16.0))
            .pt(px(10.0))
            .pb(px(16.0))
            .child(self.drop_zone(Zone::Vault, &p, cx))
            .child(self.drop_zone(Zone::Shelf, &p, cx))
            .into_any_element()
    }

    pub(super) fn has_shelf(&self) -> bool {
        !self.shared.borrow().config.settings.shelf.is_empty()
    }

    /// Shelf section of the dashboard: dropped files, click to open.
    pub(super) fn shelf_section(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let (items, title, clear, reveal, remove) = {
            let s = self.shared.borrow();
            (
                s.config.settings.shelf.clone(),
                s.t("shelf.title"),
                s.t("shelf.clear"),
                s.t("shelf.reveal"),
                s.t("shelf.remove"),
            )
        };
        let count = items.len();
        let files = items.into_iter().take(6).enumerate().map(|(i, raw)| {
            let path = PathBuf::from(&raw);
            let missing = !path.exists();
            let (open_p, reveal_p, remove_raw) = (path.clone(), path.clone(), raw.clone());
            let group: SharedString = format!("shelf-{i}").into();
            div()
                .id(("shelf", i))
                .group(group.clone())
                .relative()
                .w(px(68.0))
                .flex_none()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(6.0))
                .cursor_pointer()
                .gap(px(4.0))
                .child(
                    div()
                        .size(px(40.0))
                        .rounded(px(10.0))
                        .bg(hsla(p.overlay_tile))
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(missing, |d| d.opacity(0.4))
                        .child(ui::icon(file_icon(&path), 18.0, p.overlay_ink)),
                )
                .child(
                    div()
                        .w_full()
                        .truncate()
                        .text_center()
                        .text_size(px(11.0))
                        .text_color(hsla(if missing { p.overlay_ink_2 } else { p.overlay_ink }))
                        .child(file_name(&path)),
                )
                // Hover actions: show in folder / remove.
                .child(
                    div()
                        .absolute()
                        .top(px(-4.0))
                        .right(px(4.0))
                        .flex()
                        .flex_row()
                        .gap(px(2.0))
                        .opacity(0.0)
                        .group_hover(group, |s| s.opacity(1.0))
                        .child(self.mini_button(("shelf-reveal", i), "folder-open", reveal.clone(), move |_, cx| {
                            cx.reveal_path(&reveal_p);
                        }, cx))
                        .child(self.mini_button(("shelf-remove", i), "x", remove.clone(), move |this, _| {
                            let r = remove_raw.clone();
                            this.shared.borrow_mut().update_config(|c| c.settings.shelf.retain(|x| *x != r));
                        }, cx)),
                )
                .on_click(move |_, _, _| {
                    if open_p.exists() {
                        // Through explorer.exe: the file opens as the user, not with Halo's admin rights.
                        crate::launcher::launch(&open_p);
                    }
                })
        })
        .collect::<Vec<_>>();
        div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .min_w(px(0.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(ui::icon("folder-down", 14.0, p.overlay_ink_2))
                    .child(div().text_size(px(12.0)).text_color(hsla(p.overlay_ink_2)).child(format!("{title} · {count}")))
                    .child(div().flex_1())
                    .child(self.mini_button(("shelf-clear", 0usize), "trash", clear, |this, _| {
                        this.shared.borrow_mut().update_config(|c| c.settings.shelf.clear());
                    }, cx)),
            )
            // Two rows of tiles; extra files stay listed in the config, newest first.
            .child(div().flex().flex_row().flex_wrap().gap_x(px(4.0)).gap_y(px(8.0)).children(files))
    }

    /// Small round icon button used on the shelf.
    fn mini_button(
        &self,
        id: impl Into<ElementId>,
        icon: &'static str,
        tip: String,
        on_click: impl Fn(&mut Self, &mut App) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = Palette::dark();
        div()
            .id(id.into())
            .size(px(22.0))
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(p.overlay_track))
            .hover(|s| s.bg(hsla(p.overlay_line)))
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
            .child(ui::icon(icon, 11.0, p.overlay_ink))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    on_click(this, cx);
                    cx.notify();
                }),
            )
    }
}
