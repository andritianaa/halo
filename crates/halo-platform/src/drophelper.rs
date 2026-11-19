//! Drop helper: a separate, **non-elevated** process that receives files dragged onto
//! the notch while Halo itself runs as administrator.
//!
//! Windows forbids dragging from a normal-integrity app (Explorer) into an elevated
//! window, so the helper keeps an invisible, click-through window over the area of the
//! open notch. While a mouse button is held after pressing *outside* that area (a drag
//! that may be heading for the notch), the window stops being click-through so OLE can
//! target it; `IDropTarget` then only reacts to real file drags (`CF_HDROP`), never to
//! window moves or text selections. Hover and drops are reported to Halo through the
//! files of [`halo_core::dropipc`].

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use halo_core::dropipc::{self, DropZone, NotchArea};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, POINTL, S_OK, WPARAM};
use windows::Win32::Graphics::Gdi::HBRUSH;
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, IDropTarget, IDropTarget_Impl, OleInitialize, RegisterDragDrop,
    ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GWL_EXSTYLE, GetCursorPos, GetWindowLongW, HWND_TOPMOST, LWA_ALPHA, MSG,
    PM_REMOVE, PeekMessageW, RegisterClassW, SW_HIDE, SWP_NOACTIVATE, SWP_SHOWWINDOW, SetLayeredWindowAttributes, SetWindowLongW,
    SetWindowPos, ShowWindow, TranslateMessage, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{Ref, implement, w};

/// What the drop target shares with the polling loop.
struct Shared {
    dir: PathBuf,
    area: Cell<Option<NotchArea>>,
    /// A file drag is over the window.
    over: Cell<bool>,
    last_zone: Cell<Option<DropZone>>,
}

impl Shared {
    fn report(&self, pt: &POINTL) {
        let Some(area) = self.area.get() else { return };
        let zone = area.zone_at((pt.x, pt.y));
        if self.last_zone.get() != Some(zone) {
            self.last_zone.set(Some(zone));
            let _ = dropipc::write_atomic(&self.dir.join(dropipc::DRAG_FILE), zone.as_str());
        }
    }

    fn clear(&self) {
        self.over.set(false);
        self.last_zone.set(None);
        let _ = std::fs::remove_file(self.dir.join(dropipc::DRAG_FILE));
    }
}

fn hdrop_format() -> FORMATETC {
    FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as _ }
}

/// Paths carried by a file drag.
fn dropped_paths(data: &IDataObject) -> Vec<PathBuf> {
    let mut out = Vec::new();
    unsafe {
        let Ok(mut medium) = data.GetData(&hdrop_format()) else { return out };
        if !medium.u.hGlobal.is_invalid() {
            let hdrop = HDROP(medium.u.hGlobal.0);
            let n = DragQueryFileW(hdrop, u32::MAX, None);
            for i in 0..n {
                let len = DragQueryFileW(hdrop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(hdrop, i, Some(&mut buf));
                out.push(PathBuf::from(String::from_utf16_lossy(&buf[..len])));
            }
        }
        ReleaseStgMedium(&mut medium);
    }
    out
}

#[implement(IDropTarget)]
struct Target(Rc<Shared>);

#[allow(non_snake_case)]
impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, data: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let files = data.ok().is_ok_and(|d| unsafe { d.QueryGetData(&hdrop_format()) } == S_OK);
        unsafe { *effect = if files { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        if files {
            self.0.over.set(true);
            self.0.report(pt);
        }
        Ok(())
    }

    fn DragOver(&self, _: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        let over = self.0.over.get();
        unsafe { *effect = if over { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        if over {
            self.0.report(pt);
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        self.0.clear();
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, pt: &POINTL, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        unsafe { *effect = DROPEFFECT_COPY };
        if let (Ok(d), Some(area)) = (data.ok(), self.0.area.get()) {
            let paths = dropped_paths(d);
            if !paths.is_empty() {
                let zone = area.zone_at((pt.x, pt.y));
                let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
                let file = self.0.dir.join(format!("{}{stamp:024}.txt", dropipc::DROP_PREFIX));
                let _ = dropipc::write_atomic(&file, &dropipc::format_drop(zone, &paths));
            }
        }
        self.0.clear();
        Ok(())
    }
}

unsafe extern "system" fn wndproc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(h, msg, w, l) }
}

fn parent_alive(pid: u32) -> bool {
    unsafe {
        match OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            // WAIT_TIMEOUT (258): still running.
            Ok(h) => {
                let r = WaitForSingleObject(h, 0).0 == 258;
                let _ = windows::Win32::Foundation::CloseHandle(h);
                r
            }
            Err(_) => false,
        }
    }
}

fn read_area(dir: &Path) -> Option<NotchArea> {
    NotchArea::parse(&std::fs::read_to_string(dir.join(dropipc::AREA_FILE)).ok()?)
}

/// Run the helper until Halo (`parent_pid`) exits. Blocks.
pub fn run(dir: PathBuf, parent_pid: u32) {
    let _ = std::fs::create_dir_all(&dir);
    unsafe {
        let _ = OleInitialize(None);
        let inst = GetModuleHandleW(None).unwrap_or_default();
        let class = w!("HaloDropHelper");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: inst.into(), lpszClassName: class, hbrBackground: HBRUSH::default(), ..Default::default() };
        RegisterClassW(&wc);
        let ex = WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT;
        let Ok(hwnd) = CreateWindowExW(ex, class, w!("Halo drop"), WS_POPUP, 0, 0, 1, 1, None, None, Some(inst.into()), None) else {
            log::error!("drop helper: cannot create its window");
            return;
        };
        // Alpha 1/255: invisible, yet hit-testable when not click-through.
        let _ = SetLayeredWindowAttributes(hwnd, Default::default(), 1, LWA_ALPHA);

        let shared = Rc::new(Shared { dir: dir.clone(), area: Cell::new(read_area(&dir)), over: Cell::new(false), last_zone: Cell::new(None) });
        let target: IDropTarget = Target(shared.clone()).into();
        if let Err(e) = RegisterDragDrop(hwnd, &target) {
            log::error!("drop helper: RegisterDragDrop failed: {e}");
            return;
        }
        log::info!("drop helper running for pid {parent_pid}");

        let mut armed = false;
        let mut was_down = false;
        let mut press_inside = false;
        let mut last_check = Instant::now();
        let mut shown = false;
        loop {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if last_check.elapsed() >= Duration::from_secs(1) {
                last_check = Instant::now();
                if !parent_alive(parent_pid) {
                    break;
                }
                shared.area.set(read_area(&dir));
            }
            let mut cur = POINT::default();
            let _ = GetCursorPos(&mut cur);
            let down = GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000 != 0;
            let area = shared.area.get();
            if down && !was_down {
                press_inside = area.is_some_and(|a| a.contains((cur.x, cur.y)));
            }
            was_down = down;
            // A drag started elsewhere may be heading for the notch: become a target.
            let want = (down && !press_inside && area.is_some()) || shared.over.get();
            if want != armed {
                armed = want;
                let style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
                let style = if want { style & !WS_EX_TRANSPARENT.0 } else { style | WS_EX_TRANSPARENT.0 };
                SetWindowLongW(hwnd, GWL_EXSTYLE, style as i32);
            }
            match (want, area) {
                (true, Some(a)) => {
                    // Stay above the (topmost) notch window, which Halo re-places as it opens.
                    let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), a.x, a.y, a.w, a.h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                    shown = true;
                }
                _ if shown => {
                    let _ = ShowWindow(hwnd, SW_HIDE);
                    shown = false;
                }
                _ => {}
            }
            std::thread::sleep(Duration::from_millis(if want { 10 } else { 30 }));
        }
        let _ = RevokeDragDrop(hwnd);
        shared.clear();
    }
}
