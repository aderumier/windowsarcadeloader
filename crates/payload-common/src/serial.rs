//! Emulated serial port device (card readers, JVS I/O boards...).
//!
//! The game's serial API calls are intercepted with IAT hooks on the game executable:
//! opening the port returns a fake handle, each `WriteFile` packet goes to the device handler
//! and its reply is queued for the game's `ReadFile`. Comm configuration calls succeed.
//! Several devices per process (one per port, each `install` call), plus optionally silent
//! ports (`install_sink`).

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

use crate::{iat, log};

type P = *mut c_void;

/// Handle of the first device; device `i` is `FAKE + 4 * i`. Odd values: never real handles.
const FAKE: usize = 0x1337;
/// Handle of the silent port (`install_sink`).
const FAKE_SINK: usize = 0x1339;

fn is_fake(h: P) -> bool {
    h as usize == FAKE_SINK || device(h).is_some()
}

/// The emulated device: answers the packets the game writes.
pub type Handler = fn(&[u8]) -> Vec<u8>;

/// An emulated device: its port, the packet handler and the replies not read yet.
struct Device {
    port: String,
    handler: Handler,
    replies: Mutex<VecDeque<u8>>,
}

static DEVICES: Mutex<Vec<&'static Device>> = Mutex::new(Vec::new());
static HOOKED: AtomicBool = AtomicBool::new(false);

/// The device behind a fake handle.
fn device(h: P) -> Option<&'static Device> {
    let i = (h as usize).checked_sub(FAKE)?;
    if i % 4 != 0 {
        return None;
    }
    DEVICES.lock().unwrap().get(i / 4).copied()
}
/// Ports that accept everything and never answer (e.g. the gun board of Type X gun games,
/// whose inputs are written in the game's memory instead).
static SINK: OnceLock<Vec<String>> = OnceLock::new();
/// Reported in `GetCommModemStatus` (JVS sense line / reader ready).
pub static READY: AtomicBool = AtomicBool::new(false);
/// `GetCommModemStatus` value before / after [`READY`] (default: nothing, then CTS).
pub static MODEM_STATUS: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0x10)];
static TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);
static SINK_TRACE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Installs the device on `port` (e.g. `COM2`, also matched as `\\.\COM2`).
pub fn install(port: &str, handler: Handler) {
    install_in(port, handler, &[unsafe { GetModuleHandleW(std::ptr::null()) } as usize]);
}

/// Same as [`install`], for the serial calls of the modules loaded at `modules` (e.g. the
/// game's I/O library instead of the executable). The hooks are installed by the first call.
pub fn install_in(port: &str, handler: Handler, modules: &[usize]) {
    let device = Box::leak(Box::new(Device { port: port.to_string(), handler, replies: Mutex::new(VecDeque::new()) }));
    DEVICES.lock().unwrap().push(device);
    if HOOKED.swap(true, Ordering::Relaxed) {
        return;
    }
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
    let kernel32 = unsafe { GetModuleHandleW(windows_sys::w!("kernel32.dll")) };
    for (idx, (name, f)) in hooks.into_iter().enumerate() {
        for &module in modules {
            if let Some(o) = unsafe { iat::hook_module(module, "kernel32.dll", name, f) } {
                // a later module's entry is already ours
                if o != f {
                    ORIG[idx].store(o, Ordering::Relaxed);
                }
            }
        }
        // functions not imported by every module still need their original
        if ORIG[idx].load(Ordering::Relaxed) == 0 {
            let cname = format!("{name}\0");
            if let Some(p) = unsafe { GetProcAddress(kernel32, cname.as_ptr()) } {
                ORIG[idx].store(p as usize, Ordering::Relaxed);
            }
        }
    }
}

/// Opens `ports` (comma separated) as silent devices: writes succeed, reads return nothing.
/// A `!` prefix (`!COM3`) makes the port absent instead: opening it fails.
/// Call after [`install`], which installs the hooks.
pub fn install_sink(ports: &str) {
    let list: Vec<String> = ports.split(',').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
    log!("serial: {} silent", list.join(", "));
    let _ = SINK.set(list);
}

fn strip_device(name: &str) -> &str {
    name.strip_prefix("\\\\.\\").unwrap_or(name)
}

fn is_sink(name: &str) -> bool {
    SINK.get().is_some_and(|s| s.iter().any(|p| strip_device(name).eq_ignore_ascii_case(p)))
}

/// `!COMn` entries of the silent port list: opening them fails, as on a PC without the port.
fn is_absent(name: &str) -> bool {
    SINK.get().is_some_and(|s| s.iter().any(|p| p.strip_prefix('!').is_some_and(|p| strip_device(name).eq_ignore_ascii_case(p))))
}

/// `INVALID_HANDLE_VALUE` with `ERROR_FILE_NOT_FOUND`, for an absent port.
fn absent() -> P {
    unsafe { windows_sys::Win32::Foundation::SetLastError(2) };
    usize::MAX as P
}

/// Logs the first packets exchanged with the game.
fn trace(request: &[u8], reply: &[u8]) {
    if TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 40 {
        let hex = |d: &[u8]| d.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ");
        log!("serial: <- {}  -> {}", hex(request), hex(reply));
    }
}

/// Fake handle of the device on port `name`.
fn port_handle(name: &str) -> Option<P> {
    let devices = DEVICES.lock().unwrap();
    let name = name.strip_prefix("\\\\.\\").unwrap_or(name);
    if let Some(i) = devices.iter().position(|d| name.eq_ignore_ascii_case(&d.port)) {
        log!("serial: {} opened", devices[i].port);
        return Some((FAKE + 4 * i) as P);
    }
    if name.len() <= 5 && name.to_ascii_uppercase().starts_with("COM") && !is_sink(name) {
        let ports: Vec<&str> = devices.iter().map(|d| d.port.as_str()).collect();
        log!("serial: game opens {name}, not emulated (devices on {})", ports.join(", "));
    }
    None
}

fn port_a(name: *const u8) -> Option<P> {
    if name.is_null() {
        return None;
    }
    port_handle(&unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy())
}

fn port_w(name: *const u16) -> Option<P> {
    if name.is_null() {
        return None;
    }
    let mut len = 0;
    while unsafe { *name.add(len) } != 0 {
        len += 1;
    }
    port_handle(&String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) }))
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
    if !name.is_null() && is_absent(&unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy()) {
        return absent();
    }
    if !name.is_null() && is_sink(&unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy()) {
        log!("serial: silent port opened");
        return FAKE_SINK as P;
    }
    if let Some(h) = port_a(name) {
        return h;
    }
    let h = unsafe { orig::<unsafe extern "system" fn(*const u8, u32, u32, P, u32, u32, P) -> P>(0)(name, a, s, sa, d, f, t) };
    if trace_files() && !name.is_null() {
        trace_open(unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy().into_owned(), h);
    }
    h
}

unsafe extern "system" fn create_file_w(name: *const u16, a: u32, s: u32, sa: P, d: u32, f: u32, t: P) -> P {
    if !name.is_null() {
        let mut len = 0;
        while unsafe { *name.add(len) } != 0 {
            len += 1;
        }
        let wide = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) });
        if is_absent(&wide) {
            return absent();
        }
        if is_sink(&wide) {
            log!("serial: silent port opened");
            return FAKE_SINK as P;
        }
    }
    if let Some(h) = port_w(name) {
        return h;
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
    if h as usize == FAKE_SINK {
        if SINK_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 40 {
            let hex = unsafe { std::slice::from_raw_parts(buf, n as usize) }.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ");
            log!("serial: silent port <- {hex}");
        }
        if !written.is_null() {
            unsafe { *written = n };
        }
        return 1;
    }
    let Some(device) = device(h) else {
        let r = unsafe { orig::<unsafe extern "system" fn(P, *const u8, u32, *mut u32, P) -> i32>(2)(h, buf, n, written, ov) };
        if is_traced_pipe(h) {
            log!("file: pipe {h:?} write {n} bytes overlapped {} -> {r} err {}", !ov.is_null(), unsafe { windows_sys::Win32::Foundation::GetLastError() });
        }
        return r;
    };
    let packet = unsafe { std::slice::from_raw_parts(buf, n as usize) };
    let reply = (device.handler)(packet);
    trace(packet, &reply);
    device.replies.lock().unwrap().extend(reply);
    if !written.is_null() {
        unsafe { *written = n };
    }
    unsafe { complete(ov, n) };
    1
}

/// Overlapped calls complete at once: `OVERLAPPED { Internal, InternalHigh, .. }` set to
/// success and the byte count, its event signaled.
unsafe fn complete(ov: P, n: u32) {
    if ov.is_null() {
        return;
    }
    let o = ov as *mut windows_sys::Win32::System::IO::OVERLAPPED;
    unsafe {
        (*o).Internal = 0;
        (*o).InternalHigh = n as usize;
        if !(*o).hEvent.is_null() {
            windows_sys::Win32::System::Threading::SetEvent((*o).hEvent);
        }
    }
}

unsafe extern "system" fn read_file(h: P, buf: *mut u8, n: u32, read: *mut u32, ov: P) -> i32 {
    if h as usize == FAKE_SINK {
        if !read.is_null() {
            unsafe { *read = 0 };
        }
        return 1;
    }
    let Some(device) = device(h) else {
        let r = unsafe { orig::<unsafe extern "system" fn(P, *mut u8, u32, *mut u32, P) -> i32>(3)(h, buf, n, read, ov) };
        if is_traced_pipe(h) {
            let got = if read.is_null() { 0 } else { unsafe { *read } };
            log!("file: pipe {h:?} read {n} -> {r} got {got} overlapped {} err {}", !ov.is_null(), unsafe { windows_sys::Win32::Foundation::GetLastError() });
        }
        return r;
    };
    let mut q = device.replies.lock().unwrap();
    let count = (n as usize).min(q.len());
    for i in 0..count {
        unsafe { *buf.add(i) = q.pop_front().unwrap() };
    }
    if !read.is_null() {
        unsafe { *read = count as u32 };
    }
    unsafe { complete(ov, count as u32) };
    1
}

unsafe extern "system" fn close_handle(h: P) -> i32 {
    if is_fake(h) {
        return 1;
    }
    if is_traced_pipe(h) {
        log!("file: pipe {h:?} closed");
        PIPES.lock().unwrap().retain(|p| *p != h as usize);
    }
    unsafe { orig::<unsafe extern "system" fn(P) -> i32>(4)(h) }
}

unsafe extern "system" fn get_comm_modem_status(h: P, stat: *mut u32) -> i32 {
    if h as usize == FAKE_SINK {
        if !stat.is_null() {
            unsafe { *stat = 0 };
        }
        return 1;
    }
    if device(h).is_none() {
        return unsafe { orig::<unsafe extern "system" fn(P, *mut u32) -> i32>(5)(h, stat) };
    }
    if !stat.is_null() {
        unsafe { *stat = MODEM_STATUS[READY.load(Ordering::Relaxed) as usize].load(Ordering::Relaxed) };
    }
    1
}

unsafe extern "system" fn clear_comm_error(h: P, errors: *mut u32, stat: *mut u32) -> i32 {
    if !is_fake(h) {
        return unsafe { orig::<unsafe extern "system" fn(P, *mut u32, *mut u32) -> i32>(6)(h, errors, stat) };
    }
    let queued = device(h).map_or(0, |d| d.replies.lock().unwrap().len() as u32);
    if !errors.is_null() {
        unsafe { *errors = 0 };
    }
    if !stat.is_null() {
        // COMSTAT { flags, cbInQue, cbOutQue }
        unsafe { std::ptr::write_bytes(stat, 0, 3) };
        unsafe { *stat.add(1) = queued };
    }
    1
}

/// Serial configuration calls: accepted as is on the fake port.
macro_rules! accept_on_port {
    ($($idx:literal $name:ident ($($a:ident: $t:ty),*);)*) => {$(
        unsafe extern "system" fn $name(h: P, $($a: $t),*) -> i32 {
            if is_fake(h) {
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
