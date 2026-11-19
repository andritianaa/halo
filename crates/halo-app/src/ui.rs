//! Small shared UI building blocks for Halo windows — all values from `halo_ui::tokens`
//! (see the `halo-design-system` skill).

use gpui_kit::*;
use halo_ui::hsla;
use halo_ui::tokens::{self, Color, Palette};

pub fn icon(name: &str, size: f32, color: Color) -> Svg {
    svg().path(format!("icons/{name}.svg")).size(px(size)).flex_none().text_color(hsla(color))
}

pub fn h1(p: &Palette, family: &str, text: impl Into<SharedString>) -> Div {
    div()
        .font_family(SharedString::from(family.to_string()))
        .text_size(px(tokens::TEXT_H1.size))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(hsla(p.ink))
        .child(text.into())
}

pub fn h2(p: &Palette, text: impl Into<SharedString>) -> Div {
    div().text_size(px(tokens::TEXT_H2.size)).font_weight(FontWeight::SEMIBOLD).text_color(hsla(p.ink)).child(text.into())
}

pub fn label(p: &Palette, text: impl Into<SharedString>) -> Div {
    div().text_size(px(tokens::TEXT_LABEL.size)).text_color(hsla(p.ink_2)).child(text.into())
}

pub fn mono(p: &Palette, family: &str, text: impl Into<SharedString>) -> Div {
    div()
        .font_family(SharedString::from(family.to_string()))
        .text_size(px(tokens::TEXT_MICRO.size))
        .text_color(hsla(p.ink_2))
        .child(text.into())
}

/// Fluent card (SettingsCard): layer background, 1 px stroke, 4 px corners.
pub fn card(p: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(tokens::SPACE_3))
        .p(px(tokens::SPACE_4))
        .rounded(px(tokens::RADIUS_MD))
        .bg(hsla(p.surface))
        .border_1()
        .border_color(hsla(p.line))
}

/// Status badge: tinted pill with a dot, text in the readable `*_text` token.
pub fn badge(label: impl Into<SharedString>, fg: Color, bg: Color) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(7.0))
        .px(px(11.0))
        .py(px(tokens::SPACE_1))
        .rounded_full()
        .bg(hsla(bg))
        .text_size(px(tokens::TEXT_MICRO.size))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(hsla(fg))
        .child(div().size(px(7.0)).rounded_full().bg(hsla(fg)))
        .child(label.into())
}

/// Horizontal metric bar (track `surface_3`, fill `accent`).
pub fn meter(p: &Palette, fraction: f32) -> Div {
    div()
        .h(px(6.0))
        .w_full()
        .rounded(px(3.0))
        .bg(hsla(p.surface_3))
        .child(div().h_full().w(relative(fraction.clamp(0.0, 1.0))).rounded(px(3.0)).bg(hsla(p.accent)))
}

/// Circular progress ring (track + accent arc from 12 o'clock, clockwise).
pub fn ring(size: f32, stroke: f32, frac: f32, track: Color, fill: Color) -> impl IntoElement {
    let frac = frac.clamp(0.0, 1.0);
    let (track, fill) = (hsla(track), hsla(fill));
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let r = (size - stroke) / 2.0;
            let c = point(bounds.origin.x + px(size / 2.0), bounds.origin.y + px(size / 2.0));
            let at = |a: f32| point(c.x + px(r * a.cos()), c.y + px(r * a.sin()));
            let start = -std::f32::consts::FRAC_PI_2;
            // Track: full circle.
            let mut b = PathBuilder::stroke(px(stroke));
            b.move_to(at(start));
            b.arc_to(point(px(r), px(r)), px(0.0), false, true, at(start + std::f32::consts::PI));
            b.arc_to(point(px(r), px(r)), px(0.0), false, true, at(start));
            if let Ok(p) = b.build() {
                window.paint_path(p, track);
            }
            if frac > 0.001 {
                let end = start + frac.min(0.9999) * std::f32::consts::TAU;
                let mut b = PathBuilder::stroke(px(stroke));
                b.move_to(at(start));
                b.arc_to(point(px(r), px(r)), px(0.0), frac > 0.5, true, at(end));
                if let Ok(p) = b.build() {
                    window.paint_path(p, fill);
                }
            }
        },
    )
    .size(px(size))
    .flex_none()
}

/// Area chart of 0–100 values (oldest first): flat translucent fill + 1.5 px line,
/// faint 50 % / 100 % grid lines. No gradient (charter).
pub fn chart(values: Vec<f32>, height: f32, line: Color, grid: Color) -> impl IntoElement {
    let (line_c, fill_c, grid_c) = (hsla(line), hsla(line.with_alpha_pct(16)), hsla(grid));
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let o = bounds.origin;
            for frac in [0.0f32, 0.5] {
                let y = o.y + px(h * frac);
                let mut b = PathBuilder::stroke(px(1.0));
                b.move_to(point(o.x, y));
                b.line_to(point(o.x + px(w), y));
                if let Ok(p) = b.build() {
                    window.paint_path(p, grid_c);
                }
            }
            if values.len() < 2 {
                return;
            }
            let n = values.len() as f32 - 1.0;
            let pt = |i: usize, v: f32| point(o.x + px(w * i as f32 / n), o.y + px(h - h * v.clamp(0.0, 100.0) / 100.0));
            let mut area = PathBuilder::fill();
            area.move_to(point(o.x, o.y + px(h)));
            for (i, v) in values.iter().enumerate() {
                area.line_to(pt(i, *v));
            }
            area.line_to(point(o.x + px(w), o.y + px(h)));
            area.close();
            if let Ok(p) = area.build() {
                window.paint_path(p, fill_c);
            }
            let mut l = PathBuilder::stroke(px(1.5));
            l.move_to(pt(0, values[0]));
            for (i, v) in values.iter().enumerate().skip(1) {
                l.line_to(pt(i, *v));
            }
            if let Ok(p) = l.build() {
                window.paint_path(p, line_c);
            }
        },
    )
    .w_full()
    .h(px(height))
}

/// gpui-kit Icon from a Lucide name (for Button::icon). Never use emoji/glyphs as icons.
pub fn lucide(name: &str) -> gpui_kit::component::Icon {
    gpui_kit::component::Icon::empty().path(format!("icons/{name}.svg"))
}

/// Detect an encoded image's format from its magic bytes.
pub fn image_format(bytes: &[u8]) -> ImageFormat {
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

/// Copy text to the clipboard (best effort).
pub fn copy_text(text: &str) -> bool {
    arboard::Clipboard::new().and_then(|mut c| c.set_text(text.to_string())).is_ok()
}

/// Copy a secret (password): flagged so clipboard managers — Windows clipboard
/// history, cloud clipboard and Halo's own history — do not keep it.
pub fn copy_secret(text: &str) -> bool {
    use arboard::SetExtWindows;
    arboard::Clipboard::new()
        .and_then(|mut c| c.set().exclude_from_monitoring().exclude_from_history().exclude_from_cloud().text(text.to_string()))
        .is_ok()
}
