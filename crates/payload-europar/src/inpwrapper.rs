//! The cabinet shell's `InpWrapper.exe` (Daytona Championship USA): the force feedback board.
//!
//! The game starts `c:\sega\shell\InpWrapper.exe -h=<its window> -p=<parallel port>` (`0x528b70`),
//! a 64-bit program writing to the parallel port for it. InpWrapper makes itself known with a
//! `WM_COPYDATA` to the game's window (`dwData` 3, its window handle: `0x4dcd90`); the game then
//! sends it each byte for the port (`dwData` 0x20, 1 byte, `0x529570`), again until the answer
//! is 1.
//!
//! `WAL_EUROPAR_INPWRAPPER=1`: the payload plays InpWrapper (the program is not started): its
//! window answers the bytes, which go to the force feedback of `port_ffb` (`WAL_EUROPAR_FFB`,
//! `WAL_EUROPAR_FFB_FORMAT=daytona`, data port 0x378).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExA, DefWindowProcA, DispatchMessageA, GetMessageA, MSG, RegisterClassA, SMTO_ABORTIFHUNG,
    SendMessageTimeoutA, TranslateMessage, WM_COPYDATA, WNDCLASSA, WS_POPUP,
};

/// `dwData` of the game's port bytes.
const MSG_PORT_BYTE: usize = 0x20;
/// `dwData` of InpWrapper's window, to the game.
const MSG_WINDOW: usize = 3;
const DATA_PORT: u16 = 0x378;

static ORIG_CREATE_PROCESS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_COPYDATA && lparam != 0 {
        let cds = unsafe { &*(lparam as *const COPYDATASTRUCT) };
        if cds.dwData == MSG_PORT_BYTE && cds.cbData >= 1 && !cds.lpData.is_null() {
            let byte = unsafe { *(cds.lpData as *const u8) };
            wal_payload_common::port_ffb::out("WAL_EUROPAR", DATA_PORT, byte as u16);
            return 1;
        }
        log!("inpwrapper: message {:#x} ({} bytes)", cds.dwData, cds.cbData);
        return 1;
    }
    unsafe { DefWindowProcA(hwnd, msg, wparam, lparam) }
}

/// InpWrapper's window (its own thread pumps it), then made known to the game's window.
fn start(game: isize) {
    std::thread::spawn(move || unsafe {
        let class = b"WalInpWrapper\0";
        let wc = WNDCLASSA { lpfnWndProc: Some(wndproc), lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
        RegisterClassA(&wc);
        let hwnd = CreateWindowExA(0, class.as_ptr(), class.as_ptr(), WS_POPUP, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
        log!("inpwrapper: window {:#x} for the game's {game:#x}", hwnd as usize);
        // from another thread: the game's window thread may be the one starting InpWrapper
        let own = hwnd as usize;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            let value = own as u32;
            let cds = COPYDATASTRUCT { dwData: MSG_WINDOW, cbData: 4, lpData: &value as *const u32 as *mut _ };
            let mut result = 0usize;
            let ok = SendMessageTimeoutA(game as HWND, WM_COPYDATA, own, &cds as *const COPYDATASTRUCT as LPARAM, SMTO_ABORTIFHUNG, 5000, &mut result);
            log!("inpwrapper: window sent to the game ({})", if ok != 0 { "done" } else { "no answer" });
        });
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageA(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageA(&msg);
        }
    });
}

type CreateProcessA = unsafe extern "system" fn(
    *const u8,
    *mut u8,
    *const std::ffi::c_void,
    *const std::ffi::c_void,
    i32,
    u32,
    *const std::ffi::c_void,
    *const u8,
    *const std::ffi::c_void,
    *mut std::ffi::c_void,
) -> i32;

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn create_process(
    app: *const u8,
    cmd: *mut u8,
    pa: *const std::ffi::c_void,
    ta: *const std::ffi::c_void,
    inherit: i32,
    flags: u32,
    env: *const std::ffi::c_void,
    dir: *const u8,
    si: *const std::ffi::c_void,
    pi: *mut std::ffi::c_void,
) -> i32 {
    let text = |p: *const u8| if p.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_string_lossy().into_owned() };
    let app_s = text(app);
    if app_s.to_ascii_lowercase().ends_with("inpwrapper.exe") {
        let args = text(cmd);
        // `-h=%08x -p=%08x`
        let game = args.split_whitespace().find_map(|a| a.strip_prefix("-h=")).and_then(|h| isize::from_str_radix(h, 16).ok());
        log!("inpwrapper: {app_s} {args}: played by the payload");
        if let Some(game) = game {
            start(game);
        }
        // not started (the game only closes the thread handle of a started one)
        return 0;
    }
    let f: CreateProcessA = unsafe { std::mem::transmute(ORIG_CREATE_PROCESS.load(Ordering::Relaxed)) };
    unsafe { f(app, cmd, pa, ta, inherit, flags, env, dir, si, pi) }
}

pub(crate) fn init() {
    if !std::env::var("WAL_EUROPAR_INPWRAPPER").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    if let Some(o) = unsafe { iat::hook("kernel32.dll", "CreateProcessA", create_process as *const () as usize) } {
        ORIG_CREATE_PROCESS.store(o, Ordering::Relaxed);
    }
}
