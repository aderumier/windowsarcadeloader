//! `WAL_ALLS_CHILDREN=<program>,...`: programs the game starts itself (`CreateProcessA`) started by
//! `wal-loader` with this payload too: Initial D ARCADE STAGE Zero's store server
//! (`ServerBoxD8_Nu_x64`, started by cabinet 1, DIP switch 1; segatools injects idzhook there
//! too). In the child its windows stay hidden (the store server's status window, a full screen
//! picture, would cover the game).

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

static REAL: AtomicUsize = AtomicUsize::new(0);

type CreateProcessA = unsafe extern "system" fn(*const c_char, *mut c_char, *const c_void, *const c_void, i32, u32, *const c_void, *const c_char, *const c_void, *mut c_void) -> i32;

/// This payload's file (`wal_alls.dll` / `wal_alls32.dll`).
fn payload_path() -> Option<String> {
    let mut buf = [0u16; 512];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(crate::module(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    (n > 0).then(|| String::from_utf16_lossy(&buf[..n]))
}

/// The child's program name (the command line's first word, without `.exe`) when it is one of
/// `WAL_ALLS_CHILDREN`.
fn child(app: Option<&str>, cmdline: Option<&str>) -> Option<String> {
    let first = app.or(cmdline)?.trim().trim_start_matches('"');
    let first = first.split(['"', ' ']).next()?;
    let name = first.rsplit(['\\', '/']).next()?.trim_end_matches(".exe").trim_end_matches(".EXE").to_string();
    let list = std::env::var("WAL_ALLS_CHILDREN").ok()?;
    list.split(',').map(str::trim).any(|c| c.eq_ignore_ascii_case(&name)).then_some(name)
}


unsafe extern "system" fn create_process_a(app: *const c_char, cmdline: *mut c_char, pa: *const c_void, ta: *const c_void, inherit: i32, flags: u32, env: *const c_void, cwd: *const c_char, si: *const c_void, pi: *mut c_void) -> i32 {
    let real: CreateProcessA = unsafe { std::mem::transmute(REAL.load(Ordering::Relaxed)) };
    let text = |p: *const c_char| (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned());
    let (app_s, cmd_s) = (text(app), text(cmdline));
    if let (Some(name), Some(payload), Some(dir)) = (child(app_s.as_deref(), cmd_s.as_deref()), payload_path(), std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.display().to_string()))) {
        // the child's own arguments after its name
        let rest = cmd_s.as_deref().map(|c| {
            let c = c.trim_start();
            if let Some(q) = c.strip_prefix('"') { q.split_once('"').map_or("", |(_, r)| r) } else { c.split_once(' ').map_or("", |(_, r)| r) }
        }).unwrap_or("");
        let line = format!("\"{dir}\\wal-loader.exe\" \"{payload}\" \"{dir}\\{name}.exe\" {}", rest.trim());
        log!("alls: {name} started with the payload: {line}");
        if let Ok(mut c) = CString::new(line.trim_end()).map(|c| c.into_bytes_with_nul()) {
            return unsafe { real(std::ptr::null(), c.as_mut_ptr().cast(), pa, ta, inherit, flags, env, cwd, si, pi) };
        }
    }
    unsafe { real(app, cmdline, pa, ta, inherit, flags, env, cwd, si, pi) }
}

pub(crate) fn init() {
    if std::env::var("WAL_ALLS_CHILDREN").is_err() {
        return;
    }
    match unsafe { iat::hook("kernel32.dll", "CreateProcessA", create_process_a as *const () as usize) } {
        Some(o) => REAL.store(o, Ordering::Relaxed),
        None => log!("alls: CreateProcessA not imported"),
    }
}

/// This process is one of `WAL_ALLS_CHILDREN`.
pub(crate) fn is_child() -> bool {
    let exe = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
    child(None, Some(&exe)).is_some()
}

static SHOW_WINDOW: AtomicUsize = AtomicUsize::new(0);
static CREATE_WINDOW_W: AtomicUsize = AtomicUsize::new(0);
static CREATE_WINDOW_A: AtomicUsize = AtomicUsize::new(0);
static SET_WINDOW_POS: AtomicUsize = AtomicUsize::new(0);

const WS_VISIBLE: u32 = 0x1000_0000;
const SWP_SHOWWINDOW: u32 = 0x40;

unsafe extern "system" fn show_window(hwnd: *mut c_void, _cmd: i32) -> i32 {
    let real: unsafe extern "system" fn(*mut c_void, i32) -> i32 = unsafe { std::mem::transmute(SHOW_WINDOW.load(Ordering::Relaxed)) };
    unsafe { real(hwnd, 0) }
}

type CreateWindow = unsafe extern "system" fn(u32, *const c_void, *const c_void, u32, i32, i32, i32, i32, *mut c_void, *mut c_void, *mut c_void, *const c_void) -> *mut c_void;

unsafe extern "system" fn create_window_w(ex: u32, class: *const c_void, name: *const c_void, style: u32, x: i32, y: i32, w: i32, h: i32, parent: *mut c_void, menu: *mut c_void, inst: *mut c_void, param: *const c_void) -> *mut c_void {
    let real: CreateWindow = unsafe { std::mem::transmute(CREATE_WINDOW_W.load(Ordering::Relaxed)) };
    unsafe { real(ex, class, name, style & !WS_VISIBLE, x, y, w, h, parent, menu, inst, param) }
}

unsafe extern "system" fn create_window_a(ex: u32, class: *const c_void, name: *const c_void, style: u32, x: i32, y: i32, w: i32, h: i32, parent: *mut c_void, menu: *mut c_void, inst: *mut c_void, param: *const c_void) -> *mut c_void {
    let real: CreateWindow = unsafe { std::mem::transmute(CREATE_WINDOW_A.load(Ordering::Relaxed)) };
    unsafe { real(ex, class, name, style & !WS_VISIBLE, x, y, w, h, parent, menu, inst, param) }
}

unsafe extern "system" fn set_window_pos(hwnd: *mut c_void, after: *mut c_void, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32 {
    let real: unsafe extern "system" fn(*mut c_void, *mut c_void, i32, i32, i32, i32, u32) -> i32 = unsafe { std::mem::transmute(SET_WINDOW_POS.load(Ordering::Relaxed)) };
    unsafe { real(hwnd, after, x, y, w, h, flags & !SWP_SHOWWINDOW) }
}

/// In the child: its windows never shown.
pub(crate) fn hide_windows() {
    let hooks: [(&str, usize, &AtomicUsize); 4] = [
        ("ShowWindow", show_window as *const () as usize, &SHOW_WINDOW),
        ("CreateWindowExW", create_window_w as *const () as usize, &CREATE_WINDOW_W),
        ("CreateWindowExA", create_window_a as *const () as usize, &CREATE_WINDOW_A),
        ("SetWindowPos", set_window_pos as *const () as usize, &SET_WINDOW_POS),
    ];
    for (name, f, slot) in hooks {
        if let Some(o) = unsafe { iat::hook("user32.dll", name, f) } {
            slot.store(o, Ordering::Relaxed);
        }
    }
    log!("alls: windows hidden");
}

static ACCEPT: AtomicUsize = AtomicUsize::new(0);
static GET_PEER_NAME: AtomicUsize = AtomicUsize::new(0);

/// A peer on the loopback address (the game beside it, its connections to the cabinet's own
/// address made local, `allnet.rs`): the cabinet's address, as on the LAN.
fn peer_from_cabinet(addr: *mut u8, len: *const i32) {
    if addr.is_null() || len.is_null() || unsafe { *len } < 8 {
        return;
    }
    let a = unsafe { std::slice::from_raw_parts_mut(addr, 8) };
    if a[0] == 2 && a[4] == 127 {
        a[4..8].copy_from_slice(&crate::netenv::iface());
    }
}

unsafe extern "system" fn accept(socket: usize, addr: *mut u8, len: *mut i32) -> usize {
    let real: unsafe extern "system" fn(usize, *mut u8, *mut i32) -> usize = unsafe { std::mem::transmute(ACCEPT.load(Ordering::Relaxed)) };
    let s = unsafe { real(socket, addr, len) };
    if s != usize::MAX {
        peer_from_cabinet(addr, len);
    }
    s
}

unsafe extern "system" fn get_peer_name(socket: usize, addr: *mut u8, len: *mut i32) -> i32 {
    let real: unsafe extern "system" fn(usize, *mut u8, *mut i32) -> i32 = unsafe { std::mem::transmute(GET_PEER_NAME.load(Ordering::Relaxed)) };
    let r = unsafe { real(socket, addr, len) };
    if r == 0 {
        peer_from_cabinet(addr, len);
    }
    r
}

/// In the child (the store server): the game's connections seen from the cabinet's address.
pub(crate) fn peers_from_cabinet() {
    let exe = iat::game_module();
    for (ordinal, name, f, slot) in [(1u16, "accept", accept as *const () as usize, &ACCEPT), (5, "getpeername", get_peer_name as *const () as usize, &GET_PEER_NAME)] {
        match unsafe { iat::hook_ordinal(exe, "WS2_32.dll", ordinal, f) }.or_else(|| unsafe { iat::hook("ws2_32.dll", name, f) }) {
            Some(o) => slot.store(o, Ordering::Relaxed),
            None => log!("alls: WS2_32 {name} not imported"),
        }
    }
}

