//! Thrill Drive 4: Crazy Streets (2007, game code F04): the payload is the game's `libacio.dll`
//! (the PJEI board: switches, coins, the wheel's and pedals' potentiometers, security plugs; the
//! card reader). The wheel's motor is a D44H board on a second ACIO bus, which td4.exe reaches
//! through `libtd4io.dll` (`LoadLibraryA` + `GetProcAddress` of 27 functions, every one needed,
//! rva 0x48cd0): that load gets this DLL too, one payload instance for both buses.
//!
//! td4.ini's `[IO] HANDLE` picks the wheel (`KONAMI` the D44H board, `HAPP` a Happ wheel through
//! the PJEI board, `KEYBOAD` a development mode reading the PC keyboard); without the file the
//! test menu's setting decides. The profile hides the dump's td4.ini (`KEYBOAD`).
//!
//! The PJEI status (`pjei.rs`): byte 4 START 0x04, the shift lever's UP 0x20 / DOWN 0x40, VIEW
//! 0x80 (the game's switch flags 0x800, 0x100, 0x80, 0x10: rva 0x4ac1b), byte 16 test 0x20,
//! service 0x10, coin mech 0x04; +8 / +10 / +12 the wheel, accelerator and brake (u16). The game
//! maps them with its calibration (min 0x7e3974, max 0x7e39b4, wheel center 0x7e39a8; read at
//! rva 0x4a9b9): the sticks are scaled onto the calibration it has.
//!
//! The D44H motor (`ac_io_d44h_set_senddata`, 4 bytes, 0x7e3920, GTI Club's HGTH layout): byte 0
//! the torque -127..127 (rva 0x475e0; in a race +-80 against the wheel's turn: wheel right, torque
//! negative, pushing left), byte 1 bits 0-5 a vibration level 0..63 (0x47620: crashes, decaying),
//! byte 2 0x0f and byte 3 0..254 (from the speed): the constant force and the vibration
//! (`WAL_KONAMI_FFB` percent, default 100).
//!
//! Boot patches: the wheel kind is the D44H board's (the export build's default is a Happ wheel
//! without motor), the DATA CHECK is skipped (it sums the payload's DLLs too), the operator clock
//! and the test menu's calibration are reported set, and the motorized CALIBRATION sees the wheel
//! follow the motor to its end stops.
//!
//! The D44S board (the seat: SEATBELT on the boot screen, `shockstrong`, `shockmild`) is
//! reported absent: its requests fail, the boot shows it OK ( NO DEVICE ).
//!
//! Native inputs for `native_map`: `test service coin start view shift_up shift_down`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log, send_output};
use wal_protocol::{Axis, output};
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

static ACTIVE: AtomicBool = AtomicBool::new(false);
static MODULE: AtomicUsize = AtomicUsize::new(0);
static ORIG_LOAD_LIBRARY: AtomicUsize = AtomicUsize::new(0);

/// td4.exe (or a renamed copy of it, td4_go.exe).
pub fn detected() -> bool {
    crate::file_name(std::ptr::null_mut()).to_ascii_lowercase().starts_with("td4")
}

pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

type LoadLibraryA = unsafe extern "system" fn(*const u8) -> *mut c_void;

/// `LoadLibraryA("libtd4io.dll")`: this DLL (loaded again by its own path).
unsafe extern "system" fn load_library(name: *const u8) -> *mut c_void {
    let original: LoadLibraryA = unsafe { std::mem::transmute(ORIG_LOAD_LIBRARY.load(Ordering::Relaxed)) };
    if !name.is_null() && unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes().eq_ignore_ascii_case(b"libtd4io.dll") {
        let mut buf = [0u16; 260];
        let n = unsafe { GetModuleFileNameW(MODULE.load(Ordering::Relaxed) as _, buf.as_mut_ptr(), buf.len() as u32) } as usize;
        log!("td4: libtd4io.dll (the D44H bus) is the payload");
        let mut ansi: Vec<u8> = String::from_utf16_lossy(&buf[..n]).into_bytes();
        ansi.push(0);
        return unsafe { original(ansi.as_ptr()) };
    }
    unsafe { original(name) }
}

/// `GetLogicalDriveStringsA`: C: alone (the boot reads every drive's volume and free space).
unsafe extern "system" fn get_logical_drive_strings(size: u32, out: *mut u8) -> u32 {
    const DRIVES: &[u8] = b"C:\\\0\0";
    if out.is_null() || (size as usize) < DRIVES.len() {
        return DRIVES.len() as u32;
    }
    unsafe { std::ptr::copy_nonoverlapping(DRIVES.as_ptr(), out, DRIVES.len()) };
    DRIVES.len() as u32 - 1
}

/// Writes `new` at `at` when the code there is `expected`.
unsafe fn patch(what: &str, at: usize, expected: &[u8], new: &[u8]) {
    let current = unsafe { std::slice::from_raw_parts(at as *const u8, expected.len()) };
    if current != expected {
        log!("td4: {what}: code differs ({current:02x?}), not patched");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, new.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(new.as_ptr(), at as *mut u8, new.len());
        VirtualProtect(at as *const c_void, new.len(), old, &mut old);
    }
    log!("td4: {what}");
}

pub fn init(module: *mut c_void) {
    ACTIVE.store(true, Ordering::Relaxed);
    MODULE.store(module as usize, Ordering::Relaxed);
    match unsafe { iat::hook("kernel32.dll", "LoadLibraryA", load_library as *const () as usize) } {
        Some(o) => ORIG_LOAD_LIBRARY.store(o, Ordering::Relaxed),
        None => log!("td4: LoadLibraryA import not found, the D44H bus is the dump's libtd4io.dll"),
    }
    unsafe { iat::hook("kernel32.dll", "GetLogicalDriveStringsA", get_logical_drive_strings as *const () as usize) };
    unsafe {
        // The wheel kind (rva 0x48460): td4.ini's HANDLE, else the test menu's setting, else
        // the cabinet's default (HAPP for this export build): the D44H motor board always.
        patch(
            "Konami motorized wheel (D44H board)",
            0x448460,
            &[0xa0, 0x48, 0x29, 0x7f, 0x00, 0x84, 0xc0, 0x74, 0x0f],
            &[0xc6, 0x05, 0x12, 0x3a, 0x7e, 0x00, 0x00, 0xc3, 0x90],
        );
        // The boot's DATA CHECK sums every file of data/td4check.bin (libacio.dll and
        // libtd4io.dll among them: the payload) at a pace that takes tens of minutes for the 1 GB
        // td4pack.dat: skipped, as HOLD LEVER UP + START does (rva 0xd8a47).
        patch(
            "boot data check skipped",
            0x4d8a47,
            &[0xe8, 0x74, 0xe3, 0xf6, 0xff, 0x84, 0xc0],
            &[0xeb, 0x35, 0x90, 0x90, 0x90, 0x90, 0x90],
        );
        // The operator's clock setting (0x98f64c, getter rva 0x108640): unset on a fresh
        // CONF/NVRAM, PLEASE SET THE CLOCK BEFORE OPERATION stops the boot. The game reads the
        // PC clock: reported set.
        // The test menu's steering, accelerator and brake calibrated flags (0x98f59c-e,
        // getter rva 0x108610): unset on a fresh CONF/NVRAM, PLEASE CALIBRATION BEFORE
        // OPERATION stops the boot. The sticks are scaled onto the calibration the game has:
        // reported calibrated.
        patch(
            "calibration reported done",
            0x508610,
            &[0xa0, 0x9c, 0xf5, 0x98, 0x00, 0x84, 0xc0],
            &[0xb0, 0x01, 0xc3, 0x90, 0x90, 0x90, 0x90],
        );
        patch("clock reported set", 0x508640, &[0xa0, 0x4c, 0xf6, 0x98, 0x00, 0xc3], &[0xb0, 0x01, 0xc3, 0x90, 0x90, 0xc3]);
    }
    crate::avs::init();
}

/// The boot's CALIBRATION (rva 0x49b30, step 0x7e3b0c 1-5 until 0x7e3b10 is set): the motor
/// turns the wheel to each end stop and reads the potentiometer there (at least 50000 apart),
/// which gives the wheel's calibration. The wheel follows the motor meanwhile: a negative
/// torque turns it left, to the high end.
fn calibrating() -> bool {
    let step = unsafe { (0x7e3b0c as *const u32).read_volatile() };
    let done = unsafe { (0x7e3b10 as *const u8).read_volatile() };
    (1..=5).contains(&step) && done == 0
}

fn motor_wheel() -> u16 {
    match LAST_SEND.load(Ordering::Relaxed) as u8 as i8 {
        t if t < -8 => 0xffc0,
        t if t > 8 => 0x0040,
        _ => 0x8000,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    Sw(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("start", Native::Sw(0x04)),
    ("shift_up", Native::Sw(0x20)),
    ("shift_down", Native::Sw(0x40)),
    ("view", Native::Sw(0x80)),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("b1", "shift_up"),
    ("b2", "shift_down"),
    ("b3", "view"),
];

static MAP: Mutex<Option<ButtonMap<Native>>> = Mutex::new(None);

/// The game's calibration arrays (min, max; wheel, accelerator, brake) and the wheel's center.
const CAL_MIN: usize = 0x7e3974;
const CAL_MAX: usize = 0x7e39b4;
const CAL_CENTER: usize = 0x7e39a8;

fn calibration(array: usize, i: usize) -> f32 {
    unsafe { ((array + i * 4) as *const u32).read_volatile() as f32 }
}

/// The raw value the game reads as `t` (wheel -1 left..1 right, pedals 0..1). The wheel's
/// potentiometer reads high on the left (default calibration min 0xff40 = left, center 0x7a40,
/// max 0 = right): left goes to the higher end of whatever calibration the game has, right to
/// the lower. The test menu's CALIBRATION shows the wheel with a neutral one (min 0, center
/// 0x8000, max 0xffff): following its min / max there showed the wheel inverted.
fn raw(i: usize, t: f32) -> u16 {
    let (min, max) = (calibration(CAL_MIN, i), calibration(CAL_MAX, i));
    let v = if i == 0 {
        let (low, high) = (min.min(max), min.max(max));
        let center = calibration(CAL_CENTER, 0);
        let center = if center > low && center < high { center } else { (low + high) / 2.0 };
        if t < 0.0 { center - t * (high - center) } else { center - t * (center - low) }
    } else {
        min + t * (max - min)
    };
    v.clamp(0.0, 65535.0) as u16
}

/// Player 1's stick as the PJEI status (40 bytes), and whether the coin switch is down.
pub(crate) fn status() -> ([u8; 40], bool) {
    let stick = wal_payload_common::input(0);
    let mut status = [0u8; 40];
    let mut coin = false;
    let mut guard = MAP.lock().unwrap();
    let map = guard.get_or_insert_with(|| ButtonMap::new(NATIVES, DEFAULT_MAP));
    map.for_each_pressed(&stick, |n| match n {
        Native::Test => status[16] |= 0x20,
        Native::Service => status[16] |= 0x10,
        Native::Coin => {
            status[16] |= 0x04;
            coin = true;
        }
        Native::Sw(bit) => status[4] |= bit,
    });
    let wheel = (stick.axis(Axis::LeftX) as f32 / 32767.0).clamp(-1.0, 1.0);
    let accel = stick.axis(Axis::Accel).max(0) as f32 / 32767.0;
    let brake = stick.axis(Axis::Brake).max(0) as f32 / 32767.0;
    for (i, t) in [wheel, accel, brake].into_iter().enumerate() {
        status[8 + i * 2..10 + i * 2].copy_from_slice(&raw(i, t).to_le_bytes());
    }
    if calibrating() {
        status[8..10].copy_from_slice(&motor_wheel().to_le_bytes());
    }
    (status, coin)
}

// The D44H board: the wheel's motor.

static PERCENT: OnceLock<i32> = OnceLock::new();
static LAST_SEND: AtomicU32 = AtomicU32::new(0);
static LAST_FORCE: AtomicI32 = AtomicI32::new(0);
static LAST_VIBRATION: AtomicI32 = AtomicI32::new(0);
static LOGGED: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44h_set_senddata(data: *const u8) -> bool {
    if data.is_null() {
        return true;
    }
    let bytes = unsafe { std::slice::from_raw_parts(data, 4) };
    let packed = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let calibrating = calibrating();
    if LAST_SEND.swap(packed, Ordering::Relaxed) != packed && !calibrating && LOGGED.fetch_add(1, Ordering::Relaxed) < 60 {
        let wheel = wal_payload_common::input(0).axis(Axis::LeftX);
        log!("d44h: motor {bytes:02x?} (wheel {wheel})");
    }
    let percent = *PERCENT.get_or_init(|| {
        std::env::var("WAL_KONAMI_FFB").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(100)
    });
    if calibrating {
        return true;
    }
    let force = (bytes[0] as i8 as i32 * output::FFB_MAX / 127 * percent / 100).clamp(-output::FFB_MAX, output::FFB_MAX);
    if LAST_FORCE.swap(force, Ordering::Relaxed) != force {
        send_output(0, output::FFB_CONSTANT, force);
    }
    let vibration = ((bytes[1] & 0x3f) as i32 * output::FFB_MAX / 63 * percent / 100).min(output::FFB_MAX);
    if LAST_VIBRATION.swap(vibration, Ordering::Relaxed) != vibration {
        send_output(0, output::FFB_VIBRATION, vibration);
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44h_update_recvdata() -> bool {
    true
}

/// The board's receive buffer: nothing the game uses (the wheel's position is the PJEI's).
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44h_get_recvdata(out: *mut u8) -> bool {
    if !out.is_null() {
        unsafe { std::ptr::write_bytes(out, 0, 16) };
    }
    true
}

/// A direct request (one argument, its buffer: called every frame with the D44H bus up, rva
/// 0x4aeec): nothing to do.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44h_directreq_sendandrecv(_buffer: *mut u8) -> bool {
    true
}

/// The wheel's travel limit (the motor's end stops), applied at once.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44h_directreq_set_handle_limit(_limit: u8, status: *mut u32) -> bool {
    if !status.is_null() {
        unsafe { *status = 1 };
    }
    true
}

/// 2: SET HANDLE LIMIT SUCCESS (3 failed).
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44h_directreq_set_handle_limit_isfinished(status: *mut u32) -> bool {
    if !status.is_null() {
        unsafe { *status = 2 };
    }
    true
}

// The D44S board (the seat): absent, every request fails.

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44s_directreq_initialize(_status: *mut u32) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44s_directreq_selfdiagnosis(_status: *mut u32) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44s_directreq_gamestart(_status: *mut u32) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44s_directreq_gameover(_status: *mut u32) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44s_directreq_shockstrong(_status: *mut u32) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44s_directreq_shockmild(_status: *mut u32) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44s_directreq_isfinished(_request: u32, _status: *mut u32) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44s_update_recvstatusdata() -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44s_get_statusdata(out: *mut u8) -> bool {
    if !out.is_null() {
        unsafe { std::ptr::write_bytes(out, 0, 8) };
    }
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44s_directreq_getstatusdata(_status: *mut u32) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44s_update_recvdiagnosisdata() -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44s_get_diagnosisdata(out: *mut u8) -> bool {
    if !out.is_null() {
        unsafe { std::ptr::write_bytes(out, 0, 8) };
    }
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_d44s_directreq_getdiagnosisdata(_status: *mut u32) -> bool {
    false
}
