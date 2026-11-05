//! The APM3 system's e-money UI window. On a cabinet `emoneyUI.exe` runs beside the game; Blade
//! Strangers waits at boot until a top-level window titled `emoneyUI` exists (`EnumWindows` +
//! `GetWindowTextW`, every 10 ms). The payload makes one: hidden, its own thread and message
//! loop, nothing drawn (`WAL_APM3_EMONEY_WINDOW=0`: none).

use wal_payload_common::log;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, RegisterClassW, TranslateMessage, WNDCLASSW,
    WS_EX_TOOLWINDOW, WS_POPUP,
};

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub(crate) fn start() {
    if crate::env("WAL_APM3_EMONEY_WINDOW").is_some_and(|v| v == "0") {
        return;
    }
    std::thread::spawn(|| unsafe {
        let class = wide("WalApm3EmoneyUI");
        let title = wide("emoneyUI");
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            1,
            1,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if hwnd.is_null() {
            log!("apm3: emoneyUI window not created");
            return;
        }
        log!("apm3: emoneyUI window {hwnd:?} (hidden)");
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    });
}
