//! Gashaaaan (F24, 2006): the payload is its `iob.dll`, the I/O board library.
//!
//! `bin/ballshooting.exe` runs `system.dll`, which boots AVS and two serial boards through two
//! libraries of the game: `iob.dll` (the I/O board: switches, coin counters, lamps, black light,
//! coin blockers, the ball supply motors, the security plugs) and `ballio.dll` (the camera
//! board, "bs2": where the thrown balls hit the screen). Both are MSVC C++ (cdecl, mangled
//! names, `libacio.def`).
//!
//! - iob: answered here. `iob_get_status` 3 (ready), the plugs checked (1), the firmware up to
//!   date. Buttons (`iob_f24_button(n)`, `_trg` the press of this step): 0-3 the players' START,
//!   4 TEST, 5 SERVICE, 6-7 the coin mechs; `iob_f24_nr_coin(slot)` a 6-bit coin counter per
//!   slot (the game counts its steps). The ball supply motors report done, no error.
//! - ballio: its functions are pointed at the payload (a jump at each one's entry; the stage
//!   DLLs, loaded later, import them too). `ball_io_point_get` gives the hits of the current
//!   step, (int16 x, y) in the game's scene pixels (1248x468, `f24_screen.rs`): a
//!   player's pointer (gun or mouse) and throw button, one hit per press, another every 150 ms
//!   while held. The camera board reports booted (`bs2_get_status` 2, calibrated, no error).
//!
//! - backup data: AVS keeps the times of its `F:\` files (`/dev/nvram`) in `F:\info.bin` and
//!   takes a file of other times for a corrupted one (OPTION SETTINGS / BOOKKEEPING BACKUP DATA
//!   ERROR). A SquashFS image keeps whole seconds only: the times AVS reads are rounded down to
//!   the second (`libavs-win32.dll`'s `GetFileInformationByHandle` and `FindFirstFileA`), so the
//!   backup data written and the dump's survive the image. A dump whose backup data was saved
//!   before (its times lost in the image) is repaired at once: on the error screen (system.dll's
//!   boot check, state 6) its TEST press is answered, as the operator's would be.
//!
//! Native inputs for `native_map`: `test service coin start throw`.

use std::ffi::{c_char, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::{iat, log};
use wal_payload_common::mapping::ButtonMap;
use wal_protocol::{Axis, MAX_PLAYERS};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    Start,
    Throw,
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("start", Native::Start),
    ("throw", Native::Throw),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("b1", "throw"),
];

/// `iob_f24_button` numbers.
const BUTTON_START: usize = 0;
const BUTTON_TEST: usize = 4;
const BUTTON_SERVICE: usize = 5;
const BUTTON_COIN: usize = 6;
const BUTTONS: usize = 10;

/// A held throw button throws again after this.
const REPEAT: Duration = Duration::from_millis(150);
/// Hits of one step at most (ballio's buffer).
const MAX_HITS: usize = 16;

struct Io {
    maps: Vec<ButtonMap<Native>>,
    buttons: [bool; BUTTONS],
    previous: [bool; BUTTONS],
    /// The coin counters of slots 1 and 2 (6 bits).
    coins: [u8; 2],
    /// When each player's held throw button threw last.
    thrown: [Option<Instant>; MAX_PLAYERS],
    /// The hits of the current step, in screen pixels.
    hits: Vec<(i16, i16)>,
    /// ballio's state: hits read while enabled and in a mode other than 0.
    enabled: bool,
    mode: i32,
    mujinkun: u8,
}

static IO: Mutex<Option<Io>> = Mutex::new(None);

fn with_io<R>(f: impl FnOnce(&mut Io) -> R) -> R {
    let mut guard = IO.lock().unwrap();
    let io = guard.get_or_insert_with(|| Io {
        maps: (0..MAX_PLAYERS)
            .map(|p| if p == 0 { ButtonMap::new(NATIVES, DEFAULT_MAP) } else { ButtonMap::with_defaults(NATIVES, DEFAULT_MAP) })
            .collect(),
        buttons: [false; BUTTONS],
        previous: [false; BUTTONS],
        coins: [0; 2],
        thrown: [None; MAX_PLAYERS],
        hits: Vec::new(),
        enabled: true,
        mode: 0,
        mujinkun: 0,
    });
    f(io)
}

// iob.dll (cdecl; `libacio.def` gives the MSVC names).

#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_boot(mode: i32, model: *const c_char) {
    let model = if model.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(model) }.to_string_lossy().into_owned() };
    log!("f24: iob_boot({mode}, model {model})");
}

#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_shutdown() {}

#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_get_status() -> i32 {
    3
}

#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_get_last_error(error: *mut u32, code: *mut u32) {
    unsafe {
        if !error.is_null() {
            *error = 0;
        }
        if !code.is_null() {
            *code = 0;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_get_error_count(_board: i32, a: *mut i32, b: *mut i32) {
    unsafe {
        if !a.is_null() {
            *a = 0;
        }
        if !b.is_null() {
            *b = 0;
        }
    }
}

/// 1: both security plugs (system and game) found.
#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_check_plug(missing: *mut i32) -> i32 {
    if !missing.is_null() {
        unsafe { *missing = 0 };
    }
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_reload_plug_init() {}

#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_reload_plug_wait() -> u8 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_firmupdate_start() {}

/// 1: done.
#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_firmupdate_get_status() -> i32 {
    1
}

/// Writes `text` (NUL terminated, cut to `size`) to `out`.
unsafe fn write_str(out: *mut c_char, size: i32, text: &str) {
    if out.is_null() || size <= 0 {
        return;
    }
    let n = text.len().min(size as usize - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr(), out.cast(), n);
        *out.add(n) = 0;
    }
}

/// The security plugs' ids: "0140" and the plug's 16 hex digits.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn f24_iob_get_software_id(out: *mut c_char, size: i32) -> u8 {
    unsafe { write_str(out, size, "01400123456789ABCDEF") };
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn f24_iob_get_network_id(out: *mut c_char, size: i32) -> u8 {
    unsafe { write_str(out, size, "0140FEDCBA9876543210") };
    1
}

/// "0100" and the board's 6 bytes in hex.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn f24_iob_get_hardware_id(out: *mut c_char, size: i32, id: *mut u8) {
    const ID: [u8; 6] = [0x00, 0x12, 0x34, 0x56, 0x78, 0x9a];
    if !id.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(ID.as_ptr(), id, ID.len()) };
    }
    let text: String = std::iter::once("0100".to_string()).chain(ID.iter().map(|b| format!("{b:02X}"))).collect();
    unsafe { write_str(out, size, &text) };
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn f24_iob_get_firmware_version(_board: i32, out: *mut c_char, size: i32) {
    unsafe { write_str(out, size, "1.0.0") };
}

/// One I/O step (the game's, every frame): the buttons and coins of this step.
#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_step() {
    with_io(|io| {
        io.previous = io.buttons;
        let mut buttons = [false; BUTTONS];
        for player in 0..MAX_PLAYERS {
            io.maps[player].for_each_pressed(&wal_payload_common::input(player), |n| match n {
                Native::Test => buttons[BUTTON_TEST] = true,
                Native::Service => buttons[BUTTON_SERVICE] = true,
                Native::Coin => buttons[BUTTON_COIN] = true,
                Native::Start if player < 4 => buttons[BUTTON_START + player] = true,
                _ => {}
            });
        }
        if buttons[BUTTON_COIN] && !io.previous[BUTTON_COIN] {
            io.coins[0] = (io.coins[0] + 1) & 0x3f;
        }
        io.buttons = buttons;
    });
}

#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_button(button: i32) -> u8 {
    with_io(|io| io.buttons.get(button as usize).copied().unwrap_or(false) as u8)
}

/// system.dll's boot check state (6: the backup data error screen, TEST repairs), from its
/// `mov [ebp-4], state` at rva 0x366e; 0 when the code differs.
fn boot_check_state() -> usize {
    static STATE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *STATE.get_or_init(|| {
        let system = unsafe { GetModuleHandleA(c"system.dll".as_ptr().cast()) } as usize;
        if system == 0 {
            return 0;
        }
        let code = unsafe { std::slice::from_raw_parts((system + 0x366e) as *const u8, 7) };
        let poll = unsafe { std::slice::from_raw_parts((system + 0x38a4) as *const u8, 4) };
        if code[..3] != [0xc7, 0x45, 0xfc] || poll != [0x6a, 0x04, 0xff, 0x15] {
            log!("f24: boot check signature mismatch, backup data not repaired by itself");
            return 0;
        }
        u32::from_le_bytes([code[3], code[4], code[5], code[6]]) as usize
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_button_trg(button: i32) -> u8 {
    if button as usize == BUTTON_TEST {
        let state = boot_check_state();
        if state != 0 && unsafe { (state as *const u32).read_volatile() } == 6 {
            log!("f24: backup data error, repaired (TEST)");
            return 1;
        }
    }
    with_io(|io| {
        let b = button as usize;
        (b < BUTTONS && io.buttons[b] && !io.previous[b]) as u8
    })
}

/// The coin counter of slot 0 / 1; `max` its wrap mask.
#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_nr_coin(slot: i32, max: *mut i32) -> i32 {
    if !max.is_null() {
        unsafe { *max = 0x3f };
    }
    with_io(|io| io.coins.get(slot as usize).copied().unwrap_or(0) as i32)
}

#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_add_coin_counter(_slot: i32, _count: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_set_lamp_status(_lamp: i32, _on: u8) {}

#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_set_black_light_status(_lights: u8, _on: u8) {}

#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_set_coinblocker_status(_slot: i32, _open: u8) {}

/// The ball supply motors (units 0-1: players 1-2, 3-4).
#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_ballout_go(_unit: i32, _count: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_ballout_stop(_unit: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_ballout_stop_immediate(_unit: i32) {}

/// Request accepted (the motor's status is then read).
#[unsafe(no_mangle)]
pub extern "C" fn f24_iob_ballout_status_req(_unit: i32) -> u8 {
    1
}

/// Status read: no error, nothing pending.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn f24_iob_ballout_status_get(_unit: i32, error: *mut u8, a: *mut u8, b: *mut u8) -> u8 {
    for p in [error, a, b] {
        if !p.is_null() {
            unsafe { *p = 0 };
        }
    }
    1
}

/// The unattended test mode (automatic start presses): kept off.
#[unsafe(no_mangle)]
pub extern "C" fn f24_mujinkun_set_mode(mode: u8) {
    if mode != 0 {
        log!("f24: unattended mode {mode} requested, ignored");
    }
    with_io(|io| io.mujinkun = mode);
}

#[unsafe(no_mangle)]
pub extern "C" fn f24_mujinkun_get_mode() -> u8 {
    with_io(|io| io.mujinkun)
}

// ballio.dll functions, reached through a jump at their entry.

extern "C" fn ball_io_boot(_window: *mut c_void, use_hardware: i32) {
    log!("f24: ball_io_boot({use_hardware}): camera board emulated");
}

extern "C" fn ball_io_finish() {}

extern "C" fn ball_io_enable() {
    with_io(|io| io.enabled = true);
}

extern "C" fn ball_io_disable() {
    with_io(|io| io.enabled = false);
}

extern "C" fn ball_io_mode_set(mode: i32) {
    with_io(|io| io.mode = mode);
}

/// The balls thrown this step: one per throw press (another every `REPEAT` while held), at
/// the player's pointer.
extern "C" fn ball_io_step() {
    with_io(|io| {
        io.hits.clear();
        let now = Instant::now();
        for player in 0..MAX_PLAYERS {
            let stick = wal_payload_common::input(player);
            let mut throw = false;
            io.maps[player].for_each_pressed(&stick, |n| throw |= n == Native::Throw);
            if !throw {
                io.thrown[player] = None;
                continue;
            }
            if io.thrown[player].is_some_and(|t| now.duration_since(t) < REPEAT) {
                continue;
            }
            io.thrown[player] = Some(now);
            let x = (stick.axis(Axis::LeftX) as f32 + 32768.0) / 65535.0;
            let y = (stick.axis(Axis::LeftY) as f32 + 32768.0) / 65535.0;
            // a throw outside the picture misses the screen
            let Some((x, y)) = crate::f24_screen::to_scene(x, y) else { continue };
            let hit = (x as i16, y as i16);
            if io.enabled && io.hits.len() < MAX_HITS {
                log!("f24: player {} hit at {},{}", player + 1, hit.0, hit.1);
                io.hits.push(hit);
            }
        }
    });
}

/// The hits of this step into `out` (`size` bytes, 4 per hit), their number.
unsafe extern "C" fn ball_io_point_get(out: *mut [i16; 2], size: i32) -> i32 {
    with_io(|io| {
        if io.mode == 0 || out.is_null() {
            return 0;
        }
        let n = io.hits.len().min(size.max(0) as usize / 4);
        for (i, hit) in io.hits[..n].iter().enumerate() {
            unsafe { out.add(i).write_unaligned([hit.0, hit.1]) };
        }
        n as i32
    })
}

extern "C" fn ball_io_point_disp() {}

/// 2: booted and running.
extern "C" fn bs2_get_status() -> i32 {
    2
}

extern "C" fn bs2_get_error_code() -> i32 {
    0
}

extern "C" fn bs2_calibration_get_state() -> u8 {
    1
}

/// ballio's functions answered here.
fn ballio_overrides() -> [(&'static std::ffi::CStr, usize); 11] {
    [
    (c"?ball_io_boot@@YAXPAUHWND__@@H@Z", ball_io_boot as *const () as usize),
    (c"?ball_io_finish@@YAXXZ", ball_io_finish as *const () as usize),
    (c"?ball_io_enable@@YAXXZ", ball_io_enable as *const () as usize),
    (c"?ball_io_disable@@YAXXZ", ball_io_disable as *const () as usize),
    (c"?ball_io_mode_set@@YAXH@Z", ball_io_mode_set as *const () as usize),
    (c"?ball_io_step@@YAXXZ", ball_io_step as *const () as usize),
    (c"?ball_io_point_get@@YAHPAUT_BALL_IO_POINT@@H@Z", ball_io_point_get as *const () as usize),
    (c"?ball_io_point_disp@@YAXXZ", ball_io_point_disp as *const () as usize),
    (c"?bs2_get_status@@YAHXZ", bs2_get_status as *const () as usize),
    (c"?bs2_get_error_code@@YAHXZ", bs2_get_error_code as *const () as usize),
    (c"?bs2_calibration_get_state@@YAEXZ", bs2_calibration_get_state as *const () as usize),
    ]
}

/// A `jmp target` at `at`.
pub(crate) unsafe fn jump(at: usize, target: usize) {
    let mut code = [0xe9u8, 0, 0, 0, 0];
    code[1..].copy_from_slice(&(target.wrapping_sub(at + 5) as u32).to_le_bytes());
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, code.len());
        VirtualProtect(at as *const c_void, code.len(), old, &mut old);
    }
}

/// This payload is the game's iob.dll.
pub fn detected(module: windows_sys::Win32::Foundation::HINSTANCE) -> bool {
    crate::file_name(module).eq_ignore_ascii_case("iob.dll")
}

static ORIG_FILE_INFO: AtomicUsize = AtomicUsize::new(0);
static ORIG_FIND_FIRST: AtomicUsize = AtomicUsize::new(0);

/// The creation, last access and last write FILETIMEs (offsets 4, 12, 20 of both
/// BY_HANDLE_FILE_INFORMATION and WIN32_FIND_DATAA) rounded down to the second.
unsafe fn whole_seconds(info: *mut u8) {
    for offset in [4, 12, 20] {
        let time = unsafe { info.add(offset) }.cast::<u64>();
        let t = unsafe { time.read_unaligned() };
        unsafe { time.write_unaligned(t - t % 10_000_000) };
    }
}

unsafe extern "system" fn get_file_information_by_handle(file: *mut c_void, info: *mut u8) -> i32 {
    let original: unsafe extern "system" fn(*mut c_void, *mut u8) -> i32 =
        unsafe { std::mem::transmute(ORIG_FILE_INFO.load(Ordering::Relaxed)) };
    let ok = unsafe { original(file, info) };
    if ok != 0 && !info.is_null() {
        unsafe { whole_seconds(info) };
    }
    ok
}

unsafe extern "system" fn find_first_file(name: *const c_char, data: *mut u8) -> *mut c_void {
    let original: unsafe extern "system" fn(*const c_char, *mut u8) -> *mut c_void =
        unsafe { std::mem::transmute(ORIG_FIND_FIRST.load(Ordering::Relaxed)) };
    let find = unsafe { original(name, data) };
    // INVALID_HANDLE_VALUE
    if find as isize != -1 && !data.is_null() {
        unsafe { whole_seconds(data) };
    }
    find
}

/// AVS's file times rounded to the second (module docs: backup data).
fn backup_times() {
    let avs = unsafe { GetModuleHandleA(c"libavs-win32.dll".as_ptr().cast()) } as usize;
    if avs == 0 {
        log!("f24: libavs-win32.dll not loaded, backup data times left alone");
        return;
    }
    let hooks: [(&str, usize, &AtomicUsize); 2] = [
        ("GetFileInformationByHandle", get_file_information_by_handle as *const () as usize, &ORIG_FILE_INFO),
        ("FindFirstFileA", find_first_file as *const () as usize, &ORIG_FIND_FIRST),
    ];
    for (name, replacement, original) in hooks {
        match unsafe { iat::hook_module(avs, "kernel32.dll", name, replacement) } {
            Some(o) => original.store(o, Ordering::Relaxed),
            None => log!("f24: libavs-win32.dll imports no {name}"),
        }
    }
}

pub fn init() {
    backup_times();
    // ballio.dll imports iob.dll: it is mapped when iob's DllMain runs
    let ballio = unsafe { GetModuleHandleA(c"ballio.dll".as_ptr().cast()) };
    if ballio.is_null() {
        log!("f24: ballio.dll not loaded, camera board not emulated");
        return;
    }
    for (name, target) in ballio_overrides() {
        match unsafe { GetProcAddress(ballio, name.as_ptr().cast()) } {
            Some(f) => unsafe { jump(f as usize, target) },
            None => log!("f24: ballio {} not found", name.to_string_lossy()),
        }
    }
    crate::f24_screen::init();
    log!("f24: I/O board and camera board emulated (Gashaaaan)");
}
