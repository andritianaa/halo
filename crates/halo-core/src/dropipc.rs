//! File-based channel between Halo (elevated) and its drop helper (not elevated).
//!
//! Windows blocks drag-and-drop from a normal app (Explorer) into an elevated one, so
//! a non-elevated helper process keeps an invisible drop window over the notch and
//! passes what is dropped back to Halo through small files in a shared folder:
//! - `notch-area`: where the open notch is (written by Halo);
//! - `drag-over`: a file drag hovers the notch, with the zone under the cursor
//!   (written/removed by the helper);
//! - `drop-*.txt`: one dropped batch — zone on the first line, then one path per line.
//!
//! Plain files keep both sides simple and avoid cross-integrity IPC permissions.

use std::path::{Path, PathBuf};

pub const AREA_FILE: &str = "notch-area";
pub const DRAG_FILE: &str = "drag-over";
pub const DROP_PREFIX: &str = "drop-";

/// Drop zones of the open notch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropZone {
    Vault,
    Shelf,
}

impl DropZone {
    pub fn as_str(self) -> &'static str {
        match self {
            DropZone::Vault => "vault",
            DropZone::Shelf => "shelf",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "vault" => Some(DropZone::Vault),
            "shelf" => Some(DropZone::Shelf),
            _ => None,
        }
    }
}

/// Screen rectangle (physical px) of the open notch, and its layout direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotchArea {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// Side anchors: zones are stacked (vault on top) instead of side by side.
    pub vertical: bool,
}

impl NotchArea {
    pub fn to_line(self) -> String {
        format!("{} {} {} {} {}", self.x, self.y, self.w, self.h, self.vertical as u8)
    }

    pub fn parse(s: &str) -> Option<Self> {
        let v: Vec<i32> = s.split_whitespace().map(|t| t.parse().ok()).collect::<Option<_>>()?;
        match v[..] {
            [x, y, w, h, vert] if w > 0 && h > 0 => Some(Self { x, y, w, h, vertical: vert != 0 }),
            _ => None,
        }
    }

    pub fn contains(self, (px, py): (i32, i32)) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }

    /// Zone under a screen point: the vault is the first half (left, or top).
    pub fn zone_at(self, (px, py): (i32, i32)) -> DropZone {
        let first = if self.vertical { py < self.y + self.h / 2 } else { px < self.x + self.w / 2 };
        if first { DropZone::Vault } else { DropZone::Shelf }
    }
}

/// Contents of a drop file.
pub fn format_drop(zone: DropZone, paths: &[PathBuf]) -> String {
    let mut s = String::from(zone.as_str());
    for p in paths {
        s.push('\n');
        s.push_str(&p.display().to_string());
    }
    s
}

pub fn parse_drop(s: &str) -> Option<(DropZone, Vec<PathBuf>)> {
    let mut lines = s.lines();
    let zone = DropZone::parse(lines.next()?)?;
    let paths: Vec<PathBuf> = lines.map(str::trim).filter(|l| !l.is_empty()).map(PathBuf::from).collect();
    (!paths.is_empty()).then_some((zone, paths))
}

/// Write a file atomically (temp + rename) so the reader never sees half of it.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Take (read and delete) every pending drop in `dir`, oldest first.
pub fn take_drops(dir: &Path) -> Vec<(DropZone, Vec<PathBuf>)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(DROP_PREFIX) && n.ends_with(".txt"))
        })
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|f| {
            let text = std::fs::read_to_string(&f).ok();
            let _ = std::fs::remove_file(&f);
            parse_drop(&text?)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_roundtrip_and_zones() {
        let a = NotchArea { x: 100, y: 0, w: 800, h: 300, vertical: false };
        assert_eq!(NotchArea::parse(&a.to_line()), Some(a));
        assert!(a.contains((500, 10)) && !a.contains((50, 10)) && !a.contains((500, 300)));
        assert_eq!(a.zone_at((200, 100)), DropZone::Vault);
        assert_eq!(a.zone_at((700, 100)), DropZone::Shelf);
        let v = NotchArea { vertical: true, ..a };
        assert_eq!(v.zone_at((700, 50)), DropZone::Vault);
        assert_eq!(v.zone_at((200, 250)), DropZone::Shelf);
        assert_eq!(NotchArea::parse("1 2 0 4 0"), None, "empty area rejected");
        assert_eq!(NotchArea::parse("junk"), None);
    }

    #[test]
    fn drops_roundtrip_through_files() {
        let dir = std::env::temp_dir().join(format!("halo-dropipc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let paths = vec![PathBuf::from(r"C:\a b\report.pdf"), PathBuf::from(r"D:\x.png")];
        write_atomic(&dir.join("drop-0001.txt"), &format_drop(DropZone::Shelf, &paths)).unwrap();
        write_atomic(&dir.join("drop-0002.txt"), "vault\n").unwrap(); // no paths: ignored
        std::fs::write(dir.join("notch-area"), "0 0 1 1 0").unwrap(); // not a drop
        let got = take_drops(&dir);
        assert_eq!(got, vec![(DropZone::Shelf, paths)]);
        assert!(take_drops(&dir).is_empty(), "consumed");
        assert!(dir.join("notch-area").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
