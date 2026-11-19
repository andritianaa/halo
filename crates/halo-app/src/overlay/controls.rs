//! Quick controls in the notch (WinNotch-style): volume and brightness sliders, Wi-Fi
//! and Bluetooth toggles, a HUD in the compact notch when volume or brightness
//! changes, and the "peek" preview shown on hover in Peek mode.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use halo_platform::controls::{Brightness, RadioType};
use halo_platform::sys::Volume;
use halo_ui::hsla;
use halo_ui::tokens::{self, Palette};

use super::Overlay;
use crate::ui;

/// A draggable control.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ctl {
    Volume,
    Brightness,
}

/// How long the HUD stays after the last change.
const HUD_FOR: Duration = Duration::from_millis(1600);

/// Live state of the quick controls, owned by the overlay.
pub struct Controls {
    pub brightness: Brightness,
    /// Wi-Fi / Bluetooth radio states, refreshed by a worker thread (WinRT calls block).
    pub radios: Arc<Mutex<(Option<bool>, Option<bool>)>>,
    pub volume: Option<Volume>,
    last_volume: Option<Volume>,
    last_brightness: Option<u8>,
    /// HUD shown in the compact notch: what changed, until when.
    pub hud: Option<(Ctl, Instant)>,
    /// Slider being dragged.
    pub dragging: Option<Ctl>,
    bounds: [Rc<Cell<Bounds<Pixels>>>; 2],
}

impl Controls {
    pub fn new() -> Self {
        let radios = Arc::new(Mutex::new((None, None)));
        let r2 = radios.clone();
        let _ = std::thread::Builder::new().name("halo-radios".into()).spawn(move || loop {
            let w = halo_platform::controls::radio_on(RadioType::Wifi);
            let b = halo_platform::controls::radio_on(RadioType::Bluetooth);
            *r2.lock().unwrap() = (w, b);
            std::thread::sleep(Duration::from_secs(3));
        });
        Self {
            brightness: Brightness::spawn(),
            radios,
            volume: None,
            last_volume: None,
            last_brightness: None,
            hud: None,
            dragging: None,
            bounds: [Rc::new(Cell::new(Bounds::default())), Rc::new(Cell::new(Bounds::default()))],
        }
    }

    /// Sample volume/brightness. Returns true when the HUD appeared, changed or ended.
    pub fn poll(&mut self, hud_enabled: bool, expanded: bool) -> bool {
        let vol = halo_platform::sys::volume_fast();
        let bri = self.brightness.level();
        let now = Instant::now();
        let mut changed = false;
        // A change made from the open dashboard needs no HUD.
        let show = hud_enabled && !expanded && self.dragging.is_none();
        if let (Some(old), Some(new)) = (self.last_volume, vol) {
            if (old.level - new.level).abs() > 0.004 || old.muted != new.muted {
                if show {
                    changed |= self.hud.map(|(c, _)| c) != Some(Ctl::Volume);
                    self.hud = Some((Ctl::Volume, now + HUD_FOR));
                }
            }
        }
        if let (Some(old), Some(new)) = (self.last_brightness, bri) {
            if old != new && show {
                changed |= self.hud.map(|(c, _)| c) != Some(Ctl::Brightness);
                self.hud = Some((Ctl::Brightness, now + HUD_FOR));
            }
        }
        if vol != self.volume {
            changed = true;
        }
        self.volume = vol;
        self.last_volume = vol;
        self.last_brightness = bri;
        if self.hud.is_some_and(|(_, until)| now >= until) {
            self.hud = None;
            changed = true;
        }
        changed
    }

    fn bounds(&self, ctl: Ctl) -> Rc<Cell<Bounds<Pixels>>> {
        self.bounds[ctl as usize].clone()
    }

    fn fraction(&self, ctl: Ctl) -> Option<f32> {
        match ctl {
            Ctl::Volume => self.volume.map(|v| if v.muted { 0.0 } else { v.level }),
            Ctl::Brightness => self.brightness.level().map(|b| b as f32 / 100.0),
        }
    }

    fn apply(&mut self, ctl: Ctl, x: Pixels) {
        let b = self.bounds(ctl).get();
        let w = f32::from(b.size.width);
        if w <= 0.0 {
            return;
        }
        let frac = ((f32::from(x) - f32::from(b.origin.x)) / w).clamp(0.0, 1.0);
        match ctl {
            Ctl::Volume => {
                halo_platform::sys::set_volume(frac);
                if self.volume.is_some_and(|v| v.muted) && frac > 0.0 {
                    halo_platform::sys::set_mute(false);
                }
                self.volume = Some(Volume { level: frac, muted: false });
                self.last_volume = self.volume;
            }
            Ctl::Brightness => {
                let p = (frac * 100.0).round() as u8;
                self.brightness.set(p);
                self.last_brightness = Some(p);
            }
        }
    }

    fn toggle_radio(&self, kind: RadioType) {
        let radios = self.radios.clone();
        let current = {
            let r = radios.lock().unwrap();
            if kind == RadioType::Wifi { r.0 } else { r.1 }
        };
        let Some(on) = current else { return };
        // Optimistic: flip now, confirm from the worker.
        {
            let mut r = radios.lock().unwrap();
            if kind == RadioType::Wifi { r.0 = Some(!on) } else { r.1 = Some(!on) }
        }
        std::thread::spawn(move || {
            halo_platform::controls::set_radio(kind, !on);
            let state = halo_platform::controls::radio_on(kind);
            let mut r = radios.lock().unwrap();
            if kind == RadioType::Wifi { r.0 = state } else { r.1 = state }
        });
    }
}

fn volume_icon(v: Option<Volume>) -> &'static str {
    match v {
        Some(v) if v.muted || v.level < 0.005 => "volume-x",
        Some(v) if v.level < 0.34 => "volume",
        Some(v) if v.level < 0.67 => "volume-1",
        _ => "volume-2",
    }
}

impl Overlay {
    /// Horizontal slider drawn with notch tokens (gpui-kit's slider follows the hub
    /// theme, the notch is always dark).
    fn slider(&self, ctl: Ctl, p: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let frac = self.controls.fraction(ctl).unwrap_or(0.0);
        let cell = self.controls.bounds(ctl);
        let id = match ctl {
            Ctl::Volume => "slider-volume",
            Ctl::Brightness => "slider-brightness",
        };
        div()
            .id(id)
            .flex_1()
            .min_w(px(0.0))
            .h(px(22.0))
            .relative()
            .flex()
            .items_center()
            .cursor_pointer()
            .child(canvas(move |b, _, _| cell.set(b), |_, _, _, _| {}).absolute().size_full())
            .child(
                div()
                    .w_full()
                    .h(px(6.0))
                    .rounded_full()
                    .bg(hsla(p.overlay_track))
                    .child(div().h_full().w(relative(frac)).rounded_full().bg(hsla(p.overlay_ink))),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.controls.dragging = Some(ctl);
                    this.controls.apply(ctl, ev.position.x);
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                if this.controls.dragging == Some(ctl) && ev.pressed_button == Some(MouseButton::Left) {
                    this.controls.apply(ctl, ev.position.x);
                    cx.notify();
                }
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, _| this.controls.dragging = None))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, _| this.controls.dragging = None))
    }

    fn control_row(&self, ctl: Ctl, p: &Palette, cx: &mut Context<Self>) -> Option<Div> {
        let frac = self.controls.fraction(ctl)?;
        let mono = SharedString::from(self.shared.borrow().typo.mono.clone());
        let icon: AnyElement = match ctl {
            Ctl::Volume => self
                .icon_button("ctl-mute", volume_icon(self.controls.volume), 26.0, false, |this, _| {
                    let muted = this.controls.volume.is_some_and(|v| v.muted);
                    halo_platform::sys::set_mute(!muted);
                }, cx)
                .into_any_element(),
            Ctl::Brightness => div().size(px(26.0)).flex().items_center().justify_center().child(ui::icon("sun", 15.0, p.overlay_ink)).into_any_element(),
        };
        Some(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(icon)
                .child(self.slider(ctl, p, cx))
                .child(
                    div()
                        .w(px(34.0))
                        .text_right()
                        .font_family(mono)
                        .text_size(px(11.0))
                        .text_color(hsla(p.overlay_ink_2))
                        .child(format!("{:.0}%", frac * 100.0)),
                ),
        )
    }

    /// Wi-Fi / Bluetooth pill: click toggles the radio, the chevron opens Settings.
    fn radio_pill(&self, kind: RadioType, p: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let (on, label, icon_on, icon_off, uri, id) = {
            let r = *self.controls.radios.lock().unwrap();
            let s = self.shared.borrow();
            match kind {
                RadioType::Wifi => {
                    let w = self.snap.wifi.as_ref();
                    let label = match (r.0, w) {
                        (Some(false), _) => s.t("controls.off"),
                        (_, Some(w)) if w.connected && !w.ssid.is_empty() => w.ssid.clone(),
                        _ => s.t("controls.wifi"),
                    };
                    (r.0, label, "wifi", "wifi-off", "ms-settings:network-wifi", "radio-wifi")
                }
                RadioType::Bluetooth => {
                    let label = match (r.1, self.snap.bluetooth.connected.first()) {
                        (Some(false), _) => s.t("controls.off"),
                        (_, Some(dev)) => dev.clone(),
                        _ => s.t("indicator.bluetooth"),
                    };
                    (r.1, label, "bluetooth", "bluetooth-off", "ms-settings:bluetooth", "radio-bt")
                }
            }
        };
        let active = on == Some(true);
        let fg = if active { p.on_accent } else { p.overlay_ink };
        div()
            .id(id)
            .flex_1()
            .min_w(px(0.0))
            .h(px(32.0))
            .pl(px(10.0))
            .pr(px(4.0))
            .rounded_full()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .when(active, |d| d.bg(hsla(tokens::ACCENT)))
            .when(!active, |d| d.bg(hsla(p.overlay_tile)).hover(|s| s.bg(hsla(p.overlay_track))))
            .when(on.is_none(), |d| d.opacity(0.5))
            .child(ui::icon(if on == Some(false) { icon_off } else { icon_on }, 14.0, fg))
            .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(12.0)).text_color(hsla(fg)).child(label))
            .child(
                div()
                    .id((id, 1usize))
                    .size(px(20.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|s| s.bg(hsla(p.overlay_line)))
                    .child(ui::icon("chevron-right", 12.0, fg))
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                        cx.stop_propagation();
                        halo_platform::controls::open_settings(uri);
                    }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.controls.toggle_radio(kind);
                    cx.notify();
                }),
            )
    }

    /// Dashboard section: volume, brightness, Wi-Fi, Bluetooth.
    pub(super) fn controls_section(&self, p: &Palette, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(8.0))
            .px(px(4.0))
            .children(self.control_row(Ctl::Volume, p, cx))
            .children(self.control_row(Ctl::Brightness, p, cx))
            .child(
                div()
                    .pt(px(4.0))
                    .flex()
                    .flex_row()
                    .gap(px(8.0))
                    .child(self.radio_pill(RadioType::Wifi, p, cx))
                    .child(self.radio_pill(RadioType::Bluetooth, p, cx)),
            )
    }

    /// Compact HUD: icon, bar, value.
    pub(super) fn hud_view(&self, ctl: Ctl) -> AnyElement {
        let p = Palette::dark();
        let frac = self.controls.fraction(ctl).unwrap_or(0.0);
        let icon = match ctl {
            Ctl::Volume => volume_icon(self.controls.volume),
            Ctl::Brightness => "sun",
        };
        let mono = SharedString::from(self.shared.borrow().typo.mono.clone());
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(10.0))
            .child(ui::icon(icon, 15.0, p.overlay_ink))
            .child(
                div()
                    .w(px(150.0))
                    .h(px(5.0))
                    .rounded_full()
                    .bg(hsla(p.overlay_track))
                    .child(div().h_full().w(relative(frac)).rounded_full().bg(hsla(p.overlay_ink))),
            )
            .child(div().w(px(32.0)).font_family(mono).text_size(px(12.0)).text_color(hsla(p.overlay_ink)).child(format!("{:.0}", frac * 100.0)))
            .into_any_element()
    }

    /// Peek preview size (horizontal notches only).
    pub(super) fn peek_size(&self) -> (f32, f32) {
        (self.compact_along().max(440.0), 72.0)
    }

    /// Peek mode: what is playing (with play/pause), else the time and the next task.
    pub(super) fn peek_content(&self, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::dark();
        let display = SharedString::from(self.shared.borrow().typo.display.clone());
        let media = self.shared.borrow().handles.media.get().filter(|n| !n.title.is_empty());
        let row = div().size_full().flex().flex_row().items_center().gap(px(12.0)).px(px(18.0));
        if let Some(n) = media {
            let art = match &n.art {
                Some(bytes) => img(Arc::new(Image::from_bytes(super::dashboard::image_format(bytes), bytes.clone())))
                    .size(px(42.0))
                    .flex_none()
                    .rounded(px(10.0))
                    .into_any_element(),
                None => div().size(px(42.0)).rounded(px(10.0)).bg(hsla(p.overlay_track)).flex().items_center().justify_center().child(ui::icon("music", 18.0, tokens::ACCENT)).into_any_element(),
            };
            let h = self.shared.borrow().handles.media.clone();
            let h2 = h.clone();
            return row
                .child(art)
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(div().truncate().font_family(display).text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(hsla(p.overlay_ink)).child(n.title.clone()))
                        .child(div().truncate().text_size(px(12.0)).text_color(hsla(p.overlay_ink_2)).child(n.artist.clone())),
                )
                .child(self.icon_button("peek-play", if n.playing { "pause" } else { "play" }, 32.0, true, move |_, _| h.send(halo_platform::media::Command::Toggle), cx))
                .child(self.icon_button("peek-next", "skip-forward", 28.0, false, move |_, _| h2.send(halo_platform::media::Command::Next), cx))
                .into_any_element();
        }
        let next = {
            let s = self.shared.borrow();
            s.handles.tasks.as_ref().and_then(|t| t.all_open().into_iter().next()).map(|t| t.text)
        };
        let date = self.snap.date_label(self.shared.borrow().config.lang.locale_code());
        row.child(div().font_family(display).text_size(px(24.0)).font_weight(FontWeight::SEMIBOLD).text_color(hsla(p.overlay_ink)).child(self.snap.clock.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .child(div().text_size(px(12.0)).text_color(hsla(p.overlay_ink_2)).child(date))
                    .children(next.map(|t| div().truncate().text_size(px(13.0)).text_color(hsla(p.overlay_ink)).child(t))),
            )
            .child(div().text_size(px(11.0)).text_color(hsla(p.overlay_ink_2)).child(self.shared.borrow().t("controls.click_open")))
            .into_any_element()
    }
}
