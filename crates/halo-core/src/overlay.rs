//! Overlay placement and auto-hide logic (spec M1), independent of the windowing layer.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use halo_ui::tokens::{self, overlay as m};

/// Screen anchor of the pill.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Anchor {
    #[default]
    TopCenter,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    LeftCenter,
    RightCenter,
}

impl Anchor {
    pub const ALL: [Anchor; 7] = [
        Anchor::TopCenter,
        Anchor::TopLeft,
        Anchor::TopRight,
        Anchor::BottomLeft,
        Anchor::BottomRight,
        Anchor::LeftCenter,
        Anchor::RightCenter,
    ];

    pub fn orientation(self) -> Orientation {
        match self {
            Anchor::LeftCenter | Anchor::RightCenter => Orientation::Vertical,
            _ => Orientation::Horizontal,
        }
    }

    /// i18n key of the anchor label.
    pub fn label_key(self) -> &'static str {
        match self {
            Anchor::TopCenter => "anchor.top-center",
            Anchor::TopLeft => "anchor.top-left",
            Anchor::TopRight => "anchor.top-right",
            Anchor::BottomLeft => "anchor.bottom-left",
            Anchor::BottomRight => "anchor.bottom-right",
            Anchor::LeftCenter => "anchor.left-center",
            Anchor::RightCenter => "anchor.right-center",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

/// Integer rectangle in physical pixels (Win32 coordinates).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && py >= self.y && px < self.x + self.w && py < self.y + self.h
    }

    /// Distance from a point to the rectangle (0 inside).
    pub fn distance(&self, px: i32, py: i32) -> f32 {
        let dx = (self.x - px).max(0).max(px - (self.x + self.w - 1));
        let dy = (self.y - py).max(0).max(py - (self.y + self.h - 1));
        ((dx * dx + dy * dy) as f32).sqrt()
    }
}

/// Screen edge the notch is attached to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

impl Anchor {
    pub fn edge(self) -> Edge {
        match self {
            Anchor::TopCenter | Anchor::TopLeft | Anchor::TopRight => Edge::Top,
            Anchor::BottomLeft | Anchor::BottomRight => Edge::Bottom,
            Anchor::LeftCenter => Edge::Left,
            Anchor::RightCenter => Edge::Right,
        }
    }
}

/// Notch body size `(along, depth)` in logical px (ears excluded).
/// `along` is parallel to the attached edge, `depth` goes into the screen.
pub fn notch_size(orientation: Orientation, expanded: bool, compact_along: f32) -> (f32, f32) {
    match (orientation, expanded) {
        (Orientation::Horizontal, false) => (compact_along, m::COMPACT_DEPTH),
        (Orientation::Horizontal, true) => (m::EXPANDED_ALONG, m::EXPANDED_DEPTH),
        (Orientation::Vertical, false) => (compact_along, m::COMPACT_V_DEPTH),
        (Orientation::Vertical, true) => (m::EXPANDED_V_ALONG, m::EXPANDED_V_DEPTH),
    }
}

/// Window rectangle (physical px) holding a notch body of `(along, depth)` logical px plus
/// its two ears, flush against the anchor's edge of the monitor work area.
pub fn window_rect(anchor: Anchor, work_area: Rect, scale: f32, body: (f32, f32)) -> Rect {
    let along = ((body.0 + 2.0 * m::EAR) * scale).round() as i32;
    let depth = (body.1 * scale).round() as i32;
    let inset = (m::CORNER_INSET * scale).round() as i32;
    let wa = work_area;
    match anchor.edge() {
        Edge::Top | Edge::Bottom => {
            let x = match anchor {
                Anchor::TopLeft | Anchor::BottomLeft => wa.x + inset,
                Anchor::TopRight | Anchor::BottomRight => wa.x + wa.w - along - inset,
                _ => wa.x + (wa.w - along) / 2,
            };
            let y = if anchor.edge() == Edge::Top { wa.y } else { wa.y + wa.h - depth };
            Rect::new(x, y, along, depth)
        }
        Edge::Left | Edge::Right => {
            let y = wa.y + (wa.h - along) / 2;
            let x = if anchor.edge() == Edge::Left { wa.x } else { wa.x + wa.w - depth };
            Rect::new(x, y, depth, along)
        }
    }
}

/// Outline of the notch inside its window, in logical px, as a closed polyline of
/// segments: `Line(to)` or `Quad(ctrl, to)` (quadratic Bézier). Starts at `start`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Line((f32, f32)),
    Quad((f32, f32), (f32, f32)),
}

/// Notch outline for a window of `size` (logical, including ears), attached to `edge`.
/// Square against the edge, concave ears of radius `EAR`, inner corners of `radius`.
pub fn notch_outline(edge: Edge, size: (f32, f32), radius: f32) -> ((f32, f32), Vec<Seg>) {
    // Work in edge space: u along the edge (0..len), v away from it (0..depth).
    let (len, depth) = match edge {
        Edge::Top | Edge::Bottom => size,
        Edge::Left | Edge::Right => (size.1, size.0),
    };
    let e = m::EAR.min(len / 2.0);
    let r = radius.min(depth).min((len - 2.0 * e) / 2.0).max(0.0);
    let map = |(u, v): (f32, f32)| match edge {
        Edge::Top => (u, v),
        Edge::Bottom => (u, size.1 - v),
        Edge::Left => (v, u),
        Edge::Right => (size.0 - v, u),
    };
    let ear_v = e.min(depth);
    let segs = vec![
        Seg::Quad(map((e, 0.0)), map((e, ear_v))),
        Seg::Line(map((e, depth - r))),
        Seg::Quad(map((e, depth)), map((e + r, depth))),
        Seg::Line(map((len - e - r, depth))),
        Seg::Quad(map((len - e, depth)), map((len - e, depth - r))),
        Seg::Line(map((len - e, ear_v))),
        Seg::Quad(map((len - e, 0.0)), map((len, 0.0))),
        Seg::Line(map((0.0, 0.0))),
    ];
    (map((0.0, 0.0)), segs)
}

/// Auto-hide policy (spec M1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "mode")]
pub enum AutoHide {
    AlwaysVisible,
    FadeAfter { secs: u32 },
}

impl Default for AutoHide {
    fn default() -> Self {
        AutoHide::FadeAfter { secs: tokens::OVERLAY_IDLE_SECS }
    }
}

/// What triggers expansion.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ExpandOn {
    #[default]
    Hover,
    Click,
    /// Hover shows a small preview (peek); a click opens the full dashboard, which
    /// closes again when the pointer leaves.
    Peek,
}

/// Inputs sampled each tick by the app.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pointer {
    /// Cursor over the pill.
    pub over: bool,
    /// Cursor within the proximity radius of the pill.
    pub near: bool,
}

/// Overlay presentation state machine: expansion + fade, driven by time and pointer.
#[derive(Clone, Debug)]
pub struct OverlayState {
    pub auto_hide: AutoHide,
    pub expand_on: ExpandOn,
    expanded: bool,
    /// Opened by click/hotkey: stays open until clicked again or Esc.
    pinned_open: bool,
    hidden: bool,
    /// Time without the pointer over/near the pill (drives the fade).
    idle: Duration,
    /// Time since the pointer left the pill (drives hover collapse).
    outside: Duration,
    /// A file is being dragged over the notch: stay open to show the drop zones.
    held: bool,
    /// Peek mode: pointer over the notch while it is not expanded.
    peeking: bool,
}

/// Leaving the notch collapses it right away (on the first poll outside), so the
/// shrink follows the pointer. The hit rect is the full expanded window, not the
/// animated shape, so this does not flicker while the notch is still growing.
const COLLAPSE_GRACE: Duration = Duration::ZERO;

impl OverlayState {
    pub fn new(auto_hide: AutoHide, expand_on: ExpandOn) -> Self {
        Self {
            auto_hide,
            expand_on,
            expanded: false,
            pinned_open: false,
            hidden: false,
            idle: Duration::ZERO,
            outside: Duration::ZERO,
            held: false,
            peeking: false,
        }
    }

    /// Peek preview visible (Peek mode, pointer over, not expanded).
    pub fn is_peeking(&self) -> bool {
        self.peeking && !self.expanded && !self.hidden
    }

    /// Keep the notch open while a file drag hovers it (any `expand_on` mode);
    /// releasing collapses it unless it was pinned open by a click.
    pub fn hold_open(&mut self, on: bool) {
        if on == self.held {
            return;
        }
        self.held = on;
        if on {
            self.idle = Duration::ZERO;
            self.hidden = false;
            self.expanded = true;
        } else if !self.pinned_open {
            self.expanded = false;
        }
    }

    pub fn is_held(&self) -> bool {
        self.held
    }

    pub fn is_expanded(&self) -> bool {
        self.expanded && !self.hidden
    }

    pub fn is_hidden(&self) -> bool {
        self.hidden
    }

    /// Advance by `dt` with the current pointer sample.
    pub fn tick(&mut self, dt: Duration, pointer: Pointer) {
        if self.held {
            self.idle = Duration::ZERO;
            self.outside = Duration::ZERO;
            return;
        }
        if pointer.over || pointer.near || self.pinned_open {
            self.idle = Duration::ZERO;
        } else {
            self.idle += dt;
        }
        self.peeking = pointer.over && self.expand_on == ExpandOn::Peek;
        if pointer.over {
            self.outside = Duration::ZERO;
            if self.expand_on == ExpandOn::Hover && !self.hidden {
                self.expanded = true;
            }
        } else {
            self.outside += dt;
            if self.expanded && !self.pinned_open && self.outside >= COLLAPSE_GRACE {
                self.expanded = false;
            }
        }
    }

    /// Click on the pill background: pins it open, or closes it if already pinned.
    pub fn click(&mut self) {
        self.idle = Duration::ZERO;
        self.hidden = false;
        if self.expand_on == ExpandOn::Peek {
            // Peek → full dashboard, closed again by leaving (or another click).
            if self.expanded {
                self.collapse();
            } else {
                self.expanded = true;
            }
            return;
        }
        if self.pinned_open {
            self.collapse();
        } else {
            self.expanded = true;
            self.pinned_open = true;
        }
    }

    /// Esc / click outside.
    pub fn collapse(&mut self) {
        self.expanded = false;
        self.pinned_open = false;
    }

    /// Global shortcut: hidden → shown+expanded → collapsed+shown.
    pub fn toggle_from_hotkey(&mut self) {
        self.idle = Duration::ZERO;
        if self.hidden {
            self.hidden = false;
            self.expanded = true;
            self.pinned_open = true;
        } else if self.expanded {
            self.collapse();
        } else {
            self.expanded = true;
            self.pinned_open = true;
        }
    }

    pub fn set_hidden(&mut self, hidden: bool) {
        self.hidden = hidden;
        if hidden {
            self.collapse();
        }
    }

    /// Target opacity for the fade spring.
    pub fn target_opacity(&self) -> f32 {
        if self.hidden {
            return 0.0;
        }
        match self.auto_hide {
            AutoHide::AlwaysVisible => 1.0,
            AutoHide::FadeAfter { .. } if self.expanded => 1.0,
            AutoHide::FadeAfter { secs } if self.idle >= Duration::from_secs(secs as u64) => {
                tokens::OVERLAY_FADED_OPACITY
            }
            AutoHide::FadeAfter { .. } => 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Rect = Rect::new(0, 0, 1920, 1040);

    #[test]
    fn orientation_follows_anchor() {
        assert_eq!(Anchor::LeftCenter.orientation(), Orientation::Vertical);
        assert_eq!(Anchor::RightCenter.orientation(), Orientation::Vertical);
        for a in [Anchor::TopCenter, Anchor::BottomLeft, Anchor::BottomRight, Anchor::TopLeft] {
            assert_eq!(a.orientation(), Orientation::Horizontal);
        }
    }

    #[test]
    fn top_center_notch_is_flush_with_top_and_centered() {
        let r = window_rect(Anchor::TopCenter, WORK, 1.0, (200.0, 32.0));
        assert_eq!(r.w, 200 + 2 * m::EAR as i32);
        assert_eq!(r.h, 32);
        assert_eq!(r.y, 0, "flush with the top edge");
        assert_eq!(r.x + r.w / 2, 960);
    }

    #[test]
    fn every_anchor_is_flush_with_its_edge_and_on_screen() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for a in Anchor::ALL {
                for expanded in [false, true] {
                    let body = notch_size(a.orientation(), expanded, 160.0);
                    let r = window_rect(a, WORK, scale, body);
                    assert!(r.x >= 0 && r.y >= 0, "{a:?} {scale}");
                    assert!(r.x + r.w <= WORK.w && r.y + r.h <= WORK.h, "{a:?} {scale}");
                    match a.edge() {
                        Edge::Top => assert_eq!(r.y, 0),
                        Edge::Bottom => assert_eq!(r.y + r.h, WORK.h),
                        Edge::Left => assert_eq!(r.x, 0),
                        Edge::Right => assert_eq!(r.x + r.w, WORK.w),
                    }
                }
            }
        }
    }

    #[test]
    fn secondary_monitor_offset_is_respected() {
        let work = Rect::new(-1280, 200, 1280, 984);
        let r = window_rect(Anchor::BottomRight, work, 1.0, (160.0, 32.0));
        assert!(r.x < 0 && r.x + r.w <= 0);
        assert_eq!(r.y + r.h, 200 + 984);
    }

    #[test]
    fn outline_is_closed_square_on_edge_and_within_window() {
        for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            let size = match edge {
                Edge::Top | Edge::Bottom => (224.0, 32.0),
                _ => (40.0, 224.0),
            };
            let (start, segs) = notch_outline(edge, size, m::RADIUS_COMPACT);
            let end = match segs.last().unwrap() {
                Seg::Line(p) | Seg::Quad(_, p) => *p,
            };
            assert_eq!(start, end, "{edge:?} closed");
            for s in &segs {
                let (Seg::Line(p) | Seg::Quad(_, p)) = s;
                assert!(p.0 >= 0.0 && p.0 <= size.0 && p.1 >= 0.0 && p.1 <= size.1, "{edge:?} {p:?}");
            }
        }
        // Top notch: the closing segment runs along y = 0 (square against the edge).
        let (_, segs) = notch_outline(Edge::Top, (224.0, 32.0), 12.0);
        assert!(matches!(segs[6], Seg::Quad(_, (x, y)) if x == 224.0 && y == 0.0));
    }

    #[test]
    fn fades_after_idle_and_recovers_on_hover() {
        let mut s = OverlayState::new(AutoHide::FadeAfter { secs: 5 }, ExpandOn::Hover);
        s.tick(Duration::from_secs(4), Pointer::default());
        assert_eq!(s.target_opacity(), 1.0);
        s.tick(Duration::from_secs(2), Pointer::default());
        assert_eq!(s.target_opacity(), tokens::OVERLAY_FADED_OPACITY);
        s.tick(Duration::from_millis(16), Pointer { over: false, near: true });
        assert_eq!(s.target_opacity(), 1.0);
    }

    #[test]
    fn always_visible_never_fades() {
        let mut s = OverlayState::new(AutoHide::AlwaysVisible, ExpandOn::Hover);
        s.tick(Duration::from_secs(600), Pointer::default());
        assert_eq!(s.target_opacity(), 1.0);
    }

    #[test]
    fn hover_expands_and_leaving_collapses_immediately() {
        let mut s = OverlayState::new(AutoHide::default(), ExpandOn::Hover);
        s.tick(Duration::from_millis(16), Pointer { over: true, near: true });
        assert!(s.is_expanded());
        s.tick(Duration::from_millis(16), Pointer { over: false, near: true });
        assert!(!s.is_expanded(), "collapses on the first sample outside");
    }

    #[test]
    fn peek_mode_hover_previews_click_opens_leave_closes() {
        let mut s = OverlayState::new(AutoHide::default(), ExpandOn::Peek);
        let over = Pointer { over: true, near: true };
        s.tick(Duration::from_millis(16), over);
        assert!(s.is_peeking() && !s.is_expanded(), "hover = peek only");
        s.click();
        assert!(s.is_expanded() && !s.is_peeking(), "click opens the dashboard");
        s.tick(Duration::from_millis(16), over);
        assert!(s.is_expanded());
        s.tick(Duration::from_millis(16), Pointer { over: false, near: true });
        assert!(!s.is_expanded() && !s.is_peeking(), "leaving closes everything");
    }

    #[test]
    fn file_drag_holds_the_notch_open_even_in_click_mode() {
        let mut s = OverlayState::new(AutoHide::default(), ExpandOn::Click);
        s.hold_open(true);
        assert!(s.is_expanded());
        s.tick(Duration::from_secs(2), Pointer::default());
        assert!(s.is_expanded(), "pointer outside while dragging: still open");
        s.hold_open(false);
        assert!(!s.is_expanded());
    }

    #[test]
    fn leaving_does_not_collapse_a_pinned_notch() {
        let mut s = OverlayState::new(AutoHide::default(), ExpandOn::Hover);
        s.click();
        s.tick(Duration::from_millis(16), Pointer::default());
        assert!(s.is_expanded());
    }

    #[test]
    fn click_mode_only_expands_on_click_and_stays_open() {
        let mut s = OverlayState::new(AutoHide::default(), ExpandOn::Click);
        s.tick(Duration::from_millis(16), Pointer { over: true, near: true });
        assert!(!s.is_expanded());
        s.click();
        assert!(s.is_expanded());
        s.tick(Duration::from_secs(3), Pointer::default());
        assert!(s.is_expanded(), "pinned");
        s.click();
        assert!(!s.is_expanded());
    }

    #[test]
    fn hotkey_reveals_hidden_overlay() {
        let mut s = OverlayState::new(AutoHide::default(), ExpandOn::Hover);
        s.set_hidden(true);
        assert_eq!(s.target_opacity(), 0.0);
        s.toggle_from_hotkey();
        assert!(!s.is_hidden() && s.is_expanded());
        s.toggle_from_hotkey();
        assert!(!s.is_expanded());
    }

    #[test]
    fn rect_distance() {
        let r = Rect::new(10, 10, 10, 10);
        assert_eq!(r.distance(15, 15), 0.0);
        assert_eq!(r.distance(25, 15), 6.0);
    }
}
