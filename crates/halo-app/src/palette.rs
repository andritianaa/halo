//! Launcher (Ctrl+Space, PowerToys-Run style) under the notch: apps, Halo features &
//! settings, calculations/conversions/clocks, tasks/reminders/notes, web search.
//! Everything is matched locally; the web search only runs when explicitly chosen.

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use halo_modules::command::{self, Action};
use halo_ui::hsla;
use halo_ui::tokens::{self, Palette};

use crate::state::{Section, Shared};
use crate::ui;

pub struct CommandPalette {
    shared: Shared,
    input: Entity<InputState>,
    selected: usize,
    _subs: Vec<Subscription>,
}

/// One row of results.
#[derive(Clone)]
enum Item {
    Command(command::Outcome),
    App(crate::launcher::App),
    Setting(Section, String),
    Web(String),
}

const W: f32 = 680.0;
const H: f32 = 420.0;

/// Opens (or focuses) the launcher. Returns the new window's HWND so the caller can
/// restyle it *after* the GPUI update (Win32 restyling inside an update re-enters GPUI).
pub fn open(cx: &mut App, shared: Shared) -> Option<isize> {
    if let Some(h) = shared.borrow().palette_window {
        if h.update(cx, |_, window, _| window.activate_window()).is_ok() {
            return None;
        }
    }
    let m = halo_platform::monitor_for(shared.borrow().config.overlay.display);
    let (wx, wy) = (m.work_area.x as f32 / m.scale, m.work_area.y as f32 / m.scale);
    let ww = m.work_area.w as f32 / m.scale;
    let bounds = Bounds::new(point(px(wx + (ww - W) / 2.0), px(wy + 96.0)), size(px(W), px(H)));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: true,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        window_decorations: Some(WindowDecorations::Client),
        ..Default::default()
    };
    let for_view = shared.clone();
    let mut hwnd = None;
    let res = cx.open_window(options, |window, cx| {
        hwnd = crate::overlay::raw_hwnd(window);
        cx.new(|cx| {
            let placeholder = for_view.borrow().t("launcher.placeholder");
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            input.update(cx, |s, cx| s.focus(window, cx));
            let subs = vec![
                cx.subscribe_in(&input, window, |this: &mut CommandPalette, state, ev: &InputEvent, window, cx| match ev {
                    InputEvent::PressEnter { .. } => {
                        let text = state.read(cx).value().to_string();
                        let items = this.items(&text);
                        if let Some(item) = items.get(this.selected.min(items.len().saturating_sub(1))).cloned() {
                            this.run(item, window, cx);
                        }
                    }
                    InputEvent::Change => {
                        this.selected = 0;
                        cx.notify();
                    }
                    _ => {}
                }),
                cx.observe_window_activation(window, |_: &mut CommandPalette, window, _| {
                    if !window.is_window_active() {
                        window.remove_window();
                    }
                }),
            ];
            CommandPalette { shared: for_view, input, selected: 0, _subs: subs }
        })
    });
    if let Ok(h) = res {
        shared.borrow_mut().palette_window = Some(h.into());
        return hwnd;
    }
    None
}

impl CommandPalette {
    fn items(&self, text: &str) -> Vec<Item> {
        let text = text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        let fr = self.shared.borrow().fr();
        let mut out = Vec::new();
        if let Some(o) = command::eval(text, chrono::Local::now()) {
            out.push(Item::Command(o));
        }
        out.extend(crate::launcher::find(text, 5).into_iter().map(Item::App));
        out.extend(crate::search::search(text, fr).into_iter().take(4).map(|h| Item::Setting(h.section, h.label)));
        out.push(Item::Web(text.to_string()));
        out
    }

    fn run(&mut self, item: Item, window: &mut Window, _cx: &mut Context<Self>) {
        let mut s = self.shared.borrow_mut();
        match item {
            Item::App(a) => crate::launcher::launch(&a.path),
            Item::Web(q) => crate::launcher::web_search(&q),
            Item::Setting(section, _) => s.open_hub(section),
            Item::Command(out) => match out.action {
                Some(Action::Copy(v)) => {
                    ui::copy_text(&v);
                }
                Some(Action::AddTask { text, due }) => {
                    if let Some(t) = &s.handles.tasks {
                        t.add(&text, due, false);
                    }
                }
                Some(Action::AddReminder { text, at }) => {
                    if let Some(t) = &s.handles.tasks {
                        t.add(&text, Some(at), true);
                    }
                }
                Some(Action::AddNote(text)) => {
                    if let Some(n) = &s.handles.notes {
                        n.create(&text, "");
                    }
                }
                Some(Action::Open(id)) => match id {
                    "picker" => s.want_picker = true,
                    "focus" => s.handles.focus.with(|f| f.start(std::time::Instant::now())),
                    "spotlight" => {
                        let c = halo_platform::cursor_pos().unwrap_or_default();
                        s.handles.spotlight.trigger(c);
                    }
                    "theme" => {
                        let pref = if s.mode == tokens::Mode::Dark {
                            halo_core::config::ThemePref::Light
                        } else {
                            halo_core::config::ThemePref::Dark
                        };
                        s.update_config(|c| c.theme = pref);
                    }
                    "notes" => s.open_hub(Section::Notes),
                    "clipboard" => s.open_hub(Section::Clipboard),
                    "images" => s.open_hub(Section::Images),
                    "music" => s.open_hub(Section::Music),
                    "settings" => s.open_hub(Section::General),
                    _ => s.open_hub(Section::Modules),
                },
                None => {}
            },
        }
        drop(s);
        window.remove_window();
    }

    fn row(&self, p: &Palette, i: usize, item: &Item, _fr: bool) -> Div {
        let selected = i == self.selected;
        let (icon, title, hint): (&str, String, String) = match item {
            Item::Command(o) => (o.icon, o.preview.clone(), self.shared.borrow().t("launcher.enter")),
            Item::App(a) => ("app-window", a.name.clone(), self.shared.borrow().t("launcher.app")),
            Item::Setting(_, label) => ("settings", label.clone(), "Halo".into()),
            Item::Web(q) => ("globe", self.shared.borrow().tf("launcher.web", &[("q", q)]), "DuckDuckGo".into()),
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .px(px(12.0))
            .py(px(9.0))
            .rounded(px(12.0))
            .when(selected, |d| d.bg(hsla(p.overlay_track)))
            .child(ui::icon(icon, 18.0, if selected { tokens::ACCENT } else { p.overlay_ink_2 }))
            .child(div().flex_1().text_size(px(15.0)).truncate().text_color(hsla(p.overlay_ink)).child(title))
            .child(div().text_size(px(11.0)).text_color(hsla(p.overlay_ink_2)).child(hint))
            .when(selected, |d| d.child(ui::icon("corner-down-left", 14.0, p.overlay_ink_2)))
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::dark();
        let text = self.input.read(cx).value().to_string();
        let items = self.items(&text);
        self.selected = self.selected.min(items.len().saturating_sub(1));
        let fr = self.shared.borrow().fr();
        let hint_keys: [(&str, &str); 6] = [
            ("app-window", "launcher.hint.apps"),
            ("calculator", "launcher.hint.calc"),
            ("globe", "launcher.hint.clock"),
            ("list-checks", "launcher.hint.task"),
            ("bell", "launcher.hint.remind"),
            ("settings", "launcher.hint.settings"),
        ];
        let hints: Vec<(&str, String)> = hint_keys.iter().map(|(i, k)| (*i, self.shared.borrow().t(k))).collect();
        let rows: Vec<AnyElement> = items.iter().enumerate().map(|(i, it)| self.row(&p, i, it, fr).into_any_element()).collect();
        let n = items.len();
        div().size_full().p(px(1.0)).child(
            div()
                .size_full()
                .rounded(px(22.0))
                .bg(hsla(p.overlay_bg))
                .border_1()
                .border_color(hsla(p.overlay_line))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .p(px(14.0))
                .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, window, cx| match ev.keystroke.key.as_str() {
                    "escape" => window.remove_window(),
                    "down" => {
                        this.selected = (this.selected + 1).min(n.saturating_sub(1));
                        cx.notify();
                    }
                    "up" => {
                        this.selected = this.selected.saturating_sub(1);
                        cx.notify();
                    }
                    _ => {}
                }))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(10.0))
                        .px(px(4.0))
                        .child(ui::icon("search", 18.0, tokens::ACCENT))
                        .child(div().flex_1().child(Input::new(&self.input).appearance(false))),
                )
                .child(div().h(px(1.0)).bg(hsla(p.overlay_line)))
                .when(rows.is_empty(), |d| {
                    d.children(hints.iter().map(|(icon, h)| {
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(12.0))
                            .px(px(12.0))
                            .py(px(6.0))
                            .child(ui::icon(icon, 16.0, p.overlay_ink_2))
                            .child(div().text_size(px(13.0)).text_color(hsla(p.overlay_ink_2)).child(h.to_string()))
                    }))
                })
                .children(rows)
                .child(div().flex_1())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(px(14.0))
                        .px(px(4.0))
                        .text_size(px(11.0))
                        .text_color(hsla(p.overlay_ink_2))
                        .child(self.shared.borrow().t("launcher.local"))
                        .child(div().flex_1())
                        .child(self.shared.borrow().t("launcher.keys")),
                ),
        )
    }
}
