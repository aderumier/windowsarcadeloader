//! Raw Thrills / ICE "Frogger" (2011, `sdaemon.exe`, the "S" engine): a 2 player redemption
//! cabinet. `WAL_RAWTHRILLS_GAME=frogger`.
//!
//! Three pieces of cabinet hardware the game opens itself, plus a copy-protection dongle:
//!
//! * **USBIO watchdog** (`0x254e40` detects it): nibble-tagged status reports. The game finds
//!   it through SetupAPI (class GUID `96 df 5d 32 8c 93 d3 11 9e 34 00 80 c8 27 27 f4`), opens
//!   the device path it is told with `CreateFileA` (`FILE_FLAG_OVERLAPPED`) and drives it with
//!   `DeviceIoControl`: a version read (code `0x80942006`, must report words `0xc70` / `0x774`
//!   at offsets 8 / 10 of an 18 byte blob), an init (`0x80942024`), a capability query
//!   (`0x8094206c`, a 12 byte reply whose leading word must be `0x141`) and a pipe-role set
//!   (`0x80942078`, one byte: `0x81` the input pipe, `2` the output pipe). The board is opened
//!   three times: the main object, the input pipe and the output pipe; the input pipe is read as
//!   4 byte sequences in the game's I/O thread. Reads become pending between status reports;
//!   an endless stream of synchronous reports traps the game's update loop in its drain loop.
//! * **Serial JAMMA board** (probed on `COM1`..`COM16`, `0x253aa0`): the probe writes
//!   `\nC0001\r1B` / `\nU00\rCC` / `\nv\r8D` and accepts the port answering `\nv` with
//!   7 bytes `0a 76 .. .. .. .. 0d` (checksum mode off: frame = `\n` + data + `\r`, the two
//!   uppercase hex checksum chars are appended only when the object's `0x40` flag is set, and
//!   equal `sum(\n..\r) & 0xff`). The board is faked on `COM2`: the probe is answered,
//!   `jamma_open` succeeds and the game's reader / writer threads run against the fake. Every
//!   board input is fed as spontaneous S reports: eight hex-encoded bytes carrying modulo-16
//!   button edge counters. The decoder at 0x2526f7 and table at 0x2efe58 map starts, coins,
//!   service, test and volume. Object +4 holds configuration flags, not live button state.
//! * **LED display board** (`0x24f730`, the bonus LED display): 6 byte frames, opened on
//!   `COM1` (the probe fails through it and finds the JAMMA board on `COM2`). The fake accepts
//!   the open, logs the frames and never answers reads.
//! * **HASP dongle** (dynamically loaded `API_1LNM.DLL` / `HLW32Proc`, not in the IAT): the
//!   check lives in the encrypted `.protect` section, so it is not statically analyzable. The
//!   loader, the entry point and the device opens are hooked and logged; a stub answers
//!   `HLW32Proc` so the game does not stall, and the run logs decide whether a fuller fake is
//!   needed.
//!
//! Everything is done with IAT hooks and fakes (no code patching): `CreateFileA` hands out
//! sentinel handles, `DeviceIoControl` / `ReadFile` / `WriteFile` answer per board, and the
//! SetupAPI calls return a device that exists. Initialization and input changes are logged.

use std::ffi::CStr;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use wal_payload_common::{input, log};
use wal_protocol::button;

// ── Win32 ABI of the hooked imports ───────────────────────────────────────────────────────────

type CreateFileA = unsafe extern "system" fn(
    name: *const i8,
    access: u32,
    share: u32,
    sa: *const std::ffi::c_void,
    disp: u32,
    flags: u32,
    tmpl: usize,
) -> isize;

type DeviceIoControl = unsafe extern "system" fn(
    handle: usize,
    code: u32,
    in_buf: *const std::ffi::c_void,
    in_size: u32,
    out_buf: *mut std::ffi::c_void,
    out_size: u32,
    bytes_ret: *mut u32,
    overlapped: *mut std::ffi::c_void,
) -> i32;

type ReadFile =
    unsafe extern "system" fn(handle: usize, buf: *mut std::ffi::c_void, size: u32, bytes_ret: *mut u32, overlapped: *mut std::ffi::c_void) -> i32;

type WriteFile =
    unsafe extern "system" fn(handle: usize, buf: *const std::ffi::c_void, size: u32, bytes_ret: *mut u32, overlapped: *mut std::ffi::c_void) -> i32;

type GetOverlappedResult =
    unsafe extern "system" fn(handle: usize, overlapped: *mut std::ffi::c_void, bytes_ret: *mut u32, wait: u32) -> i32;

type CloseHandle = unsafe extern "system" fn(handle: usize) -> i32;

type LoadLibraryA = unsafe extern "system" fn(name: *const i8) -> usize;

type GetProcAddress = unsafe extern "system" fn(module: usize, name: *const i8) -> usize;

type GetCommState =
    unsafe extern "system" fn(handle: usize, dcb: *mut std::ffi::c_void) -> i32;

type SetCommState =
    unsafe extern "system" fn(handle: usize, dcb: *const std::ffi::c_void) -> i32;

type SetupComm = unsafe extern "system" fn(handle: usize, inq: u32, outq: u32) -> i32;

type PurgeComm = unsafe extern "system" fn(handle: usize, flags: u32) -> i32;

type GetCommTimeouts =
    unsafe extern "system" fn(handle: usize, t: *mut std::ffi::c_void) -> i32;

type SetCommTimeouts =
    unsafe extern "system" fn(handle: usize, t: *const std::ffi::c_void) -> i32;

type ClearCommError =
    unsafe extern "system" fn(handle: usize, errors: *mut u32, stats: *mut std::ffi::c_void) -> i32;

type WaitCommEvent = unsafe extern "system" fn(
    handle: usize,
    mask: *mut u32,
    ov: *mut std::ffi::c_void,
) -> i32;

type SetCommMask = unsafe extern "system" fn(handle: usize, mask: u32) -> i32;

// ── Sentinels and the fixed board replies ─────────────────────────────────────────────────────

/// The three USBIO handles (`CreateFileA` hands them out in the open order: main, input, output).
const USBIO_MAIN: usize = 0xf5b0001;
const USBIO_INPUT: usize = 0xf5b0002;
const USBIO_OUTPUT: usize = 0xf5b0003;
const USBIO_HANDLES: [usize; 3] = [USBIO_MAIN, USBIO_INPUT, USBIO_OUTPUT];
/// The `SetupDiGetClassDevsA` result (must be non NULL: the detect treats it as "board present").
const HDEVINFO_FAKE: usize = 0xf5b0d00;
/// The fake `API_1LNM.DLL` module.
const HASP_MODULE: usize = 0xf5a5001;
/// The LED display board, on `COM1`.
const SER_LED: usize = 0xf5b0005;
/// The serial JAMMA board, on `COM2`.
const SER_JAMMA: usize = 0xf5b0006;
const SER_HANDLES: [usize; 2] = [SER_LED, SER_JAMMA];
const INVALID_HANDLE: isize = -1;

/// The 18 byte version blob (`0x80942006`): words `0xc70` / `0x774` at offsets 8 / 10.
const VERSION_BLOB: [u8; 18] = [
    0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x70, 0x0c, 0x74, 0x07, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00,
];
/// The 12 byte capability reply (`0x8094206c`): leading word `0x141`, bits 4 / 5 / 8 set.
const CAP_REPLY: [u8; 12] = [0x41, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x30, 0x01, 0x00, 0x00];
/// The USBIO input pipe's idle 4 byte sequence (what the game's I/O thread reads at rest).
const USBIO_IDLE: [u8; 4] = [0x1c, 0x20, 0x34, 0x40];
/// `ERROR_IO_PENDING`: what the serial reader loop polls on.
const ERROR_IO_PENDING: u32 = 0x3e5;
/// The reply to the probe's `v` poll: 7 bytes, `0a` at 0, `76` at 1, `0d` at 6.
const JAMMA_V_REPLY: [u8; 7] = [0x0a, 0x76, 0x31, 0x2e, 0x30, 0x31, 0x0d];

// ── Originals of the hooked imports ───────────────────────────────────────────────────────────

static ORIG_CREATE_FILE: AtomicUsize = AtomicUsize::new(0);
static ORIG_DEVICE_IO: AtomicUsize = AtomicUsize::new(0);
static ORIG_READ_FILE: AtomicUsize = AtomicUsize::new(0);
static ORIG_WRITE_FILE: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_OVERLAPPED: AtomicUsize = AtomicUsize::new(0);
static ORIG_CLOSE_HANDLE: AtomicUsize = AtomicUsize::new(0);
static ORIG_LOAD_LIBRARY: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_PROC: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_COMM_STATE: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_COMM_STATE: AtomicUsize = AtomicUsize::new(0);
static ORIG_SETUP_COMM: AtomicUsize = AtomicUsize::new(0);
static ORIG_PURGE_COMM: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_COMM_TIMEOUTS: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_COMM_TIMEOUTS: AtomicUsize = AtomicUsize::new(0);
static ORIG_CLEAR_COMM_ERROR: AtomicUsize = AtomicUsize::new(0);
static ORIG_WAIT_COMM_EVENT: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_COMM_MASK: AtomicUsize = AtomicUsize::new(0);

// ── Runtime state ─────────────────────────────────────────────────────────────────────────────

/// How many times the USBIO device path was opened (0 main, 1 input, 2 output, then wraps).
static USBIO_OPENED: AtomicU32 = AtomicU32::new(0);
/// The bytes the fake JAMMA board sent and the game has not read yet.
static JAMMA_RX: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// The raw game-to-board stream, for the frame parser (checksums verified, frames logged).
static JAMMA_TX: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// How many times the JAMMA port was opened (1 the probe, 2 the real open).
static JAMMA_OPENED: AtomicU32 = AtomicU32::new(0);
struct UsbRead {
    buffer: usize,
    size: u32,
    bytes: usize,
    overlapped: usize,
}
struct UsbInput {
    available: bool,
    pending: Option<UsbRead>,
}
static USB_INPUT: Mutex<UsbInput> = Mutex::new(UsbInput { available: true, pending: None });

/// The USB device is the watchdog, with nibble-tagged status words. It is polled by
/// the game until no report remains, so reads must become pending between reports.
unsafe fn complete_usb_read(read: &UsbRead) {
    let count = read.size.min(4);
    unsafe {
        if read.buffer != 0 {
            std::ptr::copy_nonoverlapping(USBIO_IDLE.as_ptr(), read.buffer as *mut u8, count as usize);
        }
        if read.bytes != 0 {
            (read.bytes as *mut u32).write_volatile(count);
        }
        if read.overlapped != 0 {
            let ov = read.overlapped as *mut usize;
            ov.write_volatile(0);
            ov.add(1).write_volatile(count as usize);
            let event = ov.add(4).read_volatile();
            if event != 0 {
                windows_sys::Win32::System::Threading::SetEvent(event as _);
            }
        }
    }
}

fn publish_usb_status() {
    let mut state = USB_INPUT.lock().unwrap();
    if let Some(read) = state.pending.take() {
        unsafe { complete_usb_read(&read) };
    } else {
        state.available = true;
    }
}
/// Last virtual state reported (for change-only logging).
static LAST_PRESSED: AtomicU32 = AtomicU32::new(!0);
/// The last logged USBIO / LED write, so the per-frame lamp streams do not flood the log.
static LAST_USBIO_OUT: AtomicU64 = AtomicU64::new(u64::MAX);
static LAST_LED: AtomicU64 = AtomicU64::new(u64::MAX);

// ── init ──────────────────────────────────────────────────────────────────────────────────────

/// Fakes the cabinet hardware when `WAL_RAWTHRILLS_GAME=frogger`.
pub(crate) fn init(name: &str) -> bool {
    if !name.eq_ignore_ascii_case("frogger") {
        return false;
    }
    hook_crash();
    hook_create_file();
    hook_device_io();
    hook_read_write();
    hook_comm();
    hook_setupapi();
    hook_library();
    std::thread::spawn(input_thread);
    log!("frogger: USBIO / serial JAMMA / LED / HASP fakes active, all I/O logged");
    true
}

// ── CreateFileA ───────────────────────────────────────────────────────────────────────────────

unsafe extern "system" fn frogger_create_file(
    name: *const i8,
    access: u32,
    share: u32,
    sa: *const std::ffi::c_void,
    disp: u32,
    flags: u32,
    tmpl: usize,
) -> isize {
    let n = read_cstr(name).unwrap_or_default();
    if is_usbio_name(name) {
        let i = USBIO_OPENED.fetch_add(1, Ordering::SeqCst) as usize % USBIO_HANDLES.len();
        let handle = USBIO_HANDLES[i];
        log!("frogger: CreateFileA(USBIO #{i}) access {access:#x} flags {flags:#x} -> {handle:#x}");
        return handle as isize;
    }
    if let Some(port) = com_port(name) {
        return match port {
            // The LED display board: the open succeeds, the frames are logged, the reads get nothing.
            1 => {
                log!("frogger: CreateFileA(COM1) -> LED {SER_LED:#x}");
                SER_LED as isize
            }
            // The JAMMA board: open 1 is the probe (it answers the v poll), open 2 the real one.
            2 => {
                let n = JAMMA_OPENED.fetch_add(1, Ordering::SeqCst) + 1;
                JAMMA_RX.lock().unwrap().clear();
                JAMMA_TX.lock().unwrap().clear();
                log!("frogger: CreateFileA(COM2) -> JAMMA {SER_JAMMA:#x} (open {n})");
                SER_JAMMA as isize
            }
            // Nothing on the other ports: the probe moves on.
            _ => {
                log!("frogger: CreateFileA(COM{port}) -> INVALID");
                unsafe { windows_sys::Win32::Foundation::SetLastError(2) };
                INVALID_HANDLE
            }
        };
    }
    if is_hasp_name(&n) {
        log!("frogger: CreateFileA(hasp: {n:?})");
    }
    let f = unsafe { std::mem::transmute::<usize, CreateFileA>(ORIG_CREATE_FILE.load(Ordering::Relaxed)) };
    unsafe { f(name, access, share, sa, disp, flags, tmpl) }
}

fn hook_create_file() {
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "CreateFileA", frogger_create_file as *const () as usize) } {
        ORIG_CREATE_FILE.store(o, Ordering::Relaxed);
    }
}

// ── DeviceIoControl ───────────────────────────────────────────────────────────────────────────

unsafe extern "system" fn frogger_device_io(
    handle: usize,
    code: u32,
    in_buf: *const std::ffi::c_void,
    in_size: u32,
    out_buf: *mut std::ffi::c_void,
    out_size: u32,
    bytes_ret: *mut u32,
    overlapped: *mut std::ffi::c_void,
) -> i32 {
    if !is_usbio_handle(handle) {
        // The HASP device or the display board: observed, then left to the original.
        log!(
            "frogger: DeviceIoControl({handle:#x}) code {code:#x} in {in_size:#x} [{}] out {out_size:#x}",
            hex(&bytes_at(in_buf, in_size))
        );
        let f = unsafe { std::mem::transmute::<usize, DeviceIoControl>(ORIG_DEVICE_IO.load(Ordering::Relaxed)) };
        return unsafe { f(handle, code, in_buf, in_size, out_buf, out_size, bytes_ret, overlapped) };
    }

    match code {
        // capability query: 12 byte reply (leading word 0x141).
        0x8094206c => {
            log!("frogger: USBIO {handle:#x} capability query ({code:#x})");
            let (buf, size) = out_slots(out_buf, out_size, CAP_REPLY.len());
            write_reply(&CAP_REPLY, buf, size, bytes_ret);
        }
        // version read: 8 byte in (the command byte at offset 4), 18 byte out blob.
        0x80942006 => {
            log!("frogger: USBIO {handle:#x} version ({code:#x}) in [{}]", hex(&bytes_at(in_buf, in_size)));
            let (buf, size) = out_slots(out_buf, out_size, VERSION_BLOB.len());
            write_reply(&VERSION_BLOB, buf, size, bytes_ret);
        }
        // init (0x104 in, no out) and reinit (no in / out): accepted.
        0x80942024 | 0x80942054 => {
            log!(
                "frogger: USBIO {handle:#x} {} ({code:#x}) in {in_size:#x}",
                if code == 0x80942024 { "init" } else { "reinit" },
            );
        }
        // pipe role: one byte (0x81 input, 2 output) in, no out.
        0x80942078 => {
            log!("frogger: USBIO {handle:#x} role ({code:#x}) [{}]", hex(&bytes_at(in_buf, in_size)));
        }
        other => {
            log!(
                "frogger: USBIO {handle:#x} DeviceIoControl {other:#x} in {in_size:#x} [{}] out {out_size:#x}",
                hex(&bytes_at(in_buf, in_size))
            );
        }
    }
    1
}

/// The (buffer, size) the reply goes to: the game's DiControl wrapper may pass them in either
/// order, so pick the argument that looks like a pointer.
fn out_slots(out_buf: *mut std::ffi::c_void, out_size: u32, want: usize) -> (*mut u8, u32) {
    if is_ptr(out_buf as usize) && out_size as usize >= want {
        (out_buf as *mut u8, out_size)
    } else if is_ptr(out_size as usize) {
        (out_size as *mut u8, want as u32)
    } else {
        (out_buf as *mut u8, out_size)
    }
}

fn write_reply(data: &[u8], buf: *mut u8, size: u32, bytes_ret: *mut u32) {
    unsafe {
        if !buf.is_null() && size != 0 {
            std::ptr::copy_nonoverlapping(data.as_ptr(), buf, (data.len() as u32).min(size) as usize);
        }
        if !bytes_ret.is_null() {
            bytes_ret.write_volatile((data.len() as u32).min(size));
        }
    }
}

fn is_usbio_handle(handle: usize) -> bool {
    USBIO_HANDLES.contains(&handle)
}

fn hook_device_io() {
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "DeviceIoControl", frogger_device_io as *const () as usize) } {
        ORIG_DEVICE_IO.store(o, Ordering::Relaxed);
    }
}

// ── ReadFile / WriteFile ──────────────────────────────────────────────────────────────────────

unsafe extern "system" fn frogger_read_file(
    handle: usize,
    buf: *mut std::ffi::c_void,
    size: u32,
    bytes_ret: *mut u32,
    overlapped: *mut std::ffi::c_void,
) -> i32 {
    if is_serial_handle(handle) {
        static READ_LOGGED: AtomicU32 = AtomicU32::new(0);
        if READ_LOGGED.fetch_add(1, Ordering::Relaxed) < 200 {
            log!("frogger: ReadFile({handle:#x}) size {size}");
        }
        // The serial fakes: hand over what the board queued, nothing pending otherwise. The
        // reader's ReadFile passes its byte-count slot where the overlapped goes and answers
        // are checked out of that slot, so a synchronous fill is what its path expects.
        let n = if handle == SER_JAMMA {
            let mut rx = JAMMA_RX.lock().unwrap();
            let n = (size as usize).min(rx.len());
            if n != 0 {
                unsafe { std::ptr::copy_nonoverlapping(rx.as_ptr(), buf as *mut u8, n) };
                rx.drain(..n);
            }
            n
        } else {
            0
        };
        if n != 0 {
            unsafe {
                if !bytes_ret.is_null() {
                    bytes_ret.write_volatile(n as u32);
                }
            };
            return 1;
        }
        unsafe { windows_sys::Win32::Foundation::SetLastError(ERROR_IO_PENDING) };
        return 0;
    }
    if handle == USBIO_INPUT || handle == USBIO_OUTPUT {
        let read = UsbRead { buffer: buf as usize, size, bytes: bytes_ret as usize, overlapped: overlapped as usize };
        let mut state = USB_INPUT.lock().unwrap();
        if state.available {
            state.available = false;
            unsafe { complete_usb_read(&read) };
            return 1;
        }
        unsafe {
            if !bytes_ret.is_null() {
                bytes_ret.write_volatile(0);
            }
            if !overlapped.is_null() {
                let ov = overlapped as *mut usize;
                ov.write_volatile(0x103); // STATUS_PENDING
                ov.add(1).write_volatile(0);
                let event = ov.add(4).read_volatile();
                if event != 0 {
                    windows_sys::Win32::System::Threading::ResetEvent(event as _);
                }
            }
            windows_sys::Win32::Foundation::SetLastError(ERROR_IO_PENDING);
        };
        state.pending = Some(read);
        return 0;
    }
    let f = unsafe { std::mem::transmute::<usize, ReadFile>(ORIG_READ_FILE.load(Ordering::Relaxed)) };
    unsafe { f(handle, buf, size, bytes_ret, overlapped) }
}

unsafe extern "system" fn frogger_write_file(
    handle: usize,
    buf: *const std::ffi::c_void,
    size: u32,
    bytes_ret: *mut u32,
    overlapped: *mut std::ffi::c_void,
) -> i32 {
    let data = bytes_at(buf, size);
    if is_serial_handle(handle) {
        static WRITE_LOGGED: AtomicU32 = AtomicU32::new(0);
        if WRITE_LOGGED.fetch_add(1, Ordering::Relaxed) < 200 {
            log!("frogger: WriteFile({handle:#x}) size {size} [{}]", hex(&data[..data.len().min(16)]));
        }
    }
    match handle {
        // The game-to-board stream: parse and log the frames, answer the probe's v poll.
        SER_JAMMA => jamma_on_write(&data),
        // The LED display: logged, changes only (the probe scrubs C / U / v frames over it too).
        SER_LED => {
            if frame_changed(&LAST_LED, &data) {
                log!("frogger: LED write [{}]", hex(&data));
            }
        }
        _ => {
            if is_usbio_handle(handle) {
                // The lamp stream is written per frame: log the changes only.
                let idx = match handle {
                    USBIO_MAIN => 0u64,
                    USBIO_INPUT => 1,
                    USBIO_OUTPUT => 2,
                    _ => 3,
                };
                let v = (idx << 56) | stamp(&data, 7);
                if LAST_USBIO_OUT.swap(v, Ordering::Relaxed) != v {
                    log!("frogger: USBIO {handle:#x} write [{}]", hex(&data));
                }
            } else if size <= 64 {
                log!("frogger: WriteFile({handle:#x}) [{}]", hex(&data));
            }
        }
    }
    if is_serial_handle(handle) || is_usbio_handle(handle) {
        unsafe {
            if !bytes_ret.is_null() {
                bytes_ret.write_volatile(size);
            }
        };
        return 1;
    }
    let f = unsafe { std::mem::transmute::<usize, WriteFile>(ORIG_WRITE_FILE.load(Ordering::Relaxed)) };
    unsafe { f(handle, buf, size, bytes_ret, overlapped) }
}

unsafe extern "system" fn frogger_get_overlapped(
    handle: usize,
    overlapped: *mut std::ffi::c_void,
    bytes_ret: *mut u32,
    wait: u32,
) -> i32 {
    if is_usbio_handle(handle) && !overlapped.is_null() {
        let _state = USB_INPUT.lock().unwrap();
        unsafe {
            let ov = overlapped as *const usize;
            if ov.read_volatile() == 0x103 {
                windows_sys::Win32::Foundation::SetLastError(996); // ERROR_IO_INCOMPLETE
                return 0;
            }
            if !bytes_ret.is_null() {
                bytes_ret.write_volatile(ov.add(1).read_volatile() as u32);
            }
        }
        return 1;
    }
    if is_serial_handle(handle) {
        // The reader loop reaches here with no read pending: report it and pace the poll.
        std::thread::sleep(Duration::from_millis(1));
        unsafe { windows_sys::Win32::Foundation::SetLastError(ERROR_IO_PENDING) };
        return 0;
    }
    if !is_usbio_handle(handle) {
        log!("frogger: GetOverlappedResult({handle:#x}) wait {wait}");
    }
    let f = unsafe { std::mem::transmute::<usize, GetOverlappedResult>(ORIG_GET_OVERLAPPED.load(Ordering::Relaxed)) };
    unsafe { f(handle, overlapped, bytes_ret, wait) }
}

unsafe extern "system" fn frogger_close_handle(handle: usize) -> i32 {
    if handle == SER_JAMMA {
        log!("frogger: CloseHandle(JAMMA {handle:#x})");
    } else if is_usbio_handle(handle) {
        log!("frogger: CloseHandle(USBIO {handle:#x})");
    }
    let f = unsafe { std::mem::transmute::<usize, CloseHandle>(ORIG_CLOSE_HANDLE.load(Ordering::Relaxed)) };
    unsafe { f(handle) }
}

fn hook_read_write() {
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "ReadFile", frogger_read_file as *const () as usize) } {
        ORIG_READ_FILE.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "WriteFile", frogger_write_file as *const () as usize) } {
        ORIG_WRITE_FILE.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "GetOverlappedResult", frogger_get_overlapped as *const () as usize) } {
        ORIG_GET_OVERLAPPED.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "CloseHandle", frogger_close_handle as *const () as usize) } {
        ORIG_CLOSE_HANDLE.store(o, Ordering::Relaxed);
    }
}

// ── The comm APIs, faked for the two serial boards ────────────────────────────────────────────

unsafe extern "system" fn frogger_get_comm_state(handle: usize, dcb: *mut std::ffi::c_void) -> i32 {
    if !is_serial_handle(handle) {
        let f = unsafe { std::mem::transmute::<usize, GetCommState>(ORIG_GET_COMM_STATE.load(Ordering::Relaxed)) };
        return unsafe { f(handle, dcb) };
    }
    log!("frogger: GetCommState({handle:#x}) dcb {dcb:p}");
    unsafe {
        if !dcb.is_null() {
            // The game's DCB locals are exactly as big as the cbSize they declare (0x1c),
            // not the full 0x34 struct: zeroing more overruns the caller's frame.
            let cb = std::ptr::read_volatile(dcb as *const u32) as usize;
            std::ptr::write_bytes(dcb as *mut u8, 0, cb.clamp(8, 0x1c));
            (dcb as *mut u32).write_volatile(0x1c); // DCBlength
            (dcb as *mut u32).add(1).write_volatile(0x1c200);
            (dcb as *mut u32).add(2).write_volatile(0x1013);
            (dcb as *mut u8).add(0x0c).write_volatile(8);
        }
    };
    1
}

unsafe extern "system" fn frogger_set_comm_state(handle: usize, dcb: *const std::ffi::c_void) -> i32 {
    if !is_serial_handle(handle) {
        let f = unsafe { std::mem::transmute::<usize, SetCommState>(ORIG_SET_COMM_STATE.load(Ordering::Relaxed)) };
        return unsafe { f(handle, dcb) };
    }
    log!("frogger: SetCommState({handle:#x})");
    1
}

unsafe extern "system" fn frogger_setup_comm(handle: usize, inq: u32, outq: u32) -> i32 {
    if !is_serial_handle(handle) {
        let f = unsafe { std::mem::transmute::<usize, SetupComm>(ORIG_SETUP_COMM.load(Ordering::Relaxed)) };
        return unsafe { f(handle, inq, outq) };
    }
    log!("frogger: SetupComm({handle:#x}) {inq:#x} {outq:#x}");
    1
}

unsafe extern "system" fn frogger_purge_comm(handle: usize, flags: u32) -> i32 {
    if !is_serial_handle(handle) {
        let f = unsafe { std::mem::transmute::<usize, PurgeComm>(ORIG_PURGE_COMM.load(Ordering::Relaxed)) };
        return unsafe { f(handle, flags) };
    }
    static PURGE_LOGGED: AtomicU32 = AtomicU32::new(0);
    if PURGE_LOGGED.fetch_add(1, Ordering::Relaxed) < 20 {
        log!(
            "frogger: PurgeComm({handle:#x}) {flags:#x} (tid {})",
            unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
        );
    }
    // The reader purges on every idle iteration; clearing the queue here would drop queued
    // replies, so the fake keeps them (its frames are always well formed, no resync needed).
    1
}

unsafe extern "system" fn frogger_get_comm_timeouts(handle: usize, t: *mut std::ffi::c_void) -> i32 {
    if !is_serial_handle(handle) {
        let f = unsafe { std::mem::transmute::<usize, GetCommTimeouts>(ORIG_GET_COMM_TIMEOUTS.load(Ordering::Relaxed)) };
        return unsafe { f(handle, t) };
    }
    log!("frogger: GetCommTimeouts({handle:#x})");
    unsafe {
        if !t.is_null() {
            std::ptr::write_bytes(t as *mut u8, 0, 0x14);
        }
    };
    1
}

unsafe extern "system" fn frogger_set_comm_timeouts(handle: usize, t: *const std::ffi::c_void) -> i32 {
    if !is_serial_handle(handle) {
        let f = unsafe { std::mem::transmute::<usize, SetCommTimeouts>(ORIG_SET_COMM_TIMEOUTS.load(Ordering::Relaxed)) };
        return unsafe { f(handle, t) };
    }
    log!("frogger: SetCommTimeouts({handle:#x})");
    1
}

unsafe extern "system" fn frogger_clear_comm_error(handle: usize, errors: *mut u32, stats: *mut std::ffi::c_void) -> i32 {
    if !is_serial_handle(handle) {
        let f = unsafe { std::mem::transmute::<usize, ClearCommError>(ORIG_CLEAR_COMM_ERROR.load(Ordering::Relaxed)) };
        return unsafe { f(handle, errors, stats) };
    }
    log!("frogger: ClearCommError({handle:#x})");
    unsafe {
        if !errors.is_null() {
            errors.write_volatile(0);
        }
        if !stats.is_null() {
            std::ptr::write_bytes(stats as *mut u8, 0, 0x0c);
            (stats as *mut u32).add(1).write_volatile(JAMMA_RX.lock().unwrap().len() as u32); // cbInQue
        }
    };
    1
}

unsafe extern "system" fn frogger_wait_comm_event(handle: usize, mask: *mut u32, ov: *mut std::ffi::c_void) -> i32 {
    if !is_serial_handle(handle) {
        let f = unsafe { std::mem::transmute::<usize, WaitCommEvent>(ORIG_WAIT_COMM_EVENT.load(Ordering::Relaxed)) };
        return unsafe { f(handle, mask, ov) };
    }
    static WCE_LOGGED: AtomicU32 = AtomicU32::new(0);
    if WCE_LOGGED.fetch_add(1, Ordering::Relaxed) < 30 {
        log!(
            "frogger: WaitCommEvent({handle:#x}) (tid {})",
            unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() }
        );
    }
    let has_rx = handle == SER_JAMMA && !JAMMA_RX.lock().unwrap().is_empty();
    unsafe {
        if !mask.is_null() {
            mask.write_volatile(if has_rx { 1 } else { 0 }); // CE_RXCHAR, or an idle completion
        }
    };
    // Complete idle polls synchronously. Returning ERROR_IO_PENDING here makes the game
    // repeatedly treat a normal no-data state as a board error and spins its reader thread.
    1
}

unsafe extern "system" fn frogger_set_comm_mask(handle: usize, mask: u32) -> i32 {
    if !is_serial_handle(handle) {
        let f = unsafe { std::mem::transmute::<usize, SetCommMask>(ORIG_SET_COMM_MASK.load(Ordering::Relaxed)) };
        return unsafe { f(handle, mask) };
    }
    log!("frogger: SetCommMask({handle:#x}) {mask:#x}");
    1
}

fn hook_comm() {
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "GetCommState", frogger_get_comm_state as *const () as usize) } {
        ORIG_GET_COMM_STATE.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "SetCommState", frogger_set_comm_state as *const () as usize) } {
        ORIG_SET_COMM_STATE.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "SetupComm", frogger_setup_comm as *const () as usize) } {
        ORIG_SETUP_COMM.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "PurgeComm", frogger_purge_comm as *const () as usize) } {
        ORIG_PURGE_COMM.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "GetCommTimeouts", frogger_get_comm_timeouts as *const () as usize) } {
        ORIG_GET_COMM_TIMEOUTS.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "SetCommTimeouts", frogger_set_comm_timeouts as *const () as usize) } {
        ORIG_SET_COMM_TIMEOUTS.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "ClearCommError", frogger_clear_comm_error as *const () as usize) } {
        ORIG_CLEAR_COMM_ERROR.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "WaitCommEvent", frogger_wait_comm_event as *const () as usize) } {
        ORIG_WAIT_COMM_EVENT.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "SetCommMask", frogger_set_comm_mask as *const () as usize) } {
        ORIG_SET_COMM_MASK.store(o, Ordering::Relaxed);
    }
}

/// Feeds a chunk of the game-to-board stream and answers the frames it contains.
fn jamma_on_write(data: &[u8]) {
    let mut tx = JAMMA_TX.lock().unwrap();
    tx.extend_from_slice(data);
    loop {
        // Up to the frame start.
        match tx.iter().position(|&b| b == 0x0a) {
            None => {
                tx.clear();
                return;
            }
            Some(0) => {}
            Some(p) => {
                tx.drain(..p);
            }
        }
        // Probe frames have two checksum digits; normal traffic ends at CR.
        let Some(rl) = tx[1..].iter().position(|&b| b == 0x0d).map(|p| p + 1) else {
            return;
        };
        let has_checksum = tx.len() >= rl + 3
            && tx[rl + 1..rl + 3].iter().all(u8::is_ascii_hexdigit);
        let length = rl + if has_checksum { 3 } else { 1 };
        let frame: Vec<u8> = tx.drain(..length).collect();
        jamma_frame(&frame);
    }
}

/// Logs one board-bound frame; the v poll of the probe (the first open) gets the version reply.
fn jamma_frame(frame: &[u8]) {
    let rl = frame.iter().position(|&b| b == 0x0d).unwrap();
    let cmd = frame[1];
    let payload = &frame[2..rl];
    if frame.len() >= rl + 3 {
        let sum = frame[..rl + 1].iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte));
        let ck = std::str::from_utf8(&frame[rl + 1..rl + 3]).unwrap_or("??");
        if u8::from_str_radix(ck, 16).ok() != Some(sum) {
            log!("frogger: JAMMA bad frame cmd {cmd:?} [{}] sum {sum:02x} ck {ck}", hex(payload));
            return;
        }
    }
    static FRAMES_LOGGED: AtomicU32 = AtomicU32::new(0);
    if FRAMES_LOGGED.fetch_add(1, Ordering::Relaxed) < 30 {
        log!("frogger: JAMMA frame cmd {cmd:?} [{}]", hex(payload));
    }
    if cmd == b'v' && JAMMA_OPENED.load(Ordering::SeqCst) == 1 {
        JAMMA_RX.lock().unwrap().extend_from_slice(&JAMMA_V_REPLY);
    }
}

/// The leading `max` bytes as one stamp, so a per-frame stream logs its changes only.
fn stamp(data: &[u8], max: usize) -> u64 {
    let mut v = 0u64;
    for &b in data.iter().take(max) {
        v = (v << 8) | b as u64;
    }
    v
}

/// Whether `data` differs from the last value stamped into `last` (which is updated in place).
fn frame_changed(last: &AtomicU64, data: &[u8]) -> bool {
    let v = stamp(data, 8);
    last.swap(v, Ordering::Relaxed) != v
}

fn is_serial_handle(handle: usize) -> bool {
    SER_HANDLES.contains(&handle)
}

// ── SetupAPI: the USBIO board detection ───────────────────────────────────────────────────────

unsafe extern "system" fn setup_get_class_devs(
    _classguid: *const std::ffi::c_void,
    _enumerator: *const i8,
    _hwnd: usize,
    flags: u32,
) -> usize {
    log!("frogger: SetupDiGetClassDevsA(flags {flags:#x}) -> fake devinfo");
    HDEVINFO_FAKE
}

unsafe extern "system" fn setup_enum_interfaces(
    _hdevinfo: usize,
    _devid: *const std::ffi::c_void,
    _guid: *const std::ffi::c_void,
    idx: u32,
    _data: *mut std::ffi::c_void,
) -> i32 {
    // One interface, on the first member: the open proceeds.
    if idx == 0 {
        1
    } else {
        unsafe { windows_sys::Win32::Foundation::SetLastError(0x103) };
        0
    }
}

unsafe extern "system" fn setup_detail(
    _hdevinfo: usize,
    _data: *const std::ffi::c_void,
    detail: *mut std::ffi::c_void,
    size: u32,
    required: *mut u32,
    _mode: *mut std::ffi::c_void,
) -> i32 {
    if detail.is_null() {
        // First call: report the buffer size the game allocates (0x40).
        unsafe {
            if !required.is_null() {
                required.write_volatile(0x40);
            }
        };
        return 1;
    }
    // Second call: fill the device path at offset 4 (the `cbSize` at offset 0 is the game's).
    let path = b"\\\\.\\WALUSBIO0\0";
    unsafe {
        if size as usize >= 4 + path.len() {
            std::ptr::copy_nonoverlapping(path.as_ptr(), (detail as *mut u8).offset(4), path.len());
        }
        if !required.is_null() {
            required.write_volatile(4 + path.len() as u32);
        }
    };
    1
}

fn hook_setupapi() {
    if unsafe { wal_payload_common::iat::hook("setupapi.dll", "SetupDiGetClassDevsA", setup_get_class_devs as *const () as usize) }.is_some() {
        log!("frogger: SetupDiGetClassDevsA hooked (USBIO present)");
    }
    if unsafe { wal_payload_common::iat::hook("setupapi.dll", "SetupDiEnumDeviceInterfaces", setup_enum_interfaces as *const () as usize) }.is_some() {
        log!("frogger: SetupDiEnumDeviceInterfaces hooked");
    }
    if unsafe { wal_payload_common::iat::hook("setupapi.dll", "SetupDiGetDeviceInterfaceDetailA", setup_detail as *const () as usize) }.is_some() {
        log!("frogger: SetupDiGetDeviceInterfaceDetailA hooked");
    }
}

// ── Library loading: the HASP dongle ──────────────────────────────────────────────────────────

unsafe extern "system" fn frogger_load_library(name: *const i8) -> usize {
    let n = read_cstr(name).unwrap_or_default();
    if is_hasp_name(&n) {
        log!("frogger: LoadLibraryA(hasp: {n:?}) -> fake module");
        return HASP_MODULE;
    }
    let f = unsafe { std::mem::transmute::<usize, LoadLibraryA>(ORIG_LOAD_LIBRARY.load(Ordering::Relaxed)) };
    unsafe { f(name) }
}

unsafe extern "system" fn frogger_get_proc(module: usize, name: *const i8) -> usize {
    let n = read_cstr(name).unwrap_or_default();
    if module == HASP_MODULE {
        log!("frogger: GetProcAddress(HASP, {n:?}) -> stub");
        return hasp_proc as *const () as usize;
    }
    let f = unsafe { std::mem::transmute::<usize, GetProcAddress>(ORIG_GET_PROC.load(Ordering::Relaxed)) };
    unsafe { f(module, name) }
}

/// `HLW32Proc` stub: the dongle entry point. Logged (first command seen) and answered "success";
/// the run logs show which commands the check issues so a fuller fake can be built.
unsafe extern "C" fn hasp_proc(cmd: i32, a1: i32, a2: i32, a3: i32, a4: i32) -> i32 {
    static SEEN: AtomicU32 = AtomicU32::new(0);
    let mask = 1u32 << (cmd & 31);
    if SEEN.fetch_or(mask, Ordering::Relaxed) & mask == 0 {
        log!("frogger: HLW32Proc(cmd {cmd:#x}, {a1:#x}, {a2:#x}, {a3:#x}, {a4:#x})");
    }
    0
}

fn hook_library() {
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "LoadLibraryA", frogger_load_library as *const () as usize) } {
        ORIG_LOAD_LIBRARY.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "GetProcAddress", frogger_get_proc as *const () as usize) } {
        ORIG_GET_PROC.store(o, Ordering::Relaxed);
    }
}

// ── name helpers ──────────────────────────────────────────────────────────────────────────────

fn is_usbio_name(name: *const i8) -> bool {
    matches!(read_cstr(name), Some(ref s) if s.eq_ignore_ascii_case("\\\\.\\WALUSBIO0") || s.eq_ignore_ascii_case("WALUSBIO0"))
}

fn com_port(name: *const i8) -> Option<u32> {
    read_cstr(name)?.to_ascii_uppercase().strip_prefix("COM").and_then(|n| n.parse().ok())
}

fn is_hasp_name(s: &str) -> bool {
    let u = s.to_ascii_uppercase();
    u.contains("HARDLOCK") || u.contains("FENTE") || u.contains("API_1") || u.contains("HLW32")
}

fn read_cstr(s: *const i8) -> Option<String> {
    if s.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(s) }.to_str().ok().map(|s| s.to_string())
}

/// The bytes `ptr` points to (up to `len`), for the logs.
fn bytes_at(ptr: *const std::ffi::c_void, len: u32) -> Vec<u8> {
    if ptr.is_null() || len == 0 {
        return Vec::new();
    }
    unsafe { std::slice::from_raw_parts(ptr as *const u8, len.min(256) as usize).to_vec() }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|v| format!("{v:02x}")).collect::<Vec<_>>().join(" ")
}

/// Whether `v` looks like a user-mode pointer (as opposed to a small size the wrapper may put
/// in the buffer slot).
fn is_ptr(v: usize) -> bool {
    (0x0040_0000..0x8000_0000).contains(&v)
}

// ── Crash catcher ─────────────────────────────────────────────────────────────────────────────
//
// The game wild-jumps to a heap address a second after `JammaInit` (exit 5, no ExitProcess,
// the stack at `esp` all zeros). A vectored exception handler sees the real 32-bit context
// before the process dies: registers plus the stack around `esp`, on every run.

type RtlAddVec = unsafe extern "system" fn(first: u32, handler: usize) -> u32;

static CRASH_DUMPS: AtomicU32 = AtomicU32::new(0);

/// The 32-bit `CONTEXT` register offsets (after the 164 byte FPU area).
const CTX_EAX: usize = 0xd4;
const CTX_EBX: usize = 0xd8;
const CTX_ECX: usize = 0xdc;
const CTX_EDX: usize = 0xe0;
const CTX_EBP: usize = 0xe4;
const CTX_ESP: usize = 0xe8;
const CTX_EIP: usize = 0xec;
const CTX_ESI: usize = 0xf0;
const CTX_EDI: usize = 0xf4;

unsafe extern "system" fn crash_handler(ep: *const u8) -> i32 {
    // `EXCEPTION_POINTERS { PEXCEPTION_RECORD; PCONTEXT; }`
    let (record, ctx) = unsafe { (*(ep as *const *const u32), *((ep as *const *const u8).add(1))) };
    if record.is_null() || ctx.is_null() {
        return 0; // EXCEPTION_CONTINUE_SEARCH
    }
    if CRASH_DUMPS.fetch_add(1, Ordering::Relaxed) >= 8 {
        return 0;
    }
    let code = unsafe { record.read_unaligned() };
    let xaddr = unsafe { record.add(3).read_unaligned() };
    let access = unsafe { record.add(4).read_unaligned() }; // 0 read, 1 write
    let fault = unsafe { record.add(5).read_unaligned() }; // faulting memory address
    let dw = |o: usize| unsafe { ((ctx as usize + o) as *const u32).read_unaligned() };
    let eip = dw(CTX_EIP);
    // Wild jumps only: `eip` neither in the game image (0x230000..0x596000) nor a loaded
    // system DLL (at 0x60000000 and up).
    if (0x230_000..0x596_000).contains(&eip) || eip >= 0x6000_0000 {
        return 0;
    }
    let esp = dw(CTX_ESP) as usize;
    let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    log!(
        "frogger: CRASH tid {tid} code {code:#x} ({} {fault:#x}) eip {eip:#x} eax {:#x} ebx {:#x} ecx {:#x} edx {:#x} ebp {:#x} esp {esp:#x} esi {:#x} edi {:#x}",
        if access == 1 { "writing" } else { "reading" },
        dw(CTX_EAX), dw(CTX_EBX), dw(CTX_ECX), dw(CTX_EDX), dw(CTX_EBP), dw(CTX_ESI), dw(CTX_EDI)
    );
    // The whole region around `esp`: the current frame, its return address and the caller's.
    let start = esp.wrapping_sub(0x60);
    if unsafe { windows_sys::Win32::System::Memory::IsBadReadPtr(start as *const _, 72 * 64) } != 0 {
        return 0;
    }
    for line in 0..72 {
        let mut text = String::new();
        for k in 0..16 {
            let at = start + (line * 16 + k) * 4;
            let p = at as *const u32;
            let v = unsafe { p.read_volatile() };
            text.push_str(&format!("{v:08x} "));
        }
        log!("frogger: CRASH {:#010x} {}", start + line * 64, text.trim_end());
    }
    0
}

fn hook_crash() {
    let ntdll = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(b"ntdll.dll\0".as_ptr()) };
    if ntdll.is_null() {
        log!("frogger: crash catcher: ntdll not found");
        return;
    }
    let p = unsafe { windows_sys::Win32::System::LibraryLoader::GetProcAddress(ntdll, b"RtlAddVectoredExceptionHandler\0".as_ptr()) };
    let Some(p) = p else {
        log!("frogger: crash catcher: RtlAddVectoredExceptionHandler not found");
        return;
    };
    let f: RtlAddVec = unsafe { std::mem::transmute(p) };
    let r = unsafe { f(1, crash_handler as usize) };
    log!("frogger: crash catcher installed ({r:#x})");
}

// ── The input thread ──────────────────────────────────────────────────────────────────────────

/// The JAMMA object's slots worth watching (the event callback among them). The ring slots
/// (0x1c4..0x1f0) change every tick and are left out.
const OBJ_SLOTS: [usize; 10] = [0x00, 0x04, 0x08, 0x10, 0x14, 0x18, 0x1b0, 0x1b4, 0x208, 0x20c];
static OBJ_LAST: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Logs the JAMMA object's pointer slots on change. The object is the static at 0x371668 (the
/// compiler hardcodes its address; the open flag is its first byte).
fn watch_jamma_object() {
    let obj = 0x371668usize;
    let mut vals = [0u32; OBJ_SLOTS.len()];
    for (i, &o) in OBJ_SLOTS.iter().enumerate() {
        let p = (obj + o) as *const u32;
        vals[i] = unsafe { p.read_volatile() };
    }
    let mut last = OBJ_LAST.lock().unwrap();
    if last.len() != vals.len() {
        *last = vals.to_vec();
        log!("frogger: JAMMA object {obj:#x} (watching)");
        return;
    }
    for (i, (&o, &v)) in OBJ_SLOTS.iter().zip(vals.iter()).enumerate() {
        if last[i] != v {
            log!("frogger: JAMMA object +{o:02x}: {:#x} -> {v:#x}", last[i]);
            last[i] = v;
        }
    }
}

/// Reads virtual sticks and publishes serial JAMMA button edge counters.
fn input_thread() {
    log!("frogger: input thread started");
    let mut previous_inputs = 0u32;
    let mut counters = [0u8; 8];
    counters[0] = 0x80; // first-report synchronization flag, decoded S[1]
    loop {
        publish_usb_status();
        watch_jamma_object();
        let p1 = input(0);
        let p2 = input(1);
        let switches = (if p1.pressed(button::COIN) { 0x01 } else { 0 })
            | (if p1.pressed(button::START) { 0x02 } else { 0 })
            | (if p2.pressed(button::COIN) { 0x04 } else { 0 })
            | (if p1.pressed(button::SERVICE) { 0x08 } else { 0 })
            | (if p1.pressed(button::TEST) { 0x10 } else { 0 })
            | (if p1.pressed(button::UP) { 0x20 } else { 0 })
            | (if p1.pressed(button::DOWN) { 0x80 } else { 0 });
        let switches = switches | if p2.pressed(button::START) { 0x100 } else { 0 };
        // Serial S reports carry modulo-16 edge counters, not held switch bits. The
        // game's table at 0x2efe58 maps S[4..7] to player starts, coins, volume,
        // test and service. Increment on both press and release so the game sees
        // the same held state and one-shot presses as it does on the cabinet.
        let changed = switches ^ previous_inputs;
        for (bit, slot, shift) in [
            (0x02, 3, 4), (0x100, 3, 0),
            (0x01, 4, 4), (0x04, 4, 0),
            (0x20, 5, 4), (0x80, 5, 0),
            (0x10, 6, 4), (0x08, 6, 0),
        ] {
            if changed & bit != 0 {
                let value = ((counters[slot] >> shift).wrapping_add(1)) & 0x0f;
                counters[slot] = (counters[slot] & !(0x0f << shift)) | (value << shift);
            }
        }
        previous_inputs = switches;
        if JAMMA_OPENED.load(Ordering::Relaxed) >= 2 {
            let mut frame = [0u8; 19];
            frame[..2].copy_from_slice(b"\nS");
            const HEX: &[u8; 16] = b"0123456789abcdef";
            for (index, value) in counters.iter().enumerate() {
                frame[2 + index * 2] = HEX[(value >> 4) as usize];
                frame[3 + index * 2] = HEX[(value & 0x0f) as usize];
            }
            frame[18] = b'\r';
            let mut rx = JAMMA_RX.lock().unwrap();
            if rx.len() < 128 {
                rx.extend_from_slice(&frame);
            }
        }
        let pressed = switches;
        let previous = LAST_PRESSED.swap(pressed, Ordering::Relaxed);
        if pressed != previous {
            log!(
                "frogger: P1 coin {} start {} | P2 coin {} start {} | service {} test {} volup {} voldown {}",
                state(pressed, 0x01),
                state(pressed, 0x02),
                state(pressed, 4),
                state(pressed, 0x100),
                state(pressed, 8),
                state(pressed, 16),
                state(pressed, 32),
                state(pressed, 128),
            );
        }
        std::thread::sleep(Duration::from_millis(16));
    }
}

fn state(v: u32, b: u32) -> &'static str {
    if v & b != 0 { "on" } else { "off" }
}
