//! Keeps `themes/halo.json` (gpui-kit) in sync with `halo_ui::tokens`, and bans gradients.

use halo_ui::tokens::{Color, Palette, ACCENT};
use serde_json::Value;

fn theme_file() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../themes/halo.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("themes/halo.json")).expect("valid JSON")
}

fn theme<'a>(file: &'a Value, name: &str) -> &'a Value {
    file["themes"].as_array().unwrap().iter().find(|t| t["name"] == name).unwrap_or_else(|| panic!("{name}"))
}

fn color(theme: &Value, key: &str) -> String {
    theme["colors"][key].as_str().unwrap_or_else(|| panic!("missing colour {key}")).to_uppercase()
}

fn check(name: &str, p: Palette) {
    let file = theme_file();
    let t = theme(&file, name);
    let expect = |key: &str, c: Color| assert_eq!(color(t, key), c.to_css(), "{name}: {key}");
    expect("background", p.bg);
    expect("foreground", p.ink);
    expect("border", p.line);
    expect("muted.background", p.surface_2);
    expect("muted.foreground", p.ink_2);
    expect("primary.background", p.accent);
    expect("primary.foreground", p.on_accent);
    expect("secondary.background", p.surface_2);
    expect("popover.background", p.surface);
    expect("switch.background", p.surface_3);
    expect("ring", ACCENT);
    expect("success.background", p.success);
    expect("warning.background", p.warning);
    expect("danger.background", p.danger);
    expect("info.background", p.info);
    expect("base.green", p.success_text);
    expect("base.red", p.danger_text);
    expect("base.yellow", p.warning_text);
    expect("base.cyan", p.info_text);
    expect("base.blue", p.accent_text);
}

#[test]
fn dark_theme_matches_tokens() {
    check("Halo Dark", Palette::dark());
}

#[test]
fn light_theme_matches_tokens() {
    check("Halo Light", Palette::light());
}

#[test]
fn no_gradients_anywhere() {
    let raw = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../themes/halo.json")).unwrap();
    assert!(!raw.to_lowercase().contains("gradient"), "charter: no gradients");
}
