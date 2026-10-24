//! The RingEdge kernel drivers amLib talks to, emulated with IAT hooks on the game executable
//! (`CreateFileA/W`, `DeviceIoControl`, `CloseHandle`, the SetupDi device enumeration):
//!
//! * `\\.\mxsram` (amSram, amBackup: the battery-backed SRAM): the real file `SRAM.bin` of the
//!   save folder, read and written by the game itself; `DeviceIoControl` gives the driver
//!   version (`0x9C406000`) and the size (`IOCTL_DISK_GET_DRIVE_GEOMETRY`, 2 MB).
//! * The SMBus driver (amEeprom, amDipsw, amHm), found by its device interface class
//!   `5c49e1fe-3fec-4b8d-a4b5-76be7025d842`: version (`0x9C406008`), I2C block transfers
//!   (`0x9C40200C`: `{u8 status, u8 protocol (9 read, 8 write), u16 address, u16 register, u8
//!   length, u8 data[32]}`) and byte transfers (`0x9C402004`). The EEPROM (address 0x57) is
//!   `E2PROM.bin` of the save folder; other devices read as zero, but the PCA9535 I/O
//!   expander's push switches (TEST, SERVICE: [`smbus_byte`]) and DIP switches
//!   ([`dip_switches`]).
//! * `\\.\columba` (amPlatform): physical memory reads (`0x9C406104`: `{u64 address, u32 1,
//!   u32 size}`, the bytes back) of the BIOS area, where amPlatform finds the SMBIOS tables
//!   (legacy `_DMI_` entry point) and reads the BIOS version and date, the system manufacturer
//!   (`NEC`: RingEdge, `Supermicro`: RingWide) and the platform id (third OEM string: `AAL`
//!   RingEdge, `AAM` RingWide (`WAL_RINGEDGE_PLATFORM`);
//!   with `WAL_RINGEDGE_BOARD=ringedge2` a fifth one, `AAL2`: RingEdge 2).
//!   A small SMBIOS image is served at 0xF0000.
//! * `\\.\mxsuperio` (amHm: hardware monitor): a Winbond-like Super I/O chip ([`superio`]),
//!   other calls logged.
//! * `\\.\mxjvs` (amJvs' driver mode): a serial port of the JVS board (configured with the comm
//!   calls), each packet exchanged with `DeviceIoControl` `0x9C402000` (packet in, reply out).
//!
//! The EEPROM's static record (amLib's system data record 0, written by the system at install,
//! zero in a new `E2PROM.bin`) is written with `WAL_RINGEDGE_REGION` ([`write_static_record`]),
//! the SRAM's credit settings record (record 7, made by the system menu) with
//! `WAL_RINGEDGE_CREDIT_RECORD=1` ([`write_credit_record`]).
//!
//! Save folder: `WAL_RINGEDGE_SAVE` (default `sv`, relative to the game directory).
//!
//! The cabinet's user profile folder `C:\Documents and Settings\AppUser\` (Medal no Gunman
//! keeps its boot state there: `gunman_initialize.txt`, `gunman_lamp_sate.txt`...; without it
//! every boot ends on "Error Information" and hands over to the test menu) is the save folder's
//! `AppUser`, the files staying with the dump.

use std::ffi::c_void;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log, serial};

type P = *mut c_void;

const SRAM_SIZE: u64 = 2 * 1024 * 1024;
const EEPROM_SIZE: usize = 8192;
const EEPROM_ADDRESS: u16 = 0x57;
const SMBUS_GUID_DATA1: u32 = 0x5c49e1fe;

/// Fake handles: odd values, never real handles.
const H_SMBUS: usize = 0x5245_0001;
const H_SUPERIO: usize = 0x5245_0005;
const H_COLUMBA: usize = 0x5245_0009;
/// Fake device information set of the SetupDi enumeration.
const H_DEVINFO: usize = 0x5245_1001;

/// Path given for the SMBus device interface.
const SMBUS_PATH: &str = "\\\\.\\mxsmbus";

/// The real handle of `\\.\mxsram` (the SRAM file).
static SRAM_HANDLE: AtomicUsize = AtomicUsize::new(0);
static EEPROM: Mutex<Option<std::fs::File>> = Mutex::new(None);

/// `WAL_RINGEDGE_BOARD=ringedge2`: a RingEdge 2 board.
fn ringedge2() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAL_RINGEDGE_BOARD").is_ok_and(|b| b == "ringedge2"))
}

/// The save folder, absolute (the game changes its current directory).
fn save_dir() -> String {
    static DIR: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let dir = std::env::var("WAL_RINGEDGE_SAVE").unwrap_or_else(|_| "sv".into());
        match std::env::current_dir() {
            Ok(cwd) if std::path::Path::new(&dir).is_relative() => cwd.join(dir).to_string_lossy().into_owned(),
            _ => dir,
        }
    })
    .clone()
}

fn is_fake(h: P) -> bool {
    matches!(h as usize, H_SMBUS | H_SUPERIO | H_COLUMBA)
}

// Original functions, by hook index.
const CREATE_A: usize = 0;
const CREATE_W: usize = 1;
const IOCTL: usize = 2;
const CLOSE: usize = 3;
const CLASS_DEVS: usize = 4;
const ENUM_IF: usize = 5;
const IF_DETAIL: usize = 6;
const DESTROY: usize = 7;
static ORIG: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];

fn orig<F: Copy>(idx: usize) -> F {
    let p = ORIG[idx].load(Ordering::Relaxed);
    unsafe { std::mem::transmute_copy(&p) }
}

pub(crate) fn init() {
    // read now: an environment lookup inside a hook sets the thread's last error
    let _ = ringedge2();
    let _ = dip_switches();
    let dir = save_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::create_dir_all(format!("{dir}/AppUser"));
    // the SRAM file: created at its full size, the game formats it
    let sram = format!("{dir}/SRAM.bin");
    if let Ok(f) = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&sram) {
        if f.metadata().map(|m| m.len()).unwrap_or(0) < SRAM_SIZE {
            let _ = f.set_len(SRAM_SIZE);
        }
        write_credit_record(&f);
    }
    let eeprom = format!("{dir}/E2PROM.bin");
    match std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&eeprom) {
        Ok(f) => {
            if f.metadata().map(|m| m.len()).unwrap_or(0) < EEPROM_SIZE as u64 {
                let _ = f.set_len(EEPROM_SIZE as u64);
            }
            write_static_record(&f);
            *EEPROM.lock().unwrap() = Some(f);
        }
        Err(e) => log!("devices: cannot open {eeprom}: {e}"),
    }
    let hooks: [(&str, &str, usize); 8] = [
        ("kernel32.dll", "CreateFileA", create_file_a as *const () as usize),
        ("kernel32.dll", "CreateFileW", create_file_w as *const () as usize),
        ("kernel32.dll", "DeviceIoControl", device_io_control as *const () as usize),
        ("kernel32.dll", "CloseHandle", close_handle as *const () as usize),
        ("setupapi.dll", "SetupDiGetClassDevsA", get_class_devs as *const () as usize),
        ("setupapi.dll", "SetupDiEnumDeviceInterfaces", enum_interfaces as *const () as usize),
        ("setupapi.dll", "SetupDiGetDeviceInterfaceDetailA", interface_detail as *const () as usize),
        ("setupapi.dll", "SetupDiDestroyDeviceInfoList", destroy_info_list as *const () as usize),
    ];
    for (i, (dll, name, f)) in hooks.into_iter().enumerate() {
        match unsafe { iat::hook(dll, name, f) } {
            Some(o) => ORIG[i].store(o, Ordering::Relaxed),
            None => log!("devices: {dll}!{name} not imported"),
        }
    }
    log!("devices: SRAM and EEPROM in {dir}");
    crate::pcp::init_ds_file(&dir);
}

/// Device name of a path opened by the game (`\\.\mxsram` -> `mxsram`), lower case.
fn device_name(path: &str) -> Option<String> {
    let name = path.strip_prefix("\\\\.\\")?;
    Some(name.to_ascii_lowercase())
}

/// The save folder's file for a path in the cabinet's `C:\Documents and Settings\AppUser\`.
fn app_user_file(path: &str) -> Option<Vec<u16>> {
    const PREFIX: &str = "c:\\documents and settings\\appuser\\";
    let path = path.replace('/', "\\");
    let head = path.get(..PREFIX.len())?;
    if !head.eq_ignore_ascii_case(PREFIX) {
        return None;
    }
    let file = format!("{}\\AppUser\\{}", save_dir(), &path[PREFIX.len()..]);
    Some(file.encode_utf16().chain([0]).collect())
}

/// The handle for a device path, `None` for other files.
fn open_device(path: &str, access: u32, share: u32) -> Option<P> {
    let name = device_name(path)?;
    let h = match name.as_str() {
        "mxsram" => {
            let file: Vec<u16> = format!("{}\\SRAM.bin", save_dir()).encode_utf16().chain([0]).collect();
            let create: unsafe extern "system" fn(*const u16, u32, u32, P, u32, u32, P) -> P = orig(CREATE_W);
            // OPEN_EXISTING (made by init), the game's access and sharing
            let h = unsafe { create(file.as_ptr(), access, share, std::ptr::null_mut(), 3, 0x80, std::ptr::null_mut()) };
            if h as isize != -1 {
                SRAM_HANDLE.store(h as usize, Ordering::Relaxed);
            }
            h
        }
        "mxsmbus" => H_SMBUS as P,
        "mxsuperio" => H_SUPERIO as P,
        "columba" => H_COLUMBA as P,
        _ => return None,
    };
    log!("devices: {path} opened ({:#x})", h as usize);
    Some(h)
}

unsafe extern "system" fn create_file_a(name: *const u8, a: u32, s: u32, sa: P, d: u32, f: u32, t: P) -> P {
    if !name.is_null() {
        let path = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy().into_owned();
        if let Some(h) = open_device(&path, a, s) {
            return h;
        }
        if let Some(file) = app_user_file(&path) {
            return unsafe { orig::<unsafe extern "system" fn(*const u16, u32, u32, P, u32, u32, P) -> P>(CREATE_W)(file.as_ptr(), a, s, sa, d, f, t) };
        }
        if let Some(file) = crate::pcp::ds_file_replacement(&path) {
            let file: Vec<u16> = file.to_string_lossy().encode_utf16().chain([0]).collect();
            return unsafe { orig::<unsafe extern "system" fn(*const u16, u32, u32, P, u32, u32, P) -> P>(CREATE_W)(file.as_ptr(), a, s, sa, d, f, t) };
        }
    }
    unsafe { orig::<unsafe extern "system" fn(*const u8, u32, u32, P, u32, u32, P) -> P>(CREATE_A)(name, a, s, sa, d, f, t) }
}

unsafe extern "system" fn create_file_w(name: *const u16, a: u32, s: u32, sa: P, d: u32, f: u32, t: P) -> P {
    if !name.is_null() {
        let mut len = 0;
        while unsafe { *name.add(len) } != 0 {
            len += 1;
        }
        let path = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) });
        if let Some(h) = open_device(&path, a, s) {
            return h;
        }
        if let Some(file) = app_user_file(&path) {
            return unsafe { orig::<unsafe extern "system" fn(*const u16, u32, u32, P, u32, u32, P) -> P>(CREATE_W)(file.as_ptr(), a, s, sa, d, f, t) };
        }
        if let Some(file) = crate::pcp::ds_file_replacement(&path) {
            let file: Vec<u16> = file.to_string_lossy().encode_utf16().chain([0]).collect();
            return unsafe { orig::<unsafe extern "system" fn(*const u16, u32, u32, P, u32, u32, P) -> P>(CREATE_W)(file.as_ptr(), a, s, sa, d, f, t) };
        }
    }
    unsafe { orig::<unsafe extern "system" fn(*const u16, u32, u32, P, u32, u32, P) -> P>(CREATE_W)(name, a, s, sa, d, f, t) }
}

unsafe extern "system" fn close_handle(h: P) -> i32 {
    if is_fake(h) {
        return 1;
    }
    if h as usize == SRAM_HANDLE.load(Ordering::Relaxed) {
        SRAM_HANDLE.store(0, Ordering::Relaxed);
    }
    unsafe { orig::<unsafe extern "system" fn(P) -> i32>(CLOSE)(h) }
}

/// Number of unknown calls logged per device.
static UNKNOWN_LOGGED: AtomicUsize = AtomicUsize::new(0);

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

unsafe extern "system" fn device_io_control(
    h: P,
    code: u32,
    input: *const u8,
    in_size: u32,
    output: *mut u8,
    out_size: u32,
    returned: *mut u32,
    ov: P,
) -> i32 {
    let sram = h as usize != 0 && h as usize == SRAM_HANDLE.load(Ordering::Relaxed);
    // \\.\mxjvs: the JVS board of the serial emulation, one packet per call
    if code == 0x9C40_2000 && !input.is_null() && !output.is_null() {
        let packet = unsafe { std::slice::from_raw_parts(input, in_size as usize) };
        if let Some(reply) = serial::transact(h, packet) {
            let n = reply.len().min(out_size as usize);
            unsafe { std::ptr::copy_nonoverlapping(reply.as_ptr(), output, n) };
            return unsafe { complete(returned, ov, n) };
        }
    }
    if !sram && !is_fake(h) {
        let f: unsafe extern "system" fn(P, u32, *const u8, u32, *mut u8, u32, *mut u32, P) -> i32 = orig(IOCTL);
        return unsafe { f(h, code, input, in_size, output, out_size, returned, ov) };
    }
    let req = if input.is_null() { &[][..] } else { unsafe { std::slice::from_raw_parts(input, in_size as usize) } };
    let out = if output.is_null() { &mut [][..] } else { unsafe { std::slice::from_raw_parts_mut(output, out_size as usize) } };
    let reply: Option<usize> = match (h as usize, code) {
        // driver versions: 1.x
        (_, 0x9C40_6000 | 0x9C40_6008) if out.len() >= 4 => {
            out[..4].copy_from_slice(&0x0001_0001u32.to_le_bytes());
            Some(4)
        }
        // access unit (RingEdge 2's amSram): 4 bytes, its backup records are 4-byte aligned
        (_, 0x9C40_6004) if sram && out.len() >= 4 => {
            let unit: u32 = if ringedge2() { 4 } else { 512 };
            out[..4].copy_from_slice(&unit.to_le_bytes());
            Some(4)
        }
        // IOCTL_DISK_GET_DRIVE_GEOMETRY: 4096 sectors of 512 bytes
        (_, 0x0007_0000) if sram && out.len() >= 24 => {
            out[..24].fill(0);
            out[0..8].copy_from_slice(&1u64.to_le_bytes()); // cylinders
            out[8..12].copy_from_slice(&12u32.to_le_bytes()); // FixedMedia
            out[12..16].copy_from_slice(&1u32.to_le_bytes()); // tracks per cylinder
            out[16..20].copy_from_slice(&((SRAM_SIZE / 512) as u32).to_le_bytes());
            out[20..24].copy_from_slice(&512u32.to_le_bytes());
            Some(24)
        }
        (H_SMBUS, 0x9C40_200C) if req.len() >= 7 && out.len() >= 7 => Some(smbus_block(req, out)),
        (H_SMBUS, 0x9C40_2004) if req.len() >= 7 && out.len() >= 7 => Some(smbus_byte(req, out)),
        (H_SUPERIO, 0x9C40_2004 | 0x9C40_200C) if req.len() >= 3 && out.len() >= req.len() => Some(superio(code, req, out)),
        (H_SUPERIO, 0x9C40_A010) if req.len() >= 3 => {
            superio_write(req);
            Some(0)
        }
        (H_COLUMBA, 0x9C40_6104) if req.len() >= 16 => {
            let address = u64::from_le_bytes(req[0..8].try_into().unwrap());
            let size = (u32::from_le_bytes(req[12..16].try_into().unwrap()) as usize).min(out.len());
            read_bios(address, &mut out[..size]);
            Some(size)
        }
        _ => None,
    };
    let Some(n) = reply else {
        if UNKNOWN_LOGGED.fetch_add(1, Ordering::Relaxed) < 200 {
            log!("devices: {:#x} ioctl {code:#x} in [{}] out {out_size} not emulated", h as usize, hex(req));
        }
        unsafe { windows_sys::Win32::Foundation::SetLastError(1) }; // ERROR_INVALID_FUNCTION
        return 0;
    };
    unsafe { complete(returned, ov, n) }
}

/// Success with `n` bytes; overlapped calls (amDipsw, amJvs) complete at once.
unsafe fn complete(returned: *mut u32, ov: P, n: usize) -> i32 {
    if !returned.is_null() {
        unsafe { *returned = n as u32 };
    }
    if !ov.is_null() {
        let o = ov as *mut windows_sys::Win32::System::IO::OVERLAPPED;
        unsafe {
            (*o).Internal = 0;
            (*o).InternalHigh = n;
            if !(*o).hEvent.is_null() {
                windows_sys::Win32::System::Threading::SetEvent((*o).hEvent);
            }
        }
    }
    1
}

/// SMBus traces: `WAL_RINGEDGE_SMBUS_TRACE=<n>` first transfers (default 0).
fn smbus_trace(what: &str, req: &[u8], out: &[u8]) {
    static LIMIT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    let limit = *LIMIT.get_or_init(|| std::env::var("WAL_RINGEDGE_SMBUS_TRACE").ok().and_then(|v| v.parse().ok()).unwrap_or(0));
    if COUNT.fetch_add(1, Ordering::Relaxed) < limit {
        log!("smbus: {what} <- [{}] -> [{}]", hex(req), hex(out));
    }
}

/// I2C block transfer: `{status, protocol, address u16, register u16, length, data[32]}`.
fn smbus_block(req: &[u8], out: &mut [u8]) -> usize {
    let n = req.len().min(out.len());
    out[..n].copy_from_slice(&req[..n]);
    let protocol = req[1];
    let address = u16::from_le_bytes([req[2], req[3]]);
    let register = u16::from_le_bytes([req[4], req[5]]) as usize;
    let len = (req[6] as usize).min(32).min(n.saturating_sub(7));
    out[0] = 0; // success
    if address == EEPROM_ADDRESS {
        let mut guard = EEPROM.lock().unwrap();
        if let Some(f) = guard.as_mut() {
            let at = (register % EEPROM_SIZE) as u64;
            let _ = f.seek(SeekFrom::Start(at));
            match protocol {
                9 => {
                    let buf = &mut out[7..7 + len];
                    buf.fill(0xFF);
                    let _ = f.read(buf);
                }
                8 => {
                    let _ = f.write_all(&req[7..7 + len]);
                    let _ = f.flush();
                }
                _ => log!("smbus: EEPROM protocol {protocol:#x}"),
            }
        }
    } else if protocol == 9 {
        out[7..7 + len].fill(0);
    }
    smbus_trace("block", req, &out[..n]);
    n
}

/// The PCA9535 I/O expander of the main board (amDipsw).
const PCA9535_ADDRESS: u8 = 0x20;
/// Its input port 1: the board's push switches, active low.
const PUSHSW_TEST: u8 = 0x04;
const PUSHSW_SERVICE: u8 = 0x08;

/// The main board's DIP switches, PCA9535 input port 0 (active low): `WAL_RINGEDGE_DIPSW`
/// (hex, the switches on as amDipsw returns them; default FF, all on). Games read their
/// monitor resolution there: all on, Virtua Tennis 4, Koihime Enbu and Under Night render
/// 1920x1080 (all off: 640x480, 640x480, 1280x720). Ge-Sen Love Plus Pengo! only accepts bits
/// 0x70 at 0x00, 0x10 or 0x40 ("ERROR 0910 Wrong Resolution Setting" otherwise).
/// The EEPROM's static record (amLibReadStaticInfo: record 0 at 0, its copy at 0x200, 32
/// bytes: CRC-32 of bytes 4-31, the region at 12, the serial number at 15). Without it amLib
/// takes a region of 0: Dream Raiders ANDs it with the keychip's region, error 0903 "wrong
/// region". `WAL_RINGEDGE_REGION=<hex>` writes it with that region, when it differs.
fn write_static_record(mut f: &std::fs::File) {
    let Some(region) = std::env::var("WAL_RINGEDGE_REGION").ok().and_then(|v| u8::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok()) else {
        return;
    };
    let mut record = [0u8; 32];
    if f.seek(SeekFrom::Start(0)).is_ok() && f.read_exact(&mut record).is_ok() && u32::from_le_bytes(record[..4].try_into().unwrap()) == crc32(&record[4..]) && record[12] == region {
        return;
    }
    let mut record = [0u8; 32];
    record[12] = region;
    record[15..31].copy_from_slice(b"INVALID_SERIALNO");
    let crc = crc32(&record[4..]);
    record[..4].copy_from_slice(&crc.to_le_bytes());
    for at in [0, 0x200] {
        if f.seek(SeekFrom::Start(at)).and_then(|_| f.write_all(&record)).is_err() {
            log!("devices: cannot write the EEPROM's static record");
            return;
        }
    }
    log!("devices: EEPROM static record written, region {region:02X}");
}

/// The SRAM's credit settings record (amSysData record 7 at 0, its copy at 0x1000, 512 bytes:
/// CRC-32 of bytes 4-511, then the settings), made by the system menu: a new `SRAM.bin` has
/// none. Dream Raiders reads it at boot and stops on error 0025 "main board malfunctioning"
/// without it (the error screen then spins: amCredit is not initialized yet).
/// `WAL_RINGEDGE_CREDIT_RECORD=1` writes an all-zero one (the game's defaults) when neither
/// copy is valid.
fn write_credit_record(mut f: &std::fs::File) {
    if std::env::var("WAL_RINGEDGE_CREDIT_RECORD").is_err() {
        return;
    }
    let mut record = [0u8; 512];
    for at in [0, 0x1000] {
        if f.seek(SeekFrom::Start(at)).is_ok() && f.read_exact(&mut record).is_ok() && u32::from_le_bytes(record[..4].try_into().unwrap()) == crc32(&record[4..]) {
            return;
        }
    }
    let mut record = [0u8; 512];
    let crc = crc32(&record[4..]);
    record[..4].copy_from_slice(&crc.to_le_bytes());
    for at in [0, 0x1000] {
        if f.seek(SeekFrom::Start(at)).and_then(|_| f.write_all(&record)).is_err() {
            log!("devices: cannot write the SRAM's credit record");
            return;
        }
    }
    log!("devices: SRAM credit record written");
}

/// CRC-32 (IEEE), amBackup's record checksum.
fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |crc, &b| (0..8).fold(crc ^ b as u32, |c, _| if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 }))
}

fn dip_switches() -> u8 {
    static VALUE: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var("WAL_RINGEDGE_DIPSW").ok().and_then(|v| u8::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok()).unwrap_or(0xFF)
    })
}

/// Byte transfer (`0x9C402004`): `{status, protocol (5 read, 4 write), address, register, 0,
/// data}`. The PCA9535's input port 1 has the main board's TEST and SERVICE push switches (the
/// test menu reads them; the game reads the JVS board's), from any player's test / service.
/// Other reads are zero (input port 0: the DIP switches).
fn smbus_byte(req: &[u8], out: &mut [u8]) -> usize {
    let n = req.len().min(out.len());
    out[..n].copy_from_slice(&req[..n]);
    out[0] = 0;
    if req[1] == 5 && req[2] == PCA9535_ADDRESS && req[3] == 1 && n > 5 {
        let pressed = (0..wal_protocol::MAX_PLAYERS).fold(0, |a, p| a | wal_payload_common::input(p).buttons);
        let mut port = 0xFF;
        if pressed & wal_protocol::button::TEST != 0 {
            port &= !PUSHSW_TEST;
        }
        if pressed & wal_protocol::button::SERVICE != 0 {
            port &= !PUSHSW_SERVICE;
        }
        out[5] = port;
    } else if req[1] == 5 && req[2] == PCA9535_ADDRESS && req[3] == 0 && n > 5 {
        // the DIP switches, active low
        out[5] = !dip_switches();
    }
    smbus_trace("byte", req, &out[..n]);
    n
}

// ── mxsuperio: the hardware monitor's Super I/O chip ─────────────────────────────────────

/// The hardware monitor registers of the 2 chips, written by amHm (`0x9C40A010`).
static MONITOR: Mutex<[[u8; 256]; 2]> = Mutex::new(monitor_defaults());

const fn monitor_defaults() -> [[u8; 256]; 2] {
    let mut r = [[0u8; 256]; 2];
    r[0][0x48] = 0x2D;
    r[1][0x48] = 0x2D;
    r
}

/// `0x9C402004` `{chip, 0x0B, register, data}`: configuration registers, the chip id
/// (0x20:0x21 = 0xA020, a Winbond hardware monitor: amHm accepts 0xA020 and 0xA230);
/// `0x9C40200C` `{chip, register, data}`: the hardware monitor registers ([`MONITOR`]: 0x48
/// is the monitor's address, 0x10..0x6F), written with `0x9C40A010`. Transformers: Human
/// Alliance does not start without a hardware monitor (amHmInit).
fn superio(code: u32, req: &[u8], out: &mut [u8]) -> usize {
    let n = req.len();
    out[..n].copy_from_slice(req);
    let (reg, data) = if code == 0x9C40_2004 { (req[2], 3) } else { (req[1], 2) };
    let value = match (code, reg) {
        (0x9C40_2004, 0x20) => 0xA0,
        (0x9C40_2004, 0x21) => 0x20,
        (0x9C40_200C, _) => MONITOR.lock().unwrap()[req[0] as usize & 1][reg as usize],
        _ => 0,
    };
    if data < n {
        out[data] = value;
    }
    smbus_trace("superio", req, &out[..n]);
    n
}

/// `0x9C40A010` `{chip, register, data}`: a hardware monitor register write.
fn superio_write(req: &[u8]) {
    MONITOR.lock().unwrap()[req[0] as usize & 1][req[1] as usize] = req[2];
    smbus_trace("superio write", req, &[]);
}

// ── columba: the BIOS area with an SMBIOS table ──────────────────────────────────────────

const BIOS_BASE: u64 = 0xF0000;
/// amPlatform reads 64 KB from the table address and wants them all: the table at 0xF0000, the
/// entry point after it, inside the first 32 KB it searches.
const DMI_TABLE: usize = 0;
const DMI_ENTRY: usize = 0x7000;

/// One SMBIOS structure: type, formatted area after the header, strings.
fn smbios_struct(out: &mut Vec<u8>, kind: u8, handle: u16, formatted: &[u8], strings: &[&str]) {
    out.push(kind);
    out.push(4 + formatted.len() as u8);
    out.extend_from_slice(&handle.to_le_bytes());
    out.extend_from_slice(formatted);
    for s in strings {
        out.extend_from_slice(s.as_bytes());
        out.push(0);
    }
    if strings.is_empty() {
        out.push(0);
    }
    out.push(0);
}

/// The 64 KB at 0xF0000: the structure table and its `_DMI_` entry point.
fn bios_image() -> &'static [u8] {
    static IMAGE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    IMAGE.get_or_init(|| {
        let platform = std::env::var("WAL_RINGEDGE_PLATFORM").unwrap_or_else(|_| "AAL".into());
        let mut table = Vec::new();
        // BIOS: vendor, version, release date (string numbers 1-3)
        smbios_struct(&mut table, 0, 0, &[1, 2, 0, 0xE8, 3, 0x0F, 0, 0, 0, 0, 0, 0, 0, 0], &["American Megatrends Inc.", "080015", "07/30/2010"]);
        // system: manufacturer, product; amPlatform's board type is the manufacturer with the
        // platform id: NEC + AAL RingEdge, Supermicro + AAM RingWide (NEC + AAM: no board)
        let (maker, product) = if platform == "AAM" { ("Supermicro", "RINGWIDE") } else { ("NEC", "RINGEDGE") };
        smbios_struct(&mut table, 1, 1, &[1, 2, 0, 0], &[maker, product]);
        // OEM strings: the third is the platform id, the fifth `AAL2` on a RingEdge 2 (amBackup's
        // SRAM layout: 4-byte aligned records)
        if ringedge2() {
            smbios_struct(&mut table, 11, 2, &[5], &["SEGA", "RINGEDGE2", &platform, "-", "AAL2"]);
        } else {
            smbios_struct(&mut table, 11, 2, &[3], &["SEGA", "RINGEDGE", &platform]);
        }
        // inactive structures, a table of a real BIOS's size: the 2009 amPlatform (Medal no
        // Gunman) stops at its length with a count that also adds a byte of each structure's
        // strings, before the OEM strings of a 121-byte table (no platform id: no backup)
        for handle in 4..8 {
            smbios_struct(&mut table, 126, handle, &[0; 251], &[]);
        }
        smbios_struct(&mut table, 127, 3, &[], &[]);
        let mut image = vec![0u8; 0x10000];
        let mut entry = [0u8; 15];
        entry[..5].copy_from_slice(b"_DMI_");
        entry[6..8].copy_from_slice(&(table.len() as u16).to_le_bytes());
        entry[8..12].copy_from_slice(&((BIOS_BASE as u32) + DMI_TABLE as u32).to_le_bytes());
        entry[12..14].copy_from_slice(&4u16.to_le_bytes());
        entry[14] = 0x24;
        entry[5] = 0u8.wrapping_sub(entry.iter().fold(0u8, |a, b| a.wrapping_add(*b)));
        image[DMI_ENTRY..DMI_ENTRY + 15].copy_from_slice(&entry);
        image[DMI_TABLE..DMI_TABLE + table.len()].copy_from_slice(&table);
        image
    })
}

/// Physical memory: the BIOS image, zero elsewhere.
fn read_bios(address: u64, out: &mut [u8]) {
    let image = bios_image();
    for (i, b) in out.iter_mut().enumerate() {
        let a = address + i as u64;
        *b = a.checked_sub(BIOS_BASE).and_then(|o| image.get(o as usize)).copied().unwrap_or(0);
    }
}

// ── SetupDi: one device, the SMBus driver ─────────────────────────────────────────────────

/// Class of the last `SetupDiGetClassDevsA` (GUID Data1).
static CLASS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn get_class_devs(guid: *const u32, enumerator: P, parent: P, flags: u32) -> P {
    let data1 = if guid.is_null() { 0 } else { unsafe { *guid } };
    if data1 != SMBUS_GUID_DATA1 {
        log!("devices: SetupDiGetClassDevsA class {data1:#010x}: passed through");
        let f: unsafe extern "system" fn(*const u32, P, P, u32) -> P = orig(CLASS_DEVS);
        return unsafe { f(guid, enumerator, parent, flags) };
    }
    CLASS.store(data1 as usize, Ordering::Relaxed);
    H_DEVINFO as P
}

unsafe extern "system" fn enum_interfaces(set: P, info: P, guid: P, index: u32, data: *mut u32) -> i32 {
    if set as usize != H_DEVINFO {
        let f: unsafe extern "system" fn(P, P, P, u32, *mut u32) -> i32 = orig(ENUM_IF);
        return unsafe { f(set, info, guid, index, data) };
    }
    if index > 0 {
        unsafe { windows_sys::Win32::Foundation::SetLastError(259) }; // ERROR_NO_MORE_ITEMS
        return 0;
    }
    // SP_DEVICE_INTERFACE_DATA { cbSize, InterfaceClassGuid, Flags, Reserved }
    if !data.is_null() {
        unsafe { *data.add(5) = 1 }; // SPINT_ACTIVE
    }
    1
}

unsafe extern "system" fn interface_detail(set: P, data: P, detail: *mut u8, size: u32, required: *mut u32, info: P) -> i32 {
    if set as usize != H_DEVINFO {
        let f: unsafe extern "system" fn(P, P, *mut u8, u32, *mut u32, P) -> i32 = orig(IF_DETAIL);
        return unsafe { f(set, data, detail, size, required, info) };
    }
    let path = SMBUS_PATH.as_bytes();
    let needed = 4 + path.len() as u32 + 1;
    if !required.is_null() {
        unsafe { *required = needed };
    }
    if detail.is_null() || size < needed {
        unsafe { windows_sys::Win32::Foundation::SetLastError(122) }; // ERROR_INSUFFICIENT_BUFFER
        return 0;
    }
    // SP_DEVICE_INTERFACE_DETAIL_DATA_A { cbSize, DevicePath[] }
    unsafe {
        std::ptr::copy_nonoverlapping(path.as_ptr(), detail.add(4), path.len());
        *detail.add(4 + path.len()) = 0;
    }
    1
}

unsafe extern "system" fn destroy_info_list(set: P) -> i32 {
    if set as usize == H_DEVINFO {
        return 1;
    }
    unsafe { orig::<unsafe extern "system" fn(P) -> i32>(DESTROY)(set) }
}
