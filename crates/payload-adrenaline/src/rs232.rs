//! `RS_232_DLL.dll` (Teunis van Beelen's RS-232 library, cdecl: ports by index, `COM1` 0) on
//! the emulated serial ports: Hot Wheels: Victory Lap's `SBK.IOBoardLib.SerialProtocol` (its
//! steering board, LED strips, seats) opens its ports with it, not with Mono's `SerialPort`.
//!
//! `RS232_GetPortIndexByName("COM3")` 2, `RS232_OpenComport(index, baud, mode, flow)` 0 when
//! open (the port's `CreateFileA` through `steering.rs`: the steering board's port, exclusive;
//! the others absent), `RS232_PollComport` the port's `ReadFile` (never waits),
//! `RS232_SendBuf` its `WriteFile`: the game writes a frame in pieces (`FE`, its header and
//! payload, its sum), gathered into one packet for the board (`FE src cmd len(u16) payload..
//! sum`, the sum escaped as `FE FE`).

use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use wal_payload_common::{log, serial};

type P = *mut c_void;

/// The library's ports (`COM1` to `COM32`).
const PORTS: usize = 32;
const INVALID: usize = usize::MAX;

/// Each port's handle, if open.
static HANDLES: Mutex<[usize; PORTS]> = Mutex::new([0; PORTS]);
/// Each port's last poll that read data.
static LAST_DATA: Mutex<[Option<Instant>; PORTS]> = Mutex::new([None; PORTS]);
/// After a poll with data, the next ones read nothing for this long.
const QUIET: Duration = Duration::from_millis(10);
/// Each port's frame being written.
static PENDING: Mutex<[Vec<u8>; PORTS]> = Mutex::new([const { Vec::new() }; PORTS]);

/// The length of the complete frame at the start of `b`, if any (0: junk before a `FE`).
fn frame_len(b: &[u8]) -> Option<usize> {
    if b.first() != Some(&0xFE) {
        return Some(b.iter().position(|&x| x == 0xFE).unwrap_or(b.len()));
    }
    let len = usize::from(u16::from_le_bytes([*b.get(3)?, *b.get(4)?]));
    let sum = 5 + len;
    let end = if *b.get(sum)? == 0xFE { sum + 2 } else { sum + 1 };
    (b.len() >= end).then_some(end)
}

fn handle(port: i32) -> Option<P> {
    let h = *HANDLES.lock().unwrap().get(usize::try_from(port).ok()?)?;
    (h != 0).then_some(h as P)
}

pub fn resolve(name: &str) -> Option<usize> {
    Some(match name {
        "RS232_GetPortIndexByName" => index_by_name as *const () as usize,
        "RS232_OpenComport" => open as *const () as usize,
        "RS232_CloseComport" => close as *const () as usize,
        "RS232_PollComport" => poll as *const () as usize,
        "RS232_SendBuf" => send_buf as *const () as usize,
        "RS232_SendByte" => send_byte as *const () as usize,
        "RS232_flushRX" | "RS232_flushTX" | "RS232_flushRXTX" | "RS232_enableDTR" | "RS232_disableDTR" | "RS232_enableRTS" | "RS232_disableRTS"
        | "RS232_enableBREAK" | "RS232_disableBREAK" => nothing as *const () as usize,
        "RS232_IsCTSEnabled" | "RS232_IsDSREnabled" | "RS232_IsDCDEnabled" | "RS232_IsRINGEnabled" | "RS232_GetLAstError" => zero as *const () as usize,
        _ => return None,
    })
}

/// `COM3` or `\\.\COM3`: 2; -1 otherwise.
unsafe extern "C" fn index_by_name(name: *const i8) -> i32 {
    if name.is_null() {
        return -1;
    }
    let name = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy();
    let n = name.trim_start_matches("\\\\.\\").to_ascii_uppercase();
    n.strip_prefix("COM").and_then(|d| d.parse::<i32>().ok()).filter(|&d| (1..=PORTS as i32).contains(&d)).map_or(-1, |d| d - 1)
}

unsafe extern "C" fn open(port: i32, baud: i32, _mode: *const i8, _flow: i32) -> i32 {
    let Some(index) = usize::try_from(port).ok().filter(|&p| p < PORTS) else { return 1 };
    let create = crate::steering::resolve_kernel32("CreateFileA").unwrap();
    type F = unsafe extern "system" fn(*const u8, u32, u32, P, u32, u32, P) -> P;
    let name = format!("\\\\.\\COM{}\0", index + 1);
    let h = unsafe { std::mem::transmute::<usize, F>(create)(name.as_ptr(), 0xC000_0000, 0, std::ptr::null_mut(), 3, 0, std::ptr::null_mut()) };
    if h.is_null() || h as usize == INVALID {
        return 1;
    }
    log!("rs232: COM{} open ({baud} bauds)", index + 1);
    HANDLES.lock().unwrap()[index] = h as usize;
    0
}

unsafe extern "C" fn close(port: i32) {
    let Some(index) = usize::try_from(port).ok().filter(|&p| p < PORTS) else { return };
    let h = std::mem::take(&mut HANDLES.lock().unwrap()[index]);
    if h != 0 {
        let close = crate::steering::resolve_kernel32("CloseHandle").unwrap();
        unsafe { std::mem::transmute::<usize, unsafe extern "system" fn(P) -> i32>(close)(h as P) };
    }
}

unsafe extern "C" fn poll(port: i32, buf: *mut u8, size: i32) -> i32 {
    let Some(h) = handle(port) else { return -1 };
    let mut last = LAST_DATA.lock().unwrap();
    let last = &mut last[port as usize];
    if last.is_some_and(|t| t.elapsed() < QUIET) {
        return 0;
    }
    let read = serial::replacement("ReadFile").unwrap();
    type F = unsafe extern "system" fn(P, *mut u8, u32, *mut u32, P) -> i32;
    let mut got = 0u32;
    unsafe { std::mem::transmute::<usize, F>(read)(h, buf, size.max(0) as u32, &mut got, std::ptr::null_mut()) };
    if got > 0 {
        *last = Some(Instant::now());
    }
    got as i32
}

unsafe extern "C" fn send_buf(port: i32, buf: *const u8, size: i32) -> i32 {
    let Some(h) = handle(port) else { return -1 };
    if buf.is_null() || size <= 0 {
        return 0;
    }
    let write = serial::replacement("WriteFile").unwrap();
    type F = unsafe extern "system" fn(P, *const u8, u32, *mut u32, P) -> i32;
    let mut pending = PENDING.lock().unwrap();
    let pending = &mut pending[port as usize];
    pending.extend_from_slice(unsafe { std::slice::from_raw_parts(buf, size as usize) });
    while let Some(n) = frame_len(pending) {
        if n == 0 {
            break;
        }
        let packet: Vec<u8> = pending.drain(..n).collect();
        if packet[0] == 0xFE {
            let mut written = 0u32;
            unsafe { std::mem::transmute::<usize, F>(write)(h, packet.as_ptr(), packet.len() as u32, &mut written, std::ptr::null_mut()) };
        }
    }
    size
}

unsafe extern "C" fn send_byte(port: i32, byte: u8) -> i32 {
    // 0 sent, 1 error
    i32::from(unsafe { send_buf(port, &byte, 1) } != 1)
}

extern "C" fn nothing(_port: i32) {}

extern "C" fn zero(_port: i32) -> i32 {
    0
}
