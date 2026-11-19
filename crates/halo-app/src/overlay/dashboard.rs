//! Expanded notch = dashboard (layout inspired by Islita, styling by WinNotch's flat
//! look): sections sit directly on the notch black, separated by hairlines — no
//! tiles. Header (date, clock, status, settings), now playing, today's tasks, focus
//! timer, system rings, file shelf, command bar + quick tabs.

use std::time::Instant;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use halo_modules::focus::Phase;
use halo_modules::media::mmss;
use halo_platform::media::Command;
use halo_ui::hsla;
use halo_ui::tokens::{self, Palette};

use super::Overlay;
use crate::state::Section;
use crate::ui;

/// Section inside the notch: flat on the notch background (no tile).
fn card(_p: &Palette) -> Div {
    div().flex().flex_col().gap(px(8.0)).px(px(4.0)).py(px(2.0))
}

/// Hairline between sections.
fn divider(p: &Palette, vertical: bool) -> AnyElement {
    let d = div().flex_none().bg(hsla(p.overlay_sep));
    if vertical { d.w(px(1.0)).h_full() } else { d.h(px(1.0)).w_full() }.into_any_element()
}

fn caption(p: &Palette, text: impl Into<SharedString>) -> Div {
    div().text_size(px(12.0)).text_color(hsla(p.overlay_ink_2)).child(text.into())
}

pub(super) fn image_format(bytes: &[u8]) -> ImageFormat {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        ImageFormat::Png
    } else if bytes.starts_with(b"RIFF") {
        ImageFormat::Webp
    } else if bytes.starts_with(b"GIF") {
        ImageFormat::Gif
    } else if bytes.starts_with(b"BM") {
        ImageFormat::Bmp
    } else {
        ImageFormat::Jpeg
    }
}

/// Shortcuts available in the bar: (id, Lucide icon, action).
pub const TABS: &[(&str, &str, fn(&mut Overlay))] = &[
    ("picker", "pipette", |o| o.shared.borrow_mut().want_picker = true),
    ("notes", "notebook-pen", |o| o.shared.borrow_mut().open_hub(Section::Notes)),
    ("clipboard", "clipboard", |o| o.shared.borrow_mut().open_hub(Section::Clipboard)),
    ("system", "activity", |o| o.shared.borrow_mut().open_hub(Section::Resources)),
    ("music", "music", |o| o.shared.borrow_mut().open_hub(Section::Music)),
    ("productivity", "timer", |o| o.shared.borrow_mut().open_hub(Section::Productivity)),
    ("spotlight", "mouse-pointer-2", |o| {
        let c = halo_platform::cursor_pos().unwrap_or_default();
        o.shared.borrow().handles.spotlight.trigger(c);
    }),
    ("theme", "sun-moon", |o| o.toggle_theme()),
    ("wintheme", "monitor-cog", |o| {
        o.shared.borrow().handles.wintheme.toggle();
    }),
    ("screenshot", "camera", |o| o.shared.borrow_mut().want_region = Some(crate::state::RegionPurpose::Screenshot)),
    ("record", "video", |o| o.toggle_recording()),
    ("dnd", "moon", |o| o.shared.borrow_mut().update_config(|c| c.settings.dnd_manual = !c.settings.dnd_manual)),
    ("notifications", "bell", |o| o.shared.borrow_mut().open_hub(Section::Notifications)),
    ("settings", "settings", |o| o.shared.borrow_mut().open_hub(Section::General)),
];

impl Overlay {
    pub(super) fn icon_button(
        &self,
        id: &'static str,
        icon: &'static str,
        size: f32,
        filled: bool,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = Palette::dark();
        div()
            .id(id)
            .size(px(size))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .when(filled, |d| d.bg(hsla(p.overlay_track)))
            .hover(|s| s.bg(hsla(p.overlay_track)))
            .child(ui::icon(icon, size * 0.45, p.overlay_ink))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    on_click(this, cx);
                    cx.notify();
                }),
            )
    }

    fn media_card(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let np = self.shared.borrow().handles.media.get().filter(|n| !n.title.is_empty()).or_else(|| {
            // Halo's own player (not registered with the system media session).
            let st = self.shared.borrow().handles.music.get();
            st.queue.current().map(|t| halo_platform::media::NowPlaying {
                title: t.title.clone(),
                artist: t.artist.clone(),
                album: t.album.clone(),
                source: "halo".into(),
                playing: st.playing,
                position_s: st.position_s,
                duration_s: t.duration_s,
                art: st.cover.clone(),
            })
        });

        let display = self.shared.borrow().typo.display.clone();
        let Some(n) = np else {
            return card(p)
                .flex_1()
                .justify_center()
                .items_center()
                .child(ui::icon("music", 26.0, p.overlay_ink_2))
                .child(caption(p, self.shared.borrow().t("dash.nothing_playing")));
        };
        let art = match &n.art {
            Some(bytes) => img(std::sync::Arc::new(Image::from_bytes(image_format(bytes), bytes.clone())))
                .size(px(84.0))
                .flex_none()
                .rounded(px(14.0))
                .into_any_element(),
            None => div()
                .size(px(84.0))
                .flex_none()
                .rounded(px(14.0))
                .bg(hsla(p.overlay_track))
                .flex()
                .items_center()
                .justify_center()
                .child(ui::icon("music", 26.0, tokens::ACCENT))
                .into_any_element(),
        };
        let frac = if n.duration_s > 0.0 { (n.position_s / n.duration_s) as f32 } else { 0.0 };
        let h = self.shared.borrow().handles.media.clone();
        let music = self.shared.borrow().handles.music.clone();
        let own = n.source == "halo";
        let (h1, h2, h3) = (h.clone(), h.clone(), h);
        let (mu1, mu2, mu3) = (music.clone(), music.clone(), music);
        // Art on the left; title, artist, progress and controls stacked on the right.
        card(p).flex_1().justify_center().child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(14.0))
                .child(art)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .font_family(SharedString::from(display))
                                .text_size(px(15.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(hsla(p.overlay_ink))
                                .truncate()
                                .child(n.title.clone()),
                        )
                        .child(div().text_size(px(12.0)).text_color(hsla(p.overlay_ink_2)).truncate().child(n.artist.clone()))
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap(px(8.0))
                                .pt(px(4.0))
                                .child(div().text_size(px(10.5)).text_color(hsla(p.overlay_ink_2)).child(mmss(n.position_s)))
                                .child(
                                    div()
                                        .flex_1()
                                        .h(px(3.0))
                                        .rounded(px(2.0))
                                        .bg(hsla(p.overlay_track))
                                        .child(div().h_full().w(relative(frac.clamp(0.0, 1.0))).rounded(px(2.0)).bg(hsla(p.overlay_ink))),
                                )
                                .child(div().text_size(px(10.5)).text_color(hsla(p.overlay_ink_2)).child(mmss(n.duration_s))),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .justify_center()
                                .items_center()
                                .gap(px(10.0))
                                .child(self.icon_button("m-prev", "skip-back", 28.0, false, move |_, _| if own { mu1.previous() } else { h1.send(Command::Previous) }, cx))
                                .child(self.icon_button(
                                    "m-play",
                                    if n.playing { "pause" } else { "play" },
                                    32.0,
                                    true,
                                    move |_, _| if own { mu2.toggle() } else { h2.send(Command::Toggle) },
                                    cx,
                                ))
                                .child(self.icon_button("m-next", "skip-forward", 28.0, false, move |_, _| if own { mu3.next() } else { h3.send(Command::Next) }, cx)),
                        ),
                ),
        )
    }

    fn today_card(&self, p: &Palette, cx: &mut Context<Self>) -> Div {

        let Some(tasks) = self.shared.borrow().handles.tasks.clone() else { return card(p).flex_1() };
        let start = halo_modules::activity::day_start(0);
        let list = tasks.today(start, start + 86_400);
        let pending = list.iter().filter(|t| !t.done).count();
        let reminder = tasks
            .all_open()
            .into_iter()
            .filter(|t| t.remind && t.due.is_some_and(|d| d >= halo_modules::store::now()))
            .min_by_key(|t| t.due);
        let rows = list.into_iter().take(4).map(|t| {
            let id = t.id;
            let tasks = tasks.clone();
            div()
                .id(("task", id as u64))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .cursor_pointer()
                .child(if t.done {
                    ui::icon("check", 16.0, p.success).into_any_element()
                } else {
                    div().size(px(16.0)).flex_none().rounded_full().border_2().border_color(hsla(p.success)).into_any_element()
                })
                .child(
                    div()
                        .text_size(px(14.0))
                        .truncate()
                        .text_color(hsla(if t.done { p.overlay_ink_2 } else { p.overlay_ink }))
                        .when(t.done, |d| d.line_through())
                        .child(t.text.clone()),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_, _: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        tasks.toggle(id);
                        cx.notify();
                    }),
                )
        });
        card(p)
            .flex_1()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .child(caption(p, self.shared.borrow().t("dash.today")))
                    .child(caption(p, self.shared.borrow().tf("dash.pending", &[("n", &pending.to_string())]))),
            )
            .children(rows)
            .when(pending == 0, |d| d.child(caption(p, self.shared.borrow().t("dash.no_tasks"))))
            .child(div().flex_1())
            .children(reminder.map(|r| {
                let at = chrono::DateTime::from_timestamp(r.due.unwrap_or(0), 0)
                    .map(|d| d.with_timezone(&chrono::Local).format("%H:%M").to_string())
                    .unwrap_or_default();
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .text_size(px(13.0))
                    .text_color(hsla(p.info))
                    .child(ui::icon("bell", 14.0, p.info))
                    .child(format!("{at} · {}", r.text))
            }))
    }

    fn focus_card(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        let fr = self.shared.borrow().fr();
        let display = self.shared.borrow().typo.display.clone();
        let f = self.shared.borrow().handles.focus.clone();
        let now = Instant::now();
        let (phase, running, rem, prog) = f.with(|f| (f.phase(), f.running(), f.remaining(now), f.progress(now)));
        let label = match (phase, fr) {
            (Phase::Idle, true) => "Focus · 25 min",
            (Phase::Idle, false) => "Focus · 25 min",
            (Phase::Focus, true) => "Concentration",
            (Phase::Focus, false) => "Focus",
            (Phase::Break, true) => "Pause",
            (Phase::Break, false) => "Break",
        };
        let f2 = f.clone();
        card(p)
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .child(ui::ring(44.0, 4.0, if phase == Phase::Idle { 0.0 } else { prog }, p.overlay_track, tokens::ACCENT))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_family(SharedString::from(display))
                            .text_size(px(20.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(p.overlay_ink))
                            .child(mmss(rem.as_secs_f64())),
                    )
                    .child(caption(p, label).truncate()),
            )
            .child(self.icon_button(
                "focus-toggle",
                if running { "pause" } else { "play" },
                34.0,
                true,
                move |_, _| f2.with(|f| f.toggle(Instant::now())),
                cx,
            ))
    }

    fn system_card(&self, p: &Palette) -> Div {
        let s = &self.snap;
        let mono = SharedString::from(self.shared.borrow().typo.mono.clone());
        let gauge = |frac: f32, label: &str| {
            div()
                .relative()
                .size(px(48.0))
                .flex_none()
                .child(ui::ring(48.0, 4.0, frac, p.overlay_track, tokens::ACCENT))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_family(mono.clone())
                        .text_size(px(11.0))
                        .text_color(hsla(p.overlay_ink))
                        .child(label.to_string()),
                )
        };
        let battery = s.battery.percent.map(|b| format!("{b}%")).unwrap_or_else(|| "—".into());
        card(p)
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .child(gauge(s.cpu / 100.0, &format!("{:.0}%", s.cpu)))
            .child(gauge(s.ram_percent() / 100.0, &format!("{:.0}%", s.ram_percent())))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(caption(p, "CPU · RAM"))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(4.0))
                            .text_size(px(14.0))
                            .text_color(hsla(p.overlay_ink))
                            .child(ui::icon(if s.battery.charging { "battery-charging" } else { "battery" }, 14.0, p.overlay_ink_2))
                            .child(battery),
                    ),
            )
    }

    fn clock_card(&self, p: &Palette) -> Div {
        let display = self.shared.borrow().typo.display.clone();

        let now = chrono::Local::now();
        let world = [("Tokyo", chrono_tz::Asia::Tokyo), ("New York", chrono_tz::America::New_York)];
        card(p)
            .justify_center()
            .child(
                div()
                    .font_family(SharedString::from(display))
                    .text_size(px(30.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(hsla(p.overlay_ink))
                    .child(self.snap.clock.clone()),
            )
            .child(caption(p, self.snap.date_label(self.shared.borrow().config.lang.locale_code())))
            .children(world.iter().map(|(n, tz)| caption(p, format!("{n} {}", now.with_timezone(tz).format("%H:%M")))))
    }

    fn command_row(&self, p: &Palette, cx: &mut Context<Self>) -> Div {

        // Each shortcut explains itself on hover (translated `tab.<id>`).
        let tab = |this: &Self, id: &'static str, icon: &'static str, f: fn(&mut Overlay), cx: &mut Context<Self>| {
            let tip = this.shared.borrow().t(&format!("tab.{id}"));
            this.icon_button(id, icon, 32.0, false, move |o, _| f(o), cx)
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .id("cmd-pill")
                    .flex_1()
                    .h(px(34.0))
                    .rounded_full()
                    .border_1()
                    .border_color(hsla(p.overlay_line))
                    .hover(|s| s.bg(hsla(p.overlay_tile)))
                    .cursor_text()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .px(px(14.0))
                    .child(ui::icon("search", 16.0, tokens::ACCENT))
                    .child(caption(p, self.shared.borrow().t("dash.command")))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.shared.borrow_mut().want_palette = true;
                        }),
                    ),
            )
            .children(self.shared.borrow().config.overlay.tabs.clone().into_iter().filter_map(|id| {
                let (eid, icon, f) = TABS.iter().find(|(t, ..)| *t == id).map(|(t, i, f)| (*t, *i, *f))?;
                Some(tab(self, eid, icon, f, cx))
            }))
    }

    /// Something is playing or loaded (any app via the media session, or Halo's player).
    pub(super) fn has_media(&self) -> bool {
        let s = self.shared.borrow();
        s.handles.media.get().is_some_and(|n| !n.title.is_empty()) || s.handles.music.get().queue.current().is_some()
    }

    /// At least one task for today (pending, or completed today).
    pub(super) fn has_tasks(&self) -> bool {
        let start = halo_modules::activity::day_start(0);
        self.shared.borrow().handles.tasks.as_ref().is_some_and(|t| !t.today(start, start + 86_400).is_empty())
    }

    /// Number of cards that will actually be shown (empty ones are skipped).
    pub(super) fn visible_cards(&self) -> usize {
        let ids = self.shared.borrow().config.overlay.cards.clone();
        ids.iter()
            .filter(|id| match id.as_str() {
                "media" => self.has_media(),
                "today" => self.has_tasks(),
                _ => true,
            })
            .count()
    }

    /// Configured cards laid out left to right: wide cards (media, today) get their
    /// own column; consecutive compact cards (focus, system, clock) stack by two.
    fn card_columns(&self, p: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let ids = self.shared.borrow().config.overlay.cards.clone();
        let mut cols: Vec<AnyElement> = Vec::new();
        let mut stack: Vec<AnyElement> = Vec::new();
        let flush = |stack: &mut Vec<AnyElement>, cols: &mut Vec<AnyElement>| {
            if !stack.is_empty() {
                // Stacked sections are separated by a horizontal hairline.
                let mut items = Vec::new();
                for (i, el) in std::mem::take(stack).into_iter().enumerate() {
                    if i > 0 {
                        items.push(divider(p, false));
                    }
                    items.push(el);
                }
                cols.push(div().flex_1().min_w(px(0.0)).flex().flex_col().justify_center().gap(px(12.0)).children(items).into_any_element());
            }
        };
        for id in ids {
            match id.as_str() {
                "media" if !self.has_media() => {}
                "today" if !self.has_tasks() => {}
                "media" => {
                    flush(&mut stack, &mut cols);
                    cols.push(self.media_card(p, cx).flex_1().min_w(px(0.0)).into_any_element());
                }
                "today" => {
                    flush(&mut stack, &mut cols);
                    cols.push(self.today_card(p, cx).min_w(px(0.0)).into_any_element());
                }
                "controls" => {
                    flush(&mut stack, &mut cols);
                    cols.push(self.controls_section(p, cx).w(px(300.0)).flex_none().into_any_element());
                }
                small @ ("focus" | "system" | "clock") => {
                    if stack.len() == 2 {
                        flush(&mut stack, &mut cols);
                    }
                    stack.push(match small {
                        "focus" => self.focus_card(p, cx).into_any_element(),
                        "system" => self.system_card(p).into_any_element(),
                        _ => self.clock_card(p).flex_1().into_any_element(),
                    });
                }
                _ => {}
            }
        }
        flush(&mut stack, &mut cols);
        if self.has_shelf() {
            cols.push(self.shelf_section(p, cx).flex_1().into_any_element());
        }
        // Vertical hairlines between columns.
        let mut out = Vec::with_capacity(cols.len() * 2);
        for (i, c) in cols.into_iter().enumerate() {
            if i > 0 {
                out.push(divider(p, true));
            }
            out.push(c);
        }
        out
    }

    /// Full horizontal dashboard.
    pub(super) fn dashboard(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::dark();
        let (typo, _fr) = {
            let s = self.shared.borrow();
            (s.typo.clone(), s.fr())
        };
        let s = &self.snap;
        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .px(px(16.0))
            .pt(px(8.0))
            .pb(px(14.0))
            .child(
                // Header: date · clock · status + settings (WinNotch keeps a gear top-right).
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(px(24.0))
                    .child(div().w(px(150.0)).child(caption(&p, s.date_label(self.shared.borrow().config.lang.locale_code()))))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .justify_center()
                            .font_family(SharedString::from(typo.display.clone()))
                            .text_size(px(17.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(p.overlay_ink))
                            .child(s.clock.clone()),
                    )
                    .child(
                        div()
                            .w(px(150.0))
                            .flex()
                            .flex_row()
                            .items_center()
                            .justify_end()
                            .gap(px(10.0))
                            .children(["wifi", "bluetooth"].iter().filter_map(|i| self.indicator(i, false)))
                            .child({
                                let tip = self.shared.borrow().t("tab.settings");
                                self.icon_button("hdr-settings", "settings", 24.0, false, |o, _| o.shared.borrow_mut().open_hub(Section::General), cx)
                                    .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                            }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_row()
                    .gap(px(16.0))
                    .children(self.card_columns(&p, cx)),
            )
            .child(self.command_row(&p, cx))
            .into_any_element()
    }
}
