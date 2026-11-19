//! M1 — the "Halo" notch: a solid black shape attached to a screen edge.
//!
//! Window strategy: borderless, transparent, topmost, non-activating window sized to
//! the notch (+ ears). It grows before an expansion and shrinks once a collapse has
//! settled. Win32 calls that move/resize windows run *outside* GPUI entity updates
//! (see `Deferred`) to avoid re-entrancy into GPUI's window procedure.
//!
//! The overlay loop also drives the other floating surfaces (spotlight, colour
//! picker) and opens the hub window on request.

use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use halo_core::config::ThemePref;
use halo_core::overlay::{self as geo, Anchor, Edge, Orientation, OverlayState, Pointer, Rect, Seg};
use halo_modules::system::Snapshot;
use halo_ui::hsla;
use halo_ui::motion::{Spring, lerp};
use halo_ui::tokens::{self, MOTION_SPRING, Mode, overlay as m};

use crate::state::{Section, Shared};
use crate::tray::{Action, Hotkeys, Tray};
use crate::ui;

mod controls;
mod dashboard;
mod drop;
pub use dashboard::TABS;

const POLL: Duration = Duration::from_millis(50);
const POLL_FAST: Duration = Duration::from_millis(16);
const THEME_POLL: Duration = Duration::from_secs(1);

pub struct Overlay {
    shared: Shared,
    hwnd: Option<isize>,
    styled: bool,
    state: OverlayState,
    expand: Spring,
    presence: Spring,
    last_frame: Instant,
    last_theme_poll: Instant,
    placed: Option<Rect>,
    hit_rect: Option<Rect>,
    seen_generation: u64,
    snap: Snapshot,
    chooser: halo_modules::accent::Chooser,
    tray: Option<Tray>,
    hotkeys: Option<Hotkeys>,
    floating: crate::floating::Floating,
    /// Debug automation: ignore the real cursor (tests must not depend on it).
    ignore_pointer: bool,
    /// Transient notification shown in the notch: (icon, text, until).
    toast: Option<(&'static str, String, Instant)>,
    last_reminder_check: Instant,
    last_distraction: Option<(String, Instant)>,
    fullscreen_hidden: bool,
    /// Mouse press on the notch: (cursor at press, dragging?). Resolved in 	ick.
    press: Option<((i32, i32), bool)>,
    /// Do Not Disturb as last shown (drives the moon indicator).
    dnd_shown: bool,
    /// Volume / brightness / radios, HUD and slider drags.
    controls: controls::Controls,
    /// Peek preview (Peek mode): 0 = compact, 1 = peek.
    peek: Spring,
    /// Debug automation: show the peek view without a real pointer.
    dev_peek: bool,
    /// Debug automation: animation speed (1 = normal; `slowmo 0.05` to inspect frames).
    time_scale: f32,
    /// Elevated Halo: a non-elevated helper receives file drops for the notch.
    drop_ipc: Option<std::path::PathBuf>,
    /// Last notch area sent to the helper.
    drop_area: Option<halo_core::dropipc::NotchArea>,
    /// Zone under a file drag reported by the helper.
    ext_zone: Option<halo_core::dropipc::DropZone>,
    /// Next automatic update check, and the version already announced.
    next_update_check: Instant,
    announced_update: Option<String>,
}

/// Win32 work to perform once the entity update has returned.
pub enum Deferred {
    Style(isize, bool),
    Place(isize, Rect),
    ClickThrough(isize, bool),
    /// Focusable frameless popup (command palette).
    Popup(isize),
}

pub fn run_deferred(list: Vec<Deferred>) {
    for d in list {
        match d {
            Deferred::Style(h, click_through) => {
                halo_platform::style_overlay_window(h);
                if click_through {
                    halo_platform::set_click_through(h, true);
                }
            }
            Deferred::Place(h, r) => halo_platform::place_window(h, r),
            Deferred::ClickThrough(h, on) => halo_platform::set_click_through(h, on),
            Deferred::Popup(h) => halo_platform::style_popup_window(h),
        }
    }
}

pub fn floating_options() -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(0.0), px(0.0)), size(px(160.0), px(40.0))))),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        window_decorations: Some(WindowDecorations::Client),
        ..Default::default()
    }
}

pub fn open(cx: &mut App, shared: Shared) -> anyhow::Result<()> {
    cx.open_window(floating_options(), |window, cx| {
        let hwnd = raw_hwnd(window);
        if hwnd.is_none() {
            log::error!("no Win32 handle for the overlay window — placement disabled");
        }
        cx.new(|cx| Overlay::new(shared, hwnd, cx))
    })?;
    Ok(())
}

/// When Halo runs elevated, Windows blocks drag-and-drop from Explorer into its
/// windows: start the non-elevated drop helper and return the folder used to talk to it.
fn start_drop_helper() -> Option<std::path::PathBuf> {
    if !halo_platform::is_elevated() {
        return None; // drops reach the notch directly
    }
    let dir = halo_core::config::Config::dir().join("ipc");
    let _ = std::fs::create_dir_all(&dir);
    // Stale files from a previous run.
    let _ = std::fs::remove_file(dir.join(halo_core::dropipc::DRAG_FILE));
    let _ = halo_core::dropipc::take_drops(&dir);
    let exe = std::env::current_exe().ok()?;
    let args = format!("--drop-helper {} \"{}\"", std::process::id(), dir.display());
    match halo_platform::spawn_unelevated(&exe, &args) {
        Some(pid) => log::info!("drop helper started (pid {pid})"),
        None => log::warn!("drop helper could not start: dropping files on the notch is unavailable"),
    }
    Some(dir)
}

pub fn raw_hwnd(window: &Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

impl Overlay {
    fn new(shared: Shared, hwnd: Option<isize>, cx: &mut Context<Self>) -> Self {
        let (tray, hotkeys, auto_hide, expand_on) = {
            let s = shared.borrow();
            let tray = Tray::new(&s).map_err(|e| log::error!("tray: {e:#}")).ok();
            let hotkeys = Hotkeys::new(&s).map_err(|e| log::error!("hotkeys: {e:#}")).ok();
            (tray, hotkeys, s.config.overlay.auto_hide, s.config.overlay.expand_on)
        };

        cx.spawn(async move |this, cx| {
            let mut period = POLL;
            loop {
                cx.background_executor().timer(period).await;
                let Ok((deferred, fast)) = this.update(cx, |view, cx| view.tick(cx)) else { break };
                run_deferred(deferred);
                period = if fast { POLL_FAST } else { POLL };
            }
        })
        .detach();

        Self {
            shared,
            hwnd,
            styled: false,
            state: OverlayState::new(auto_hide, expand_on),
            expand: Spring::new(MOTION_SPRING, 0.0),
            presence: Spring::new(MOTION_SPRING, 1.0),
            last_frame: Instant::now(),
            last_theme_poll: Instant::now(),
            placed: None,
            hit_rect: None,
            seen_generation: u64::MAX,
            snap: Snapshot::default(),
            chooser: Default::default(),
            tray,
            hotkeys,
            floating: Default::default(),
            ignore_pointer: false,
            toast: None,
            last_reminder_check: Instant::now(),
            last_distraction: None,
            fullscreen_hidden: false,
            press: None,
            dnd_shown: false,
            controls: controls::Controls::new(),
            peek: Spring::new(MOTION_SPRING, 0.0),
            dev_peek: false,
            time_scale: 1.0,
            drop_ipc: start_drop_helper(),
            drop_area: None,
            ext_zone: None,
            // First check shortly after startup (not during it).
            next_update_check: Instant::now() + Duration::from_secs(20),
            announced_update: None,
        }
    }

    fn anchor(&self) -> Anchor {
        self.shared.borrow().config.overlay.anchor
    }

    fn indicators(&self) -> Vec<String> {
        let list = self.shared.borrow().config.overlay.indicators.clone();
        list.into_iter().filter(|i| i != "battery" || self.snap.battery.present).collect()
    }

    fn compact_along(&self) -> f32 {
        let n = self.indicators().len().max(1) as f32;
        let live = match (&self.toast, self.live_activity()) {
            _ if self.controls.hud.is_some() => 270.0,
            _ if self.recording().is_some() => 230.0,
            (Some(_), _) => 300.0,
            (None, Some(("audio-lines", _))) => 190.0,
            (None, Some(_)) => 150.0,
            (None, None) => 0.0,
        };
        match self.anchor().orientation() {
            Orientation::Horizontal => 2.0 * m::PAD_X + n * m::INDICATOR_W + live,
            Orientation::Vertical => 2.0 * 12.0 + n * m::INDICATOR_V_H,
        }
    }

    /// Body size for the current mode, before animation (`big` = expanded layout).
    fn body(&self, big: bool) -> (f32, f32) {
        if self.chooser.visible {
            let n = self.chooser.options.len() as f32;
            return match self.anchor().orientation() {
                Orientation::Horizontal => ((2.0 * m::PAD_X + n * 40.0).max(self.compact_along()), 52.0),
                Orientation::Vertical => (2.0 * 12.0 + n * 40.0, 56.0),
            };
        }
        let (along, depth) = geo::notch_size(self.anchor().orientation(), big, self.compact_along());
        // Dashboard without cards (nothing playing, no tasks…): only header + command bar.
        if big
            && !self.state.is_held()
            && self.anchor().orientation() == Orientation::Horizontal
            && self.visible_cards() == 0
            && !self.has_shelf()
        {
            return (along, 104.0);
        }
        (along, depth)
    }

    /// One poll step. Returns Win32 work to run after the update, and whether to poll fast.
    fn tick(&mut self, cx: &mut Context<Self>) -> (Vec<Deferred>, bool) {
        let mut out = Vec::new();
        let mut dirty = false;
        let Some(hwnd) = self.hwnd else { return (out, false) };
        let click_through = self.shared.borrow().config.overlay.click_through;

        if !self.styled {
            out.push(Deferred::Style(hwnd, false));
            self.styled = true;
        }

        // Tray + hotkey actions.
        let mut actions = Vec::new();
        if let Some(t) = &self.tray {
            t.poll(&mut actions);
        }
        if let Some(h) = &self.hotkeys {
            h.poll(&mut actions);
        }
        for a in actions {
            dirty = true;
            self.handle(a, cx);
        }
        for cmd in crate::devcmd::take() {
            dirty = true;
            self.dev_command(&cmd, cx);
        }

        // Theme follows Windows when configured so.
        if self.last_theme_poll.elapsed() >= THEME_POLL {
            self.last_theme_poll = Instant::now();
            let mut s = self.shared.borrow_mut();
            if s.config.theme == ThemePref::System {
                let mode = crate::state::resolve_mode(ThemePref::System);
                if mode != s.mode {
                    s.mode = mode;
                    s.generation += 1;
                }
            }
        }

        // Config changed: resync.
        let generation = self.shared.borrow().generation;
        if generation != self.seen_generation {
            self.seen_generation = generation;
            // Microphone used by recordings.
            halo_capture::set_microphone(Some(self.shared.borrow().config.settings.mic_device.clone()));
            let (mode, cfg) = {
                let s = self.shared.borrow();
                (s.mode, s.config.overlay.clone())
            };
            self.state.auto_hide = cfg.auto_hide;
            let (fm, bm) = {
                let s = &self.shared.borrow().config.settings;
                (s.focus_minutes, s.break_minutes)
            };
            self.shared.borrow().handles.focus.with(|f| {
                f.focus_len = Duration::from_secs(fm as u64 * 60);
                f.break_len = Duration::from_secs(bm as u64 * 60);
                if f.phase() == halo_modules::focus::Phase::Idle {
                    f.reset();
                }
            });
            self.state.expand_on = cfg.expand_on;
            self.placed = None;
            halo_ui::apply_theme(cx, mode);
            if let Some(t) = &self.tray {
                t.sync(&self.shared.borrow());
            }
            cx.refresh_windows();
            dirty = true;
        }

        // Live data.
        let (snap, chooser) = {
            let s = self.shared.borrow();
            (s.handles.system.get(), s.handles.accent.get())
        };
        if snap.seq != self.snap.seq {
            self.snap = snap;
            dirty = true;
        }
        if chooser != self.chooser {
            self.chooser = chooser;
            self.placed = None;
            dirty = true;
        }

        // Pointer sampling against the notch.
        let (proximity, display) = {
            let c = &self.shared.borrow().config.overlay;
            (c.proximity_px, c.display)
        };
        let monitor = halo_platform::monitor_for(display);
        let cursor = if self.ignore_pointer { None } else { halo_platform::cursor_pos() };
        let pointer = match (cursor, self.hit_rect) {
            (Some((x, y)), Some(r)) => {
                Pointer { over: r.contains(x, y), near: r.distance(x, y) <= proximity * monitor.scale }
            }
            _ => Pointer::default(),
        };
        // Hide while an app is fullscreen (videos, games, slides); restore afterwards.
        let fullscreen = !self.ignore_pointer && halo_platform::foreground_is_fullscreen();
        if fullscreen != self.fullscreen_hidden {
            self.fullscreen_hidden = fullscreen;
            self.state.set_hidden(fullscreen);
            self.sync_springs();
            dirty = true;
        }
        let was = (self.state.is_expanded(), self.state.target_opacity(), self.peeking());
        self.state.tick(POLL, pointer);
        // Volume / brightness changes → HUD in the compact notch.
        let hud_on = self.shared.borrow().config.overlay.hud;
        if self.controls.poll(hud_on, self.state.is_expanded()) {
            self.placed = None;
            dirty = true;
            if self.controls.hud.is_some() {
                // Wake from the slim/idle state so the HUD is visible.
                self.state.tick(Duration::ZERO, Pointer { over: false, near: true });
            }
        }
        // Updates: check at startup and once a day; announce a new version once.
        if Instant::now() >= self.next_update_check {
            self.next_update_check = Instant::now() + Duration::from_secs(24 * 3600);
            if self.shared.borrow().config.settings.update_check {
                self.shared.borrow().update.check();
            }
        }
        let update = self.shared.borrow().update.status();
        if let halo_modules::update::Status::Available(r) = update {
            if self.announced_update.as_deref() != Some(r.version.as_str()) {
                self.announced_update = Some(r.version.clone());
                let msg = self.shared.borrow().tf("update.toast", &[("v", &r.version)]);
                self.notify_toast("download", msg);
                dirty = true;
            }
        }
        // Elevated: the drop helper reports file drags over the notch and the drops.
        if let Some(dir) = self.drop_ipc.clone() {
            use halo_core::dropipc::{self, NotchArea};
            let horizontal = self.anchor().orientation() == Orientation::Horizontal;
            let size = geo::notch_size(self.anchor().orientation(), true, self.compact_along());
            let r = geo::window_rect(self.anchor(), monitor.work_area, monitor.scale, size);
            let area = NotchArea { x: r.x, y: r.y, w: r.w, h: r.h, vertical: !horizontal };
            if self.drop_area != Some(area) {
                self.drop_area = Some(area);
                let _ = dropipc::write_atomic(&dir.join(dropipc::AREA_FILE), &area.to_line());
            }
            let zone = std::fs::read_to_string(dir.join(dropipc::DRAG_FILE)).ok().and_then(|s| dropipc::DropZone::parse(&s));
            if zone != self.ext_zone {
                self.ext_zone = zone;
                dirty = true;
            }
            for (z, paths) in dropipc::take_drops(&dir) {
                let zone = match z {
                    dropipc::DropZone::Vault => drop::Zone::Vault,
                    dropipc::DropZone::Shelf => drop::Zone::Shelf,
                };
                self.drop_files(zone, paths, cx);
                dirty = true;
            }
        }
        // A file dragged from Explorer onto (or near) the notch opens the drop zones.
        let file_drag = (cx.has_active_drag() && pointer.near) || self.ext_zone.is_some();
        if !self.ignore_pointer && file_drag != self.state.is_held() {
            self.state.hold_open(file_drag);
        }
        if self.chooser.visible {
            self.state.tick(Duration::ZERO, Pointer { over: false, near: true });
        }
        if (self.state.is_expanded(), self.state.target_opacity(), self.peeking()) != was {
            self.sync_springs();
            dirty = true;
        }
        if click_through {
            out.push(Deferred::ClickThrough(hwnd, !pointer.over));
        }

        // Window size: grow before expanding, shrink once the collapse has settled.
        let big = self.state.is_expanded() || self.expand.value() > 0.001;
        let body = if !big && (self.peeking() || self.peek.value() > 0.001) {
            let (pa, pd) = self.peek_size();
            let (ca, cd) = self.body(false);
            (pa.max(ca), pd.max(cd))
        } else {
            self.body(big)
        };
        let rect = geo::window_rect(self.anchor(), monitor.work_area, monitor.scale, body);
        if self.placed != Some(rect) {
            self.placed = Some(rect);
            out.push(Deferred::Place(hwnd, rect));
            dirty = true;
        }
        self.hit_rect = Some(rect);

        // Drag to reposition: snap to the nearest anchor while the button is held.
        let mut dragging = false;
        if let Some((start, moved)) = self.press {
            let held = halo_platform::key_down(0x01);
            let cur = halo_platform::cursor_pos().unwrap_or(start);
            let far = (cur.0 - start.0).abs() + (cur.1 - start.1).abs() > (24.0 * monitor.scale) as i32;
            if held && (moved || far) {
                dragging = true;
                self.press = Some((start, true));
                let wa = monitor.work_area;
                let fx = (cur.0 - wa.x) as f32 / wa.w.max(1) as f32;
                let fy = (cur.1 - wa.y) as f32 / wa.h.max(1) as f32;
                let anchor = nearest_anchor(fx, fy);
                if anchor != self.anchor() {
                    self.shared.borrow_mut().config.overlay.anchor = anchor;
                    self.placed = None;
                    dirty = true;
                }
            } else if !held {
                if moved {
                    // Persist the new position once.
                    let a = self.anchor();
                    self.shared.borrow_mut().update_config(|c| c.overlay.anchor = a);
                } else {
                    self.state.click();
                    self.sync_springs();
                }
                self.press = None;
                dirty = true;
            }
        }

        // Other floating surfaces (spotlight, picker) and the hub.
        let mut fast = self.floating.tick(&self.shared, cx, &mut out) || dragging;
        if let Some((purpose, rect)) = self.floating.tick_region(&self.shared, cx, &mut out) {
            self.on_region(purpose, rect);
            dirty = true;
        }
        fast |= self.floating.region_active();
        let want_palette = std::mem::take(&mut self.shared.borrow_mut().want_palette);
        if want_palette {
            if let Some(h) = crate::palette::open(cx, self.shared.clone()) {
                out.push(Deferred::Popup(h));
            }
        }
        if self.toast.as_ref().is_some_and(|t| Instant::now() >= t.2) {
            self.toast = None;
            self.placed = None;
            dirty = true;
        }
        if self.last_reminder_check.elapsed() >= Duration::from_secs(2) {
            self.last_reminder_check = Instant::now();
            let due = self.shared.borrow().handles.tasks.as_ref().map(|t| t.take_due_reminders(halo_modules::store::now()));
            let dnd = self.dnd();
            if let Some(r) = due.and_then(|d| d.into_iter().next()).filter(|_| !dnd) {
                self.notify_toast("bell", r.text);
                dirty = true;
            }
            let fr = self.shared.borrow().fr();
            let ended = self.shared.borrow().handles.focus.with(|f| f.tick(Instant::now()));
            if let Some(phase) = ended {
                let (mins, pause_media) = {
                    let s = &self.shared.borrow().config.settings;
                    (s.focus_minutes, s.focus_pause_media)
                };
                if phase == halo_modules::focus::Phase::Focus {
                    if let Some(t) = &self.shared.borrow().handles.tasks {
                        t.log_focus(mins);
                    }
                    if pause_media && self.shared.borrow().handles.media.get().is_some_and(|n| n.playing) {
                        self.shared.borrow().handles.media.send(halo_platform::media::Command::Toggle);
                    }
                }
                let msg = match (phase, fr) {
                    (halo_modules::focus::Phase::Focus, true) => "Bravo ! Pause de 5 min",
                    (halo_modules::focus::Phase::Focus, false) => "Well done! 5 min break",
                    (_, true) => "Pause terminée, on s'y remet",
                    (_, false) => "Break over, back to focus",
                };
                self.notify_toast("timer", msg.into());
                dirty = true;
            }
            if self.live_activity().is_some() {
                dirty = true; // keep the compact timer ticking
            }
            // Windows notifications mirrored in the notch (unless Do Not Disturb).
            let notify = self.shared.borrow().handles.notify.clone();
            if dnd {
                notify.clear_pending();
            } else if self.toast.is_none() {
                if let Some(t) = notify.pop() {
                    let text = if t.body.is_empty() { format!("{} · {}", t.app, t.title) } else { format!("{} · {} — {}", t.app, t.title, t.body) };
                    self.notify_toast("message-square", text);
                }
            }
            if dnd != self.dnd_shown {
                self.dnd_shown = dnd;
                self.placed = None;
            }
            // Productivity mode: gentle reminder when a distraction is in front during focus.
            let in_focus = self.shared.borrow().handles.focus.with(|f| f.phase() == halo_modules::focus::Phase::Focus && f.running());
            if in_focus {
                let fg = halo_platform::proc::foreground_window();
                let exe = halo_platform::proc::process_exe(halo_platform::proc::window_pid(fg)).unwrap_or_default();
                let listed = self.shared.borrow().config.settings.distraction_apps.iter().any(|a| a.eq_ignore_ascii_case(&exe));
                let cooled = self.last_distraction.as_ref().is_none_or(|(app, at)| app != &exe || at.elapsed() >= Duration::from_secs(90));
                if listed && cooled {
                    self.last_distraction = Some((exe.clone(), Instant::now()));
                    let name = exe.trim_end_matches(".exe").to_string();
                    let msg = self.shared.borrow().tf("toast.distraction", &[("app", &name)]);
                    self.notify_toast("shield", msg);
                    dirty = true;
                }
            }
        }
        let want_hub = std::mem::take(&mut self.shared.borrow_mut().want_hub);
        if want_hub {
            crate::hub::open(cx, self.shared.clone());
        }

        if dirty {
            self.last_frame = Instant::now();
            cx.notify();
        }
        (out, fast)
    }

    /// Peek preview active (Peek mode, horizontal notch).
    fn peeking(&self) -> bool {
        (self.state.is_peeking() || (self.ignore_pointer && self.dev_peek && !self.state.is_expanded()))
            && self.anchor().orientation() == Orientation::Horizontal
            && !self.chooser.visible
    }

    fn sync_springs(&mut self) {
        self.expand.set_target(if self.state.is_expanded() { 1.0 } else { 0.0 });
        self.peek.set_target(if self.peeking() { 1.0 } else { 0.0 });
        self.presence.set_target(self.state.target_opacity());
    }

    fn handle(&mut self, action: Action, cx: &mut Context<Self>) {
        match action {
            Action::ToggleOverlay => {
                let hidden = !self.state.is_hidden();
                self.state.set_hidden(hidden);
            }
            Action::HotkeyToggle => self.state.toggle_from_hotkey(),
            Action::Picker => self.shared.borrow_mut().want_picker = true,
            Action::Palette => self.shared.borrow_mut().want_palette = true,
            Action::Screenshot => self.shared.borrow_mut().want_region = Some(crate::state::RegionPurpose::Screenshot),
            Action::Record => self.toggle_recording(),
            Action::SetAnchor(a) => self.shared.borrow_mut().update_config(|c| c.overlay.anchor = a),
            Action::SetTheme(p) => self.shared.borrow_mut().update_config(|c| c.theme = p),
            Action::OpenSettings => self.shared.borrow_mut().open_hub(Section::General),
            Action::Quit => {
                self.shared.borrow_mut().registry.shutdown();
                cx.quit();
            }
        }
        self.sync_springs();
    }

    /// Debug automation (see `devcmd`): drive the UI without touching the mouse/keyboard.
    fn dev_command(&mut self, cmd: &str, cx: &mut Context<Self>) {
        log::info!("devcmd: {cmd}");
        let mut it = cmd.split_whitespace();
        let (verb, arg) = (it.next().unwrap_or_default(), it.next().unwrap_or_default());
        match verb {
            "expand" => {
                if !self.state.is_expanded() {
                    self.state.toggle_from_hotkey();
                }
            }
            "collapse" => self.state.collapse(),
            "pointer" => self.ignore_pointer = arg == "off",
            "idle" => self.state.tick(Duration::from_secs(60), Pointer::default()),
            "hide" => self.state.set_hidden(true),
            "show" => self.state.set_hidden(false),
            "spotlight" => {
                let c = halo_platform::cursor_pos().unwrap_or_default();
                self.shared.borrow().handles.spotlight.trigger(c);
            }
            "picker" => self.shared.borrow_mut().want_picker = true,
            "palette" => self.shared.borrow_mut().want_palette = true,
            "shot" => self.shared.borrow_mut().want_region = Some(crate::state::RegionPurpose::Screenshot),
            "focus" => self.shared.borrow().handles.focus.with(|f| f.start(Instant::now())),
            "task" | "remind" => {
                let text = cmd.splitn(2, ' ').nth(1).unwrap_or_default().to_string();
                if let Some(t) = &self.shared.borrow().handles.tasks {
                    let due = (verb == "remind").then(|| halo_modules::store::now() + 3);
                    t.add(&text, due, verb == "remind");
                }
            }
            "slowmo" => self.time_scale = arg.parse::<f32>().unwrap_or(1.0).clamp(0.01, 1.0),
            "peek" => {
                self.dev_peek = arg == "on";
                self.placed = None;
                self.state.tick(Duration::ZERO, Pointer { over: false, near: true });
            }
            // `hud volume|brightness`: show the HUD as if the value had changed.
            "hud" => {
                let ctl = if arg == "brightness" { controls::Ctl::Brightness } else { controls::Ctl::Volume };
                self.controls.hud = Some((ctl, Instant::now() + Duration::from_secs(4)));
                self.placed = None;
                self.state.tick(Duration::ZERO, Pointer { over: false, near: true });
            }
            // `dropview on|off`: show the drop zones as if a file were dragged over.
            "dropview" => self.state.hold_open(arg == "on"),
            // `drop shelf|vault <path>`: simulate dropping a file on a zone.
            "drop" => {
                let rest = cmd.splitn(3, ' ').nth(2).unwrap_or_default();
                let zone = if arg == "vault" { drop::Zone::Vault } else { drop::Zone::Shelf };
                self.drop_files(zone, vec![std::path::PathBuf::from(rest)], cx);
            }
            // `note Folder|# Title\nBody` (literal \n for new lines).
            "note" => {
                let text = cmd.splitn(2, ' ').nth(1).unwrap_or_default();
                let (folder, body) = text.split_once('|').unwrap_or(("", text));
                if let Some(n) = &self.shared.borrow().handles.notes {
                    n.create(&body.replace("\\n", "\n"), folder);
                }
            }
            "cards" => {
                let list: Vec<String> = arg.split(',').map(String::from).collect();
                self.shared.borrow_mut().update_config(|c| c.overlay.cards = list);
            }
            "notchtheme" => {
                let id = arg.to_string();
                self.shared.borrow_mut().update_config(|c| c.overlay.notch_theme = id);
            }
            "theme" => {
                let pref = match arg {
                    "light" => ThemePref::Light,
                    "dark" => ThemePref::Dark,
                    _ => ThemePref::System,
                };
                self.shared.borrow_mut().update_config(|c| c.theme = pref);
            }
            "anchor" => {
                if let Some(a) = Anchor::ALL.iter().find(|a| a.label_key().ends_with(arg)) {
                    let a = *a;
                    self.shared.borrow_mut().update_config(|c| c.overlay.anchor = a);
                }
            }
            "hub" => {
                let section = crate::hub::NAV
                    .iter()
                    .find(|(s, ..)| format!("{s:?}").eq_ignore_ascii_case(arg))
                    .map(|(s, ..)| *s)
                    .unwrap_or(Section::General);
                self.shared.borrow_mut().open_hub(section);
            }
            "hubscroll" => {
                self.shared.borrow_mut().want_hub_scroll = arg.parse().ok();
                cx.refresh_windows();
            }
            "closehub" => {
                if let Some(h) = self.shared.borrow_mut().hub_window.take() {
                    let _ = h.update(cx, |_, window, _| window.remove_window());
                }
            }
            "enable" | "disable" => {
                let on = verb == "enable";
                let id = arg.to_string();
                self.shared.borrow_mut().update_config(|c| {
                    c.modules.insert(id, on);
                });
            }
            "quit" => {
                self.shared.borrow_mut().registry.shutdown();
                cx.quit();
            }
            _ => log::warn!("unknown devcmd {cmd}"),
        }
        self.sync_springs();
    }

    /// Do Not Disturb right now (manual, during focus, or scheduled).
    pub fn dnd(&self) -> bool {
        let s = self.shared.borrow();
        let focus = s.handles.focus.with(|f| f.phase() == halo_modules::focus::Phase::Focus && f.running());
        let now = chrono::Local::now();
        let minute = chrono::Timelike::hour(&now) * 60 + chrono::Timelike::minute(&now);
        halo_modules::notify::dnd_active(&s.config, focus, minute)
    }

    /// A region was selected: take the screenshot or start recording it.
    fn on_region(&mut self, purpose: crate::state::RegionPurpose, rect: Rect) {
        let (shot, video) = {
            let s = self.shared.borrow();
            (s.capture.screenshot.clone(), s.capture.video.clone())
        };
        match purpose {
            crate::state::RegionPurpose::Screenshot => match shot.capture_region(rect) {
                Ok(_) => {
                    let msg = self.shared.borrow().t("capture.saved");
                    self.notify_toast("camera", msg);
                }
                Err(e) => self.notify_toast("camera", e.to_string()),
            },
            crate::state::RegionPurpose::Record => {
                let (fps, mic, system) = {
                    let s = &self.shared.borrow().config.settings;
                    (s.capture_fps, s.capture_mic, s.capture_system)
                };
                let effects = crate::panels4::recording_effects(&self.shared.borrow().config.settings);
                if let Err(e) = video.start(rect, fps, halo_capture::AudioOpts { mic, system }, effects) {
                    self.notify_toast("video", e.to_string());
                }
            }
        }
    }

    /// Notch tab / hotkey: start a region recording, or stop the current one.
    pub fn toggle_recording(&mut self) {
        let video = self.shared.borrow().capture.video.clone();
        if video.state().recording {
            let msg = match video.stop() {
                Ok(_) => self.shared.borrow().t("capture.video_saved"),
                Err(e) => e.to_string(),
            };
            self.notify_toast("video", msg);
        } else {
            self.shared.borrow_mut().want_region = Some(crate::state::RegionPurpose::Record);
        }
    }

    /// A video or audio recording in progress: (is video, elapsed, paused).
    fn recording(&self) -> Option<(bool, Duration, bool)> {
        let s = self.shared.borrow();
        let v = s.capture.video.state();
        if v.recording {
            return Some((true, v.elapsed, v.paused));
        }
        let a = s.capture.audio.state();
        a.recording.then_some((false, a.elapsed, a.paused))
    }

    /// Compact notch while recording: red dot, timer, pause/resume, stop.
    fn recording_controls(&self, video: bool, elapsed: Duration, paused: bool, cx: &mut Context<Self>) -> AnyElement {
        let p = tokens::Palette::dark();
        let mono = SharedString::from(self.shared.borrow().typo.mono.clone());
        let (tip_pause, tip_stop) = {
            let s = self.shared.borrow();
            (s.t(if paused { "capture.resume" } else { "capture.pause" }), s.t("capture.stop"))
        };
        let dot = div().size(px(8.0)).rounded_full().bg(hsla(if paused { p.overlay_ink_2 } else { p.danger }));
        // Blink while recording (not while paused).
        let dot = if paused {
            dot.into_any_element()
        } else {
            dot.with_animation("rec-dot", Animation::new(Duration::from_millis(1200)).repeat(), |el, t| el.opacity(if t < 0.6 { 1.0 } else { 0.25 }))
                .into_any_element()
        };
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(dot)
            .child(ui::icon(if video { "video" } else { "mic" }, 13.0, p.overlay_ink_2))
            .child(div().font_family(mono).text_size(px(12.5)).text_color(hsla(p.overlay_ink)).child(halo_modules::media::mmss(elapsed.as_secs_f64())))
            .child(
                self.icon_button("rec-pause", if paused { "play" } else { "pause" }, 24.0, true, move |this, _| {
                    let s = this.shared.borrow();
                    if video {
                        if paused { s.capture.video.resume() } else { s.capture.video.pause() }
                    } else if paused {
                        s.capture.audio.resume()
                    } else {
                        s.capture.audio.pause()
                    }
                }, cx)
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip_pause.clone()).build(window, cx)),
            )
            .child(
                self.icon_button("rec-stop", "square", 24.0, true, move |this, _| {
                    if video {
                        this.toggle_recording();
                    } else {
                        let audio = this.shared.borrow().capture.audio.clone();
                        let msg = match audio.stop() {
                            Ok(_) => this.shared.borrow().t("capture.audio_saved"),
                            Err(e) => e.to_string(),
                        };
                        this.notify_toast("mic", msg);
                    }
                }, cx)
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip_stop.clone()).build(window, cx)),
            )
            .into_any_element()
    }

    /// Show a transient message in the notch for 6 s (reminders, timers…).
    pub fn notify_toast(&mut self, icon: &'static str, text: String) {
        self.toast = Some((icon, text, Instant::now() + Duration::from_secs(6)));
        self.placed = None;
        // Wake from the slim state so it is noticed.
        self.state.tick(Duration::ZERO, Pointer { over: false, near: true });
        self.sync_springs();
    }

    fn toggle_theme(&mut self) {
        let mode = self.shared.borrow().mode;
        let pref = if mode == Mode::Dark { ThemePref::Light } else { ThemePref::Dark };
        self.shared.borrow_mut().update_config(|c| c.theme = pref);
    }

    // ── Rendering ────────────────────────────────────────────────────────────

    fn tile(
        &self,
        id: &'static str,
        icon: &'static str,
        on_click: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = tokens::Palette::dark();
        div()
            .id(id)
            .flex_1()
            .min_w(px(m::TILE))
            .h(px(m::TILE))
            .rounded(px(m::TILE_RADIUS))
            .bg(hsla(p.overlay_tile))
            .hover(|s| s.bg(hsla(p.overlay_track)))
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .child(ui::icon(icon, tokens::ICON_MD, p.overlay_ink_2))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _window, cx| {
                    cx.stop_propagation();
                    on_click(this, cx);
                    cx.notify();
                }),
            )
    }

    /// One compact indicator: icon + value.
    fn indicator(&self, id: &str, vertical: bool) -> Option<AnyElement> {
        let p = tokens::Palette::dark();
        let mono = self.shared.borrow().typo.mono.clone();
        let s = &self.snap;
        let (icon, value): (Option<&str>, String) = match id {
            "clock" => (None, if s.clock.is_empty() { halo_modules::clock::now_label() } else { s.clock.clone() }),
            "cpu" => (Some("cpu"), format!("{:.0}%", s.cpu)),
            "ram" => (Some("memory-stick"), format!("{:.0}%", s.ram_percent())),
            "battery" => (
                Some(if s.battery.charging { "battery-charging" } else { battery_icon(s.battery.percent) }),
                s.battery.percent.map(|v| format!("{v}%")).unwrap_or_default(),
            ),
            "volume" => {
                // Live value (polled every tick), not the 1 s system snapshot.
                let v = self.controls.volume.or(s.volume);
                (
                    Some(match v {
                        Some(v) if v.muted || v.level == 0.0 => "volume-x",
                        _ => "volume-2",
                    }),
                    v.map(|v| format!("{:.0}", v.level * 100.0)).unwrap_or_default(),
                )
            }
            "brightness" => (Some("sun"), self.controls.brightness.level()?.to_string()),
            "wifi" => (
                Some(if s.wifi.as_ref().is_some_and(|w| w.connected) { "wifi" } else { "wifi-off" }),
                String::new(),
            ),
            "bluetooth" => (
                Some(if s.bluetooth.enabled { "bluetooth" } else { "bluetooth-off" }),
                if s.bluetooth.connected.is_empty() { String::new() } else { s.bluetooth.connected.len().to_string() },
            ),
            _ => return None,
        };
        let text = div()
            .font_family(SharedString::from(mono))
            .text_size(px(if vertical { 10.0 } else { 12.5 }))
            .text_color(hsla(p.overlay_ink))
            .child(value);
        let el = div().flex().items_center().gap(px(if vertical { 2.0 } else { 5.0 }));
        let el = if vertical { el.flex_col() } else { el.flex_row() };
        let el = match icon {
            Some(i) => el.child(ui::icon(i, if vertical { 15.0 } else { 14.0 }, p.overlay_ink_2)).child(text),
            None => el.child(text),
        };
        Some(el.into_any_element())
    }

    /// Dynamic-Island-style live activity for the compact notch: focus timer or track.
    fn live_activity(&self) -> Option<(&'static str, String)> {
        let (v, a) = {
            let s = self.shared.borrow();
            (s.capture.video.state(), s.capture.audio.state())
        };
        if v.recording {
            return Some(("video", format!("REC {}", halo_modules::media::mmss(v.elapsed.as_secs_f64()))));
        }
        if a.recording {
            return Some(("mic", format!("REC {}", halo_modules::media::mmss(a.elapsed.as_secs_f64()))));
        }
        let f = self.shared.borrow().handles.focus.clone();
        let now = Instant::now();
        if let Some(r) = f.with(|f| (f.phase() != halo_modules::focus::Phase::Idle).then(|| f.remaining(now))) {
            return Some(("timer", halo_modules::media::mmss(r.as_secs_f64())));
        }
        let np = self.shared.borrow().handles.media.get()?;
        (np.playing && !np.title.is_empty()).then(|| ("audio-lines", np.title.chars().take(18).collect()))
    }

    /// Compact "now playing": tiny cover, title and a looping equalizer.
    fn track_peek(&self, title: String) -> AnyElement {
        let p = tokens::Palette::dark();
        let art = self.shared.borrow().handles.media.get().and_then(|n| n.art);
        let cover = match art {
            Some(bytes) => img(std::sync::Arc::new(Image::from_bytes(dashboard::image_format(&bytes), bytes)))
                .size(px(18.0))
                .flex_none()
                .rounded(px(5.0))
                .into_any_element(),
            None => ui::icon("music", 14.0, tokens::ACCENT).into_any_element(),
        };
        // Four bars, each bouncing on its own phase (pure animation, no audio data).
        let bars = (0..4usize).map(|i| {
            let phase = i as f32 * 0.27;
            div()
                .w(px(2.5))
                .h(px(12.0))
                .rounded(px(1.0))
                .bg(hsla(tokens::ACCENT))
                .with_animation(
                    ("eq", i),
                    Animation::new(Duration::from_millis(900 + i as u64 * 130)).repeat(),
                    move |el, t| {
                        let x = ((t + phase) * std::f32::consts::TAU).sin() * 0.5 + 0.5;
                        el.h(px(4.0 + 9.0 * x))
                    },
                )
        });
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(cover)
            .child(div().text_size(px(12.5)).text_color(hsla(p.overlay_ink)).truncate().max_w(px(110.0)).child(title))
            .child(div().flex().flex_row().items_center().gap(px(2.0)).h(px(14.0)).children(bars))
            .into_any_element()
    }

    fn compact_content(&self, orientation: Orientation, cx: &mut Context<Self>) -> AnyElement {
        let vertical = orientation == Orientation::Vertical;
        let mut items: Vec<AnyElement> = Vec::new();
        let p = tokens::Palette::dark();
        if let Some((ctl, _)) = self.controls.hud.filter(|_| !vertical) {
            // HUD replaces the indicators while it is visible.
            return div().size_full().flex().flex_row().items_center().justify_center().child(self.hud_view(ctl)).into_any_element();
        }
        let recording = self.recording().filter(|_| !vertical);
        if let Some((video, elapsed, paused)) = recording {
            items.push(self.recording_controls(video, elapsed, paused, cx));
        } else if let Some((icon, text, _)) = &self.toast {
            items.push(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(ui::icon(icon, 15.0, tokens::ACCENT))
                    .child(div().text_size(px(13.0)).text_color(hsla(p.overlay_ink)).truncate().max_w(px(260.0)).child(text.clone()))
                    .into_any_element(),
            );
        } else if let Some((icon, text)) = self.live_activity().filter(|_| !vertical) {
            if icon == "audio-lines" {
                items.push(self.track_peek(text));
            } else {
                items.push(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(ui::icon(icon, 14.0, tokens::ACCENT))
                    .child(div().text_size(px(12.5)).text_color(hsla(p.overlay_ink)).truncate().max_w(px(120.0)).child(text))
                    .into_any_element(),
                );
            }
        }
        if self.dnd_shown {
            items.push(ui::icon("moon", 13.0, p.overlay_ink_2).into_any_element());
        }
        items.extend(self.indicators().iter().filter_map(|i| self.indicator(i, vertical)));
        let base = div().size_full().flex().items_center().justify_center();
        if vertical {
            base.flex_col().gap(px(m::GAP)).py(px(12.0)).children(items).into_any_element()
        } else {
            base.flex_row().gap(px(m::GAP + 4.0)).px(px(m::PAD_X)).children(items).into_any_element()
        }
    }

    fn chooser_content(&self) -> AnyElement {
        let p = tokens::Palette::dark();
        let ui_font = self.shared.borrow().typo.ui.clone();
        let vertical = self.anchor().orientation() == Orientation::Vertical;
        let chars = self.chooser.options.iter().enumerate().map(|(i, c)| {
            let selected = i == self.chooser.index;
            div()
                .size(px(34.0))
                .flex_none()
                .rounded(px(10.0))
                .flex()
                .items_center()
                .justify_center()
                .font_family(SharedString::from(ui_font.clone()))
                .text_size(px(19.0))
                .when(selected, |d| d.bg(hsla(tokens::ACCENT)).text_color(hsla(p.on_accent)))
                .when(!selected, |d| d.text_color(hsla(p.overlay_ink)))
                .child(c.to_string())
        });
        let base = div().size_full().flex().items_center().justify_center().gap(px(6.0));
        if vertical { base.flex_col() } else { base.flex_row() }.children(chars).into_any_element()
    }

    fn expanded_content(&self, orientation: Orientation, cx: &mut Context<Self>) -> AnyElement {
        if self.state.is_held() {
            return self.drop_zones(cx);
        }
        if orientation == Orientation::Horizontal {
            return self.dashboard(cx);
        }
        let p = tokens::Palette::dark();
        let (typo, picker_on, spot_on) = {
            let s = self.shared.borrow();
            (s.typo.clone(), s.module_running("picker"), s.module_running("spotlight"))
        };
        let mode_icon = if self.shared.borrow().mode == Mode::Dark { "sun" } else { "moon" };
        let mut tiles: Vec<AnyElement> = Vec::new();
        if picker_on {
            tiles.push(
                self.tile("t-picker", "pipette", |this, _| this.shared.borrow_mut().want_picker = true, cx)
                    .into_any_element(),
            );
        }
        if spot_on {
            tiles.push(
                self.tile(
                    "t-spot",
                    "mouse-pointer-2",
                    |this, _| {
                        let c = halo_platform::cursor_pos().unwrap_or_default();
                        this.shared.borrow().handles.spotlight.trigger(c);
                    },
                    cx,
                )
                .into_any_element(),
            );
        }
        if self.shared.borrow().module_running("notes") {
            tiles.push(
                self.tile("t-notes", "notebook-pen", |this, _| this.shared.borrow_mut().open_hub(Section::Notes), cx)
                    .into_any_element(),
            );
        }
        if self.shared.borrow().module_running("clipboard") {
            tiles.push(
                self.tile("t-clip", "clipboard", |this, _| this.shared.borrow_mut().open_hub(Section::Clipboard), cx)
                    .into_any_element(),
            );
        }
        tiles.push(self.tile("t-theme", mode_icon, |this, _| this.toggle_theme(), cx).into_any_element());
        tiles.push(
            self.tile("t-hub", "layout-dashboard", |this, _| this.shared.borrow_mut().open_hub(Section::Modules), cx)
                .into_any_element(),
        );
        tiles.push(
            self.tile("t-settings", "settings", |this, _| this.shared.borrow_mut().open_hub(Section::General), cx)
                .into_any_element(),
        );

        let s = &self.snap;
        if orientation == Orientation::Vertical {
            let items: Vec<AnyElement> =
                ["clock", "cpu", "battery"].iter().filter_map(|i| self.indicator(i, true)).collect();
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.0))
                .py(px(16.0))
                .px(px(12.0))
                .children(items)
                .children(tiles.into_iter().take(5).map(|t| div().w_full().child(t)))
                .into_any_element();
        }

        let mono = SharedString::from(typo.mono.clone());
        let meter = |label: &str, frac: f32, value: String| {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .font_family(mono.clone())
                        .text_size(px(11.0))
                        .text_color(hsla(p.overlay_ink_2))
                        .child(label.to_string())
                        .child(value),
                )
                .child(
                    div()
                        .h(px(5.0))
                        .w_full()
                        .rounded(px(3.0))
                        .bg(hsla(p.overlay_track))
                        .child(div().h_full().w(relative(frac.clamp(0.0, 1.0))).rounded(px(3.0)).bg(hsla(tokens::ACCENT))),
                )
        };
        let vol = s.volume.unwrap_or_default();
        let header = div()
            .flex()
            .flex_row()
            .items_end()
            .gap(px(10.0))
            .child(
                div()
                    .font_family(SharedString::from(typo.display.clone()))
                    .text_size(px(26.0))
                    .line_height(px(26.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(hsla(p.overlay_ink))
                    .child(s.clock.clone()),
            )
            .child(
                div()
                    .font_family(SharedString::from(typo.ui.clone()))
                    .text_size(px(12.0))
                    .text_color(hsla(p.overlay_ink_2))
                    .child(s.date_label(self.shared.borrow().config.lang.locale_code())),
            )
            .child(div().flex_1())
            .children(["wifi", "bluetooth", "battery"].iter().filter_map(|i| self.indicator(i, false)));
        let volume_row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .id("vol-mute")
                    .cursor_pointer()
                    .child(ui::icon(if vol.muted { "volume-x" } else { "volume-2" }, 15.0, p.overlay_ink_2))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.shared.borrow().handles.system.toggle_mute();
                        }),
                    ),
            )
            .child(self.step_button("vol-down", "minus", -0.1, cx))
            .child(
                div()
                    .flex_1()
                    .h(px(5.0))
                    .rounded(px(3.0))
                    .bg(hsla(p.overlay_track))
                    .child(
                        div()
                            .h_full()
                            .w(relative(if vol.muted { 0.0 } else { vol.level }))
                            .rounded(px(3.0))
                            .bg(hsla(p.overlay_ink)),
                    ),
            )
            .child(self.step_button("vol-up", "plus", 0.1, cx))
            .child(
                div()
                    .w(px(26.0))
                    .font_family(mono.clone())
                    .text_size(px(11.0))
                    .text_color(hsla(p.overlay_ink_2))
                    .child(format!("{:.0}", vol.level * 100.0)),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .px(px(m::EXPANDED_PAD))
            .pt(px(10.0))
            .pb(px(m::EXPANDED_PAD))
            .child(header)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(16.0))
                    .child(meter("CPU", s.cpu / 100.0, format!("{:.0}%", s.cpu)))
                    .child(meter(
                        "RAM",
                        s.ram_percent() / 100.0,
                        halo_modules::system::format_bytes(s.ram_used),
                    )),
            )
            .child(volume_row)
            .child(div().flex().flex_row().gap(px(m::TILE_GAP)).children(tiles))
            .into_any_element()
    }

    fn step_button(&self, id: &'static str, icon: &'static str, delta: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let p = tokens::Palette::dark();
        div()
            .id(id)
            .size(px(22.0))
            .flex_none()
            .rounded(px(7.0))
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(p.overlay_tile))
            .hover(|s| s.bg(hsla(p.overlay_track)))
            .cursor_pointer()
            .child(ui::icon(icon, 12.0, p.overlay_ink_2))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let level = this.snap.volume.map(|v| v.level).unwrap_or(0.5);
                    this.shared.borrow().handles.system.set_volume(((level + delta) * 10.0).round() / 10.0);
                    this.snap = this.shared.borrow().handles.system.get();
                    cx.notify();
                }),
            )
    }
}

/// Anchor closest to a cursor position given as fractions of the work area.
fn nearest_anchor(fx: f32, fy: f32) -> Anchor {
    if (0.25..0.75).contains(&fy) && fx < 0.15 {
        Anchor::LeftCenter
    } else if (0.25..0.75).contains(&fy) && fx > 0.85 {
        Anchor::RightCenter
    } else if fy < 0.5 {
        if fx < 0.33 {
            Anchor::TopLeft
        } else if fx > 0.66 {
            Anchor::TopRight
        } else {
            Anchor::TopCenter
        }
    } else if fx < 0.5 {
        Anchor::BottomLeft
    } else {
        Anchor::BottomRight
    }
}

fn battery_icon(p: Option<u8>) -> &'static str {
    match p {
        Some(v) if v >= 80 => "battery-full",
        Some(v) if v >= 40 => "battery-medium",
        Some(v) if v >= 15 => "battery-low",
        Some(_) => "battery",
        None => "battery",
    }
}

/// Build the GPUI path for a notch outline translated by `origin`.
/// Fill path, or (with stroke) the visible border only — the segment lying on the
/// screen edge is skipped so no line is drawn along the edge.
fn notch_path(origin: Point<Pixels>, start: (f32, f32), segs: &[Seg], stroke: Option<f32>) -> Option<Path<Pixels>> {
    let pt = |(x, y): (f32, f32)| point(origin.x + px(x), origin.y + px(y));
    let mut b = match stroke {
        Some(w) => PathBuilder::stroke(px(w)),
        None => PathBuilder::fill(),
    };
    b.move_to(pt(start));
    let segs = if stroke.is_some() { &segs[..segs.len() - 1] } else { segs };
    for s in segs {
        match *s {
            Seg::Line(to) => b.line_to(pt(to)),
            Seg::Quad(ctrl, to) => b.curve_to(pt(to), pt(ctrl)),
        }
    }
    if stroke.is_none() {
        b.close();
    }
    b.build().ok()
}

/// Smoothstep on 0..1 (clamped): soft start and end for fades.
fn ease(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

impl Render for Overlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32() * self.time_scale;
        self.last_frame = now;
        let moving = self.expand.step(dt) | self.presence.step(dt) | self.peek.step(dt);
        if moving {
            window.request_animation_frame();
        }

        let anchor = self.anchor();
        let edge = anchor.edge();
        let orientation = anchor.orientation();
        let t = self.expand.value().clamp(0.0, 1.05);
        let (c_along, c_depth) = self.body(false);
        let (e_along, e_depth) = self.body(true);
        // Peek sits between compact and expanded.
        let pk = self.peek.value().clamp(0.0, 1.05);
        let (c_along, c_depth) = if pk > 0.0 && orientation == Orientation::Horizontal {
            let (pa, pd) = self.peek_size();
            (lerp(c_along, pa, pk), lerp(c_depth, pd, pk))
        } else {
            (c_along, c_depth)
        };
        let (mut along, mut depth) = if self.chooser.visible {
            (c_along, c_depth)
        } else {
            (lerp(c_along, e_along, t), lerp(c_depth, e_depth, t))
        };

        // Idle → slim bar; hidden → gone.
        let presence = self.presence.value().clamp(0.0, 1.0);
        let faded = tokens::OVERLAY_FADED_OPACITY;
        let slim = ((1.0 - presence) / (1.0 - faded)).clamp(0.0, 1.0);
        along = lerp(along, m::SLIM_ALONG, slim);
        depth = lerp(depth, m::SLIM_DEPTH, slim);
        if presence < faded {
            depth *= presence / faded;
        }
        let content_alpha = (1.0 - slim * 1.6).clamp(0.0, 1.0);
        let radius = lerp(m::RADIUS_COMPACT, m::RADIUS_EXPANDED, t.min(1.0)).min(depth / 2.0);

        // Place the animated notch inside the (possibly larger) window.
        let vp = window.viewport_size();
        let (win_along, win_depth) = match edge {
            Edge::Top | Edge::Bottom => (f32::from(vp.width), f32::from(vp.height)),
            Edge::Left | Edge::Right => (f32::from(vp.height), f32::from(vp.width)),
        };
        let shape_along = along + 2.0 * m::EAR;
        let off_along = match anchor {
            Anchor::TopLeft | Anchor::BottomLeft => 0.0,
            Anchor::TopRight | Anchor::BottomRight => win_along - shape_along,
            _ => (win_along - shape_along) / 2.0,
        };
        let off_depth = match edge {
            Edge::Top | Edge::Left => 0.0,
            Edge::Bottom | Edge::Right => win_depth - depth,
        };
        let (sx, sy, sw, sh) = match edge {
            Edge::Top | Edge::Bottom => (off_along, off_depth, shape_along, depth),
            Edge::Left | Edge::Right => (off_depth, off_along, depth, shape_along),
        };
        let (start, segs) = geo::notch_outline(edge, (sw, sh), radius);
        // Body = shape minus ears, where content goes.
        let (bx, by, bw, bh) = match edge {
            Edge::Top | Edge::Bottom => (sx + m::EAR, sy, along, depth),
            Edge::Left | Edge::Right => (sx, sy + m::EAR, depth, along),
        };

        let theme = tokens::notch_theme(&self.shared.borrow().config.overlay.notch_theme);
        let fill = hsla(theme.bg);
        let border = theme.border.map(hsla);
        let shape = canvas(
            move |_, _, _| (),
            move |bounds, _, window, _| {
                let origin = point(bounds.origin.x + px(sx), bounds.origin.y + px(sy));
                if let Some(path) = notch_path(origin, start, &segs, None) {
                    window.paint_path(path, fill);
                }
                if let Some(b) = border {
                    if let Some(path) = notch_path(origin, start, &segs, Some(1.0)) {
                        window.paint_path(path, b);
                    }
                }
            },
        )
        .absolute()
        .size_full();

        // Cross-fade between layers: the outgoing one fades out during the first half
        // of the motion, the incoming one fades in during the second half — they never
        // overlap. Larger layers are laid out at their final size and revealed by the
        // growing shape (clipped), instead of being squeezed while it animates.
        let horizontal = orientation == Orientation::Horizontal;
        let (layer, layer_alpha, full) = if content_alpha <= 0.01 {
            (div().into_any_element(), 0.0, None)
        } else if self.chooser.visible {
            (self.chooser_content(), 1.0, None)
        } else if t > 0.5 {
            (self.expanded_content(orientation, cx), ease((t - 0.5) / 0.45), Some((e_along, e_depth)))
        } else if pk > 0.5 && horizontal {
            (self.peek_content(cx), ease((pk - 0.5) / 0.45), Some(self.peek_size()))
        } else {
            // Pinned at its final (compact) size too, so it does not slide while fading in.
            let pinned = (!self.chooser.visible).then(|| self.body(false));
            (self.compact_content(orientation, cx), ease((0.45 - t.max(if horizontal { pk } else { 0.0 })) / 0.4), pinned)
        };
        let content = match full {
            Some((fa, fd)) => {
                let (fw, fh) = if horizontal { (fa, fd) } else { (fd, fa) };
                let shift = match anchor {
                    Anchor::TopLeft | Anchor::BottomLeft => 0.0,
                    Anchor::TopRight | Anchor::BottomRight => along - fa,
                    _ => (along - fa) / 2.0,
                };
                let d = div().absolute().w(px(fw)).h(px(fh));
                match edge {
                    Edge::Top => d.top(px(0.0)).left(px(shift)),
                    Edge::Bottom => d.bottom(px(0.0)).left(px(shift)),
                    Edge::Left => d.left(px(0.0)).top(px(shift)),
                    Edge::Right => d.right(px(0.0)).top(px(shift)),
                }
                .child(layer)
                .into_any_element()
            }
            None => div().size_full().child(layer).into_any_element(),
        };
        let content_alpha = content_alpha * layer_alpha;

        div()
            .size_full()
            .relative()
            .child(shape)
            .child(
                div()
                    .id("halo-notch")
                    .absolute()
                    .left(px(bx))
                    .top(px(by))
                    .w(px(bw))
                    .h(px(bh))
                    .overflow_hidden()
                    .opacity(content_alpha)
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, _window, _cx| {
                            // Click vs drag is decided in 	ick once the button is released.
                            this.press = halo_platform::cursor_pos().map(|c| (c, false));
                        }),
                    )
                    .child(content),
            )
    }
}
