//! Namco USIO board on USB (WinUSB), as games linking the USIO library statically reach it
//! (Time Crisis 5): the device interface {E54BF304-66A3-476F-A456-C6DDAB8C7078} (bananatools'
//! `usio_guid`) is enumerated with SetupDi, opened with `CreateFileW` and WinUSB. Its memory is
//! read and written through two bulk pipes:
//!
//! - read: `10 00 addr16 len16` on the OUT pipe, then `len` bytes from the IN pipe;
//! - write: `90 (~addr >> 8 & 0xf0) addr16 len16`, the data, then a 1-byte answer.
//!
//! The library's thread reads 0x0000 (0x100 bytes: +0x02 board status, 0 = no error; +0x28 the
//! game's `81 73` marker, written once otherwise), 0x1080 (0x80 bytes: the switches dword,
//! then at 0x1090 / 0x1092 the coin and service counters, 14 bits, the top 2 an error), 0x1800
//! once (0x80 bytes), and writes the outputs at 0x1420 (0x20 bytes). The library's own
//! watchdog writes `99 99` at 0x0008.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

const GUID: [u8; 16] = [0x04, 0xF3, 0x4B, 0xE5, 0xA3, 0x66, 0x6F, 0x47, 0xA4, 0x56, 0xC6, 0xDD, 0xAB, 0x8C, 0x70, 0x78];
const PATH: &str = "\\\\?\\usb#vid_0b9a&pid_0f00#wal-usio#{e54bf304-66a3-476f-a456-c6ddab8c7078}";
/// Fake handles: the device list, the device file, the WinUSB interface.
const DEVINFO: usize = 0x5553_1000;
const FILE: usize = 0x5553_1004;
const IFACE: usize = 0x5553_1008;
const PIPE_IN: u8 = 0x81;
const PIPE_OUT: u8 = 0x02;
const PIPE_INT: u8 = 0x83;

/// The inputs the board reports.
#[derive(Default, Clone, Copy)]
pub(crate) struct Inputs {
    pub switches: u32,
    /// Coin and service counters (they only count up).
    pub coins: [u16; 2],
    /// The analog inputs (0x10a0, eight words: `winusio.dll`'s `GetAnalogIn`).
    pub analog: [u16; 8],
}

enum Pending {
    None,
    Read(usize, usize),
    Write(usize, usize),
    Ack,
}

struct Board {
    memory: Vec<u8>,
    pending: Pending,
    poll: fn() -> Inputs,
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);

static CLASS_DEVS: AtomicUsize = AtomicUsize::new(0);
static ENUM_INTERFACES: AtomicUsize = AtomicUsize::new(0);
static INTERFACE_DETAIL: AtomicUsize = AtomicUsize::new(0);
static DESTROY_LIST: AtomicUsize = AtomicUsize::new(0);
static CREATE_FILE: AtomicUsize = AtomicUsize::new(0);
static CLASS_DEVS_A: AtomicUsize = AtomicUsize::new(0);
static INTERFACE_DETAIL_A: AtomicUsize = AtomicUsize::new(0);
static CREATE_FILE_A: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" {
    fn SetLastError(code: u32);
    fn SetEvent(event: *mut c_void) -> i32;
}

const ERROR_NO_MORE_ITEMS: u32 = 259;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

fn path() -> Vec<u16> {
    PATH.encode_utf16().chain(Some(0)).collect()
}

unsafe extern "system" fn class_devs(guid: *const [u8; 16], enumerator: *const u16, hwnd: usize, flags: u32) -> usize {
    if !guid.is_null() && unsafe { *guid } == GUID {
        return DEVINFO;
    }
    let orig: unsafe extern "system" fn(*const [u8; 16], *const u16, usize, u32) -> usize = unsafe { std::mem::transmute(CLASS_DEVS.load(Ordering::Relaxed)) };
    unsafe { orig(guid, enumerator, hwnd, flags) }
}

unsafe extern "system" fn enum_interfaces(set: usize, info: *mut c_void, guid: *const [u8; 16], index: u32, data: *mut u8) -> i32 {
    if set != DEVINFO {
        let orig: unsafe extern "system" fn(usize, *mut c_void, *const [u8; 16], u32, *mut u8) -> i32 = unsafe { std::mem::transmute(ENUM_INTERFACES.load(Ordering::Relaxed)) };
        return unsafe { orig(set, info, guid, index, data) };
    }
    if index != 0 || data.is_null() {
        unsafe { SetLastError(ERROR_NO_MORE_ITEMS) };
        return 0;
    }
    // SP_DEVICE_INTERFACE_DATA: cbSize, class guid, flags (active), reserved
    unsafe {
        std::ptr::copy_nonoverlapping(GUID.as_ptr(), data.add(4), 16);
        *(data.add(20) as *mut u32) = 1;
    }
    1
}

unsafe extern "system" fn interface_detail(set: usize, data: *mut u8, detail: *mut u8, size: u32, required: *mut u32, info: *mut c_void) -> i32 {
    if set != DEVINFO {
        let orig: unsafe extern "system" fn(usize, *mut u8, *mut u8, u32, *mut u32, *mut c_void) -> i32 = unsafe { std::mem::transmute(INTERFACE_DETAIL.load(Ordering::Relaxed)) };
        return unsafe { orig(set, data, detail, size, required, info) };
    }
    // SP_DEVICE_INTERFACE_DETAIL_DATA_W: cbSize, then the path
    let path = path();
    let need = 4 + path.len() as u32 * 2;
    if !required.is_null() {
        unsafe { *required = need };
    }
    if detail.is_null() || size < need {
        unsafe { SetLastError(ERROR_INSUFFICIENT_BUFFER) };
        return 0;
    }
    unsafe { std::ptr::copy_nonoverlapping(path.as_ptr(), detail.add(4) as *mut u16, path.len()) };
    1
}

unsafe extern "system" fn class_devs_a(guid: *const [u8; 16], enumerator: *const u8, hwnd: usize, flags: u32) -> usize {
    if !guid.is_null() && unsafe { *guid } == GUID {
        return DEVINFO;
    }
    let orig: unsafe extern "system" fn(*const [u8; 16], *const u8, usize, u32) -> usize = unsafe { std::mem::transmute(CLASS_DEVS_A.load(Ordering::Relaxed)) };
    unsafe { orig(guid, enumerator, hwnd, flags) }
}

unsafe extern "system" fn interface_detail_a(set: usize, data: *mut u8, detail: *mut u8, size: u32, required: *mut u32, info: *mut c_void) -> i32 {
    if set != DEVINFO {
        let orig: unsafe extern "system" fn(usize, *mut u8, *mut u8, u32, *mut u32, *mut c_void) -> i32 = unsafe { std::mem::transmute(INTERFACE_DETAIL_A.load(Ordering::Relaxed)) };
        return unsafe { orig(set, data, detail, size, required, info) };
    }
    // SP_DEVICE_INTERFACE_DETAIL_DATA_A: cbSize, then the path
    let need = 4 + PATH.len() as u32 + 1;
    if !required.is_null() {
        unsafe { *required = need };
    }
    if detail.is_null() || size < need {
        unsafe { SetLastError(ERROR_INSUFFICIENT_BUFFER) };
        return 0;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(PATH.as_ptr(), detail.add(4), PATH.len());
        *detail.add(4 + PATH.len()) = 0;
    }
    1
}

unsafe extern "system" fn create_file_a(name: *const u8, access: u32, share: u32, sa: *mut c_void, disposition: u32, flags: u32, template: usize) -> usize {
    if !name.is_null() && unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes().eq_ignore_ascii_case(PATH.as_bytes()) {
        log!("usio: device opened");
        return FILE;
    }
    type CreateFileA = unsafe extern "system" fn(*const u8, u32, u32, *mut c_void, u32, u32, usize) -> usize;
    let orig: CreateFileA = unsafe { std::mem::transmute(CREATE_FILE_A.load(Ordering::Relaxed)) };
    unsafe { orig(name, access, share, sa, disposition, flags, template) }
}

unsafe extern "system" fn destroy_list(set: usize) -> i32 {
    if set == DEVINFO {
        return 1;
    }
    let orig: unsafe extern "system" fn(usize) -> i32 = unsafe { std::mem::transmute(DESTROY_LIST.load(Ordering::Relaxed)) };
    unsafe { orig(set) }
}

unsafe extern "system" fn create_file(name: *const u16, access: u32, share: u32, sa: *mut c_void, disposition: u32, flags: u32, template: usize) -> usize {
    if !name.is_null() {
        let len = (0..).take_while(|&i| unsafe { *name.add(i) } != 0).count();
        let name = unsafe { std::slice::from_raw_parts(name, len) };
        if String::from_utf16_lossy(name).eq_ignore_ascii_case(PATH) {
            log!("usio: device opened");
            return FILE;
        }
    }
    type CreateFileW = unsafe extern "system" fn(*const u16, u32, u32, *mut c_void, u32, u32, usize) -> usize;
    let orig: CreateFileW = unsafe { std::mem::transmute(CREATE_FILE.load(Ordering::Relaxed)) };
    unsafe { orig(name, access, share, sa, disposition, flags, template) }
}

unsafe extern "system" fn initialize(file: usize, iface: *mut usize) -> i32 {
    if file != FILE || iface.is_null() {
        return 0;
    }
    unsafe { *iface = IFACE };
    1
}

unsafe extern "system" fn free(_iface: usize) -> i32 {
    1
}

unsafe extern "system" fn device_information(_iface: usize, _kind: u32, len: *mut u32, buf: *mut u8) -> i32 {
    // DEVICE_SPEED: high speed
    unsafe {
        if !buf.is_null() && !len.is_null() && *len >= 1 {
            *buf = 3;
            *len = 1;
        }
    }
    1
}

unsafe extern "system" fn interface_settings(_iface: usize, _alt: u8, desc: *mut u8) -> i32 {
    // USB_INTERFACE_DESCRIPTOR: 3 endpoints, vendor class
    let d = [9u8, 4, 0, 0, 3, 0xFF, 0, 0, 0];
    unsafe { std::ptr::copy_nonoverlapping(d.as_ptr(), desc, d.len()) };
    1
}

unsafe extern "system" fn query_pipe(_iface: usize, _alt: u8, index: u8, info: *mut u8) -> i32 {
    // WINUSB_PIPE_INFORMATION: type (2 bulk, 3 interrupt), id, max packet size, interval
    let (kind, id, size) = match index {
        0 => (2u32, PIPE_IN, 512u16),
        1 => (2, PIPE_OUT, 512),
        2 => (3, PIPE_INT, 64),
        _ => return 0,
    };
    unsafe {
        std::ptr::write_bytes(info, 0, 12);
        *(info as *mut u32) = kind;
        *info.add(4) = id;
        *(info.add(6) as *mut u16) = size;
        *info.add(8) = 1;
    }
    1
}

/// A completed transfer: the count, the overlapped result and its event.
unsafe fn complete(n: usize, transferred: *mut u32, ov: *mut usize) -> i32 {
    unsafe {
        if !transferred.is_null() {
            *transferred = n as u32;
        }
        if !ov.is_null() {
            *ov = 0; // Internal: STATUS_SUCCESS
            *ov.add(1) = n; // InternalHigh: the count
            let event = *ov.add(3) as *mut c_void;
            if !event.is_null() {
                SetEvent(event);
            }
        }
    }
    1
}

unsafe extern "system" fn write_pipe(_iface: usize, _pipe: u8, buf: *const u8, len: u32, transferred: *mut u32, ov: *mut usize) -> i32 {
    let data = unsafe { std::slice::from_raw_parts(buf, len as usize) };
    let mut guard = BOARD.lock().unwrap();
    let Some(board) = guard.as_mut() else { return 0 };
    let word = |i: usize| u16::from_le_bytes([data[i], data[i + 1]]) as usize;
    match board.pending {
        Pending::Write(addr, n) => {
            let n = n.min(data.len()).min(board.memory.len() - addr);
            board.memory[addr..addr + n].copy_from_slice(&data[..n]);
            board.pending = Pending::Ack;
        }
        _ if data.len() == 6 && data[0] == 0x10 => {
            board.pending = Pending::Read(word(2), word(4));
        }
        _ if data.len() == 6 && data[0] == 0x90 => {
            let (addr, n) = (word(2), word(4));
            if addr != 0x1420 && addr != 0x0008 {
                log!("usio: write {addr:#06x} ({n} bytes)");
            }
            board.pending = Pending::Write(addr, n);
        }
        _ => log!("usio: unknown request {data:02X?}"),
    }
    unsafe { complete(data.len(), transferred, ov) }
}

unsafe extern "system" fn read_pipe(_iface: usize, _pipe: u8, buf: *mut u8, len: u32, transferred: *mut u32, ov: *mut usize) -> i32 {
    let out = unsafe { std::slice::from_raw_parts_mut(buf, len as usize) };
    let mut guard = BOARD.lock().unwrap();
    let Some(board) = guard.as_mut() else { return 0 };
    let n = match std::mem::replace(&mut board.pending, Pending::None) {
        Pending::Read(addr, n) => {
            if std::env::var("WAL_NAMCOES3_USIO_TRACE").is_ok_and(|v| v == "1") {
                log!("usio: read {addr:#06x} ({n} bytes)");
            }
            if (0x1080..0x1100).contains(&addr) {
                let inputs = (board.poll)();
                board.memory[0x1080..0x1084].copy_from_slice(&inputs.switches.to_le_bytes());
                board.memory[0x1090..0x1092].copy_from_slice(&(inputs.coins[0] & 0x3FFF).to_le_bytes());
                board.memory[0x1092..0x1094].copy_from_slice(&(inputs.coins[1] & 0x3FFF).to_le_bytes());
                for (i, a) in inputs.analog.iter().enumerate() {
                    board.memory[0x10a0 + 2 * i..0x10a2 + 2 * i].copy_from_slice(&a.to_le_bytes());
                }
            }
            let n = n.min(out.len()).min(board.memory.len() - addr);
            out[..n].copy_from_slice(&board.memory[addr..addr + n]);
            n
        }
        Pending::Ack => {
            out.first_mut().map(|b| *b = 0);
            out.len().min(1)
        }
        _ => 0,
    };
    unsafe { complete(n, transferred, ov) }
}

unsafe extern "system" fn overlapped_result(_iface: usize, ov: *const usize, transferred: *mut u32, _wait: i32) -> i32 {
    unsafe {
        if !transferred.is_null() && !ov.is_null() {
            *transferred = *ov.add(1) as u32;
        }
    }
    1
}

/// Sets board memory at `addr` (identity fields read by the library: `winusio.dll`'s firmware
/// version is the word at 0x12 when 0x06 is 0, its board name the string at 0x1800).
pub(crate) fn set_memory(addr: usize, data: &[u8]) {
    if let Some(board) = BOARD.lock().unwrap().as_mut() {
        board.memory[addr..addr + data.len()].copy_from_slice(data);
    }
}

/// Installs the board on the game executable's imports (`base`); `poll` gives its inputs.
pub(crate) fn init(base: usize, poll: fn() -> Inputs) {
    let mut memory = vec![0u8; 0x10000];
    // the game's marker (else written at boot)
    memory[0x28..0x2A].copy_from_slice(&0x8173u16.to_le_bytes());
    *BOARD.lock().unwrap() = Some(Board { memory, pending: Pending::None, poll });
    let hooks: &[(&str, &str, usize, Option<&AtomicUsize>)] = &[
        ("setupapi.dll", "SetupDiGetClassDevsW", class_devs as *const () as usize, Some(&CLASS_DEVS)),
        ("setupapi.dll", "SetupDiEnumDeviceInterfaces", enum_interfaces as *const () as usize, Some(&ENUM_INTERFACES)),
        ("setupapi.dll", "SetupDiGetDeviceInterfaceDetailW", interface_detail as *const () as usize, Some(&INTERFACE_DETAIL)),
        ("setupapi.dll", "SetupDiDestroyDeviceInfoList", destroy_list as *const () as usize, Some(&DESTROY_LIST)),
        ("kernel32.dll", "CreateFileW", create_file as *const () as usize, Some(&CREATE_FILE)),
        // the USIO library as its own DLL (`winusio.dll`, JoJo's Bizarre Adventure): ANSI
        ("setupapi.dll", "SetupDiGetClassDevsA", class_devs_a as *const () as usize, Some(&CLASS_DEVS_A)),
        ("setupapi.dll", "SetupDiGetDeviceInterfaceDetailA", interface_detail_a as *const () as usize, Some(&INTERFACE_DETAIL_A)),
        ("kernel32.dll", "CreateFileA", create_file_a as *const () as usize, Some(&CREATE_FILE_A)),
        ("winusb.dll", "WinUsb_Initialize", initialize as *const () as usize, None),
        ("winusb.dll", "WinUsb_Free", free as *const () as usize, None),
        ("winusb.dll", "WinUsb_QueryDeviceInformation", device_information as *const () as usize, None),
        ("winusb.dll", "WinUsb_QueryInterfaceSettings", interface_settings as *const () as usize, None),
        ("winusb.dll", "WinUsb_QueryPipe", query_pipe as *const () as usize, None),
        ("winusb.dll", "WinUsb_ReadPipe", read_pipe as *const () as usize, None),
        ("winusb.dll", "WinUsb_WritePipe", write_pipe as *const () as usize, None),
        ("winusb.dll", "WinUsb_GetOverlappedResult", overlapped_result as *const () as usize, None),
    ];
    for &(dll, name, f, slot) in hooks {
        match unsafe { iat::hook_module(base, dll, name, f) } {
            Some(orig) => {
                if let Some(slot) = slot {
                    slot.store(orig, Ordering::Relaxed);
                }
            }
            None => log!("usio: {dll}!{name} not hooked"),
        }
    }
    log!("usio: USB I/O board");
}
