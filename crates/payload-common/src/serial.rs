//! Emulated serial port device (card readers, JVS I/O boards...).
//!
//! The game's serial API calls are intercepted with IAT hooks on the game executable:
//! opening the port returns a fake handle, each `WriteFile` packet goes to the device handler
//! and its reply is queued for the game's `ReadFile`. Comm configuration calls succeed.
//! Several devices per process (one per port, each `install` call), plus optionally silent
//! ports (`install_sink`). The other COM ports the game opens are absent (opening fails), whatever
//! serial ports the machine has.

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
    is_sink_handle(h) || device(h).is_some()
}

/// The emulated device: answers the packets the game writes.
pub type Handler = fn(&[u8]) -> Vec<u8>;

/// Data a device sends on its own (a touch panel's coordinate stream), asked when the game
/// polls the port and nothing is queued.
pub type Idle = fn() -> Vec<u8>;

/// An emulated device: its port, the packet handler and the replies not read yet.
struct Device {
    port: String,
    handler: Handler,
    idle: Option<Idle>,
    replies: Mutex<VecDeque<u8>>,
    /// A pending overlapped `WaitCommEvent` (OVERLAPPED, event mask), completed by a reply.
    wait: Mutex<Option<(usize, usize)>>,
    /// A pending overlapped read (buffer, size, OVERLAPPED), completed by a reply
    /// (`WAL_SERIAL_PENDING_READS`).
    read: Mutex<Option<(usize, u32, usize)>>,
}

impl Device {
    /// The queued replies, topped up with the device's own data when empty.
    fn queue(&self) -> std::sync::MutexGuard<'_, VecDeque<u8>> {
        let mut q = self.replies.lock().unwrap();
        if q.is_empty()
            && let Some(idle) = self.idle
        {
            q.extend(idle());
        }
        q
    }

    /// Queues a reply; completes a pending read, a pending `WaitCommEvent` (a character
    /// received).
    fn reply(&self, reply: Vec<u8>) {
        let mut q = self.replies.lock().unwrap();
        q.extend(reply);
        if !q.is_empty()
            && let Some((buf, n, ov)) = self.read.lock().unwrap().take()
        {
            let count = (n as usize).min(q.len());
            for i in 0..count {
                unsafe { *(buf as *mut u8).add(i) = q.pop_front().unwrap() };
            }
            unsafe { complete(ov as P, count as u32) };
        }
        if !q.is_empty()
            && let Some((ov, mask)) = self.wait.lock().unwrap().take()
        {
            if mask != 0 {
                unsafe { *(mask as *mut u32) = 1 }; // EV_RXCHAR
            }
            unsafe { complete(ov as P, 0) };
        }
    }
}

static DEVICES: Mutex<Vec<&'static Device>> = Mutex::new(Vec::new());
static HOOKED: AtomicBool = AtomicBool::new(false);
/// The modules whose serial calls are already ours.
static HOOKED_MODULES: Mutex<Vec<usize>> = Mutex::new(Vec::new());

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
    install_device(Device { port: port.to_string(), handler, idle: None, replies: Mutex::new(VecDeque::new()), wait: Mutex::new(None), read: Mutex::new(None) }, modules);
}

/// Same as [`install`], for a device that also sends data on its own (`idle`).
pub fn install_streaming(port: &str, handler: Handler, idle: Idle) {
    let device = Device { port: port.to_string(), handler, idle: Some(idle), replies: Mutex::new(VecDeque::new()), wait: Mutex::new(None), read: Mutex::new(None) };
    install_device(device, &[unsafe { GetModuleHandleW(std::ptr::null()) } as usize]);
}

/// Same as [`install_streaming`], for the serial calls of the modules loaded at `modules`.
pub fn install_streaming_in(port: &str, handler: Handler, idle: Idle, modules: &[usize]) {
    let device = Device { port: port.to_string(), handler, idle: Some(idle), replies: Mutex::new(VecDeque::new()), wait: Mutex::new(None), read: Mutex::new(None) };
    install_device(device, modules);
}

fn install_device(device: Device, modules: &[usize]) {
    let device = Box::leak(Box::new(device));
    DEVICES.lock().unwrap().push(device);
    if !HOOKED.swap(true, Ordering::Relaxed) {
        // read now: an environment lookup inside a hook sets the thread's last error
        trace_files();
        pending_reads();
    }
    hook_modules(modules);
}

/// The serial functions and their replacements.
fn hooks() -> [(&'static str, usize); COUNT] {
    [
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
        ("WriteFileEx", write_file_ex as *const () as usize),
        ("ReadFileEx", read_file_ex as *const () as usize),
        ("WaitCommEvent", wait_comm_event as *const () as usize),
    ]
}

/// Our replacement of the serial function `name` (`CreateFileW`, `ReadFile`...), for callers
/// that resolve them with `GetProcAddress` (Mono's `DllImport("kernel32")`: `SerialPort`). The
/// originals are looked up at the first [`install`].
pub fn replacement(name: &str) -> Option<usize> {
    hooks().into_iter().find(|(n, _)| *n == name).map(|(_, f)| f)
}

/// Points the serial calls of the modules at `modules` at the emulated devices (each device's
/// install hooks its own modules). A module already hooked is left alone: its entries may now
/// be another hook's chaining to ours (Medal no Gunman: JVS, the RingEdge devices, then its
/// COM1 board), which hooking it again would skip.
fn hook_modules(modules: &[usize]) {
    let modules: Vec<usize> = {
        let mut hooked = HOOKED_MODULES.lock().unwrap();
        let new: Vec<usize> = modules.iter().copied().filter(|m| !hooked.contains(m)).collect();
        hooked.extend(&new);
        new
    };
    let hooks = hooks();
    let kernel32 = unsafe { GetModuleHandleW(windows_sys::w!("kernel32.dll")) };
    for (idx, (name, f)) in hooks.into_iter().enumerate() {
        for &module in &modules {
            if let Some(o) = unsafe { iat::hook_module(module, "kernel32.dll", name, f) } {
                // a later module's entry is already ours; the first original stays
                if o != f {
                    let _ = ORIG[idx].compare_exchange(0, o, Ordering::Relaxed, Ordering::Relaxed);
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

/// One request/reply exchange with the device behind the fake handle `h` (drivers that pass
/// the packets with `DeviceIoControl`, e.g. RingEdge's `\\.\mxjvs`). `None`: not a device handle.
pub fn transact(h: P, packet: &[u8]) -> Option<Vec<u8>> {
    let device = device(h)?;
    let reply = (device.handler)(packet);
    trace(&device.port, packet, &reply);
    Some(reply)
}

/// The port of the emulated device behind the fake handle `h` (e.g. to answer its
/// `DeviceIoControl` calls). `None`: not a device handle.
pub fn port_of(h: P) -> Option<String> {
    device(h).map(|d| d.port.clone())
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

fn sink_handle(name: &str) -> Option<P> {
    let name = strip_device(name);
    let sinks = SINK.get()?;
    sinks.iter().filter(|p| !p.starts_with('!'))
        .position(|p| name.eq_ignore_ascii_case(p))
        .map(|i| (FAKE_SINK + i * 4) as P)
}

fn sink_index(h: P) -> Option<usize> {
    let raw = h as usize;
    let i = raw.checked_sub(FAKE_SINK)? / 4;
    (raw == FAKE_SINK + i * 4 && SINK.get()?.iter().filter(|p| !p.starts_with('!')).count() > i).then_some(i)
}

fn is_sink_handle(h: P) -> bool { sink_index(h).is_some() }

fn sink_name(h: P) -> &'static str {
    let i = sink_index(h).unwrap_or(0);
    SINK.get().and_then(|s| s.iter().filter(|p| !p.starts_with('!')).nth(i)).map(String::as_str).unwrap_or("silent")
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

/// Number of packets traced: `WAL_SERIAL_TRACE=<n>` (default 40).
fn trace_limit() -> usize {
    static LIMIT: OnceLock<usize> = OnceLock::new();
    *LIMIT.get_or_init(|| std::env::var("WAL_SERIAL_TRACE").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(40))
}

/// Logs the first packets exchanged with the game.
fn trace(port: &str, request: &[u8], reply: &[u8]) {
    if TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < trace_limit() {
        let hex = |d: &[u8]| d.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ");
        log!("serial: {port} <- {}  -> {}", hex(request), hex(reply));
    }
}

/// Fake handle of the device on port `name`.
fn port_handle(name: &str) -> Option<P> {
    // the log written with the lock released: its WriteFile may be our hook (packed
    // executables: the system's WriteFile hot patched), which takes the lock
    let ports: Vec<String> = DEVICES.lock().unwrap().iter().map(|d| d.port.clone()).collect();
    let name = name.strip_prefix("\\\\.\\").unwrap_or(name);
    // `COM*`: any COM port the game opens (a game trying the ports Windows lists, in their order)
    let any_com = |p: &str| p.eq_ignore_ascii_case("COM*") && name.len() > 3 && name[..3].eq_ignore_ascii_case("COM") && name[3..].bytes().all(|b| b.is_ascii_digit());
    if let Some(i) = ports.iter().position(|p| name.eq_ignore_ascii_case(p) || any_com(p)) {
        log!("serial: {} opened", ports[i]);
        return Some((FAKE + 4 * i) as P);
    }
    if name.len() <= 6 && name.to_ascii_uppercase().starts_with("COM") && !is_sink(name) {
        // Absent, as on a PC without serial ports: Wine maps the machine's own ports (Batocera:
        // COM1 = /dev/ttyS0), where nothing answers (D1GP: card reader error at boot).
        log!("serial: game opens {name}, not emulated (devices on {}): absent", ports.join(", "));
        return Some(absent());
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
const COUNT: usize = 18;
static ORIG: [AtomicUsize; COUNT] = [const { AtomicUsize::new(0) }; COUNT];

fn orig<F: Copy>(idx: usize) -> F {
    let p = ORIG[idx].load(Ordering::Relaxed);
    unsafe { std::mem::transmute_copy(&p) }
}

/// `WAL_SERIAL_PENDING_READS=1`: overlapped reads of an emulated port with nothing to read stay
/// pending until a reply (else they complete at once with 0 bytes).
fn pending_reads() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAL_SERIAL_PENDING_READS").is_ok_and(|v| v.trim() == "1"))
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

/// Runs `f` (logging) without changing the thread's last error, which the game reads after
/// the call we pass through (Unreal's overlapped ReadFile: ERROR_IO_PENDING).
fn keep_last_error<R>(f: impl FnOnce() -> R) -> R {
    use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
    let error = unsafe { GetLastError() };
    let r = f();
    unsafe { SetLastError(error) };
    r
}

fn trace_open(name: String, handle: P) {
    keep_last_error(|| trace_open_inner(name, handle))
}

fn trace_open_inner(name: String, handle: P) {
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
        let port = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy();
        log!("serial: silent port {port} opened");
        return sink_handle(&port).unwrap_or(FAKE_SINK as P);
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
            log!("serial: silent port {wide} opened");
            return sink_handle(&wide).unwrap_or(FAKE_SINK as P);
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
    if is_sink_handle(h) {
        if SINK_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 40 {
            let hex = unsafe { std::slice::from_raw_parts(buf, n as usize) }.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ");
            log!("serial: {} <- {hex}", sink_name(h));
        }
        if !written.is_null() {
            unsafe { *written = n };
        }
        return 1;
    }
    let Some(device) = device(h) else {
        let r = unsafe { orig::<unsafe extern "system" fn(P, *const u8, u32, *mut u32, P) -> i32>(2)(h, buf, n, written, ov) };
        if keep_last_error(|| is_traced_pipe(h)) {
            let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            keep_last_error(|| log!("file: pipe {h:?} write {n} bytes overlapped {} -> {r} err {err}", !ov.is_null()));
        }
        return r;
    };
    let packet = unsafe { std::slice::from_raw_parts(buf, n as usize) };
    let reply = (device.handler)(packet);
    trace(&device.port, packet, &reply);
    if !written.is_null() {
        unsafe { *written = n };
    }
    unsafe { complete(ov, n) };
    if pending_reads() && !ov.is_null() {
        // the reply after the write's completion, as on the line (31 bytes at 115200 baud:
        // 3 ms): a reader that sends its next request on a reply drops it while its write
        // is not done (Teratoma's gun board, polled 5 times a second instead of every frame)
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(2));
            device.reply(reply);
        });
    } else {
        device.reply(reply);
    }
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
    if is_sink_handle(h) {
        if SINK_TRACE_COUNT.fetch_add(1, Ordering::Relaxed) < 40 {
            log!("serial: {} read requested {n} bytes (silent)", sink_name(h));
        }
        if !read.is_null() {
            unsafe { *read = 0 };
        }
        return 1;
    }
    let Some(device) = device(h) else {
        let r = unsafe { orig::<unsafe extern "system" fn(P, *mut u8, u32, *mut u32, P) -> i32>(3)(h, buf, n, read, ov) };
        if keep_last_error(|| is_traced_pipe(h)) {
            let got = if read.is_null() { 0 } else { unsafe { *read } };
            let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            keep_last_error(|| log!("file: pipe {h:?} read {n} -> {r} got {got} overlapped {} err {err}", !ov.is_null()));
        }
        return r;
    };
    let mut q = device.queue();
    // a real port's overlapped read waits for its bytes (a reader that issues its next read
    // only after receiving: Teratoma's gun board)
    if q.is_empty() && !ov.is_null() && pending_reads() {
        *device.read.lock().unwrap() = Some((buf as usize, n, ov as usize));
        unsafe { windows_sys::Win32::Foundation::SetLastError(windows_sys::Win32::Foundation::ERROR_IO_PENDING) };
        return 0;
    }
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

type IoCompletion = Option<unsafe extern "system" fn(u32, u32, *mut windows_sys::Win32::System::IO::OVERLAPPED)>;

/// The ES3 game uses the Ex variants for its COM1 JVS stream. Complete fake-port
/// operations inline so the callback sees the same byte counts as the synchronous path.
unsafe extern "system" fn write_file_ex(h: P, buf: *const u8, n: u32, ov: *mut windows_sys::Win32::System::IO::OVERLAPPED, cb: IoCompletion) -> i32 {
    let Some(device) = device(h) else {
        return unsafe { orig::<unsafe extern "system" fn(P, *const u8, u32, *mut windows_sys::Win32::System::IO::OVERLAPPED, IoCompletion) -> i32>(15)(h, buf, n, ov, cb) };
    };
    let packet = unsafe { std::slice::from_raw_parts(buf, n as usize) };
    let reply = (device.handler)(packet);
    trace(&device.port, packet, &reply);
    device.reply(reply);
    unsafe { complete(ov.cast(), n) };
    if let Some(cb) = cb { unsafe { cb(0, n, ov) }; }
    1
}

unsafe extern "system" fn read_file_ex(h: P, buf: *mut u8, n: u32, ov: *mut windows_sys::Win32::System::IO::OVERLAPPED, cb: IoCompletion) -> i32 {
    let Some(device) = device(h) else {
        return unsafe { orig::<unsafe extern "system" fn(P, *mut u8, u32, *mut windows_sys::Win32::System::IO::OVERLAPPED, IoCompletion) -> i32>(16)(h, buf, n, ov, cb) };
    };
    let mut q = device.queue();
    // a real port's overlapped read waits for its bytes (a reader that issues its next read
    // only after receiving: Teratoma's gun board)
    if q.is_empty() && !ov.is_null() && pending_reads() {
        *device.read.lock().unwrap() = Some((buf as usize, n, ov as usize));
        unsafe { windows_sys::Win32::Foundation::SetLastError(windows_sys::Win32::Foundation::ERROR_IO_PENDING) };
        return 0;
    }
    let count = (n as usize).min(q.len());
    for i in 0..count { unsafe { *buf.add(i) = q.pop_front().unwrap() }; }
    drop(q);
    unsafe { complete(ov.cast(), count as u32) };
    if let Some(cb) = cb { unsafe { cb(0, count as u32, ov) }; }
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
    if is_sink_handle(h) {
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
    let queued = device(h).map_or(0, |d| d.queue().len() as u32);
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

/// `WaitCommEvent` on an emulated port: overlapped, it completes when a reply is queued
/// (Teratoma's gun board reader reads on that event), a silent port's stays pending; else a
/// received character after 10 ms.
unsafe extern "system" fn wait_comm_event(h: P, mask: *mut u32, ov: P) -> i32 {
    if device(h).is_none() && !is_sink_handle(h) {
        return unsafe { orig::<unsafe extern "system" fn(P, *mut u32, P) -> i32>(17)(h, mask, ov) };
    }
    if !ov.is_null() {
        if let Some(device) = device(h) {
            if device.queue().is_empty() {
                *device.wait.lock().unwrap() = Some((ov as usize, mask as usize));
            } else {
                if !mask.is_null() {
                    unsafe { *mask = 1 }; // EV_RXCHAR
                }
                unsafe { complete(ov, 0) };
                return 1;
            }
        }
        unsafe { windows_sys::Win32::Foundation::SetLastError(windows_sys::Win32::Foundation::ERROR_IO_PENDING) };
        return 0;
    }
    std::thread::sleep(std::time::Duration::from_millis(10));
    if !mask.is_null() {
        unsafe { *mask = 1 }; // EV_RXCHAR
    }
    1
}
