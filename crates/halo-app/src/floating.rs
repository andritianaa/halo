//! Transient floating surfaces driven by the overlay loop:
//! - M16 spotlight: full-monitor dim with a hole around the cursor (click-through).
//! - M11 colour picker: magnifier that follows the cursor; clicks are captured by a
//!   low-level mouse hook only while picking.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use halo_core::overlay::Rect;
use halo_modules::colorpicker::{Format, Rgb};
use halo_platform::Monitor;
use halo_platform::hooks::{HookThread, MouseEvent};
use halo_ui::hsla;
use halo_ui::tokens::{self, Palette};

use crate::overlay::{Deferred, floating_options, raw_hwnd};
use crate::state::Shared;

#[derive(Default)]
pub struct Floating {
    spotlight: Option<(AnyWindowHandle, isize)>,
    picker: Option<PickerSession>,
    region: Option<RegionSession>,
    pending_region: Option<(crate::state::RegionPurpose, Rect, std::time::Instant)>,
}

struct PickerSession {
    window: AnyWindowHandle,
    hwnd: isize,
    capture: Arc<AtomicBool>,
    _hook: HookThread,
    clicks: Receiver<MouseEvent>,
    started: Instant,
}

fn monitor_at(x: i32, y: i32) -> Monitor {
    let all = halo_platform::monitors();
    all.iter()
        .copied()
        .find(|m| m.work_area.contains(x, y))
        .or_else(|| all.first().copied())
        .unwrap_or(halo_platform::monitor_for(0))
}

impl Floating {
    /// Returns true while something needs fast (60 Hz) polling.
    pub fn tick(&mut self, shared: &Shared, cx: &mut App, out: &mut Vec<Deferred>) -> bool {
        let fast_spot = self.tick_spotlight(shared, cx, out);
        let fast_pick = self.tick_picker(shared, cx, out);
        fast_spot || fast_pick
    }

    fn tick_spotlight(&mut self, shared: &Shared, cx: &mut App, out: &mut Vec<Deferred>) -> bool {
        let st = shared.borrow().handles.spotlight.get();
        let active = st.active_until.is_some_and(|t| Instant::now() < t);
        match (&self.spotlight, active) {
            (None, true) => {
                let m = monitor_at(st.cursor.0, st.cursor.1);
                // Cover the whole monitor (work area + taskbar strip is fine).
                let r = Rect::new(m.work_area.x, m.work_area.y, m.work_area.w, m.work_area.h + 80);
                let (radius, dim) = {
                    let s = &shared.borrow().config.settings;
                    (s.spotlight_radius, s.spotlight_dim)
                };
                let handles = shared.borrow().handles.spotlight.clone();
                let mut hwnd = None;
                let res = cx.open_window(floating_options(), |window, cx| {
                    hwnd = raw_hwnd(window);
                    cx.new(|_| SpotlightView {
                        origin: (r.x, r.y),
                        scale: m.scale,
                        radius,
                        dim,
                        handle: handles,
                        opened: Instant::now(),
                        // A button held when it opens (shake while dragging) is not a click.
                        was_down: halo_platform::key_down(0x01) || halo_platform::key_down(0x02),
                    })
                });
                if let (Ok(h), Some(hw)) = (res, hwnd) {
                    out.push(Deferred::Style(hw, true));
                    out.push(Deferred::Place(hw, r));
                    self.spotlight = Some((h.into(), hw));
                }
                true
            }
            (Some((h, _)), false) => {
                let _ = h.update(cx, |_, window, _| window.remove_window());
                self.spotlight = None;
                false
            }
            (Some(_), true) => true,
            (None, false) => false,
        }
    }

    fn tick_picker(&mut self, shared: &Shared, cx: &mut App, out: &mut Vec<Deferred>) -> bool {
        let want = std::mem::take(&mut shared.borrow_mut().want_picker);
        if want && self.picker.is_none() && shared.borrow().module_running("picker") {
            let capture = Arc::new(AtomicBool::new(true));
            let (hook, clicks) = halo_platform::hooks::mouse(capture.clone());
            let mut hwnd = None;
            let mono = shared.borrow().typo.mono.clone();
            let format = shared.borrow().handles.picker.get().format;
            let res = cx.open_window(floating_options(), |window, cx| {
                hwnd = raw_hwnd(window);
                cx.new(|_| PickerView { mono, format, color: Rgb(0, 0, 0), cells: Vec::new() })
            });
            if let (Ok(h), Some(hw)) = (res, hwnd) {
                out.push(Deferred::Style(hw, true));
                self.picker = Some(PickerSession {
                    window: h.into(),
                    hwnd: hw,
                    capture,
                    _hook: hook,
                    clicks,
                    started: Instant::now(),
                });
            }
        }
        let Some(session) = &self.picker else { return false };

        let (cx_, cy_) = halo_platform::cursor_pos().unwrap_or_default();
        let m = monitor_at(cx_, cy_);
        let size = ((MAG_W * m.scale) as i32, (MAG_H * m.scale) as i32);
        // Keep the magnifier on-screen, offset from the cursor.
        let off = (24.0 * m.scale) as i32;
        let mut x = cx_ + off;
        let mut y = cy_ + off;
        if x + size.0 > m.work_area.x + m.work_area.w {
            x = cx_ - off - size.0;
        }
        if y + size.1 > m.work_area.y + m.work_area.h {
            y = cy_ - off - size.1;
        }
        out.push(Deferred::Place(session.hwnd, Rect::new(x, y, size.0, size.1)));

        // Sample a 9×9 neighbourhood for the magnifier.
        let grab = halo_platform::capture::grab(Rect::new(cx_ - 4, cy_ - 4, 9, 9));
        let center = halo_platform::capture::pixel(cx_, cy_).map(|(r, g, b)| Rgb(r, g, b));
        let _ = session.window.update(cx, |view, _, cx| {
            if let Ok(v) = view.downcast::<PickerView>() {
                v.update(cx, |v, cx| {
                    if let Some(c) = center {
                        v.color = c;
                    }
                    if let Some(img) = &grab {
                        v.cells = img.rgba.chunks_exact(4).map(|p| Rgb(p[0], p[1], p[2])).collect();
                    }
                    cx.notify();
                });
            }
        });

        // Finish on click (pick) / right click or Esc (cancel). Ignore clicks in the
        // first 150 ms (the click that opened the picker from the notch).
        let mut done: Option<Option<Rgb>> = None;
        while let Ok(ev) = session.clicks.try_recv() {
            if session.started.elapsed().as_millis() < 150 {
                continue;
            }
            done = Some(match ev {
                MouseEvent::LeftDown { x, y } => halo_platform::capture::pixel(x, y).map(|(r, g, b)| Rgb(r, g, b)),
                MouseEvent::RightDown { .. } => None,
            });
        }
        if halo_platform::key_down(0x1B) {
            done = Some(None);
        }
        if let Some(result) = done {
            session.capture.store(false, Ordering::Relaxed);
            let _ = session.window.update(cx, |_, window, _| window.remove_window());
            if let Some(c) = result {
                let handle = shared.borrow().handles.picker.clone();
                let text = c.format(handle.get().format);
                crate::ui::copy_text(&text);
                handle.update(|s| s.push(c));
                log::info!("picked {text}");
            }
            self.picker = None;
            return false;
        }
        true
    }
}

const MAG_W: f32 = 168.0;
const MAG_H: f32 = 206.0;

struct PickerView {
    mono: String,
    format: Format,
    color: Rgb,
    cells: Vec<Rgb>,
}

impl Render for PickerView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::dark();
        let cell = 16.0;
        let grid = div().flex().flex_row().flex_wrap().w(px(cell * 9.0)).h(px(cell * 9.0)).children(
            self.cells.iter().enumerate().map(|(i, c)| {
                let center = i == 40;
                div()
                    .size(px(cell))
                    .bg(hsla(tokens::Color::hex(c.as_u32())))
                    .when(center, |d| d.border_2().border_color(hsla(tokens::ACCENT)))
            }),
        );
        div()
            .size_full()
            .rounded(px(tokens::RADIUS_LG))
            .bg(hsla(p.overlay_bg))
            .border_1()
            .border_color(hsla(p.overlay_line))
            .p(px(12.0))
            .flex()
            .flex_col()
            .gap(px(10.0))
            .child(div().rounded(px(tokens::RADIUS_SM)).overflow_hidden().child(grid))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().size(px(16.0)).rounded(px(5.0)).bg(hsla(tokens::Color::hex(self.color.as_u32()))))
                    .child(
                        div()
                            .font_family(SharedString::from(self.mono.clone()))
                            .text_size(px(12.0))
                            .text_color(hsla(p.overlay_ink))
                            .child(self.color.format(self.format)),
                    ),
            )
    }
}

struct SpotlightView {
    origin: (i32, i32),
    scale: f32,
    radius: f32,
    dim: u8,
    handle: halo_modules::spotlight::SpotlightHandle,
    opened: Instant,
    /// Left button state seen last frame (a click = released → pressed).
    was_down: bool,
}

/// Smoothstep on 0..1.
fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

impl Render for SpotlightView {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // Follow the cursor every frame while visible.
        window.request_animation_frame();
        let st = self.handle.get();
        let (x, y) = halo_platform::cursor_pos().unwrap_or(st.cursor);
        // A click dismisses the spotlight (it is click-through: the click still lands).
        let down = halo_platform::key_down(0x01) || halo_platform::key_down(0x02);
        if down && !self.was_down {
            self.handle.close_soon();
        }
        self.was_down = down;

        let now = Instant::now();
        let close = halo_modules::spotlight::SPOT_CLOSE.as_secs_f32();
        // Appear: the hole shrinks from 2.2× into place while the dim fades in.
        let open_t = ease(now.duration_since(self.opened).as_secs_f32() / tokens::MOTION_SLOW.as_secs_f32());
        // Disappear: during the last SPOT_CLOSE, the hole grows past the screen edges
        // (scale) and the dim fades out.
        let left = st.active_until.map(|t| t.saturating_duration_since(now).as_secs_f32()).unwrap_or(0.0);
        let close_t = ease(1.0 - left / close);
        let cxl = (x - self.origin.0) as f32 / self.scale;
        let cyl = (y - self.origin.1) as f32 / self.scale;
        let base = self.radius;
        let alpha = open_t * (1.0 - close_t);
        let dim = hsla(tokens::Color::hex_a(0x000000, (self.dim as f32 * alpha) as u32));
        let ring = hsla(tokens::ACCENT.with_alpha_pct((100.0 * alpha) as u32));
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let c = point(bounds.origin.x + px(cxl), bounds.origin.y + px(cyl));
                let diag = f32::from(bounds.size.width).hypot(f32::from(bounds.size.height));
                let r = base * (1.0 + 1.2 * (1.0 - open_t)) + diag * close_t;
                if let Some(path) = dim_with_hole(bounds, c, r) {
                    window.paint_path(path, dim);
                }
                if let Some(path) = circle(c, r, Some(3.0)) {
                    window.paint_path(path, ring);
                }
            },
        )
        .size_full()
    }
}

/// The whole surface minus a round hole: the rectangle and the circle are wound in
/// opposite directions, so the circle is cut out (a single fill — no giant strokes,
/// which tessellated into stray lines and a polygonal hole).
fn dim_with_hole(bounds: Bounds<Pixels>, c: Point<Pixels>, r: f32) -> Option<Path<Pixels>> {
    let mut b = PathBuilder::fill();
    let (o, s) = (bounds.origin, bounds.size);
    b.move_to(o);
    b.line_to(point(o.x + s.width, o.y));
    b.line_to(point(o.x + s.width, o.y + s.height));
    b.line_to(point(o.x, o.y + s.height));
    b.close();
    let left = point(c.x - px(r), c.y);
    let right = point(c.x + px(r), c.y);
    b.move_to(right);
    b.arc_to(point(px(r), px(r)), px(0.0), false, false, left);
    b.arc_to(point(px(r), px(r)), px(0.0), false, false, right);
    b.close();
    b.build().ok()
}

/// Circle path (stroked when `stroke` is set) made of two half arcs.
fn circle(c: Point<Pixels>, r: f32, stroke: Option<f32>) -> Option<Path<Pixels>> {
    let mut b = match stroke {
        Some(w) => PathBuilder::stroke(px(w)),
        None => PathBuilder::fill(),
    };
    let left = point(c.x - px(r), c.y);
    let right = point(c.x + px(r), c.y);
    b.move_to(left);
    b.arc_to(point(px(r), px(r)), px(0.0), false, true, right);
    b.arc_to(point(px(r), px(r)), px(0.0), false, true, left);
    b.close();
    b.build().ok()
}

// ── Region selector (screenshot / recording) ────────────────────────────────

pub struct RegionSession {
    window: AnyWindowHandle,
    pub purpose: crate::state::RegionPurpose,
    result: std::rc::Rc<std::cell::RefCell<Option<Option<Rect>>>>,
}

pub struct RegionView {
    origin: (i32, i32),
    scale: f32,
    start: Option<Point<Pixels>>,
    cur: Option<Point<Pixels>>,
    result: std::rc::Rc<std::cell::RefCell<Option<Option<Rect>>>>,
    hint: String,
    focus: FocusHandle,
}

impl RegionView {
    fn rect_logical(&self) -> Option<(f32, f32, f32, f32)> {
        let (a, b) = (self.start?, self.cur?);
        let (x0, y0) = (f32::from(a.x).min(f32::from(b.x)), f32::from(a.y).min(f32::from(b.y)));
        let (x1, y1) = (f32::from(a.x).max(f32::from(b.x)), f32::from(a.y).max(f32::from(b.y)));
        Some((x0, y0, x1 - x0, y1 - y0))
    }
}

impl Render for RegionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.focus(&self.focus, cx);
        let p = Palette::dark();
        let dim = hsla(tokens::Color::hex_a(0x000000, 45));
        let sel = self.rect_logical();
        let scale = self.scale;
        let mut root = div()
            .size_full()
            .relative()
            .cursor_crosshair()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, _| {
                if ev.keystroke.key == "escape" {
                    *this.result.borrow_mut() = Some(None);
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                this.start = Some(ev.position);
                this.cur = Some(ev.position);
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _: &MouseDownEvent, _, _| {
                *this.result.borrow_mut() = Some(None);
            }))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| {
                if this.start.is_some() && ev.pressed_button == Some(MouseButton::Left) {
                    this.cur = Some(ev.position);
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, _| {
                if let Some((x, y, w, h)) = this.rect_logical() {
                    let r = if w >= 4.0 && h >= 4.0 {
                        Some(Rect::new(
                            this.origin.0 + (x * this.scale) as i32,
                            this.origin.1 + (y * this.scale) as i32,
                            (w * this.scale) as i32,
                            (h * this.scale) as i32,
                        ))
                    } else {
                        None
                    };
                    *this.result.borrow_mut() = Some(r);
                }
            }));
        match sel {
            None => {
                root = root.child(div().absolute().inset_0().bg(dim));
            }
            Some((x, y, w, h)) => {
                // Four dim bands around a clear selection.
                root = root
                    .child(div().absolute().left_0().top_0().w_full().h(px(y)).bg(dim))
                    .child(div().absolute().left_0().top(px(y + h)).w_full().bottom_0().bg(dim))
                    .child(div().absolute().left_0().top(px(y)).w(px(x)).h(px(h)).bg(dim))
                    .child(div().absolute().left(px(x + w)).top(px(y)).right_0().h(px(h)).bg(dim))
                    .child(div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).border_2().border_color(hsla(tokens::ACCENT)))
                    .child(
                        div()
                            .absolute()
                            .left(px(x))
                            .top(px((y - 28.0).max(4.0)))
                            .px(px(8.0))
                            .py(px(3.0))
                            .rounded(px(6.0))
                            .bg(hsla(p.overlay_bg))
                            .text_size(px(12.0))
                            .text_color(hsla(p.overlay_ink))
                            .child(format!("{} × {}", (w * scale) as i32, (h * scale) as i32)),
                    );
            }
        }
        root.child(
            div()
                .absolute()
                .top(px(24.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(div().px(px(14.0)).py(px(8.0)).rounded_full().bg(hsla(p.overlay_bg)).text_size(px(13.0)).text_color(hsla(p.overlay_ink)).child(self.hint.clone())),
        )
    }
}

impl Floating {
    /// Opens the selector when requested; returns a finished selection (purpose, rect).
    pub fn tick_region(&mut self, shared: &Shared, cx: &mut App, out: &mut Vec<Deferred>) -> Option<(crate::state::RegionPurpose, Rect)> {
        let want = shared.borrow_mut().want_region.take();
        if let (Some(purpose), None) = (want, &self.region) {
            let (cx_, cy_) = halo_platform::cursor_pos().unwrap_or_default();
            let m = monitor_at(cx_, cy_);
            let r = Rect::new(m.work_area.x, m.work_area.y, m.work_area.w, m.work_area.h);
            let result = std::rc::Rc::new(std::cell::RefCell::new(None));
            let hint = shared.borrow().t("capture.region_hint");
            let mut hwnd = None;
            let mut opts = floating_options();
            opts.focus = true;
            let res2 = result.clone();
            let res = cx.open_window(opts, |window, cx| {
                hwnd = raw_hwnd(window);
                cx.new(|cx| RegionView { origin: (r.x, r.y), scale: m.scale, start: None, cur: None, result: res2, hint, focus: cx.focus_handle() })
            });
            if let (Ok(h), Some(hw)) = (res, hwnd) {
                out.push(Deferred::Popup(hw));
                out.push(Deferred::Place(hw, r));
                self.region = Some(RegionSession { window: h.into(), purpose, result });
            }
        }
        let done = self.region.as_ref().and_then(|s| s.result.borrow_mut().take());
        if let Some(sel) = done {
            let s = self.region.take()?;
            let _ = s.window.update(cx, |_, window, _| window.remove_window());
            // Act on the next ticks, once the dim layer is gone from the screen.
            if let Some(rect) = sel {
                self.pending_region = Some((s.purpose, rect, std::time::Instant::now()));
            }
        }
        match self.pending_region {
            Some((purpose, rect, at)) if at.elapsed() >= std::time::Duration::from_millis(180) => {
                self.pending_region = None;
                Some((purpose, rect))
            }
            _ => None,
        }
    }

    pub fn region_active(&self) -> bool {
        self.region.is_some() || self.pending_region.is_some()
    }
}
