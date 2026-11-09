//! Sega Nu's AMEX board, emulated with IAT hooks on the executable (`CreateFileA/W`,
//! `DeviceIoControl`, `CloseHandle`, the SetupDi device enumeration): amLib's "amy" layer
//! (Sonic Dash Extreme) and the Nu's `amdaemon.exe` (Mario & Sonic at the Rio 2016 Olympic
//! Games) find its drivers by their device interface classes. Per segatools' `common/amex`
//! (public domain).
//!
//! One device per class (`SetupDiGetClassDevsW`), its path ours:
//! * DS (`279a9f67-...`): the main board's 32-byte record (CRC-32, region, serial number).
//! * EEPROM (`b7970f0c-...`, 8 KB) and SRAM (`741b5fca-...`, 512 KB): files of the save folder
//!   (`EEPROM.bin`, `SRAM.bin`), read and written at their offsets (overlapped);
//!   `DeviceIoControl` gives the version and the geometry.
//! * GPIO (`e9a26688-...`): the board's DIP switches, its push switches (test, service) and
//!   LEDs.
//! * JVS (`db6bbb45-...`): the JVS board (`jvs_board`) on port `amex_jvs`, one packet per
//!   `DeviceIoControl`, the sense line asked with another. The bus reset has no reply:
//!   completed with `STATUS_NO_DATA_DETECTED`, as the driver does.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::SetLastError;
use windows_sys::Win32::System::IO::OVERLAPPED;
use windows_sys::Win32::System::Threading::SetEvent;

use crate::{iat, log, serial};

type P = *mut c_void;

/// The JVS board's port (the device path's name).
pub const JVS_PORT: &str = "amex_jvs";

/// Fake handles: odd values, never real handles.
const H_DS: usize = 0x4E55_0101;
const H_GPIO: usize = 0x4E55_0105;
/// Fake device information set of the SetupDi enumeration.
const H_DEVINFO: usize = 0x4E55_1001;

/// The real handles of the EEPROM and SRAM files.
static EEPROM_HANDLE: AtomicUsize = AtomicUsize::new(0);
static SRAM_HANDLE: AtomicUsize = AtomicUsize::new(0);

const EEPROM_SIZE: u64 = 0x2000;
const SRAM_SIZE: u64 = 0x8_0000;

/// The devices: interface class (GUID Data1), device path.
const DEVICES: [(u32, &str); 5] = [
    (0x279A_9F67, "\\\\.\\amex_ds"),
    (0xB797_0F0C, "\\\\.\\amex_eeprom"),
    (0xE9A2_6688, "\\\\.\\amex_gpio"),
    (0xDB6B_BB45, "\\\\.\\amex_jvs"),
    (0x741B_5FCA, "\\\\.\\amex_sram"),
];

/// The device path of an AMEX interface class, `None` for other classes.
fn class_path(data1: u32) -> Option<&'static str> {
    DEVICES.iter().find(|(c, _)| *c == data1).map(|(_, p)| *p)
}

/// The board's values.
pub struct Board {
    /// Folder of `EEPROM.bin` and `SRAM.bin`.
    pub save: String,
    /// The DS record's region.
    pub region: u8,
    /// The main board's serial number (16 characters).
    pub serial: &'static str,
    /// The DIP switches on (bit 0: switch 1).
    pub dip_switches: u32,
}

static BOARD: OnceLock<Board> = OnceLock::new();

fn board() -> &'static Board {
    BOARD.get().expect("amex::init")
}

fn files() -> [(String, u64); 2] {
    let dir = &board().save;
    [(format!("{dir}\\EEPROM.bin"), EEPROM_SIZE), (format!("{dir}\\SRAM.bin"), SRAM_SIZE)]
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
const WRITE: usize = 8;
static ORIG: [AtomicUsize; 9] = [const { AtomicUsize::new(0) }; 9];

/// A game's own change to its EEPROM writes: (offset, the bytes written), changed in place.
static EEPROM_WRITE_FILTER: OnceLock<fn(u64, &mut [u8])> = OnceLock::new();

/// Changes the game's EEPROM writes (CHUNITHM: its settings record kept as the distribution
/// server's), set before [`init`].
pub fn set_eeprom_write_filter(filter: fn(u64, &mut [u8])) {
    let _ = EEPROM_WRITE_FILTER.set(filter);
}

fn orig<F: Copy>(idx: usize) -> F {
    let p = ORIG[idx].load(Ordering::Relaxed);
    unsafe { std::mem::transmute_copy(&p) }
}

/// The save files at their sizes, the JVS board, the hooks.
pub fn init(b: Board) {
    let b = BOARD.get_or_init(|| b);
    let _ = std::fs::create_dir_all(&b.save);
    for (path, size) in files() {
        match std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&path) {
            Ok(f) if f.metadata().map(|m| m.len()).unwrap_or(0) != size => {
                let _ = f.set_len(size);
            }
            Ok(_) => {}
            Err(e) => log!("amex: cannot open {path}: {e}"),
        }
    }
    crate::jvs_board::init(JVS_PORT);
    let hooks: [(&str, &str, usize); 9] = [
        ("kernel32.dll", "CreateFileA", create_file_a as *const () as usize),
        ("kernel32.dll", "CreateFileW", create_file_w as *const () as usize),
        ("kernel32.dll", "DeviceIoControl", device_io_control as *const () as usize),
        ("kernel32.dll", "CloseHandle", close_handle as *const () as usize),
        ("setupapi.dll", "SetupDiGetClassDevsW", get_class_devs as *const () as usize),
        ("setupapi.dll", "SetupDiEnumDeviceInterfaces", enum_interfaces as *const () as usize),
        ("setupapi.dll", "SetupDiGetDeviceInterfaceDetailW", interface_detail as *const () as usize),
        ("setupapi.dll", "SetupDiDestroyDeviceInfoList", destroy_info_list as *const () as usize),
        ("kernel32.dll", "WriteFile", write_file as *const () as usize),
    ];
    for (i, (dll, name, f)) in hooks.into_iter().enumerate() {
        match unsafe { iat::hook(dll, name, f) } {
            Some(o) => ORIG[i].store(o, Ordering::Relaxed),
            None => log!("amex: {dll}!{name} not imported"),
        }
    }
    log!("amex: AMEX board; EEPROM and SRAM in {} (DIP switches {:02X})", b.save, b.dip_switches);
}

// ── files ─────────────────────────────────────────────────────────────────────────────────

/// The handle for a device path, `None` for other files. The EEPROM and SRAM are their files,
/// opened with the caller's access, sharing and flags (overlapped).
fn open_device(path: &str, access: u32, share: u32, flags: u32) -> Option<P> {
    let lower = path.to_ascii_lowercase();
    let h = match lower.as_str() {
        "\\\\.\\amex_ds" => H_DS as P,
        "\\\\.\\amex_gpio" => H_GPIO as P,
        "\\\\.\\amex_eeprom" | "\\\\.\\amex_sram" => {
            let (i, slot) = if lower.ends_with("eeprom") { (0, &EEPROM_HANDLE) } else { (1, &SRAM_HANDLE) };
            let file: Vec<u16> = files()[i].0.encode_utf16().chain([0]).collect();
            let create: unsafe extern "system" fn(*const u16, u32, u32, P, u32, u32, P) -> P = orig(CREATE_W);
            // OPEN_EXISTING (made by init)
            let h = unsafe { create(file.as_ptr(), access, share, std::ptr::null_mut(), 3, flags, std::ptr::null_mut()) };
            if h as isize != -1 {
                slot.store(h as usize, Ordering::Relaxed);
            }
            h
        }
        _ => return None,
    };
    log!("amex: {path} opened ({:#x})", h as usize);
    Some(h)
}

fn is_fake(h: P) -> bool {
    matches!(h as usize, H_DS | H_GPIO)
}

unsafe extern "system" fn create_file_a(name: *const u8, a: u32, s: u32, sa: P, d: u32, f: u32, t: P) -> P {
    if !name.is_null() {
        let path = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy().into_owned();
        if let Some(h) = open_device(&path, a, s, f) {
            return h;
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
        if let Some(h) = open_device(&path, a, s, f) {
            return h;
        }
    }
    unsafe { orig::<unsafe extern "system" fn(*const u16, u32, u32, P, u32, u32, P) -> P>(CREATE_W)(name, a, s, sa, d, f, t) }
}

/// The game's EEPROM writes through [`EEPROM_WRITE_FILTER`] (its buffer changed in place: an
/// overlapped write may complete after the call).
unsafe extern "system" fn write_file(h: P, buf: *const u8, n: u32, written: *mut u32, ov: *mut OVERLAPPED) -> i32 {
    if let Some(filter) = EEPROM_WRITE_FILTER.get()
        && h as usize == EEPROM_HANDLE.load(Ordering::Relaxed)
        && !buf.is_null()
        && !ov.is_null()
    {
        let offset = unsafe { (*ov).Anonymous.Anonymous.Offset as u64 | ((*ov).Anonymous.Anonymous.OffsetHigh as u64) << 32 };
        filter(offset, unsafe { std::slice::from_raw_parts_mut(buf as *mut u8, n as usize) });
    }
    unsafe { orig::<unsafe extern "system" fn(P, *const u8, u32, *mut u32, *mut OVERLAPPED) -> i32>(WRITE)(h, buf, n, written, ov) }
}

unsafe extern "system" fn close_handle(h: P) -> i32 {
    if is_fake(h) {
        return 1;
    }
    for slot in [&EEPROM_HANDLE, &SRAM_HANDLE] {
        let _ = slot.compare_exchange(h as usize, 0, Ordering::Relaxed, Ordering::Relaxed);
    }
    unsafe { orig::<unsafe extern "system" fn(P) -> i32>(CLOSE)(h) }
}

// ── DeviceIoControl ───────────────────────────────────────────────────────────────────────

const IOCTL_DISK_GET_DRIVE_GEOMETRY: u32 = 0x0007_0000;
/// `CTL_CODE(0x8000, function, METHOD_BUFFERED, access)` of the AMEX drivers.
const fn amex(function: u32, access: u32) -> u32 {
    (0x8000 << 16) | (access << 14) | (function << 2)
}
const ABI_VERSION: u32 = amex(0x800, 1);
const DS_SETUP: u32 = amex(0x801, 1);
const DS_READ_SECTOR: u32 = amex(0x804, 1);
const GPIO_SET_LEDS: u32 = amex(0x801, 2);
const GPIO_GET_PSW: u32 = amex(0x802, 1);
const GPIO_GET_DIPSW: u32 = amex(0x803, 1);
const GPIO_DESCRIBE: u32 = amex(0x805, 1);
const JVS_HELLO: u32 = amex(0x801, 1);
const JVS_TRANSACT: u32 = amex(0x802, 3);
const JVS_SENSE: u32 = amex(0x803, 1);

/// What a device call does.
enum Reply {
    /// Success with that many bytes.
    Bytes(usize),
    /// The JVS bus reset: no reply.
    NoData,
}

/// Number of unknown calls logged.
static UNKNOWN_LOGGED: AtomicUsize = AtomicUsize::new(0);

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

unsafe extern "system" fn device_io_control(h: P, code: u32, input: *const u8, in_size: u32, output: *mut u8, out_size: u32, returned: *mut u32, ov: P) -> i32 {
    let hv = h as usize;
    let file = hv != 0 && (hv == EEPROM_HANDLE.load(Ordering::Relaxed) || hv == SRAM_HANDLE.load(Ordering::Relaxed));
    let jvs = serial::port_of(h).is_some_and(|p| p.eq_ignore_ascii_case(JVS_PORT));
    if !file && !jvs && !is_fake(h) {
        let f: unsafe extern "system" fn(P, u32, *const u8, u32, *mut u8, u32, *mut u32, P) -> i32 = orig(IOCTL);
        return unsafe { f(h, code, input, in_size, output, out_size, returned, ov) };
    }
    let req = if input.is_null() { &[][..] } else { unsafe { std::slice::from_raw_parts(input, in_size as usize) } };
    let out = if output.is_null() { &mut [][..] } else { unsafe { std::slice::from_raw_parts_mut(output, out_size as usize) } };
    let reply = if file {
        file_ioctl(hv == EEPROM_HANDLE.load(Ordering::Relaxed), code, out)
    } else if jvs {
        jvs_ioctl(h, code, req, out)
    } else if hv == H_DS {
        ds_ioctl(code, out)
    } else {
        gpio_ioctl(code, out)
    };
    match reply {
        Some(Reply::Bytes(n)) => unsafe { complete(returned, ov, n) },
        // the driver completes the reset asynchronously with STATUS_NO_DATA_DETECTED
        Some(Reply::NoData) => unsafe {
            let o = ov as *mut OVERLAPPED;
            if !o.is_null() && !(*o).hEvent.is_null() {
                (*o).Internal = 0x8000_0022;
                (*o).InternalHigh = 0;
                SetEvent((*o).hEvent);
                SetLastError(997); // ERROR_IO_PENDING
            } else {
                SetLastError(1104); // ERROR_NO_DATA_DETECTED
            }
            0
        },
        None => {
            if UNKNOWN_LOGGED.fetch_add(1, Ordering::Relaxed) < 200 {
                log!("amex: {hv:#x} ioctl {code:#x} in [{}] out {out_size} not emulated", hex(req));
            }
            unsafe { SetLastError(1) }; // ERROR_INVALID_FUNCTION
            0
        }
    }
}

/// Success with `n` bytes; overlapped calls complete at once.
unsafe fn complete(returned: *mut u32, ov: P, n: usize) -> i32 {
    if !returned.is_null() {
        unsafe { *returned = n as u32 };
    }
    let o = ov as *mut OVERLAPPED;
    if !o.is_null() {
        unsafe {
            (*o).Internal = 0;
            (*o).InternalHigh = n;
            if !(*o).hEvent.is_null() {
                SetEvent((*o).hEvent);
            }
        }
    }
    1
}

fn put(out: &mut [u8], data: &[u8]) -> Option<Reply> {
    let n = data.len().min(out.len());
    out[..n].copy_from_slice(&data[..n]);
    Some(Reply::Bytes(n))
}

/// `DISK_GEOMETRY`: cylinders, media type, tracks per cylinder, sectors per track, bytes per
/// sector.
fn geometry(out: &mut [u8], cylinders: u64, media: u32, tracks: u32, sectors: u32, bytes: u32) -> Option<Reply> {
    let mut g = [0u8; 24];
    g[0..8].copy_from_slice(&cylinders.to_le_bytes());
    g[8..12].copy_from_slice(&media.to_le_bytes());
    g[12..16].copy_from_slice(&tracks.to_le_bytes());
    g[16..20].copy_from_slice(&sectors.to_le_bytes());
    g[20..24].copy_from_slice(&bytes.to_le_bytes());
    put(out, &g)
}

fn file_ioctl(eeprom: bool, code: u32, out: &mut [u8]) -> Option<Reply> {
    match code {
        ABI_VERSION => put(out, &256u16.to_le_bytes()),
        // 1 x 224 x 32 x 1 byte (FixedMedia) / 0x20000 x 1 x 1 x 4 bytes
        IOCTL_DISK_GET_DRIVE_GEOMETRY if eeprom => geometry(out, 1, 12, 224, 32, 1),
        IOCTL_DISK_GET_DRIVE_GEOMETRY => geometry(out, 0x20000, 0, 1, 1, 4),
        _ => None,
    }
}

fn jvs_ioctl(h: P, code: u32, req: &[u8], out: &mut [u8]) -> Option<Reply> {
    match code {
        JVS_HELLO => put(out, &[0, 0]),
        // the sense line: 3 not addressed, 2 addressed
        JVS_SENSE => put(out, &[if serial::READY.load(Ordering::Relaxed) { 2 } else { 3 }]),
        JVS_TRANSACT => {
            let reply = serial::transact(h, req)?;
            if reply.is_empty() { Some(Reply::NoData) } else { put(out, &reply) }
        }
        _ => None,
    }
}

fn ds_ioctl(code: u32, out: &mut [u8]) -> Option<Reply> {
    match code {
        IOCTL_DISK_GET_DRIVE_GEOMETRY => geometry(out, 1, 0, 1, 2, 32),
        ABI_VERSION => put(out, &256u16.to_le_bytes()),
        DS_SETUP => Some(Reply::Bytes(0)),
        // the record: CRC-32 of bytes 4-31, the region at 8, the main board's serial at 9
        DS_READ_SECTOR => {
            let mut r = [0u8; 32];
            r[8] = board().region;
            let serial = board().serial.as_bytes();
            let n = serial.len().min(16);
            r[9..9 + n].copy_from_slice(&serial[..n]);
            let crc = crc32(&r[4..]);
            r[..4].copy_from_slice(&crc.to_le_bytes());
            put(out, &r)
        }
        _ => None,
    }
}

fn gpio_ioctl(code: u32, out: &mut [u8]) -> Option<Reply> {
    match code {
        GPIO_DESCRIBE => {
            // {u8, 32 x {u8, u8 count, u16 type}}: 2 LEDs (1), 8 DIP switches (2), 2 push
            // switches (3)
            let mut ports = [0u8; 129];
            for (i, (count, kind)) in [(2u8, 1u16), (8, 2), (2, 3)].into_iter().enumerate() {
                ports[1 + 4 * i + 1] = count;
                ports[1 + 4 * i + 2..1 + 4 * i + 4].copy_from_slice(&kind.to_le_bytes());
            }
            put(out, &ports)
        }
        GPIO_GET_DIPSW => put(out, &board().dip_switches.to_le_bytes()),
        GPIO_GET_PSW => {
            // bit 0 test, bit 1 service, from any player's
            let pressed = (0..wal_protocol::MAX_PLAYERS).fold(0, |a, p| a | crate::input(p).buttons);
            let psw = (pressed & wal_protocol::button::TEST != 0) as u32 | ((pressed & wal_protocol::button::SERVICE != 0) as u32) << 1;
            put(out, &psw.to_le_bytes())
        }
        GPIO_SET_LEDS => Some(Reply::Bytes(0)),
        _ => None,
    }
}

fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |crc, &b| (0..8).fold(crc ^ b as u32, |c, _| if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 }))
}

// ── SetupDi: one device per AMEX interface class ──────────────────────────────────────────

/// Class of the last `SetupDiGetClassDevsW` (GUID Data1).
static CLASS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn get_class_devs(guid: *const u32, enumerator: P, parent: P, flags: u32) -> P {
    let data1 = if guid.is_null() { 0 } else { unsafe { *guid } };
    if class_path(data1).is_none() {
        log!("amex: SetupDiGetClassDevsW class {data1:#010x}: passed through");
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
        unsafe { SetLastError(259) }; // ERROR_NO_MORE_ITEMS
        return 0;
    }
    // SP_DEVICE_INTERFACE_DATA { cbSize, InterfaceClassGuid, Flags, Reserved }
    if !data.is_null() {
        unsafe { *data.add(5) = 1 }; // SPINT_ACTIVE
    }
    1
}

unsafe extern "system" fn interface_detail(set: P, data: P, detail: *mut u8, size: u32, required: *mut u32, info: P) -> i32 {
    let path = class_path(CLASS.load(Ordering::Relaxed) as u32);
    let (true, Some(path)) = (set as usize == H_DEVINFO, path) else {
        let f: unsafe extern "system" fn(P, P, *mut u8, u32, *mut u32, P) -> i32 = orig(IF_DETAIL);
        return unsafe { f(set, data, detail, size, required, info) };
    };
    let path: Vec<u16> = path.encode_utf16().chain([0]).collect();
    let needed = 4 + 2 * path.len() as u32;
    if !required.is_null() {
        unsafe { *required = needed };
    }
    if detail.is_null() || size < needed {
        unsafe { SetLastError(122) }; // ERROR_INSUFFICIENT_BUFFER
        return 0;
    }
    // SP_DEVICE_INTERFACE_DETAIL_DATA_W { cbSize, DevicePath[] }
    unsafe { std::ptr::copy_nonoverlapping(path.as_ptr().cast::<u8>(), detail.add(4), 2 * path.len()) };
    1
}

unsafe extern "system" fn destroy_info_list(set: P) -> i32 {
    if set as usize == H_DEVINFO {
        return 1;
    }
    unsafe { orig::<unsafe extern "system" fn(P) -> i32>(DESTROY)(set) }
}
