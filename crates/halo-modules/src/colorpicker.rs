//! M11 — Colour picker: formats, history and palettes (picking UI lives in the app).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use halo_core::module::{Category, Manifest, Module, ModuleContext, Permission};
use serde::{Deserialize, Serialize};

pub const MANIFEST: Manifest = Manifest {
    id: "picker",
    cdc: "M11",
    name_key: "module.picker",
    category: Category::Tools,
    icon: "pipette",
    permissions: &[Permission::ScreenCapture],
    opt_in: false,
    default_enabled: true,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    #[default]
    Hex,
    Rgb,
    Hsl,
    Hsv,
    Cmyk,
    Decimal,
}

impl Format {
    pub const ALL: [Format; 6] = [Format::Hex, Format::Rgb, Format::Hsl, Format::Hsv, Format::Cmyk, Format::Decimal];
}

impl Rgb {
    pub fn hsl(self) -> (f32, f32, f32) {
        let (r, g, b) = (self.0 as f32 / 255.0, self.1 as f32 / 255.0, self.2 as f32 / 255.0);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let l = (max + min) / 2.0;
        let d = max - min;
        if d == 0.0 {
            return (0.0, 0.0, l * 100.0);
        }
        let s = d / (1.0 - (2.0 * l - 1.0).abs());
        (hue(r, g, b, max, d), s * 100.0, l * 100.0)
    }

    pub fn hsv(self) -> (f32, f32, f32) {
        let (r, g, b) = (self.0 as f32 / 255.0, self.1 as f32 / 255.0, self.2 as f32 / 255.0);
        let max = r.max(g).max(b);
        let d = max - r.min(g).min(b);
        let s = if max == 0.0 { 0.0 } else { d / max };
        let h = if d == 0.0 { 0.0 } else { hue(r, g, b, max, d) };
        (h, s * 100.0, max * 100.0)
    }

    pub fn format(self, f: Format) -> String {
        match f {
            Format::Hex => format!("#{:02X}{:02X}{:02X}", self.0, self.1, self.2),
            Format::Rgb => format!("rgb({}, {}, {})", self.0, self.1, self.2),
            Format::Hsl => {
                let (h, s, l) = self.hsl();
                format!("hsl({:.0}, {:.0}%, {:.0}%)", h, s, l)
            }
            Format::Hsv => {
                let (h, s, v) = self.hsv();
                format!("hsv({:.0}, {:.0}%, {:.0}%)", h, s, v)
            }
            Format::Cmyk => {
                let (r, g, b) = (self.0 as f32 / 255.0, self.1 as f32 / 255.0, self.2 as f32 / 255.0);
                let k = 1.0 - r.max(g).max(b);
                if k >= 1.0 {
                    return "cmyk(0%, 0%, 0%, 100%)".into();
                }
                let c = (1.0 - r - k) / (1.0 - k);
                let m = (1.0 - g - k) / (1.0 - k);
                let y = (1.0 - b - k) / (1.0 - k);
                format!("cmyk({:.0}%, {:.0}%, {:.0}%, {:.0}%)", c * 100.0, m * 100.0, y * 100.0, k * 100.0)
            }
            Format::Decimal => format!("{}", (self.0 as u32) << 16 | (self.1 as u32) << 8 | self.2 as u32),
        }
    }

    /// The colour as `0xRRGGBB`.
    pub fn as_u32(self) -> u32 {
        (self.0 as u32) << 16 | (self.1 as u32) << 8 | self.2 as u32
    }
}

fn hue(r: f32, g: f32, b: f32, max: f32, d: f32) -> f32 {
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h * 60.0
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Store {
    pub history: Vec<Rgb>,
    pub palettes: Vec<(String, Vec<Rgb>)>,
    pub format: Format,
}

pub const HISTORY_CAP: usize = 30;

impl Store {
    /// Most recent first, de-duplicated, capped.
    pub fn push(&mut self, c: Rgb) {
        self.history.retain(|x| *x != c);
        self.history.insert(0, c);
        self.history.truncate(HISTORY_CAP);
    }
}

#[derive(Clone)]
pub struct PickerHandle {
    store: Arc<Mutex<Store>>,
    path: PathBuf,
}

impl PickerHandle {
    pub fn get(&self) -> Store {
        self.store.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn update(&self, f: impl FnOnce(&mut Store)) {
        if let Ok(mut s) = self.store.lock() {
            f(&mut s);
            if let Ok(json) = serde_json::to_string_pretty(&*s) {
                let _ = std::fs::write(&self.path, json);
            }
        }
    }
}

pub struct ColorPicker {
    handle: PickerHandle,
}

impl ColorPicker {
    pub fn new(path: PathBuf) -> Self {
        let store = std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Self { handle: PickerHandle { store: Arc::new(Mutex::new(store)), path } }
    }

    pub fn handle(&self) -> PickerHandle {
        self.handle.clone()
    }
}

impl Module for ColorPicker {
    fn manifest(&self) -> &Manifest {
        &MANIFEST
    }
    fn start(&mut self, _: &ModuleContext) -> anyhow::Result<()> {
        Ok(())
    }
    fn stop(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_accent() {
        let c = Rgb(0x81, 0x8C, 0xF8);
        assert_eq!(c.format(Format::Hex), "#818CF8");
        assert_eq!(c.format(Format::Rgb), "rgb(129, 140, 248)");
        assert_eq!(c.format(Format::Hsl), "hsl(234, 89%, 74%)");
        assert_eq!(c.format(Format::Decimal), "8490232");
        assert_eq!(Rgb(0, 0, 0).format(Format::Cmyk), "cmyk(0%, 0%, 0%, 100%)");
        assert_eq!(Rgb(255, 0, 0).format(Format::Hsv), "hsv(0, 100%, 100%)");
    }

    #[test]
    fn history_dedupes_and_caps() {
        let mut s = Store::default();
        for i in 0..40u8 {
            s.push(Rgb(i, 0, 0));
        }
        s.push(Rgb(35, 0, 0));
        assert_eq!(s.history.len(), HISTORY_CAP);
        assert_eq!(s.history[0], Rgb(35, 0, 0));
        assert_eq!(s.history.iter().filter(|c| **c == Rgb(35, 0, 0)).count(), 1);
    }

    #[test]
    fn persists() {
        let p = std::env::temp_dir().join(format!("halo-colors-{}.json", std::process::id()));
        let h = ColorPicker::new(p.clone()).handle();
        h.update(|s| s.push(Rgb(1, 2, 3)));
        assert_eq!(ColorPicker::new(p.clone()).handle().get().history[0], Rgb(1, 2, 3));
        let _ = std::fs::remove_file(p);
    }
}
