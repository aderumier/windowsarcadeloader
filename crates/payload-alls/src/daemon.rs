//! The payload in `amdaemon.exe`: the cabinet's devices it opens.
//!
//! - `\??\FddDriver`: the keychip (play counters, trace log; segatools' `nusec.c`)
//! - `\\.\sghwmonitor`: the hardware monitor (CPU temperature), `\\.\sghwreset`
//! - the "Type 4" USB I/O board, a HID device (`io4.rs`)
//!
//! Each is a fake handle answered by the hooked file calls of amdaemon's imports.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

use crate::{io4, registry};

/// Fake handles.
pub(crate) const KEYCHIP: usize = 0x5A11_0000;
pub(crate) const HWMON: usize = 0x5A11_0004;
pub(crate) const HWRESET: usize = 0x5A11_0008;
pub(crate) const IO4: usize = 0x5A11_000C;

fn is_fake(h: usize) -> bool {
    matches!(h, KEYCHIP | HWMON | HWRESET | IO4)
}

static CREATE_FILE_W: AtomicUsize = AtomicUsize::new(0);
static CREATE_FILE_A: AtomicUsize = AtomicUsize::new(0);
static DEVICE_IO_CONTROL: AtomicUsize = AtomicUsize::new(0);
static READ_FILE: AtomicUsize = AtomicUsize::new(0);
static WRITE_FILE: AtomicUsize = AtomicUsize::new(0);
static CLOSE_HANDLE: AtomicUsize = AtomicUsize::new(0);
static GET_OVERLAPPED_RESULT: AtomicUsize = AtomicUsize::new(0);
static CANCEL_IO: AtomicUsize = AtomicUsize::new(0);
static CANCEL_IO_EX: AtomicUsize = AtomicUsize::new(0);

const ERROR_IO_PENDING: u32 = 997;
const ERROR_INVALID_FUNCTION: u32 = 1;
const STATUS_PENDING: usize = 0x103;

unsafe extern "system" {
    fn SetLastError(code: u32);
    fn SetEvent(event: *mut c_void) -> i32;
    fn WaitForSingleObject(h: *mut c_void, ms: u32) -> u32;
}

#[repr(C)]
pub(crate) struct Overlapped {
    pub internal: usize,
    pub internal_high: usize,
    pub offset: u64,
    pub event: *mut c_void,
}

/// Completes an I/O at once: its result in `overlapped` (event set) or `transferred`.
pub(crate) unsafe fn complete(ov: *mut Overlapped, transferred: *mut u32, n: usize) {
    unsafe {
        if !transferred.is_null() {
            *transferred = n as u32;
        }
        if !ov.is_null() {
            (*ov).internal = 0;
            (*ov).internal_high = n;
            if !(*ov).event.is_null() {
                SetEvent((*ov).event);
            }
        }
    }
}

fn wide_str(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

fn device_of(name: &str) -> Option<usize> {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with("\\fdddriver") {
        Some(KEYCHIP)
    } else if lower == "\\\\.\\sghwmonitor" {
        Some(HWMON)
    } else if lower == "\\\\.\\sghwreset" {
        Some(HWRESET)
    } else if lower == io4::PATH {
        Some(IO4)
    } else {
        None
    }
}

static OPENED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn log_open(name: &str) {
    let mut opened = OPENED.lock().unwrap();
    if !opened.iter().any(|n| n == name) {
        opened.push(name.to_string());
        log!("alls: amdaemon opens {name}");
    }
}

type CreateFileW = unsafe extern "system" fn(*const u16, u32, u32, *mut c_void, u32, u32, usize) -> usize;
type CreateFileA = unsafe extern "system" fn(*const u8, u32, u32, *mut c_void, u32, u32, usize) -> usize;

unsafe extern "system" fn create_file_w(name: *const u16, access: u32, share: u32, sa: *mut c_void, disposition: u32, flags: u32, template: usize) -> usize {
    let s = wide_str(name);
    if let Some(h) = device_of(&s) {
        log_open(&s);
        return h;
    }
    if s.starts_with("\\\\") {
        log_open(&s);
    }
    let orig: CreateFileW = unsafe { std::mem::transmute(CREATE_FILE_W.load(Ordering::Relaxed)) };
    unsafe { orig(name, access, share, sa, disposition, flags, template) }
}

unsafe extern "system" fn create_file_a(name: *const u8, access: u32, share: u32, sa: *mut c_void, disposition: u32, flags: u32, template: usize) -> usize {
    if !name.is_null() {
        let s = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy().into_owned();
        if let Some(h) = device_of(&s) {
            log_open(&s);
            return h;
        }
        if s.starts_with("\\\\") {
            log_open(&s);
        }
    }
    let orig: CreateFileA = unsafe { std::mem::transmute(CREATE_FILE_A.load(Ordering::Relaxed)) };
    unsafe { orig(name, access, share, sa, disposition, flags, template) }
}

type DeviceIoControl = unsafe extern "system" fn(usize, u32, *const u8, u32, *mut u8, u32, *mut u32, *mut Overlapped) -> i32;

unsafe extern "system" fn device_io_control(h: usize, code: u32, input: *const u8, in_len: u32, output: *mut u8, out_len: u32, returned: *mut u32, ov: *mut Overlapped) -> i32 {
    if !is_fake(h) {
        let orig: DeviceIoControl = unsafe { std::mem::transmute(DEVICE_IO_CONTROL.load(Ordering::Relaxed)) };
        return unsafe { orig(h, code, input, in_len, output, out_len, returned, ov) };
    }
    let input = if input.is_null() { &[][..] } else { unsafe { std::slice::from_raw_parts(input, in_len as usize) } };
    let output = if output.is_null() { &mut [][..] } else { unsafe { std::slice::from_raw_parts_mut(output, out_len as usize) } };
    let answer = match h {
        KEYCHIP => keychip_ioctl(code, input, output),
        HWMON => Some(put_u32s(output, &[52])),
        HWRESET => Some(0),
        IO4 => io4::ioctl(code, input, output),
        _ => None,
    };
    match answer {
        Some(n) => {
            unsafe { complete(ov, returned, n) };
            1
        }
        None => {
            log!("alls: device {h:#x} ioctl {code:#x} unknown (in {} out {})", input.len(), output.len());
            unsafe { SetLastError(ERROR_INVALID_FUNCTION) };
            0
        }
    }
}

fn put_u32s(out: &mut [u8], values: &[u32]) -> usize {
    let mut n = 0;
    for v in values {
        if n + 4 > out.len() {
            break;
        }
        out[n..n + 4].copy_from_slice(&v.to_le_bytes());
        n += 4;
    }
    n
}

fn put_bytes(out: &mut [u8], data: &[u8]) -> usize {
    let n = data.len().min(out.len());
    out[..n].copy_from_slice(&data[..n]);
    n
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes(s.try_into().unwrap()))
}

/// The keychip's play counters (not kept across launches: a fresh keychip each time).
/// `nearfull`: the billing type (high 16 bits: 0; type 1 makes amdaemon stop the game with
/// error 8114, "ALL.Net System error (RTC)") and the play count warning level.
struct Keychip {
    play_count: u32,
    play_limit: u32,
    nearfull: u32,
    log: Vec<Vec<u8>>,
    log_tail: u32,
}

static KEYCHIP_STATE: Mutex<Keychip> = Mutex::new(Keychip { play_count: 0, play_limit: 1024, nearfull: (1 << 16) + 512, log: Vec::new(), log_tail: 0 });

/// `CTL_CODE(0x22, function, METHOD_BUFFERED, access)`: the keychip ioctls by function.
fn keychip_ioctl(code: u32, input: &[u8], out: &mut [u8]) -> Option<usize> {
    let mut k = KEYCHIP_STATE.lock().unwrap();
    let head = k.log_tail + k.log.len() as u32;
    Some(match (code >> 2) & 0xfff {
        // ping
        0x845 => 0,
        0x854 => put_u32s(out, &[k.play_count]),
        0x855 => {
            k.play_count += u32_at(input, 0);
            put_u32s(out, &[k.play_count])
        }
        0x862 => {
            // erase trace log
            let count = (u32_at(input, 0) as usize).min(k.log.len());
            k.log.drain(..count);
            k.log_tail += count as u32;
            0
        }
        0x863 | 0x869 => {
            k.log.clear();
            k.log_tail = 0;
            0
        }
        0x864 => {
            k.log.push(input.to_vec());
            0
        }
        0x865 => {
            // trace log data: records from pos, count
            let (mut pos, count) = (u32_at(input, 0), u32_at(input, 4));
            let mut n = 0;
            for _ in 0..count {
                if pos >= head {
                    break;
                }
                let Some(rec) = k.log.get((pos - k.log_tail) as usize) else { break };
                if n + rec.len() > out.len() {
                    break;
                }
                out[n..n + rec.len()].copy_from_slice(rec);
                n += rec.len();
                pos += 1;
            }
            n
        }
        0x866 => put_u32s(out, &[k.log.len() as u32, k.log_tail]),
        0x867 => put_u32s(out, &[(10 * 4096 - k.log.len()) as u32]),
        0x881 => put_u32s(out, &[k.play_limit]),
        0x882 => {
            k.play_limit = u32_at(input, 0);
            0
        }
        0x883 => put_u32s(out, &[k.nearfull]),
        0x884 => {
            k.nearfull = u32_at(input, 0);
            0
        }
        0x893 => put_u32s(out, &[10, 4096]),
        0x871 => put_bytes(out, crate::pki::CA_CERT),
        0x872 => put_bytes(out, crate::pki::BILLING_PUB),
        _ => return None,
    })
}

type ReadFile = unsafe extern "system" fn(usize, *mut u8, u32, *mut u32, *mut Overlapped) -> i32;
type WriteFile = unsafe extern "system" fn(usize, *const u8, u32, *mut u32, *mut Overlapped) -> i32;

unsafe extern "system" fn read_file(h: usize, buf: *mut u8, len: u32, read: *mut u32, ov: *mut Overlapped) -> i32 {
    if h == IO4 {
        return unsafe { io4::read(buf, len as usize, read, ov) };
    }
    if is_fake(h) {
        unsafe { complete(ov, read, 0) };
        return 1;
    }
    let orig: ReadFile = unsafe { std::mem::transmute(READ_FILE.load(Ordering::Relaxed)) };
    unsafe { orig(h, buf, len, read, ov) }
}

unsafe extern "system" fn write_file(h: usize, buf: *const u8, len: u32, written: *mut u32, ov: *mut Overlapped) -> i32 {
    if h == IO4 {
        io4::write(unsafe { std::slice::from_raw_parts(buf, len as usize) });
    }
    if is_fake(h) {
        unsafe { complete(ov, written, len as usize) };
        return 1;
    }
    let orig: WriteFile = unsafe { std::mem::transmute(WRITE_FILE.load(Ordering::Relaxed)) };
    unsafe { orig(h, buf, len, written, ov) }
}

unsafe extern "system" fn close_handle(h: usize) -> i32 {
    if is_fake(h) {
        return 1;
    }
    let orig: unsafe extern "system" fn(usize) -> i32 = unsafe { std::mem::transmute(CLOSE_HANDLE.load(Ordering::Relaxed)) };
    unsafe { orig(h) }
}

unsafe extern "system" fn get_overlapped_result(h: usize, ov: *mut Overlapped, transferred: *mut u32, wait: i32) -> i32 {
    if !is_fake(h) {
        let orig: unsafe extern "system" fn(usize, *mut Overlapped, *mut u32, i32) -> i32 = unsafe { std::mem::transmute(GET_OVERLAPPED_RESULT.load(Ordering::Relaxed)) };
        return unsafe { orig(h, ov, transferred, wait) };
    }
    unsafe {
        if (*ov).internal == STATUS_PENDING {
            if wait == 0 || (*ov).event.is_null() {
                SetLastError(996); // ERROR_IO_INCOMPLETE
                return 0;
            }
            WaitForSingleObject((*ov).event, 0xFFFF_FFFF);
        }
        if (*ov).internal != 0 {
            SetLastError(995); // ERROR_OPERATION_ABORTED
            return 0;
        }
        if !transferred.is_null() {
            *transferred = (*ov).internal_high as u32;
        }
    }
    1
}

unsafe extern "system" fn cancel_io(h: usize) -> i32 {
    if h == IO4 {
        io4::cancel();
        return 1;
    }
    let orig: unsafe extern "system" fn(usize) -> i32 = unsafe { std::mem::transmute(CANCEL_IO.load(Ordering::Relaxed)) };
    unsafe { orig(h) }
}

unsafe extern "system" fn cancel_io_ex(h: usize, ov: *mut Overlapped) -> i32 {
    if h == IO4 {
        io4::cancel();
        return 1;
    }
    let orig: unsafe extern "system" fn(usize, *mut Overlapped) -> i32 = unsafe { std::mem::transmute(CANCEL_IO_EX.load(Ordering::Relaxed)) };
    unsafe { orig(h, ov) }
}

pub(crate) const fn pending() -> (usize, u32) {
    (STATUS_PENDING, ERROR_IO_PENDING)
}

fn module_path() -> String {
    let mut buf = [0u16; 1024];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(crate::module(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    String::from_utf16_lossy(&buf[..n])
}

unsafe extern "system" fn get_computer_name_a(buf: *mut u8, size: *mut u32) -> i32 {
    // the cabinet's main board id (an ALLS main id without its hyphen)
    let name = b"ACAE01A99999999";
    unsafe {
        if size.is_null() || buf.is_null() || (*size as usize) <= name.len() {
            if !size.is_null() {
                *size = name.len() as u32 + 1;
            }
            SetLastError(111); // ERROR_BUFFER_OVERFLOW
            return 0;
        }
        std::ptr::copy_nonoverlapping(name.as_ptr(), buf, name.len());
        *buf.add(name.len()) = 0;
        *size = name.len() as u32;
    }
    1
}

/// The cabinet never reboots from amdaemon (segatools' `misc.c`).
unsafe extern "system" fn exit_windows_ex(_flags: u32, _reason: u32) -> i32 {
    log!("alls: system reboot blocked");
    1
}

static QUERY_DOS_DEVICE_W: AtomicUsize = AtomicUsize::new(0);

/// The machine's optical drives hidden (segatools' `dvd.c`).
unsafe extern "system" fn query_dos_device_w(name: *const u16, target: *mut u16, max: u32) -> u32 {
    let real: unsafe extern "system" fn(*const u16, *mut u16, u32) -> u32 = unsafe { std::mem::transmute(QUERY_DOS_DEVICE_W.load(Ordering::Relaxed)) };
    let n = unsafe { real(name, target, max) };
    if n > 0 && !target.is_null() {
        let first = unsafe { std::slice::from_raw_parts(target, (0..max as usize).take_while(|&i| *target.add(i) != 0).count()) };
        if String::from_utf16_lossy(first).contains("CdRom") {
            return 0;
        }
    }
    n
}

pub(crate) fn init() {
    let cabinet = registry::Cabinet::from_env();
    registry::write(&cabinet, &module_path());
    crate::icf::ensure(&cabinet);
    crate::sysfile::apply();
    crate::netenv::init(cabinet.subnet);
    if std::env::var("WAL_ALLS_BOARD").is_ok_and(|v| v.trim() == "chusan") {
        crate::chuni::init_daemon();
    }
    crate::allnet::init();
    crate::aimedb::init();
    crate::aime::init();
    unsafe {
        let hooks: [(&str, &str, usize, &AtomicUsize); 9] = [
            ("kernel32.dll", "CreateFileW", create_file_w as *const () as usize, &CREATE_FILE_W),
            ("kernel32.dll", "CreateFileA", create_file_a as *const () as usize, &CREATE_FILE_A),
            ("kernel32.dll", "DeviceIoControl", device_io_control as *const () as usize, &DEVICE_IO_CONTROL),
            ("kernel32.dll", "ReadFile", read_file as *const () as usize, &READ_FILE),
            ("kernel32.dll", "WriteFile", write_file as *const () as usize, &WRITE_FILE),
            ("kernel32.dll", "CloseHandle", close_handle as *const () as usize, &CLOSE_HANDLE),
            ("kernel32.dll", "GetOverlappedResult", get_overlapped_result as *const () as usize, &GET_OVERLAPPED_RESULT),
            ("kernel32.dll", "CancelIo", cancel_io as *const () as usize, &CANCEL_IO),
            ("kernel32.dll", "CancelIoEx", cancel_io_ex as *const () as usize, &CANCEL_IO_EX),
        ];
        for (dll, name, f, slot) in hooks {
            match iat::hook(dll, name, f) {
                Some(orig) => slot.store(orig, Ordering::Relaxed),
                None => log!("alls: {name} not imported"),
            }
        }
        iat::hook("kernel32.dll", "GetComputerNameA", get_computer_name_a as *const () as usize);
        iat::hook("user32.dll", "ExitWindowsEx", exit_windows_ex as *const () as usize);
        if let Some(orig) = iat::hook("kernel32.dll", "QueryDosDeviceW", query_dos_device_w as *const () as usize) {
            QUERY_DOS_DEVICE_W.store(orig, Ordering::Relaxed);
        }
    }
    if std::env::var("WAL_ALLS_JVS").is_ok_and(|v| v.trim() == "1") {
        // a Nu cabinet: the AMEX board (its JVS I/O board, the system DIP switches, EEPROM and
        // SRAM in Y:\amex)
        let dip_switches = std::env::var("WAL_ALLS_DIPSW").ok().and_then(|v| u32::from_str_radix(v.trim(), 16).ok()).unwrap_or(0x01);
        wal_payload_common::amex::init(wal_payload_common::amex::Board { save: "Y:\\amex".into(), region: cabinet.region as u8, serial: "AAVE-01A99999999", dip_switches });
    } else {
        io4::init();
    }
    // amdaemon's own patches (the profile's: its clock's server sync)
    if let Ok(spec) = std::env::var("WAL_ALLS_DAEMON_PATCHES") {
        wal_payload_common::patches::apply_list(&spec);
    }
    log!("alls: amdaemon devices emulated");
}
