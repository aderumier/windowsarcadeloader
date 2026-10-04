//! Emulated serial port device (card readers, JVS I/O boards...).
//!
//! The game's serial API calls are intercepted with IAT hooks on the game executable:
//! opening the port returns a fake handle, each `WriteFile` packet goes to the device handler
//! and its reply is queued for the game's `ReadFile`. Comm configuration calls succeed.
//! One device per process.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::{iat, log};

type P = *mut c_void;

/// Odd value: never a real handle.
const FAKE: usize = 0x1337;

/// The emulated device: answers the packets the game writes.
pub type Handler = fn(&[u8]) -> Vec<u8>;

static PORT: OnceLock<String> = OnceLock::new();
static HANDLER: OnceLock<Handler> = OnceLock::new();
static REPLIES: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
/// Reported in `GetCommModemStatus` (JVS sense line / reader ready).
pub static READY: AtomicBool = AtomicBool::new(false);
static TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Installs the device on `port` (e.g. `COM2`, also matched as `\\.\COM2`).
pub fn install(port: &str, handler: Handler) {
    let _ = PORT.set(port.to_string());
    let _ = HANDLER.set(handler);
    let hooks: [(&str, usize); COUNT] = [
        ("CreateFileA", create_file_a as *const () as usize),
        ("CreateFileW", create_file_w as *const () as usize),
        ("WriteFile", write_file as *const () as usize),
        ("ReadFile", read_file as *const () as usize),
        ("CloseHandle", close_handle as *const () as usize),
        ("GetCommModemStatus", get_comm_modem_status as *const () as usize),
        ("ClearCommError", clear_comm_error as *const () as usize),
        ("EscapeCommFunction", escape_comm_function as *const () as usize),
        ("SetupComm", setup_comm as *const () as usize),
        ("GetCommState", get_comm_state as *const () as usize),
        ("SetCommState", set_comm_state as *const () as usize),
        ("SetCommMask", set_comm_mask as *const () as usize),
        ("GetCommTimeouts", get_comm_timeouts as *const () as usize),
        ("SetCommTimeouts", set_comm_timeouts as *const () as usize),
        ("PurgeComm", purge_comm as *const () as usize),
    ];
    for (idx, (name, f)) in hooks.into_iter().enumerate() {
        if let Some(o) = unsafe { iat::hook("kernel32.dll", name, f) } {
            ORIG[idx].store(o, Ordering::Relaxed);
        }
    }
}

/// Logs the first packets exchanged with the game.
fn trace(request: &[u8], reply: &[u8]) {
    if TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 40 {
        let hex = |d: &[u8]| d.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ");
        log!("serial: <- {}  -> {}", hex(request), hex(reply));
    }
}

fn port_matches(name: &str) -> bool {
    let Some(port) = PORT.get() else { return false };
    let name = name.strip_prefix("\\\\.\\").unwrap_or(name);
    if name.eq_ignore_ascii_case(port) {
        return true;
    }
    if name.len() <= 5 && name.to_ascii_uppercase().starts_with("COM") {
        log!("serial: game opens {name}, not emulated (device on {port})");
    }
    false
}

fn is_port_a(name: *const u8) -> bool {
    !name.is_null() && port_matches(&unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy())
}

fn is_port_w(name: *const u16) -> bool {
    if name.is_null() {
        return false;
    }
    let mut len = 0;
    while unsafe { *name.add(len) } != 0 {
        len += 1;
    }
    port_matches(&String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) }))
}

// Original functions (previous IAT entries), by hook index.
const COUNT: usize = 15;
static ORIG: [AtomicUsize; COUNT] = [const { AtomicUsize::new(0) }; COUNT];

fn orig<F: Copy>(idx: usize) -> F {
    let p = ORIG[idx].load(Ordering::Relaxed);
    unsafe { std::mem::transmute_copy(&p) }
}

/// `WAL_TRACE_FILES=1`: log every file the game opens and the result.
fn trace_files() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAL_TRACE_FILES").is_ok_and(|v| v == "1"))
}

/// Pipe handles opened by the game, traced with `WAL_TRACE_FILES=1`.
static PIPES: Mutex<Vec<usize>> = Mutex::new(Vec::new());

fn is_traced_pipe(h: P) -> bool {
    trace_files() && PIPES.lock().unwrap().contains(&(h as usize))
}

fn trace_open(name: String, handle: P) {
    if trace_files() {
        if name.to_ascii_lowercase().contains("\\pipe\\") && handle as isize != -1 {
            PIPES.lock().unwrap().push(handle as usize);
        }
        let result = if handle as isize == -1 { format!("FAILED ({})", unsafe { windows_sys::Win32::Foundation::GetLastError() }) } else { "ok".into() };
        log!("file: {name} {result}");
    }
}

unsafe extern "system" fn create_file_a(name: *const u8, a: u32, s: u32, sa: P, d: u32, f: u32, t: P) -> P {
    if is_port_a(name) {
        log!("serial: {} opened", PORT.get().map_or("", |p| p.as_str()));
        return FAKE as P;
    }
    let h = unsafe { orig::<unsafe extern "system" fn(*const u8, u32, u32, P, u32, u32, P) -> P>(0)(name, a, s, sa, d, f, t) };
    if trace_files() && !name.is_null() {
        trace_open(unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy().into_owned(), h);
    }
    h
}

unsafe extern "system" fn create_file_w(name: *const u16, a: u32, s: u32, sa: P, d: u32, f: u32, t: P) -> P {
    if is_port_w(name) {
        log!("serial: {} opened", PORT.get().map_or("", |p| p.as_str()));
        return FAKE as P;
    }
    let h = unsafe { orig::<unsafe extern "system" fn(*const u16, u32, u32, P, u32, u32, P) -> P>(1)(name, a, s, sa, d, f, t) };
    if trace_files() && !name.is_null() {
        let mut len = 0;
        while unsafe { *name.add(len) } != 0 {
            len += 1;
        }
        trace_open(String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) }), h);
    }
    h
}

unsafe extern "system" fn write_file(h: P, buf: *const u8, n: u32, written: *mut u32, ov: P) -> i32 {
    if h as usize != FAKE {
        let r = unsafe { orig::<unsafe extern "system" fn(P, *const u8, u32, *mut u32, P) -> i32>(2)(h, buf, n, written, ov) };
        if is_traced_pipe(h) {
            log!("file: pipe {h:?} write {n} bytes overlapped {} -> {r} err {}", !ov.is_null(), unsafe { windows_sys::Win32::Foundation::GetLastError() });
        }
        return r;
    }
    let packet = unsafe { std::slice::from_raw_parts(buf, n as usize) };
    let reply = HANDLER.get().map_or_else(Vec::new, |h| h(packet));
    trace(packet, &reply);
    REPLIES.lock().unwrap().extend(reply);
    if !written.is_null() {
        unsafe { *written = n };
    }
    1
}

unsafe extern "system" fn read_file(h: P, buf: *mut u8, n: u32, read: *mut u32, ov: P) -> i32 {
    if h as usize != FAKE {
        let r = unsafe { orig::<unsafe extern "system" fn(P, *mut u8, u32, *mut u32, P) -> i32>(3)(h, buf, n, read, ov) };
        if is_traced_pipe(h) {
            let got = if read.is_null() { 0 } else { unsafe { *read } };
            log!("file: pipe {h:?} read {n} -> {r} got {got} overlapped {} err {}", !ov.is_null(), unsafe { windows_sys::Win32::Foundation::GetLastError() });
        }
        return r;
    }
    let mut q = REPLIES.lock().unwrap();
    let count = (n as usize).min(q.len());
    for i in 0..count {
        unsafe { *buf.add(i) = q.pop_front().unwrap() };
    }
    if !read.is_null() {
        unsafe { *read = count as u32 };
    }
    1
}

unsafe extern "system" fn close_handle(h: P) -> i32 {
    if h as usize == FAKE {
        return 1;
    }
    if is_traced_pipe(h) {
        log!("file: pipe {h:?} closed");
        PIPES.lock().unwrap().retain(|p| *p != h as usize);
    }
    unsafe { orig::<unsafe extern "system" fn(P) -> i32>(4)(h) }
}

unsafe extern "system" fn get_comm_modem_status(h: P, stat: *mut u32) -> i32 {
    if h as usize != FAKE {
        return unsafe { orig::<unsafe extern "system" fn(P, *mut u32) -> i32>(5)(h, stat) };
    }
    if !stat.is_null() {
        unsafe { *stat = if READY.load(Ordering::Relaxed) { 0x10 } else { 0 } };
    }
    1
}

unsafe extern "system" fn clear_comm_error(h: P, errors: *mut u32, stat: *mut u32) -> i32 {
    if h as usize != FAKE {
        return unsafe { orig::<unsafe extern "system" fn(P, *mut u32, *mut u32) -> i32>(6)(h, errors, stat) };
    }
    if !errors.is_null() {
        unsafe { *errors = 0 };
    }
    if !stat.is_null() {
        // COMSTAT { flags, cbInQue, cbOutQue }
        let queued = REPLIES.lock().unwrap().len() as u32;
        unsafe { std::ptr::write_bytes(stat, 0, 3) };
        unsafe { *stat.add(1) = queued };
    }
    1
}

/// Serial configuration calls: accepted as is on the fake port.
macro_rules! accept_on_port {
    ($($idx:literal $name:ident ($($a:ident: $t:ty),*);)*) => {$(
        unsafe extern "system" fn $name(h: P, $($a: $t),*) -> i32 {
            if h as usize == FAKE {
                return 1;
            }
            unsafe { orig::<unsafe extern "system" fn(P, $($t),*) -> i32>($idx)(h, $($a),*) }
        }
    )*};
}

accept_on_port! {
    7 escape_comm_function(f: u32);
    8 setup_comm(i: u32, o: u32);
    9 get_comm_state(dcb: P);
    10 set_comm_state(dcb: P);
    11 set_comm_mask(m: u32);
    12 get_comm_timeouts(t: P);
    13 set_comm_timeouts(t: P);
    14 purge_comm(f: u32);
}
