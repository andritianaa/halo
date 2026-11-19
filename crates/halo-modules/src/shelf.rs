//! File shelf helpers: files dropped on the notch are kept as shortcuts (the list
//! itself lives in `ModuleSettings::shelf`). Nothing here touches the files.

use std::path::Path;

/// Lucide icon for a file, from its extension.
pub fn file_icon(path: &Path) -> &'static str {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "avif" | "bmp" | "svg" | "heic" | "ico" => "image",
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" | "opus" => "music",
        "mp4" | "mkv" | "mov" | "avi" | "webm" => "video",
        "zip" | "7z" | "rar" | "tar" | "gz" | "xz" => "file-archive",
        "pdf" | "doc" | "docx" | "txt" | "md" | "rtf" | "odt" => "file-text",
        "xls" | "xlsx" | "csv" | "ods" => "file-spreadsheet",
        "rs" | "js" | "ts" | "py" | "json" | "toml" | "html" | "css" | "c" | "cpp" | "cs" | "go" => "file-code",
        "exe" | "msi" | "lnk" => "app-window",
        _ if path.is_dir() => "folder",
        _ => "file",
    }
}

/// Display name of a shelf entry.
pub fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icons_by_extension() {
        assert_eq!(file_icon(Path::new("C:\\x\\Q3.PDF")), "file-text");
        assert_eq!(file_icon(Path::new("a.png")), "image");
        assert_eq!(file_icon(Path::new("setup.exe")), "app-window");
        assert_eq!(file_icon(Path::new("noext")), "file");
        assert_eq!(file_name(Path::new("C:\\dir\\report.pdf")), "report.pdf");
    }
}
