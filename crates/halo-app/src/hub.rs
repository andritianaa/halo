//! The Halo hub: one window with a sidebar (General, Modules, then one panel per
//! module). gpui-kit components (Button, Switch) are themed by `themes/halo.json`;
//! custom surfaces use `halo_ui::tokens`.

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use halo_ui::hsla;
use halo_ui::tokens::{self, Palette};

use crate::panels;
use crate::state::{Section, Shared};
use crate::ui;

pub struct Hub {
    pub shared: Shared,
    pub section: Section,
    pub p2: crate::panels2::Phase2State,
    pub p3: crate::panels3::Phase3State,
    pub p4: crate::panels4::Phase4State,
    pub search: Entity<gpui_kit::component::input::InputState>,
    _search_sub: Subscription,
    /// Scroll position of the panel column (debug automation can set it).
    panel_scroll: ScrollHandle,
    /// Windows 11 Mica backdrop behind the window (Fluent); solid background otherwise.
    mica: bool,
    /// Dark/light window chrome last applied (follows the Halo theme).
    chrome_dark: Option<bool>,
}

pub fn open(cx: &mut App, shared: Shared) {
    let section = shared.borrow().hub_section;
    let existing = (shared.borrow().hub_window, shared.borrow().hub_view.clone());
    if let (Some(handle), Some(view)) = existing {
        let alive = view
            .update(cx, |hub, cx| {
                hub.section = section;
                cx.notify();
            })
            .is_ok();
        if alive && handle.update(cx, |_, window, _| window.activate_window()).is_ok() {
            return;
        }
    }
    let title = shared.borrow().t("settings.title");
    // Fluent: Mica backdrop on Windows 11.
    let mica = halo_platform::registry::supports_mica();
    let options = WindowOptions {
        window_background: if mica { WindowBackgroundAppearance::MicaBackdrop } else { WindowBackgroundAppearance::Opaque },
        window_bounds: Some(WindowBounds::centered(size(px(920.0), px(680.0)), cx)),
        focus: true,
        show: true,
        window_min_size: Some(size(px(720.0), px(480.0))),
        // Custom title bar (logo + themed window controls) drawn by gpui-kit; the title
        // still names the window in the taskbar / Alt+Tab.
        titlebar: Some(TitlebarOptions { title: Some(title.into()), ..gpui_kit::component::TitleBar::title_bar_options() }),
        ..gpui_kit::component::TitleBar::window_options()
    };
    let for_view = shared.clone();
    match gpui_kit::open_window(options, cx, move |window, cx| {
        cx.new(|cx| {
            // Live panels (monitors, history…) refresh once a second while open.
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            })
            .detach();
            let p2 = crate::panels2::Phase2State::new(window, cx);
            let p4 = crate::panels4::Phase4State::new(window, cx);
            let search = cx.new(|cx| gpui_kit::component::input::InputState::new(window, cx).placeholder(for_view.borrow().t("hub.search")));
            let _search_sub = cx.subscribe_in(&search, window, |_: &mut Hub, _, _: &gpui_kit::component::input::InputEvent, _, cx| cx.notify());
            Hub {
                shared: for_view.clone(),
                section,
                p2,
                p3: Default::default(),
                p4,
                search,
                _search_sub,
                panel_scroll: ScrollHandle::new(),
                mica,
                chrome_dark: None,
            }
        })
    }) {
        Ok((handle, view)) => {
            let mut s = shared.borrow_mut();
            s.hub_window = Some(handle);
            s.hub_view = Some(view.downgrade());
        }
        Err(err) => log::error!("hub window: {err:#}"),
    }
}

/// Sidebar entries grouped by category: (section, Lucide icon, i18n key, gating module, group key).
pub const NAV: &[(Section, &str, &str, Option<&str>, &str)] = &[
    (Section::General, "sliders-horizontal", "nav.general", None, "group.app"),
    (Section::Modules, "layout-dashboard", "nav.modules", None, "group.app"),
    (Section::Notifications, "bell", "module.notifications", None, "group.app"),
    (Section::Productivity, "timer", "nav.productivity", None, "group.productivity"),
    (Section::Notes, "notebook-pen", "module.notes", Some("notes"), "group.productivity"),
    (Section::Clipboard, "clipboard", "module.clipboard", None, "group.productivity"),
    (Section::Accent, "languages", "module.accent", Some("accent"), "group.productivity"),
    (Section::System, "activity", "module.system", Some("system"), "group.monitoring"),
    (Section::Resources, "cpu", "module.resources", Some("resources"), "group.monitoring"),
    (Section::Activity, "chart-pie", "module.activity", None, "group.monitoring"),
    (Section::Picker, "pipette", "module.picker", Some("picker"), "group.tools"),
    (Section::Images, "image", "module.images", Some("images"), "group.tools"),
    (Section::Spotlight, "mouse-pointer-2", "module.spotlight", Some("spotlight"), "group.tools"),
    (Section::Capture, "camera", "module.capture", None, "group.capture"),
    (Section::Music, "music", "module.music", Some("music"), "group.media"),
    (Section::WinTheme, "sun-moon", "module.wintheme", Some("wintheme"), "group.system"),
    (Section::Startup, "power", "module.autostart", None, "group.system"),
    (Section::Vault, "lock", "module.vault", None, "group.security"),
    (Section::Network, "globe", "module.netlimit", None, "group.security"),
    (Section::WifiKeys, "key-round", "module.wifikeys", None, "group.security"),
];

pub const GROUPS: &[&str] = &["group.app", "group.productivity", "group.monitoring", "group.tools", "group.capture", "group.media", "group.system", "group.security"];

impl Hub {
    fn nav_item(&self, p: &Palette, section: Section, icon: &str, key: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        let active = self.section == section;
        let text = self.shared.borrow().t(key);
        // Fluent NavigationViewItem: 36 px row, subtle (translucent) fills that work on
        // Mica, and a short accent pill on the left of the selected item.
        div()
            .id(SharedString::from(format!("nav-{section:?}")))
            .relative()
            .h(px(36.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(tokens::SPACE_3))
            .px(px(tokens::SPACE_3))
            .rounded(px(tokens::RADIUS_SM))
            .cursor_pointer()
            .text_size(px(tokens::TEXT_LABEL.size))
            .text_color(hsla(p.ink))
            .when(active, |d| {
                d.bg(hsla(p.ink.with_alpha_pct(7))).child(
                    div().absolute().left(px(0.0)).top(px(10.0)).w(px(3.0)).h(px(16.0)).rounded_full().bg(hsla(p.accent)),
                )
            })
            .when(!active, |d| d.hover(|s| s.bg(hsla(p.ink.with_alpha_pct(4)))))
            .child(ui::icon(icon, 16.0, p.ink))
            .child(text)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    this.section = section;
                    cx.notify();
                }),
            )
    }

    /// A gpui-kit button that updates the config.
    pub fn choice(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        selected: bool,
        f: impl Fn(&mut halo_core::config::Config) + 'static,
    ) -> Button {
        let shared = self.shared.clone();
        let b = Button::new(ElementId::Name(id.into())).label(label.into()).on_click(move |_, _, cx| {
            shared.borrow_mut().update_config(&f);
            cx.refresh_windows();
        });
        if selected { b.primary() } else { b.outline() }
    }

    /// A gpui-kit switch row bound to a config bool.
    pub fn switch_row(
        &self,
        p: &Palette,
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        hint: Option<String>,
        checked: bool,
        f: impl Fn(&mut halo_core::config::Config, bool) + 'static,
    ) -> Div {
        let shared = self.shared.clone();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(tokens::SPACE_4))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(div().text_size(px(tokens::TEXT_LABEL.size)).text_color(hsla(p.ink)).child(title.into()))
                    .children(hint.map(|h| ui::mono(p, &self.shared.borrow().typo.ui, h))),
            )
            .child(Switch::new(ElementId::Name(id.into())).checked(checked).on_click(move |v: &bool, _, cx| {
                let v = *v;
                shared.borrow_mut().update_config(|c| f(c, v));
                cx.refresh_windows();
            }))
    }
}

impl Render for Hub {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(y) = self.shared.borrow_mut().want_hub_scroll.take() {
            self.panel_scroll.set_offset(point(px(0.0), px(-y)));
        }
        let (p, typo) = {
            let s = self.shared.borrow();
            (s.palette(), s.typo.clone())
        };
        // Window chrome and Mica follow the Halo theme (not only the system setting).
        // Applied after the frame: no Win32 window calls inside a GPUI render.
        let dark = p.mode == halo_ui::tokens::Mode::Dark;
        if self.chrome_dark != Some(dark) {
            self.chrome_dark = Some(dark);
            if let Some(h) = crate::overlay::raw_hwnd(window) {
                cx.spawn(async move |_, _| halo_platform::set_dark_chrome(h, dark)).detach();
            }
        }
        // On Mica the window background stays transparent so the backdrop shows.
        let base = if self.mica { None } else { Some(hsla(p.bg)) };
        let visible: Vec<_> = NAV
            .iter()
            .filter(|(_, _, _, gate, _)| gate.is_none_or(|id| self.shared.borrow().module_running(id)))
            .copied()
            .collect();
        if !visible.iter().any(|(s, ..)| *s == self.section) {
            self.section = Section::General;
        }
        let query = self.search.read(cx).value().to_string();
        let mut nav: Vec<AnyElement> = Vec::new();
        if query.trim().is_empty() {
            for group in GROUPS {
                let items: Vec<_> = visible.iter().filter(|(.., g)| g == group).collect();
                if items.is_empty() {
                    continue;
                }
                // Fluent NavigationViewItemHeader: sentence case, Body Strong.
                nav.push(
                    div()
                        .px(px(tokens::SPACE_3))
                        .pt(px(tokens::SPACE_4))
                        .pb(px(tokens::SPACE_1))
                        .text_size(px(tokens::TEXT_MICRO.size))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(hsla(p.ink_2))
                        .child(self.shared.borrow().t(group))
                        .into_any_element(),
                );
                for (section, icon, key, ..) in items {
                    nav.push(self.nav_item(&p, *section, icon, key, cx).into_any_element());
                }
            }
        } else {
            let fr = self.shared.borrow().fr();
            let hits = crate::search::search(&query, fr);
            if hits.is_empty() {
                nav.push(ui::label(&p, self.shared.borrow().t("hub.no_results")).px(px(tokens::SPACE_3)).into_any_element());
            }
            for (i, hit) in hits.into_iter().take(12).enumerate() {
                let section = hit.section;
                nav.push(
                    div()
                        .id(("search-hit", i))
                        .flex()
                        .flex_col()
                        .px(px(tokens::SPACE_3))
                        .py(px(tokens::SPACE_2))
                        .rounded(px(tokens::RADIUS_SM))
                        .cursor_pointer()
                        .hover(|s| s.bg(hsla(p.surface_2)))
                        .child(div().text_size(px(tokens::TEXT_LABEL.size)).text_color(hsla(p.ink)).child(hit.label))
                        .child(div().text_size(px(11.0)).text_color(hsla(p.ink_3)).child(hit.place))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.section = section;
                            this.search.update(cx, |s, cx| s.set_value("", window, cx));
                            cx.notify();
                        }))
                        .into_any_element(),
                );
            }
        }
        let panel = panels::render(self, &p, window, cx);

        let title_bar = gpui_kit::component::TitleBar::new()
            // Fluent: the title bar sits on the same backdrop as the window (no separate band).
            .bg(base.unwrap_or(gpui_kit::transparent_black()))
            .border_color(gpui_kit::transparent_black())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(tokens::SPACE_2))
                    .child(svg().path("halo/mark.svg").size(px(18.0)).text_color(hsla(p.accent)))
                    .child(
                        div()
                            .font_family(SharedString::from(typo.display.clone()))
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(p.ink))
                            .child("Halo"),
                    ),
            );

        let body = div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_row()
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.modifiers.control && ev.keystroke.key == "k" {
                    this.search.update(cx, |s, cx| s.focus(window, cx));
                }
            }))
            .text_color(hsla(p.ink))
            .font_family(SharedString::from(typo.ui.clone()))
            .child(
                // Fluent navigation pane: on the window backdrop, no separate panel.
                div()
                    .w(px(280.0))
                    .h_full()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .px(px(tokens::SPACE_3))
                    .pb(px(tokens::SPACE_3))
                    .child(div().pb(px(tokens::SPACE_2)).child(gpui_kit::component::input::Input::new(&self.search)))
                    .child(div().id("nav-scroll").flex_1().min_h(px(0.0)).overflow_y_scroll().flex().flex_col().gap(px(tokens::SPACE_1)).children(nav))
                    .child(crate::panels3::sidebar_gauges(self, &p)),
            )
            .child(
                div()
                    .id("hub-panel")
                    .flex_1()
                    .track_scroll(&self.panel_scroll)
                    .min_w(px(0.0))
                    .h_full()
                    .overflow_y_scroll()
                    .p(px(tokens::SPACE_8))
                    .child(panel),
            );

        div().size_full().flex().flex_col().when_some(base, |d, c| d.bg(c)).child(title_bar).child(body)
    }
}
