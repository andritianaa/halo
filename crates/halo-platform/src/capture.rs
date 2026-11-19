//! Screen pixels via GDI (works with DWM composition): colour picker (M11),
//! screenshots (M12) and screen recording frames (M13).

use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, GetPixel, ReleaseDC, SRCCOPY, SelectObject,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::Rect;

/// RGBA8 image, rows top-down.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Colour of one screen pixel (physical coordinates).
pub fn pixel(x: i32, y: i32) -> Option<(u8, u8, u8)> {
    unsafe {
        let dc = GetDC(None);
        let c = GetPixel(dc, x, y);
        ReleaseDC(None, dc);
        if c.0 == u32::MAX {
            return None; // CLR_INVALID (outside the desktop)
        }
        Some(((c.0 & 0xFF) as u8, ((c.0 >> 8) & 0xFF) as u8, ((c.0 >> 16) & 0xFF) as u8))
    }
}

/// Bounding rectangle of all monitors (physical px).
pub fn virtual_screen() -> Rect {
    unsafe {
        Rect::new(
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

/// Copy a screen rectangle (physical px) into an RGBA image.
pub fn grab(r: Rect) -> Option<Image> {
    grab_impl(r, false)
}

/// Like [`grab`], with the mouse pointer drawn in (screen recordings).
pub fn grab_with_cursor(r: Rect) -> Option<Image> {
    grab_impl(r, true)
}

/// Draw the current cursor (if shown) into `dc`, whose origin is screen point `origin`.
unsafe fn draw_cursor(dc: windows::Win32::Graphics::Gdi::HDC, origin: (i32, i32)) {
    use windows::Win32::UI::WindowsAndMessaging::{CURSOR_SHOWING, CURSORINFO, DI_NORMAL, DrawIconEx, GetCursorInfo, GetIconInfo, HICON, ICONINFO};
    unsafe {
        let mut ci = CURSORINFO { cbSize: size_of::<CURSORINFO>() as u32, ..Default::default() };
        if GetCursorInfo(&mut ci).is_err() || ci.flags.0 & CURSOR_SHOWING.0 == 0 {
            return;
        }
        let icon = HICON(ci.hCursor.0);
        let mut ii = ICONINFO::default();
        let (hx, hy) = if GetIconInfo(icon, &mut ii).is_ok() {
            if !ii.hbmMask.is_invalid() {
                let _ = DeleteObject(ii.hbmMask.into());
            }
            if !ii.hbmColor.is_invalid() {
                let _ = DeleteObject(ii.hbmColor.into());
            }
            (ii.xHotspot as i32, ii.yHotspot as i32)
        } else {
            (0, 0)
        };
        let _ = DrawIconEx(dc, ci.ptScreenPos.x - hx - origin.0, ci.ptScreenPos.y - hy - origin.1, icon, 0, 0, 0, None, DI_NORMAL);
    }
}

fn grab_impl(r: Rect, cursor: bool) -> Option<Image> {
    if r.w <= 0 || r.h <= 0 {
        return None;
    }
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, r.w, r.h);
        let old = SelectObject(mem, bmp.into());
        let ok = BitBlt(mem, 0, 0, r.w, r.h, Some(screen), r.x, r.y, SRCCOPY | CAPTUREBLT).is_ok();
        if ok && cursor {
            draw_cursor(mem, (r.x, r.y));
        }
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: r.w,
                biHeight: -r.h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut buf = vec![0u8; (r.w * r.h * 4) as usize];
        let lines = if ok {
            GetDIBits(mem, bmp, 0, r.h as u32, Some(buf.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS)
        } else {
            0
        };
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if lines == 0 {
            return None;
        }
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2); // BGRA → RGBA
            px[3] = 255;
        }
        Some(Image { width: r.w as u32, height: r.h as u32, rgba: buf })
    }
}
