//! Phase 2 hub panels: Notes (M6), Clipboard (M7), Images (M9), Activity (M4).

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use halo_modules::activity::{self, format_duration};
use halo_modules::clipboard::Content;
use halo_modules::images::{Job, OutFormat, Resize};
use halo_ui::hsla;
use halo_ui::tokens::{self, Palette};

use crate::hub::Hub;
use crate::ui;

pub struct Phase2State {
    pub note_search: Entity<InputState>,
    pub note_editor: Entity<TextareaState>,
    pub note_id: Option<i64>,
    /// Editing the Markdown source (otherwise the note is shown formatted).
    pub note_editing: bool,
    pub note_folder: Option<String>,
    pub clip_search: Entity<InputState>,
    /// Clipboard filter: 0 all, 1 text, 2 links, 3 images.
    pub clip_filter: u8,
    pub image_files: Vec<PathBuf>,
    pub image_job: Job,
    pub activity_week: bool,
    pub task_input: Entity<InputState>,
    /// Language the input placeholders were last set for.
    pub lang: Option<halo_core::i18n::Lang>,
    _subs: Vec<Subscription>,
}

impl Phase2State {
    pub fn new(window: &mut Window, cx: &mut Context<Hub>) -> Self {
        // Placeholders are filled in the current language by `sync_placeholders`.
        let note_search = cx.new(|cx| InputState::new(window, cx));
        let note_editor = cx.new(|cx| TextareaState::new(window, cx));
        let clip_search = cx.new(|cx| InputState::new(window, cx));
        let task_input = cx.new(|cx| InputState::new(window, cx));
        let subs = vec![
            // Auto-save on every edit.
            cx.subscribe_in(&note_editor, window, |hub: &mut Hub, state, ev: &InputEvent, _, cx| {
                if matches!(ev, InputEvent::Change) {
                    if let (Some(id), Some(notes)) = (hub.p2.note_id, hub.shared.borrow().handles.notes.clone()) {
                        notes.save(id, &state.read(cx).value());
                        cx.notify();
                    }
                }
            }),
            cx.subscribe_in(&note_search, window, |_: &mut Hub, _, _: &InputEvent, _, cx| cx.notify()),
            cx.subscribe_in(&clip_search, window, |_: &mut Hub, _, _: &InputEvent, _, cx| cx.notify()),
        ];
        Self {
            note_search,
            note_editor,
            note_id: None,
            note_editing: false,
            note_folder: None,
            clip_search,
            clip_filter: 0,
            image_files: Vec::new(),
            image_job: Job::default(),
            activity_week: false,
            task_input,
            lang: None,
            _subs: subs,
        }
    }
}

fn t(hub: &Hub, key: &str) -> String {
    hub.shared.borrow().t(key)
}

fn page(p: &Palette, family: &str, title: String, intro: Option<String>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(tokens::SPACE_2)) // Fluent: cards stacked closely
        .w_full()
        .max_w(px(1000.0))
        .child(div().flex().flex_col().gap(px(tokens::SPACE_1)).pb(px(tokens::SPACE_4)).child(ui::h1(p, family, title)).children(intro.map(|i| ui::label(p, i))))
}

fn row() -> Div {
    div().flex().flex_row().flex_wrap().items_center().gap(px(tokens::SPACE_2))
}

/// Explains an opt-in module and offers to enable it (spec §7: explicit opt-in).
pub fn opt_in_card(hub: &Hub, p: &Palette, id: &'static str, what_key: &str) -> Div {
    let shared = hub.shared.clone();
    ui::card(p)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(tokens::SPACE_3))
                .child(ui::icon("shield", 20.0, p.accent_text))
                .child(ui::h2(p, t(hub, "optin.title"))),
        )
        .child(ui::label(p, t(hub, what_key)))
        .child(ui::label(p, t(hub, "optin.local")))
        .child(row().child(Button::new(ElementId::Name(format!("optin-{id}").into())).label(t(hub, "optin.enable")).primary().on_click(
            move |_, _, cx| {
                shared.borrow_mut().update_config(|c| {
                    c.modules.insert(id.into(), true);
                });
                cx.refresh_windows();
            },
        )))
}

// ── M6 Notes ────────────────────────────────────────────────────────────────

/// Re-apply input placeholders when the UI language changes (they are set once at
/// creation time, before the language is known to the inputs).
pub fn sync_placeholders(hub: &mut Hub, window: &mut Window, cx: &mut Context<Hub>) {
    let lang = hub.shared.borrow().config.lang;
    if hub.p2.lang == Some(lang) {
        return;
    }
    hub.p2.lang = Some(lang);
    let (search, body, task) = (t(hub, "notes.search"), t(hub, "notes.placeholder"), t(hub, "prod.new_task"));
    hub.p2.note_search.update(cx, |s, cx| s.set_placeholder(search.clone(), window, cx));
    hub.p2.clip_search.update(cx, |s, cx| s.set_placeholder(search, window, cx));
    hub.p2.note_editor.update(cx, |s, cx| s.set_placeholder(body, window, cx));
    hub.p2.task_input.update(cx, |s, cx| s.set_placeholder(task, window, cx));
}

/// "14:32" today, otherwise a short localized date ("12 Nov").
fn note_time(updated: i64, locale: &str) -> String {
    use chrono::{Datelike, Local, TimeZone};
    let Some(at) = Local.timestamp_opt(updated, 0).single() else {
        return String::new();
    };
    let now = Local::now();
    if at.date_naive() == now.date_naive() {
        at.format("%H:%M").to_string()
    } else {
        let loc = chrono::Locale::try_from(locale).unwrap_or(chrono::Locale::en_US);
        let fmt = if at.year() == now.year() { "%-d %b" } else { "%-d %b %Y" };
        at.format_localized(fmt, loc).to_string()
    }
}

/// Icon-only toolbar button with a tooltip.
fn tool_button(id: &'static str, icon: &str, tip: String) -> Button {
    Button::new(id).icon(ui::lucide(icon)).ghost().tooltip(tip)
}

/// Formatting applied by the notes toolbar (writes the Markdown for the user).
#[derive(Clone, Copy)]
enum Format {
    Heading,
    Bold,
    Italic,
    Bullet,
    Check,
    Quote,
}

impl Format {
    /// Line-start prefix, or `None` for inline formats that wrap the selection.
    fn prefix(self) -> Option<&'static str> {
        match self {
            Format::Heading => Some("## "),
            Format::Bullet => Some("- "),
            Format::Check => Some("- [ ] "),
            Format::Quote => Some("> "),
            Format::Bold | Format::Italic => None,
        }
    }

    fn wrap(self) -> &'static str {
        if matches!(self, Format::Bold) { "**" } else { "*" }
    }
}

/// Apply a format at the cursor/selection of the note editor, then save.
fn apply_format(hub: &mut Hub, f: Format, window: &mut Window, cx: &mut Context<Hub>) {
    hub.p2.note_editor.update(cx, |s, cx| {
        match f.prefix() {
            Some(prefix) => {
                let pos = s.cursor_position();
                s.set_cursor_position(gpui_kit::component::input::Position { line: pos.line, character: 0 }, window, cx);
                s.insert(prefix, window, cx);
            }
            None => {
                let sel = s.selected_value().to_string();
                let w = f.wrap();
                s.replace(format!("{w}{sel}{w}"), window, cx);
            }
        }
        s.focus(window, cx);
    });
    let body = hub.p2.note_editor.read(cx).value().to_string();
    if let (Some(id), Some(notes)) = (hub.p2.note_id, hub.shared.borrow().handles.notes.clone()) {
        notes.save(id, &body);
    }
    cx.notify();
}

pub fn notes(hub: &mut Hub, p: &Palette, window: &mut Window, cx: &mut Context<Hub>) -> AnyElement {
    let (display, locale) = {
        let s = hub.shared.borrow();
        (s.typo.display.clone(), s.config.lang.locale_code())
    };
    let Some(store) = hub.shared.borrow().handles.notes.clone() else {
        return ui::label(p, t(hub, "db.unavailable")).into_any_element();
    };
    let query = hub.p2.note_search.read(cx).value().to_string();
    let list = store.list(&query, hub.p2.note_folder.as_deref());
    let total = store.list("", None).len();
    // Keep a valid selection: the first visible note when nothing (or a deleted note) is selected.
    if hub.p2.note_id.is_none_or(|id| store.get(id).is_none()) {
        let first = list.first().cloned();
        hub.p2.note_id = first.as_ref().map(|n| n.id);
        let body = first.map(|n| n.body).unwrap_or_default();
        hub.p2.note_editor.update(cx, |s, cx| s.set_value(body, window, cx));
    }
    let selected = hub.p2.note_id.and_then(|id| store.get(id));
    let untitled = t(hub, "notes.untitled");

    // Fill the window height: viewport minus title bar, panel padding and page header.
    let height = (f32::from(window.viewport_size().height) - 200.0).max(440.0);

    let create = {
        let store = store.clone();
        move |hub: &mut Hub, window: &mut Window, cx: &mut Context<Hub>| {
            let folder = hub.p2.note_folder.clone().unwrap_or_default();
            let id = store.create("# ", &folder);
            hub.p2.note_id = Some(id);
            hub.p2.note_editing = true;
            hub.p2.note_editor.update(cx, |s, cx| {
                s.set_value("# ", window, cx);
                s.focus(window, cx);
            });
            cx.notify();
        }
    };

    // ── Left: search, folders, list ─────────────────────────────────────────
    let new_create = create.clone();
    let header = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(tokens::SPACE_2))
        .child(div().flex_1().min_w(px(0.0)).child(Input::new(&hub.p2.note_search).prefix(ui::icon("search", 14.0, p.ink_3)).cleanable(true)))
        .child(
            Button::new("note-new")
                .icon(ui::lucide("plus"))
                .primary()
                .tooltip(t(hub, "notes.new"))
                .on_click(cx.listener(move |hub, _, window, cx| new_create(hub, window, cx))),
        );

    let folders = store.folders();
    let chips = (!folders.is_empty()).then(|| {
        let mut all = vec![(t(hub, "notes.all"), None)];
        all.extend(folders.into_iter().map(|f| (f.clone(), Some(f))));
        row().children(all.into_iter().map(|(label, f)| {
            let active = hub.p2.note_folder == f;
            let b = Button::new(ElementId::Name(format!("folder-{label}").into())).label(label).compact().on_click(cx.listener(
                move |hub, _, _, cx| {
                    hub.p2.note_folder = f.clone();
                    hub.p2.note_id = None;
                    cx.notify();
                },
            ));
            if active { b.primary() } else { b.ghost() }
        }))
    });

    let selected_id = selected.as_ref().map(|n| n.id);
    let items = list.iter().map(|n| {
        let id = n.id;
        let body = n.body.clone();
        let active = Some(id) == selected_id;
        let title = n.title();
        let preview = n.preview();
        div()
            .id(("note", id as u64))
            .flex()
            .flex_col()
            .gap(px(2.0))
            .px(px(tokens::SPACE_3))
            .py(px(tokens::SPACE_2))
            .rounded(px(tokens::RADIUS_SM))
            .cursor_pointer()
            .when(active, |d| d.bg(hsla(p.accent_soft)))
            .when(!active, |d| d.hover(|s| s.bg(hsla(p.surface_2))))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .when(n.pinned, |d| d.child(ui::icon("pin", 12.0, p.accent_text)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(tokens::TEXT_LABEL.size))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(hsla(if title.is_empty() { p.ink_3 } else { p.ink }))
                            .child(if title.is_empty() { untitled.clone() } else { title }),
                    )
                    .child(div().flex_none().text_size(px(tokens::TEXT_MICRO.size)).text_color(hsla(p.ink_3)).child(note_time(n.updated, locale))),
            )
            .when(!preview.is_empty(), |d| {
                d.child(div().truncate().text_size(px(tokens::TEXT_MICRO.size)).text_color(hsla(p.ink_2)).child(preview))
            })
            .on_click(cx.listener(move |hub, _, window, cx| {
                hub.p2.note_id = Some(id);
                hub.p2.note_editing = false;
                let body = body.clone();
                hub.p2.note_editor.update(cx, |s, cx| s.set_value(body, window, cx));
                cx.notify();
            }))
    });
    let list_body = if list.is_empty() && total > 0 {
        div().p(px(tokens::SPACE_3)).child(ui::label(p, t(hub, "notes.no_match"))).into_any_element()
    } else {
        div().id("note-list").flex_1().min_h(px(0.0)).flex().flex_col().gap(px(2.0)).overflow_y_scroll().children(items).into_any_element()
    };

    let s_export = store.clone();
    let footer = div()
        .flex()
        .flex_row()
        .items_center()
        .pt(px(tokens::SPACE_2))
        .border_t_1()
        .border_color(hsla(p.line))
        .child(div().flex_1().text_size(px(tokens::TEXT_MICRO.size)).text_color(hsla(p.ink_3)).child(if total == 1 { t(hub, "notes.count.one") } else { hub.shared.borrow().tf("notes.count", &[("n", &total.to_string())]) }))
        .child(tool_button("note-export", "download", t(hub, "notes.export")).on_click(move |_, _, cx| {
            let dir = halo_core::config::Config::dir().join("Export notes");
            if let Ok(files) = s_export.export(&dir, true) {
                log::info!("exported {} notes to {}", files.len(), dir.display());
                cx.reveal_path(&dir);
            }
        }));

    let sidebar = ui::card(p)
        .w(px(264.0))
        .flex_none()
        .h_full()
        .child(header)
        .children(chips)
        .child(list_body)
        .child(footer);

    // ── Right: editor or empty state ────────────────────────────────────────
    let editing = hub.p2.note_editing;
    let editor = match selected {
        Some(note) => {
            let id = note.id;
            let pinned = note.pinned;
            let (s_pin, s_del) = (store.clone(), store.clone());
            let meta = format!(
                "{} · {}",
                hub.shared.borrow().tf("notes.edited", &[("v", &note_time(note.updated, locale))]),
                hub.shared.borrow().tf("notes.words", &[("n", &note.word_count().to_string())]),
            );
            let toolbar = div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(tokens::SPACE_1))
                .pb(px(tokens::SPACE_2))
                .border_b_1()
                .border_color(hsla(p.line))
                .when(!note.folder.is_empty(), |d| {
                    d.child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .mr(px(tokens::SPACE_2))
                            .child(ui::icon("folder", 14.0, p.ink_3))
                            .child(div().text_size(px(tokens::TEXT_MICRO.size)).text_color(hsla(p.ink_2)).child(note.folder.clone())),
                    )
                })
                .child(div().flex_1().text_size(px(tokens::TEXT_MICRO.size)).text_color(hsla(p.ink_3)).child(meta))
                .child({
                    // Edit ↔ formatted view.
                    let b = tool_button(
                        "note-mode",
                        if editing { "check" } else { "pencil" },
                        if editing { t(hub, "notes.done") } else { t(hub, "notes.edit") },
                    )
                    .on_click(cx.listener(move |hub, _, window, cx| {
                        hub.p2.note_editing = !hub.p2.note_editing;
                        if hub.p2.note_editing {
                            hub.p2.note_editor.update(cx, |s, cx| s.focus(window, cx));
                        }
                        cx.notify();
                    }));
                    if editing { b.primary() } else { b }
                })
                .child(
                    tool_button("note-pin", if pinned { "pin-off" } else { "pin" }, if pinned { t(hub, "notes.unpin") } else { t(hub, "notes.pin") })
                        .on_click(cx.listener(move |_, _, _, cx| {
                            s_pin.set_pinned(id, !pinned);
                            cx.notify();
                        })),
                )
                .child(tool_button("note-del", "trash", t(hub, "common.delete")).on_click(cx.listener(move |hub, _, _, cx| {
                    s_del.delete(id);
                    hub.p2.note_id = None;
                    cx.notify();
                })));
            let body: AnyElement = if editing {
                // Formatting bar: writes the Markdown so nobody has to type it.
                let fmt = |id: &'static str, icon: &str, key: &str, f: Format, hub: &Hub, cx: &mut Context<Hub>| {
                    tool_button(id, icon, t(hub, key)).compact().on_click(cx.listener(move |hub, _, window, cx| apply_format(hub, f, window, cx)))
                };
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(tokens::SPACE_2))
                    .child(
                        row()
                            .child(fmt("fmt-h", "heading", "notes.fmt.heading", Format::Heading, hub, cx))
                            .child(fmt("fmt-b", "bold", "notes.fmt.bold", Format::Bold, hub, cx))
                            .child(fmt("fmt-i", "italic", "notes.fmt.italic", Format::Italic, hub, cx))
                            .child(fmt("fmt-l", "list", "notes.fmt.list", Format::Bullet, hub, cx))
                            .child(fmt("fmt-c", "list-checks", "notes.fmt.check", Format::Check, hub, cx))
                            .child(fmt("fmt-q", "quote", "notes.fmt.quote", Format::Quote, hub, cx)),
                    )
                    .child(div().flex_1().min_h(px(0.0)).child(Textarea::new(&hub.p2.note_editor).appearance(false).h_full()))
                    .into_any_element()
            } else {
                // Formatted view; a click switches to editing.
                div()
                    .id("note-view")
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .px(px(tokens::SPACE_2))
                    .cursor_text()
                    .child(gpui_kit::component::text::TextView::markdown(("note-md", id as u64), note.body.clone()))
                    .on_click(cx.listener(|hub, _, window, cx| {
                        hub.p2.note_editing = true;
                        hub.p2.note_editor.update(cx, |s, cx| s.focus(window, cx));
                        cx.notify();
                    }))
                    .into_any_element()
            };
            ui::card(p).flex_1().min_w(px(0.0)).h_full().child(toolbar).child(body).into_any_element()
        }
        None => {
            let empty_create = create.clone();
            ui::card(p)
                .flex_1()
                .min_w(px(0.0))
                .h_full()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(tokens::SPACE_3))
                        .max_w(px(380.0))
                        .child(
                            div()
                                .size(px(56.0))
                                .rounded_full()
                                .bg(hsla(p.accent_soft))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(ui::icon("notebook-pen", 26.0, p.accent_text)),
                        )
                        .child(ui::h2(p, t(hub, "notes.empty.title")))
                        .child(div().text_center().child(ui::label(p, t(hub, "notes.empty.hint"))))
                        .child(
                            div().pt(px(tokens::SPACE_2)).child(
                                Button::new("note-new-empty")
                                    .icon(ui::lucide("plus"))
                                    .label(t(hub, "notes.new"))
                                    .primary()
                                    .on_click(cx.listener(move |hub, _, window, cx| empty_create(hub, window, cx))),
                            ),
                        ),
                )
                .into_any_element()
        }
    };

    page(p, &display, t(hub, "module.notes"), Some(t(hub, "notes.intro")))
        .max_w(px(1100.0))
        .child(div().flex().flex_row().gap(px(tokens::SPACE_4)).h(px(height)).child(sidebar).child(editor))
        .into_any_element()
}

// ── M7 Clipboard ────────────────────────────────────────────────────────────

pub fn clipboard(hub: &mut Hub, p: &Palette, cx: &mut Context<Hub>) -> AnyElement {
    let (display, mono, running, cfg) = {
        let s = hub.shared.borrow();
        (s.typo.display.clone(), s.typo.mono.clone(), s.module_running("clipboard"), s.config.settings.clone())
    };
    let pg = page(p, &display, t(hub, "module.clipboard"), Some(t(hub, "clipboard.intro")));
    if !running {
        return pg.child(opt_in_card(hub, p, "clipboard", "clipboard.what")).into_any_element();
    }
    let Some(store) = hub.shared.borrow().handles.clipboard.clone() else {
        return ui::label(p, t(hub, "db.unavailable")).into_any_element();
    };
    let q = hub.p2.clip_search.read(cx).value().to_string();
    let filter = hub.p2.clip_filter;
    let entries: Vec<_> = store
        .list(&q, 200)
        .into_iter()
        .filter(|e| match filter {
            1 => !matches!(e.content, Content::Image { .. }) && !e.is_link(),
            2 => e.is_link(),
            3 => matches!(e.content, Content::Image { .. }),
            _ => true,
        })
        .collect();
    let today = chrono::Local::now().date_naive();
    let row_for = |e: halo_modules::clipboard::Entry, hub: &Hub, cx: &mut Context<Hub>| {
        let id = e.id;
        let (s1, s2, s3) = (store.clone(), store.clone(), store.clone());
        let e2 = e.clone();
        let pinned = e.pinned;
        let link = e.is_link();
        let when = chrono::DateTime::from_timestamp(e.created, 0)
            .map(|d| {
                let d = d.with_timezone(&chrono::Local);
                if d.date_naive() == today { d.format("%H:%M").to_string() } else { d.format("%d/%m %H:%M").to_string() }
            })
            .unwrap_or_default();
        let app = e.source.trim_end_matches(".exe").trim_end_matches(".EXE").to_string();
        // Leading visual: a thumbnail for images, an icon otherwise.
        let (lead, body, size): (AnyElement, AnyElement, String) = match &e.content {
            Content::Image { png, width, height } => (
                img(std::sync::Arc::new(Image::from_bytes(ui::image_format(png), png.clone())))
                    .w(px(96.0))
                    .h(px(56.0))
                    .flex_none()
                    .rounded(px(tokens::RADIUS_SM))
                    .object_fit(ObjectFit::Cover)
                    .into_any_element(),
                div().text_size(px(tokens::TEXT_LABEL.size)).text_color(hsla(p.ink)).child(t(hub, "clipboard.image")).into_any_element(),
                format!("{width}×{height}"),
            ),
            _ => {
                let text = e.preview();
                let chars = text.chars().count();
                (
                    div()
                        .size(px(32.0))
                        .flex_none()
                        .rounded(px(tokens::RADIUS_SM))
                        .bg(hsla(p.surface_2))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(ui::icon(if link { "link" } else { "type" }, 16.0, if link { p.accent_text } else { p.ink_2 }))
                        .into_any_element(),
                    // At most two lines; the full text is copied anyway.
                    div()
                        .w_full()
                        .text_size(px(tokens::TEXT_LABEL.size))
                        .text_color(hsla(if link { p.accent_text } else { p.ink }))
                        .line_clamp(2)
                        .overflow_hidden()
                        .child(text)
                        .into_any_element(),
                    hub.shared.borrow().tf("clipboard.chars", &[("n", &chars.to_string())]),
                )
            }
        };
        let meta = [when, if app.is_empty() { "?".into() } else { app }, size].join(" · ");
        div()
            .id(("clip", id as u64))
            .group("clip-row")
            .flex()
            .flex_row()
            .items_center()
            .gap(px(tokens::SPACE_3))
            .px(px(tokens::SPACE_3))
            .py(px(tokens::SPACE_2))
            .rounded(px(tokens::RADIUS_MD))
            .bg(hsla(p.surface))
            .border_1()
            .border_color(hsla(p.line))
            .hover(|s| s.bg(hsla(p.surface_2)))
            .child(lead)
            .child(div().flex_1().min_w(px(0.0)).flex().flex_col().gap(px(2.0)).child(body).child(ui::mono(p, &mono, meta)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .gap(px(tokens::SPACE_1))
                    .child(Button::new(ElementId::Name(format!("clip-copy-{id}").into())).icon(ui::lucide("copy")).ghost().tooltip(t(hub, "common.copy")).on_click(
                        move |_, _, _| {
                            s1.copy(&e2);
                        },
                    ))
                    .child({
                        let b = Button::new(ElementId::Name(format!("clip-pin-{id}").into()))
                            .icon(ui::lucide(if pinned { "pin-off" } else { "pin" }))
                            .tooltip(if pinned { t(hub, "notes.unpin") } else { t(hub, "notes.pin") })
                            .on_click(cx.listener(move |_, _, _, cx| {
                                s2.set_pinned(id, !pinned);
                                cx.notify();
                            }));
                        if pinned { b.primary() } else { b.ghost() }
                    })
                    .child(Button::new(ElementId::Name(format!("clip-del-{id}").into())).icon(ui::lucide("trash")).ghost().tooltip(t(hub, "common.delete")).on_click(cx.listener(
                        move |_, _, _, cx| {
                            s3.delete(id);
                            cx.notify();
                        },
                    ))),
            )
    };
    let (pinned, recent): (Vec<_>, Vec<_>) = entries.into_iter().partition(|e| e.pinned);
    let section = |title: String, n: usize| {
        div().flex().flex_row().items_center().gap(px(tokens::SPACE_2)).pt(px(tokens::SPACE_2)).child(ui::label(p, title)).child(ui::mono(p, &mono, n.to_string()))
    };
    let mut list: Vec<AnyElement> = Vec::new();
    if !pinned.is_empty() {
        list.push(section(t(hub, "clipboard.pinned"), pinned.len()).into_any_element());
        for e in pinned {
            list.push(row_for(e, hub, cx).into_any_element());
        }
    }
    if !recent.is_empty() {
        list.push(section(t(hub, "clipboard.recent"), recent.len()).into_any_element());
        for e in recent {
            list.push(row_for(e, hub, cx).into_any_element());
        }
    }
    if list.is_empty() {
        list.push(div().py(px(tokens::SPACE_6)).flex().justify_center().child(ui::label(p, t(hub, "clipboard.empty"))).into_any_element());
    }
    let filters = [(0u8, "clipboard.all"), (1, "clipboard.text"), (2, "clipboard.links"), (3, "clipboard.images_f")].map(|(v, k)| {
        let b = Button::new(ElementId::Name(format!("clip-f-{v}").into())).label(t(hub, k)).compact().on_click(cx.listener(move |hub, _, _, cx| {
            hub.p2.clip_filter = v;
            cx.notify();
        }));
        if filter == v { b.primary() } else { b.ghost() }
    });
    let s_clear = store.clone();
    let count = store.count();
    let capacity = [100u32, 500, 2000].map(|v| {
        hub.choice(format!("clipcap-{v}"), v.to_string(), cfg.clip_capacity == v, move |c| c.settings.clip_capacity = v)
    });
    let autoclear = [(0u32, "clipboard.never"), (60, "clipboard.1h"), (1440, "clipboard.24h")].map(|(v, k)| {
        hub.choice(format!("clipclear-{v}"), t(hub, k), cfg.clip_autoclear_minutes == v, move |c| c.settings.clip_autoclear_minutes = v)
    });
    pg.child(
        row()
            .child(div().w(px(320.0)).child(Input::new(&hub.p2.clip_search).prefix(ui::icon("search", 14.0, p.ink_3)).cleanable(true)))
            .children(filters)
            .child(div().flex_1())
            .child(ui::mono(p, &mono, format!("{count} / {}", cfg.clip_capacity)))
            .child(Button::new("clip-clear").icon(ui::lucide("trash")).label(t(hub, "clipboard.clear")).ghost().on_click(cx.listener(move |_, _, _, cx| {
                s_clear.clear();
                cx.notify();
            }))),
    )
    .child(div().flex().flex_col().gap(px(tokens::SPACE_2)).children(list))
    .child(
        ui::card(p)
            .child(ui::h2(p, t(hub, "clipboard.privacy")))
            .child(hub.switch_row(p, "clip-enc", t(hub, "clipboard.encrypt"), Some(t(hub, "clipboard.encrypt.hint")), cfg.clip_encrypt, |c, v| {
                c.settings.clip_encrypt = v
            }))
            .child(hub.switch_row(p, "clip-img", t(hub, "clipboard.images"), None, cfg.clip_images, |c, v| c.settings.clip_images = v))
            .child(ui::label(p, t(hub, "clipboard.autoclear")))
            .child(row().children(autoclear))
            .child(ui::label(p, t(hub, "clipboard.capacity")))
            .child(row().children(capacity))
            .child(ui::label(p, format!("{} : {}", t(hub, "clipboard.excluded"), cfg.clip_exclude_apps.join(", ")))),
    )
    .into_any_element()
}

// ── M9 Images ───────────────────────────────────────────────────────────────

pub fn images(hub: &mut Hub, p: &Palette, cx: &mut Context<Hub>) -> AnyElement {
    let (display, mono) = {
        let s = hub.shared.borrow();
        (s.typo.display.clone(), s.typo.mono.clone())
    };
    let batch = hub.shared.borrow().handles.images.get();
    let job = hub.p2.image_job.clone();

    let pick = Button::new("img-pick").label(t(hub, "images.pick")).outline().on_click(cx.listener(|_, _, _, cx| {
        let rx = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: true, prompt: None });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = rx.await {
                let _ = this.update(cx, |hub, cx| {
                    hub.p2.image_files.extend(paths.into_iter().filter(|p| halo_modules::images::is_image(p)));
                    cx.notify();
                });
            }
        })
        .detach();
    }));
    let files = hub.p2.image_files.clone();
    let handle = hub.shared.borrow().handles.images.clone();
    let run_job = job.clone();
    let run = Button::new("img-run").label(t(hub, "images.run")).primary().on_click(cx.listener(move |hub, _, _, cx| {
        if !files.is_empty() {
            handle.run(files.clone(), run_job.clone());
            hub.p2.image_files.clear();
            cx.notify();
        }
    }));

    let sizes: [(&str, Resize); 5] = [
        ("Original", Resize::None),
        ("3840 px", Resize::Fit { max_w: 3840, max_h: 3840 }),
        ("1920 px", Resize::Fit { max_w: 1920, max_h: 1920 }),
        ("1280 px", Resize::Fit { max_w: 1280, max_h: 1280 }),
        ("50 %", Resize::Percent(50.0)),
    ];
    let sizes = sizes.map(|(label, r)| {
        let b = Button::new(ElementId::Name(format!("img-size-{label}").into())).label(label).on_click(cx.listener(
            move |hub, _, _, cx| {
                hub.p2.image_job.resize = r;
                cx.notify();
            },
        ));
        if job.resize == r { b.primary() } else { b.outline() }
    });
    let formats = [("WebP", OutFormat::Webp), ("JPEG", OutFormat::Jpeg), ("PNG", OutFormat::Png), ("AVIF", OutFormat::Avif), ("=", OutFormat::Same)]
        .map(|(label, f)| {
            let b = Button::new(ElementId::Name(format!("img-fmt-{label}").into())).label(label).on_click(cx.listener(
                move |hub, _, _, cx| {
                    hub.p2.image_job.format = f;
                    cx.notify();
                },
            ));
            if job.format == f { b.primary() } else { b.outline() }
        });
    let qualities = [60u8, 75, 82, 90].map(|q| {
        let b = Button::new(ElementId::Name(format!("img-q-{q}").into())).label(q.to_string()).on_click(cx.listener(
            move |hub, _, _, cx| {
                hub.p2.image_job.quality = q;
                cx.notify();
            },
        ));
        if job.quality == q { b.primary() } else { b.outline() }
    });

    let queued = hub.p2.image_files.iter().map(|f| ui::mono(p, &mono, f.display().to_string()));
    let results = batch.results.iter().map(|r| match r {
        Ok(o) => {
            let gain = 100.0 - o.after as f64 * 100.0 / o.before.max(1) as f64;
            let dst = o.dst.clone();
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(tokens::SPACE_3))
                .child(ui::icon("check", 14.0, p.success_text))
                .child(div().flex_1().text_size(px(13.0)).child(o.dst.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()))
                .child(ui::mono(
                    p,
                    &mono,
                    format!(
                        "{}×{} · {} → {} ({gain:+.0} %)",
                        o.size.0,
                        o.size.1,
                        halo_modules::system::format_bytes(o.before),
                        halo_modules::system::format_bytes(o.after)
                    )
                    .replace("+", "−"),
                ))
                .child(Button::new(ElementId::Name(format!("img-open-{}", o.dst.display()).into())).icon(ui::lucide("folder-open")).ghost().on_click(
                    move |_, _, cx| cx.reveal_path(&dst),
                ))
        }
        Err(e) => div().flex().flex_row().gap(px(tokens::SPACE_3)).child(ui::icon("x", 14.0, p.danger_text)).child(ui::mono(p, &mono, e.clone())),
    });

    page(p, &display, t(hub, "module.images"), Some(t(hub, "images.intro")))
        .child(
            div()
                .id("img-drop")
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(tokens::SPACE_2))
                .h(px(140.0))
                .rounded(px(tokens::RADIUS_LG))
                .border_2()
                .border_dashed()
                .border_color(hsla(p.line_2))
                .bg(hsla(p.surface))
                .child(ui::icon("image", 28.0, p.accent_text))
                .child(ui::label(p, t(hub, "images.drop")))
                .child(pick)
                .on_drop(cx.listener(|hub, paths: &ExternalPaths, _, cx| {
                    hub.p2.image_files.extend(paths.paths().iter().filter(|p| halo_modules::images::is_image(p)).cloned());
                    cx.notify();
                })),
        )
        .child(ui::card(p).child(ui::h2(p, t(hub, "images.size"))).child(row().children(sizes)))
        .child(ui::card(p).child(ui::h2(p, t(hub, "images.format"))).child(row().children(formats)).child(ui::label(p, t(hub, "images.quality"))).child(row().children(qualities)).child(ui::label(p, t(hub, "images.meta"))))
        .child(
            ui::card(p)
                .child(row().child(ui::h2(p, format!("{} ({})", t(hub, "images.queue"), hub.p2.image_files.len()))).child(div().flex_1()).child(run))
                .children(queued)
                .when(batch.total > 0, |d| {
                    d.child(ui::meter(p, batch.done as f32 / batch.total.max(1) as f32))
                        .child(ui::mono(p, &mono, format!("{} / {}", batch.done, batch.total)))
                })
                .children(results),
        )
        .into_any_element()
}

// ── M4 Activity ─────────────────────────────────────────────────────────────

pub fn activity(hub: &mut Hub, p: &Palette, cx: &mut Context<Hub>) -> AnyElement {
    let (display, mono, running, cfg) = {
        let s = hub.shared.borrow();
        (s.typo.display.clone(), s.typo.mono.clone(), s.module_running("activity"), s.config.settings.clone())
    };
    let pg = page(p, &display, t(hub, "module.activity"), Some(t(hub, "activity.intro")));
    if !running {
        return pg.child(opt_in_card(hub, p, "activity", "activity.what")).into_any_element();
    }
    let Some(h) = hub.shared.borrow().handles.activity.clone() else {
        return ui::label(p, t(hub, "db.unavailable")).into_any_element();
    };
    let week = hub.p2.activity_week;
    let from = activity::day_start(if week { 6 } else { 0 });
    let to = halo_modules::store::now() + 1;
    let totals = h.totals(from, to);
    let cats = h.by_category(from, to);
    let total: i64 = totals.iter().map(|t| t.seconds).sum();
    let max = totals.first().map(|t| t.seconds).unwrap_or(1).max(1);

    let ranges = [(false, "activity.today"), (true, "activity.week")].map(|(w, k)| {
        let b = Button::new(ElementId::Name(format!("act-range-{w}").into())).label(t(hub, k)).on_click(cx.listener(move |hub, _, _, cx| {
            hub.p2.activity_week = w;
            cx.notify();
        }));
        if w == week { b.primary() } else { b.outline() }
    });
    let paused = h.paused();
    let (h1, h2) = (h.clone(), h.clone());
    let cat_rows = cats.iter().map(|(c, s)| {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(tokens::SPACE_3))
            .child(div().w(px(140.0)).text_size(px(13.0)).child(c.clone()))
            .child(div().flex_1().child(ui::meter(p, *s as f32 / total.max(1) as f32)))
            .child(div().w(px(80.0)).child(ui::mono(p, &mono, format_duration(*s))))
    });
    let app_rows = totals.iter().take(15).map(|a| {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(tokens::SPACE_3))
            .child(div().w(px(180.0)).text_size(px(13.0)).child(a.app.trim_end_matches(".exe").to_string()))
            .child(div().w(px(110.0)).child(ui::mono(p, &mono, a.category.clone())))
            .child(div().flex_1().child(ui::meter(p, a.seconds as f32 / max as f32)))
            .child(div().w(px(80.0)).child(ui::mono(p, &mono, format_duration(a.seconds))))
    });
    pg.child(
        row()
            .children(ranges)
            .child(div().flex_1())
            .child(
                Button::new("act-pause")
                    .label(if paused { t(hub, "activity.resume") } else { t(hub, "activity.pause") })
                    .outline()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        h1.set_paused(!paused);
                        cx.notify();
                    })),
            )
            .child(Button::new("act-erase").label(t(hub, "activity.erase")).danger().on_click(cx.listener(move |_, _, _, cx| {
                h2.erase();
                cx.notify();
            }))),
    )
    .child(
        ui::card(p)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_end()
                    .gap(px(tokens::SPACE_3))
                    .child(div().font_family(SharedString::from(display.clone())).text_size(px(28.0)).font_weight(FontWeight::SEMIBOLD).child(format_duration(total)))
                    .child(ui::label(p, t(hub, "activity.total"))),
            )
            .children(cat_rows),
    )
    .child(ui::card(p).child(ui::h2(p, t(hub, "activity.apps"))).children(app_rows).when(totals.is_empty(), |d| d.child(ui::label(p, t(hub, "activity.none")))))
    .child(
        ui::card(p)
            .child(ui::h2(p, t(hub, "clipboard.privacy")))
            .child(hub.switch_row(p, "act-titles", t(hub, "activity.titles"), Some(t(hub, "activity.titles.hint")), cfg.activity_titles, |c, v| {
                c.settings.activity_titles = v
            }))
            .child(ui::label(p, t(hub, "activity.guarantees"))),
    )
    .into_any_element()
}

// ── Productivity mode (Pomodoro + tasks + distractions) ─────────────────────

pub fn productivity(hub: &mut Hub, p: &Palette, cx: &mut Context<Hub>) -> AnyElement {
    let (display, mono, cfg, _fr) = {
        let s = hub.shared.borrow();
        (s.typo.display.clone(), s.typo.mono.clone(), s.config.settings.clone(), s.fr())
    };
    let focus = hub.shared.borrow().handles.focus.clone();
    let tasks = hub.shared.borrow().handles.tasks.clone();
    let now = std::time::Instant::now();
    let (phase, running, rem, prog) = focus.with(|f| (f.phase(), f.running(), f.remaining(now), f.progress(now)));
    let phase_label = match phase {
        halo_modules::focus::Phase::Idle => t(hub, "prod.ready"),
        halo_modules::focus::Phase::Focus => t(hub, "prod.focus"),
        halo_modules::focus::Phase::Break => t(hub, "prod.break"),
    };
    let (f1, f2) = (focus.clone(), focus.clone());
    let timer = ui::card(p)
        .flex_row()
        .items_center()
        .gap(px(tokens::SPACE_6))
        .child(
            div()
                .relative()
                .size(px(132.0))
                .flex_none()
                .child(ui::ring(132.0, 8.0, if phase == halo_modules::focus::Phase::Idle { 0.0 } else { prog }, p.surface_3, p.accent))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .font_family(SharedString::from(display.clone()))
                                .text_size(px(28.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(halo_modules::media::mmss(rem.as_secs_f64())),
                        )
                        .child(ui::mono(p, &mono, phase_label)),
                ),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(tokens::SPACE_3))
                .child(row().children(
                    [(25u32, 5u32), (50, 10), (15, 3)].map(|(f, b)| {
                        hub.choice(format!("pomo-{f}"), format!("{f} / {b} min"), cfg.focus_minutes == f, move |c| {
                            c.settings.focus_minutes = f;
                            c.settings.break_minutes = b;
                        })
                    }),
                ))
                .child(
                    row()
                        .child(
                            Button::new("pomo-toggle")
                                .label(if running { t(hub, "activity.pause") } else { t(hub, "prod.start") })
                                .primary()
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    f1.with(|f| f.toggle(std::time::Instant::now()));
                                    cx.notify();
                                })),
                        )
                        .child(Button::new("pomo-reset").label(t(hub, "prod.reset")).outline().on_click(cx.listener(move |_, _, _, cx| {
                            f2.with(|f| f.reset());
                            cx.notify();
                        }))),
                ),
        );

    let start = halo_modules::activity::day_start(0);
    let (sessions, minutes) = tasks.as_ref().map(|t| t.focus_stats(start)).unwrap_or((0, 0));
    let stats = ui::card(p)
        .flex_row()
        .gap(px(tokens::SPACE_8))
        .child(div().flex().flex_col().child(div().font_family(SharedString::from(display.clone())).text_size(px(28.0)).font_weight(FontWeight::SEMIBOLD).child(sessions.to_string())).child(ui::label(p, t(hub, "prod.sessions"))))
        .child(div().flex().flex_col().child(div().font_family(SharedString::from(display.clone())).text_size(px(28.0)).font_weight(FontWeight::SEMIBOLD).child(format_duration(minutes as i64 * 60))).child(ui::label(p, t(hub, "prod.minutes"))));

    let task_rows: Vec<AnyElement> = tasks
        .as_ref()
        .map(|th| {
            th.today(start, start + 86_400)
                .into_iter()
                .map(|task| {
                    let (a, b) = (th.clone(), th.clone());
                    let id = task.id;
                    div()
                        .id(("ptask", id as u64))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(tokens::SPACE_3))
                        .child(
                            div()
                                .id(("ptask-check", id as u64))
                                .cursor_pointer()
                                .child(if task.done {
                                    ui::icon("circle-check", 18.0, p.success_text).into_any_element()
                                } else {
                                    ui::icon("circle", 18.0, p.ink_2).into_any_element()
                                })
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    a.toggle(id);
                                    cx.notify();
                                })),
                        )
                        .child(div().flex_1().text_size(px(14.0)).when(task.done, |d| d.line_through().text_color(hsla(p.ink_2))).child(task.text.clone()))
                        .child(Button::new(ElementId::Name(format!("ptask-del-{id}").into())).icon(ui::lucide("trash")).ghost().on_click(cx.listener(move |_, _, _, cx| {
                            b.delete(id);
                            cx.notify();
                        })))
                        .into_any_element()
                })
                .collect()
        })
        .unwrap_or_default();
    let th = tasks.clone();
    let add = Button::new("ptask-add").label(t(hub, "prod.add")).outline().on_click(cx.listener(move |hub, _, window, cx| {
        let text = hub.p2.task_input.read(cx).value().to_string();
        if !text.trim().is_empty() {
            if let Some(t) = &th {
                t.add(&text, None, false);
            }
            hub.p2.task_input.update(cx, |s, cx| s.set_value("", window, cx));
            cx.notify();
        }
    }));

    page(p, &display, t(hub, "nav.productivity"), Some(t(hub, "prod.intro")))
        .child(timer)
        .child(stats)
        .child(
            ui::card(p)
                .child(ui::h2(p, t(hub, "activity.today")))
                .child(row().child(div().flex_1().child(Input::new(&hub.p2.task_input))).child(add))
                .children(task_rows),
        )
        .child(
            ui::card(p)
                .child(ui::h2(p, t(hub, "prod.distractions")))
                .child(ui::label(p, t(hub, "prod.distractions.hint")))
                .child(ui::mono(p, &mono, cfg.distraction_apps.join(" · ")))
                .child(hub.switch_row(p, "pause-media", t(hub, "prod.pausemedia"), None, cfg.focus_pause_media, |c, v| c.settings.focus_pause_media = v))
                .child(ui::label(p, t(hub, "prod.tip"))),
        )
        .into_any_element()
}

// ── Notifications + Do Not Disturb ──────────────────────────────────────────

pub fn notifications(hub: &mut Hub, p: &Palette, cx: &mut Context<Hub>) -> AnyElement {
    let (display, mono, running, cfg) = {
        let s = hub.shared.borrow();
        (s.typo.display.clone(), s.typo.mono.clone(), s.module_running("notifications"), s.config.settings.clone())
    };
    let froms = ["21:00", "22:00", "23:00"].map(|v| hub.choice(format!("dnd-f-{v}"), v, cfg.dnd_from == v, move |c| c.settings.dnd_from = v.into()));
    let tos = ["06:00", "07:00", "08:00"].map(|v| hub.choice(format!("dnd-t-{v}"), v, cfg.dnd_to == v, move |c| c.settings.dnd_to = v.into()));
    let dnd = ui::card(p)
        .child(div().flex().flex_row().items_center().gap(px(tokens::SPACE_3)).child(ui::icon("moon", 18.0, p.accent_text)).child(ui::h2(p, t(hub, "dnd.title"))))
        .child(hub.switch_row(p, "dnd-manual", t(hub, "dnd.now"), None, cfg.dnd_manual, |c, v| c.settings.dnd_manual = v))
        .child(hub.switch_row(p, "dnd-focus", t(hub, "dnd.focus"), None, cfg.dnd_during_focus, |c, v| c.settings.dnd_during_focus = v))
        .child(hub.switch_row(p, "dnd-sched", t(hub, "dnd.schedule"), None, cfg.dnd_scheduled, |c, v| c.settings.dnd_scheduled = v))
        .when(cfg.dnd_scheduled, |d| d.child(ui::label(p, t(hub, "dnd.from"))).child(row().children(froms)).child(ui::label(p, t(hub, "dnd.to"))).child(row().children(tos)))
        .child(ui::label(p, t(hub, "dnd.windows")))
        .child(row().child(Button::new("dnd-win").icon(ui::lucide("external-link")).label(t(hub, "dnd.open_windows")).outline().on_click(|_, _, _| {
            let _ = std::process::Command::new("explorer.exe").arg("ms-settings:notifications").spawn();
        })));
    let pg = page(p, &display, t(hub, "module.notifications"), Some(t(hub, "notif.intro")));
    if !running {
        return pg.child(opt_in_card(hub, p, "notifications", "notif.what")).child(dnd).into_any_element();
    }
    let h = hub.shared.borrow().handles.notify.clone();
    let status = match h.access() {
        Some(halo_platform::notifications::Access::Allowed) => t(hub, "notif.allowed"),
        Some(halo_platform::notifications::Access::Denied) => t(hub, "notif.denied"),
        None => "...".into(),
        _ => t(hub, "notif.unavailable"),
    };
    let rows = h.recent().into_iter().map(|n| {
        let id = n.id;
        let h2 = h.clone();
        let when = chrono::DateTime::from_timestamp(n.at, 0).map(|d| d.with_timezone(&chrono::Local).format("%H:%M").to_string()).unwrap_or_default();
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(tokens::SPACE_3))
            .px(px(tokens::SPACE_4))
            .py(px(tokens::SPACE_3))
            .rounded(px(tokens::RADIUS_MD))
            .bg(hsla(p.surface))
            .border_1()
            .border_color(hsla(p.line))
            .child(ui::icon("message-square", 16.0, p.ink_2))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(14.0)).truncate().child(n.title.clone()))
                    .child(div().text_size(px(12.0)).text_color(hsla(p.ink_2)).truncate().child(n.body.clone()))
                    .child(ui::mono(p, &mono, format!("{when} · {}", n.app))),
            )
            .child(Button::new(ElementId::Name(format!("notif-x-{id}").into())).icon(ui::lucide("x")).ghost().on_click(cx.listener(move |_, _, _, cx| {
                h2.dismiss(id);
                cx.notify();
            })))
    });
    pg.child(dnd)
        .child(ui::card(p).child(ui::h2(p, t(hub, "notif.recent"))).child(ui::mono(p, &mono, status)).children(rows))
        .into_any_element()
}
