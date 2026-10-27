//! Need for Speed Underground's frontend window fitted to the screen.
//!
//! `UniverShell2.exe` is a .NET program: Windows Forms creates its framed window through
//! P/Invoke, past the import hooks of `WAL_WINDOW_POPUP` / `WAL_WINDOW_SIZE`, and renders into an
//! 800x694 panel child window (its Direct3D back buffer is the panel's size: the 800x600
//! picture and the button bar below). The payload arrives later, with the input library
//! (`NFSControl.dll` loads `GVRInputRaw.dll`): from the input updates, the frontend's top-level
//! window is made a borderless popup over the largest centered rectangle of the panel's aspect,
//! the panel filling it (the whole screen with `WAL_KEEP_ASPECT_RATIO=0`), Direct3D stretching
//! the picture to it. Checked again every few seconds: the frontend rebuilds its window after
//! each race.

use std::sync::atomic::{AtomicU32, Ordering};

use wal_payload_common::log;
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GWL_STYLE, GetClientRect, GetSystemMetrics, GetWindowLongW, GetWindowRect,
    GetWindowThreadProcessId, HWND_TOP, IsWindowVisible, SM_CXSCREEN, SM_CYSCREEN, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOZORDER, SetWindowLongW, SetWindowPos, WS_CAPTION, WS_POPUP, WS_THICKFRAME,
};

/// The frontend's render panel: its 800x600 picture and the 94-pixel button bar below.
const PANEL: (i32, i32) = (800, 694);

static UPDATES: AtomicU32 = AtomicU32::new(0);

fn enabled() -> bool {
    std::env::var("WAL_WINDOW_SIZE").is_ok_and(|v| v.trim() == "screen") && exe_name() == "univershell2.exe"
}

/// The program's file name, lowercase.
pub(crate) fn exe_name() -> String {
    let mut buf = [0u16; 260];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), 260) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    path.rsplit('\\').next().unwrap_or(&path).to_ascii_lowercase()
}

/// Called from each input update.
pub(crate) fn update() {
    // about every 3 s at the frontend's rate
    if UPDATES.fetch_add(1, Ordering::Relaxed) % 180 != 0 || !enabled() {
        return;
    }
    let mut window: HWND = std::ptr::null_mut();
    unsafe { EnumWindows(Some(find_window), &mut window as *mut HWND as LPARAM) };
    if !window.is_null() {
        fit(window);
    }
}

unsafe extern "system" fn find_window(hwnd: HWND, out: LPARAM) -> i32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == unsafe { GetCurrentProcessId() } && unsafe { IsWindowVisible(hwnd) } != 0 {
        let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetClientRect(hwnd, &mut client) };
        if client.right >= PANEL.0 && client.bottom >= PANEL.1 || client.right >= 640 && client.bottom >= 480 {
            unsafe { *(out as *mut HWND) = hwnd };
            return 0;
        }
    }
    1
}

fn fit(window: HWND) {
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let [x, y, w, h] = panel_rect(sw, sh);
    let style = unsafe { GetWindowLongW(window, GWL_STYLE) } as u32;
    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetWindowRect(window, &mut rect) };
    // the window is the panel's place (its own white background would show around the panel)
    if style & (WS_CAPTION | WS_THICKFRAME) != 0 || [rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top] != [x, y, w, h] {
        unsafe {
            SetWindowLongW(window, GWL_STYLE, ((style & !(WS_CAPTION | WS_THICKFRAME)) | WS_POPUP) as i32);
            SetWindowPos(window, HWND_TOP, x, y, w, h, SWP_FRAMECHANGED | SWP_NOACTIVATE);
        }
        log!("frontend: window {window:?} made a {w}x{h} popup at {x},{y}");
    }
    let mut panel: (HWND, [i32; 4]) = (std::ptr::null_mut(), [0, 0, w, h]);
    unsafe { EnumChildWindows(window, Some(fit_panel), &mut panel as *mut _ as LPARAM) };
}

/// The panel's place: the largest centered rectangle of its aspect, or the whole screen.
fn panel_rect(sw: i32, sh: i32) -> [i32; 4] {
    if std::env::var("WAL_KEEP_ASPECT_RATIO").is_ok_and(|v| v.trim() == "0") {
        return [0, 0, sw, sh];
    }
    let (w, h) = if sw * PANEL.1 > sh * PANEL.0 { (sh * PANEL.0 / PANEL.1, sh) } else { (sw, sw * PANEL.1 / PANEL.0) };
    [(sw - w) / 2, (sh - h) / 2, w, h]
}

unsafe extern "system" fn fit_panel(hwnd: HWND, data: LPARAM) -> i32 {
    let data = unsafe { &mut *(data as *mut (HWND, [i32; 4])) };
    let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetClientRect(hwnd, &mut rect) };
    let size = (rect.right, rect.bottom);
    let [x, y, w, h] = data.1;
    if std::env::var("WAL_GVR_FIT_TRACE").is_ok() {
        log!("frontend: child {hwnd:?} client {}x{}", size.0, size.1);
    }
    // the render panel, or already fitted
    if size == PANEL || size == (w, h) {
        if size != (w, h) {
            unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE) };
            log!("frontend: render panel {hwnd:?} {}x{} at {x},{y}", w, h);
        }
        data.0 = hwnd;
        return 0;
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterbox() {
        assert_eq!(panel_rect(1920, 1080), [338, 0, 1244, 1080]);
        assert_eq!(panel_rect(1600, 1200), [108, 0, 1383, 1200]);
    }
}
