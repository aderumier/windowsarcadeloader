//! UltraPin: the frontend (`plfe.exe`) and the tables' player (`VPinball.exe`, Global VR's Visual
//! Pinball, started by the frontend for each play: `VPinball.exe -minimized -play <table>`).
//!
//! Both link the HASP4 API (the `hasp(service, seed, port, pw1, pw2, &p1, &p2, &p3, &p4)` stub,
//! cdecl, the same library as Far Cry's) and look for the dongles on the USB ports 201 and up:
//! the base dongle ("UPCOIN") and expansion packs ("UPCNXP", whose tables the operator menu merges
//! into the base one). The stub is found by its code and jumps to [`hasp`]: a base dongle on port
//! 201 only, its 112-byte memory: game id, version, "CTRY", country, then at 0x5c a 160-bit table
//! mask (a bit cleared = the table enabled: all the tables on).
//!
//! The frontend's `CreateProcessW` starts `VPinball.exe` through `wal-loader` ([`create_process`]).
//!
//! The cabinet's buttons are its control board's DirectInput joystick (the fake wheel of the
//! profile, `WAL_DINPUT_WHEEL_BUTTONS` in the cabinet's order: 0 coin 1, 1 coin 2, 2 test,
//! 3 service, 4 bill acceptor, 5 / 6 volume up / down, 8 / 9 left flipper 1 / 2, 10 / 11 right
//! flipper 1 / 2, 12 start, 13 plunger, 14 exit game), read by both programs (the frontend at
//! `0x406442`, the tables' player as buffered events). The PC keyboard they also read (the
//! frontend's window messages: arrows, Shift, 1, 5..., Esc quits it) is hidden: their
//! `PeekMessage` / `GetMessage` return its messages as `WM_NULL` ([`filter_keys`]).
//!
//! The cabinet's monitors are turned on their side: the tables' and the frontend's pictures are
//! turned upright ([`crate::ultrapin_screen`], [`crate::ultrapin_frontend`]), the DMD (on the
//! backglass monitor in the cabinet) drawn into the table's picture.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::Foundation::{CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, HWND, LPARAM, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, INFINITE, WaitForSingleObject};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowW, GetWindowThreadProcessId, IsWindowVisible, MSG, SW_HIDE, SW_SHOW, SetForegroundWindow,
    ShowWindowAsync, WM_KEYFIRST, WM_KEYLAST, WM_NULL,
};

/// The start of the `hasp()` stub (`push ebp; mov ebp,esp; push eax/ebx/ecx/edx/edi/esi;
/// mov esi,[ebp+1c]; mov edi,[esi]; mov ebx,0; mov ebx,[ebp+8]; mov bh,bl; mov bl,0;
/// add ebx,[ebp+10]`).
const STUB: [u8; 29] = [
    0x55, 0x8B, 0xEC, 0x50, 0x53, 0x51, 0x52, 0x57, 0x56, 0x8B, 0x75, 0x1C, 0x8B, 0x3E, 0xBB, 0x00, 0x00, 0x00, 0x00, 0x8B,
    0x5D, 0x08, 0x8A, 0xFB, 0xB3, 0x00, 0x03, 0x5D, 0x10,
];

const IS_HASP: i32 = 1;
const HASP_STATUS: i32 = 5;
const READ_BLOCK: i32 = 50;
const WRITE_BLOCK: i32 = 51;

/// The port of the base dongle (the first USB key).
const BASE_PORT: i32 = 201;
/// Writes a `jmp to` at `at`.
fn patch_jump(at: usize, to: usize) {
    let mut code = [0xE9u8, 0, 0, 0, 0];
    code[1..].copy_from_slice(&(to.wrapping_sub(at + 5) as i32).to_le_bytes());
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, 5, PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, 5);
        VirtualProtect(at as *const c_void, 5, old, &mut old);
    }
}

/// The base dongle's memory (112 bytes).
static MEMORY: Mutex<[u8; 0x70]> = Mutex::new(memory());

const fn memory() -> [u8; 0x70] {
    let mut m = [0u8; 0x70];
    let head = b"UPCOIN1.0CTRYUS";
    let mut i = 0;
    while i < head.len() {
        m[i] = head[i];
        i += 1;
    }
    m
}

pub(crate) fn init() {
    let exe = crate::frontend_fit::exe_name();
    if exe != "plfe.exe" && exe != "vpinball.exe" {
        return;
    }
    crate::ushock::init();
    if exe == "vpinball.exe" {
        crate::ultrapin_screen::init();
        crate::ultrapin_screen::hook_present();
    }
    filter_keys();
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let image = unsafe { std::slice::from_raw_parts(base as *const u8, crate::exe_size()) };
    let Some(at) = image.windows(STUB.len()).position(|w| w == STUB) else {
        log!("ultrapin: {exe}: hasp() not found, dongle not emulated");
        return;
    };
    let stub = base + at;
    patch_jump(stub, hasp as *const () as usize);
    log!("ultrapin: {exe}: hasp() at {stub:#x} answered");
    if exe == "plfe.exe" {
        crate::ultrapin_movie::init();
        if crate::ultrapin_screen::rotated() {
            crate::ultrapin_frontend::init();
        }
        if let Some(o) = unsafe { iat::hook_module(base, "kernel32.dll", "CreateProcessW", create_process as *const () as usize) } {
            ORIG_CREATE_PROCESS.store(o, Ordering::Relaxed);
        }
    }
}

static ORIG_CREATE_PROCESS: AtomicUsize = AtomicUsize::new(0);

static PEEK_A: AtomicUsize = AtomicUsize::new(0);
static PEEK_W: AtomicUsize = AtomicUsize::new(0);
static GET_A: AtomicUsize = AtomicUsize::new(0);
static GET_W: AtomicUsize = AtomicUsize::new(0);

/// The PC keyboard's messages hidden from the executable (input from the launcher only).
fn filter_keys() {
    let hooks: [(&str, &AtomicUsize, usize); 4] = [
        ("PeekMessageA", &PEEK_A, peek_a as *const () as usize),
        ("PeekMessageW", &PEEK_W, peek_w as *const () as usize),
        ("GetMessageA", &GET_A, get_a as *const () as usize),
        ("GetMessageW", &GET_W, get_w as *const () as usize),
    ];
    for (name, orig, hook) in hooks {
        if let Some(o) = unsafe { iat::hook("user32.dll", name, hook) } {
            orig.store(o, Ordering::Relaxed);
        }
    }
}

/// A keyboard message made `WM_NULL`.
unsafe fn drop_key(msg: *mut MSG) {
    if !msg.is_null() && (WM_KEYFIRST..=WM_KEYLAST).contains(&unsafe { (*msg).message }) {
        unsafe { (*msg).message = WM_NULL };
    }
}

type PeekMessage = unsafe extern "system" fn(*mut MSG, usize, u32, u32, u32) -> i32;
type GetMessage = unsafe extern "system" fn(*mut MSG, usize, u32, u32) -> i32;

unsafe fn peek(orig: &AtomicUsize, msg: *mut MSG, hwnd: usize, min: u32, max: u32, remove: u32) -> i32 {
    let f: PeekMessage = unsafe { std::mem::transmute(orig.load(Ordering::Relaxed)) };
    let r = unsafe { f(msg, hwnd, min, max, remove) };
    if r != 0 {
        unsafe { drop_key(msg) };
    }
    r
}

unsafe fn get(orig: &AtomicUsize, msg: *mut MSG, hwnd: usize, min: u32, max: u32) -> i32 {
    let f: GetMessage = unsafe { std::mem::transmute(orig.load(Ordering::Relaxed)) };
    let r = unsafe { f(msg, hwnd, min, max) };
    if r > 0 {
        unsafe { drop_key(msg) };
    }
    r
}

unsafe extern "system" fn peek_a(msg: *mut MSG, hwnd: usize, min: u32, max: u32, remove: u32) -> i32 {
    unsafe { peek(&PEEK_A, msg, hwnd, min, max, remove) }
}
unsafe extern "system" fn peek_w(msg: *mut MSG, hwnd: usize, min: u32, max: u32, remove: u32) -> i32 {
    unsafe { peek(&PEEK_W, msg, hwnd, min, max, remove) }
}
unsafe extern "system" fn get_a(msg: *mut MSG, hwnd: usize, min: u32, max: u32) -> i32 {
    unsafe { get(&GET_A, msg, hwnd, min, max) }
}
unsafe extern "system" fn get_w(msg: *mut MSG, hwnd: usize, min: u32, max: u32) -> i32 {
    unsafe { get(&GET_W, msg, hwnd, min, max) }
}

fn wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

/// The frontend's `CreateProcessW`: the tables' player started through `wal-loader`.
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn create_process(
    app: *const u16,
    cmd: *mut u16,
    pa: usize,
    ta: usize,
    inherit: i32,
    flags: u32,
    env: usize,
    dir: *const u16,
    si: usize,
    pi: usize,
) -> i32 {
    type CreateProcessW = unsafe extern "system" fn(*const u16, *mut u16, usize, usize, i32, u32, usize, *const u16, usize, usize) -> i32;
    let orig: CreateProcessW = unsafe { std::mem::transmute(ORIG_CREATE_PROCESS.load(Ordering::Relaxed)) };
    let (app_s, cmd_s) = (wide(app), wide(cmd));
    log!("ultrapin: the frontend starts {app_s:?} {cmd_s:?}");
    let Some(command) = crate::beachhead::loader_command(&app_s, &cmd_s, &crate::beachhead::payload_path()) else {
        return unsafe { orig(app, cmd, pa, ta, inherit, flags, env, dir, si, pi) };
    };
    log!("ultrapin: {command}");
    let mut command: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
    let r = unsafe { orig(std::ptr::null(), command.as_mut_ptr(), pa, ta, inherit, flags, env, dir, si, pi) };
    if r != 0 && pi != 0 {
        let mut process: HANDLE = std::ptr::null_mut();
        let me = unsafe { GetCurrentProcess() };
        if unsafe { DuplicateHandle(me, *(pi as *const HANDLE), me, &mut process, 0, 0, DUPLICATE_SAME_ACCESS) } != 0 {
            let process = process as usize;
            std::thread::spawn(move || unsafe { step_aside(process as HANDLE) });
        }
    }
    r
}

/// The frontend's windows hidden while a table plays, from its window's first showing to the
/// end of `process` (the table's window, opened behind the frontend's full screen one, stays
/// behind it on the desktop: the table then plays in a black screen).
unsafe fn step_aside(process: HANDLE) {
    let table = loop {
        if unsafe { WaitForSingleObject(process, 100) } == WAIT_OBJECT_0 {
            unsafe { CloseHandle(process) };
            return;
        }
        let name: Vec<u16> = "VPPlayer\0".encode_utf16().collect();
        let window = unsafe { FindWindowW(name.as_ptr(), std::ptr::null()) };
        if !window.is_null() && unsafe { IsWindowVisible(window) } != 0 {
            break window;
        }
    };
    unsafe extern "system" fn mine(hwnd: HWND, out: LPARAM) -> i32 {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == unsafe { GetCurrentProcessId() } && unsafe { IsWindowVisible(hwnd) } != 0 {
            unsafe { (*(out as *mut Vec<usize>)).push(hwnd as usize) };
        }
        1
    }
    let mut windows: Vec<usize> = Vec::new();
    unsafe { EnumWindows(Some(mine), &mut windows as *mut Vec<usize> as LPARAM) };
    for &w in &windows {
        unsafe { ShowWindowAsync(w as HWND, SW_HIDE) };
    }
    unsafe { SetForegroundWindow(table) };
    log!("ultrapin: the frontend's {} windows hidden while the table plays", windows.len());
    unsafe { WaitForSingleObject(process, INFINITE) };
    unsafe { CloseHandle(process) };
    for &w in &windows {
        unsafe { ShowWindowAsync(w as HWND, SW_SHOW) };
    }
    if let Some(&w) = windows.first() {
        unsafe { SetForegroundWindow(w as HWND) };
    }
    log!("ultrapin: the frontend's windows shown again");
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn hasp(
    service: i32,
    _seed: i32,
    port: i32,
    _pw1: i32,
    _pw2: i32,
    p1: *mut i32,
    p2: *mut i32,
    p3: *mut i32,
    p4: *mut i32,
) {
    let (a1, a2, a4) = unsafe { (*p1, *p2, *p4) };
    let out = if port != BASE_PORT {
        [0, 0, 0, 0]
    } else {
        match service {
            // found; p2: the dongles found (the frontend's dongle count)
            IS_HASP => [1, 1, 0, 0],
            // 112-byte memory, MemoHASP, its port
            HASP_STATUS => [1, 1, port, 0],
            READ_BLOCK | WRITE_BLOCK => {
                // p1 start word, p2 words, p4 buffer
                let (start, len, buf) = (a1 as usize * 2, a2 as usize * 2, a4 as usize as *mut u8);
                let mut memory = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
                for i in 0..len {
                    let Some(byte) = memory.get_mut(start + i) else { break };
                    unsafe {
                        if service == READ_BLOCK {
                            *buf.add(i) = *byte;
                        } else {
                            *byte = *buf.add(i);
                        }
                    }
                }
                [a1, a2, 0, a4]
            }
            _ => [0, 0, 0, 0],
        }
    };
    unsafe { [*p1, *p2, *p3, *p4] = out };
    if port == BASE_PORT && service != READ_BLOCK {
        log!("ultrapin: hasp service {service} -> {out:?}");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn memory_layout() {
        let m = super::memory();
        assert_eq!(&m[..6], b"UPCOIN");
        assert_eq!(&m[6..9], b"1.0");
        assert_eq!(&m[9..13], b"CTRY");
        assert_eq!(&m[13..15], b"US");
        // the table mask at 0x5c: every table on
        assert!(m[0x5c..].iter().all(|&b| b == 0));
    }
}
