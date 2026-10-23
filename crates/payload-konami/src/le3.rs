//! Lethal Enforcers 3 (D22, 2005): the payload is its `iob.dll`, the I/O board library.
//!
//! `game.exe` (Direct3D 8) imports two Konami libraries (MSVC C++, cdecl, `libacio.def` gives
//! their names): `iob.dll`, the I/O board (switches, coin counter, lamps, bass shakers), answered
//! here, and `gunio.dll`, the two gun boards on a serial link: its functions are pointed at the
//! payload (a jump at each one's entry), the boards boot and report no error.
//!
//! - `iob_get_digital_in` fills the game's switch record (44 bytes): +0 valid, +4 TEST, +5 the
//!   coin switch, +7 SERVICE, +8/+9 the players' START, +0xa/+0xb TRIGGER, +0xc/+0xd OPTION
//!   (held), +0xe..+0x13 the same six as press counters (the game reacts to a change), +0x26
//!   the coin counter (6 bits, the game counts its steps).
//! - The boot: `iob_boot_wait` 1 (booted), no firmware update, the security checked
//!   (`iob_check_initial_security_checked` 0), `gunio_boot_status` 1 (CHECKING GUN...OK).
//! - The aim: the game's gun read (`0x51d540`, `this`, player, `{x, y, off, raw_x, raw_y}`) puts
//!   `gunio_read_data`'s raw counts through the cabinet calibration and smoothing to the
//!   player's 640x480 screen: replaced by the pointer (`gun_read`), off screen (-1: the player
//!   takes cover and reloads) outside the screen or with the `cover` control.
//!
//! Native inputs for `native_map`: `test service coin start trigger option cover`.

use std::ffi::{c_char, c_void};
use std::sync::Mutex;

use wal_payload_common::log;
use wal_payload_common::mapping::ButtonMap;
use wal_protocol::Axis;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    Start,
    Trigger,
    Option,
    Cover,
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("start", Native::Start),
    ("trigger", Native::Trigger),
    ("option", Native::Option),
    ("cover", Native::Cover),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("b1", "trigger"),
    ("b2", "option"),
    ("b6", "cover"),
];

/// The players (two guns).
const PLAYERS: usize = 2;

/// The game's switch record (`IOB_DIGITAL_IN_W`).
#[repr(C, packed)]
pub struct DigitalIn {
    valid: i32,
    test: u8,
    coin: u8,
    _unused: u8,
    service: u8,
    start: [u8; 2],
    trigger: [u8; 2],
    option: [u8; 2],
    start_count: [u8; 2],
    trigger_count: [u8; 2],
    option_count: [u8; 2],
    _other: [u8; 18],
    coin_counter: u16,
    _end: [u16; 2],
}

const _: () = assert!(std::mem::size_of::<DigitalIn>() == 44);

struct Io {
    maps: Vec<ButtonMap<Native>>,
    /// Held last read, per player: start, trigger, option.
    held: [[bool; 3]; PLAYERS],
    counts: [[u8; 3]; PLAYERS],
    coin_held: bool,
    coins: u16,
}

static IO: Mutex<Option<Io>> = Mutex::new(None);

fn with_io<R>(f: impl FnOnce(&mut Io) -> R) -> R {
    let mut guard = IO.lock().unwrap();
    let io = guard.get_or_insert_with(|| Io {
        maps: (0..PLAYERS)
            .map(|p| if p == 0 { ButtonMap::new(NATIVES, DEFAULT_MAP) } else { ButtonMap::with_defaults(NATIVES, DEFAULT_MAP) })
            .collect(),
        held: [[false; 3]; PLAYERS],
        counts: [[0; 3]; PLAYERS],
        coin_held: false,
        coins: 0,
    });
    f(io)
}

/// Whether `player` presses `native`.
fn pressed(player: usize, native: Native) -> bool {
    let stick = wal_payload_common::input(player);
    with_io(|io| {
        let mut on = false;
        io.maps[player].for_each_pressed(&stick, |n| on |= n == native);
        on
    })
}

// iob.dll (cdecl; `libacio.def` gives the MSVC names).

#[unsafe(no_mangle)]
pub extern "C" fn le3_iob_init(model: *const c_char, a: i32, b: i32) -> i32 {
    let model = if model.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(model) }.to_string_lossy().into_owned() };
    log!("le3: iob_init(model {model}, {a}, {b})");
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn le3_iob_one() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn le3_iob_zero() -> i32 {
    0
}

/// The functions without a result (lamps, LEDs, bass shakers, coin counter, gun reactions).
#[unsafe(no_mangle)]
pub extern "C" fn le3_iob_nothing() {}

/// The I/O board's id string.
#[unsafe(no_mangle)]
pub extern "C" fn le3_iob_get_id(_n: i32) -> *const c_char {
    c"GMD22EAA".as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn le3_iob_get_version() -> *const c_char {
    c"1.00".as_ptr()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn le3_iob_get_firmware_version(a: *mut u8, b: *mut u8, c: *mut u8) -> i32 {
    for (p, v) in [(a, 1), (b, 0), (c, 0)] {
        if !p.is_null() {
            unsafe { *p = v };
        }
    }
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn le3_iob_get_model_code(code: *mut u8) -> i32 {
    if !code.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(b"GMD22\0".as_ptr(), code, 6) };
    }
    1
}

/// The switches and the coin counter of this step.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn le3_iob_get_digital_in(out: *mut DigitalIn) -> i32 {
    if out.is_null() {
        return 0;
    }
    let sticks: Vec<_> = (0..PLAYERS).map(wal_payload_common::input).collect();
    let players = PLAYERS_ON.load(std::sync::atomic::Ordering::Relaxed);
    let d = with_io(|io| {
        let mut d: DigitalIn = unsafe { std::mem::zeroed() };
        d.valid = 1;
        let mut coin = false;
        for (player, stick) in sticks.iter().enumerate() {
            let mut now = [false; 3];
            io.maps[player].for_each_pressed(stick, |n| match n {
                Native::Test => d.test = 1,
                Native::Service => d.service = 1,
                Native::Coin => coin = true,
                Native::Start => now[0] = true,
                Native::Trigger => now[1] = true,
                Native::Option => now[2] = true,
                Native::Cover => {}
            });
            // player 2 does not play in the 1 player mode
            if player >= players {
                now = [false; 3];
            }
            for (i, on) in now.into_iter().enumerate() {
                if on && !io.held[player][i] {
                    io.counts[player][i] = io.counts[player][i].wrapping_add(1);
                }
            }
            io.held[player] = now;
            d.start[player] = now[0] as u8;
            d.trigger[player] = now[1] as u8;
            d.option[player] = now[2] as u8;
            d.start_count[player] = io.counts[player][0];
            d.trigger_count[player] = io.counts[player][1];
            d.option_count[player] = io.counts[player][2];
        }
        if coin && !io.coin_held {
            io.coins = (io.coins + 1) & 0x3f;
            log!("le3: coin");
        }
        io.coin_held = coin;
        d.coin = coin as u8;
        d.coin_counter = io.coins;
        d
    });
    unsafe { out.write(d) };
    0
}

// gunio.dll's functions, reached through a jump at their entry.

extern "C" fn gunio_init(a: i32, b: i32) {
    log!("le3: gunio_init({a}, {b}): gun boards emulated");
}

extern "C" fn gunio_nothing() {}

/// 1: the board booted (CHECKING GUN...OK).
extern "C" fn gunio_boot_status(_gun: i32) -> i32 {
    1
}

extern "C" fn gunio_zero() -> i32 {
    0
}

extern "C" fn gunio_get_ver(_gun: i32) -> i32 {
    0x100
}

extern "C" fn gunio_change_freq_id(_id: i32) {}

/// The board's DIP switches (the test menu shows bits 7 and 6): off.
unsafe extern "C" fn gunio_dipsw_data(_gun: i32, out: *mut u8) {
    if !out.is_null() {
        unsafe { *out = 0 };
    }
}

unsafe extern "C" fn gunio_get_errorcode(out: *mut c_char, size: i32) -> i32 {
    if !out.is_null() && size > 0 {
        unsafe { *out = 0 };
    }
    0
}

unsafe extern "C" fn gunio_get_data_error_count(_gun: i32, out: *mut u8) -> i32 {
    if !out.is_null() {
        unsafe { *out = 0 };
    }
    0
}

unsafe extern "C" fn gunio_get_ledcheck_data(_gun: i32, out: *mut u16) {
    if !out.is_null() {
        unsafe { *out = 0 };
    }
}

extern "C" fn gunio_get_serial_error_count(_gun: i32) -> i32 {
    0
}

/// Raw counts (centered), no error: the aim is `gun_read`'s.
unsafe extern "C" fn gunio_read_data(_gun: i32, x: *mut f32, y: *mut f32, error: *mut i32, raw_x: *mut i32, raw_y: *mut i32) {
    unsafe {
        for (p, v) in [(x, 320.0), (y, 240.0)] {
            if !p.is_null() {
                *p = v;
            }
        }
        for (p, v) in [(error, 0), (raw_x, 0x80), (raw_y, 0x80)] {
            if !p.is_null() {
                *p = v;
            }
        }
    }
}

fn gunio_overrides() -> [(&'static std::ffi::CStr, usize); 18] {
    [
        (c"?gunio_init@@YAXHH@Z", gunio_init as *const () as usize),
        (c"?gunio_finish@@YAXXZ", gunio_nothing as *const () as usize),
        (c"?gunio_run@@YAXXZ", gunio_nothing as *const () as usize),
        (c"?gunio_stop@@YAXXZ", gunio_nothing as *const () as usize),
        (c"?gunio_boot_status@@YAHH@Z", gunio_boot_status as *const () as usize),
        (c"?gunio_check_error@@YAHXZ", gunio_zero as *const () as usize),
        (c"?gunio_get_ver@@YAHH@Z", gunio_get_ver as *const () as usize),
        (c"?gunio_change_freq_id@@YAXH@Z", gunio_change_freq_id as *const () as usize),
        (c"?gunio_update_dipsw_data@@YAXXZ", gunio_nothing as *const () as usize),
        (c"?gunio_dipsw_data@@YAXHPAE@Z", gunio_dipsw_data as *const () as usize),
        (c"?gunio_update_ledcheck_data@@YAXXZ", gunio_nothing as *const () as usize),
        (c"?gunio_get_ledcheck_data@@YAXHPAG@Z", gunio_get_ledcheck_data as *const () as usize),
        (c"?gunio_get_errorcode@@YAHPADH@Z", gunio_get_errorcode as *const () as usize),
        (c"?gunio_get_data_error_count@@YAHHPAE@Z", gunio_get_data_error_count as *const () as usize),
        (c"?gunio_get_serial_error_count@@YAHH@Z", gunio_get_serial_error_count as *const () as usize),
        (c"?gunio_read_data@@YAXHPAM0PAH11@Z", gunio_read_data as *const () as usize),
        (c"?gunio_get_calibration@@YAXHPAUT_GUNIO_CALIBRATION_DATA@@@Z", gunio_nothing as *const () as usize),
        (c"?gunio_set_calibration@@YAXHPAUT_GUNIO_CALIBRATION_DATA@@@Z", gunio_nothing as *const () as usize),
    ]
}

/// The game's gun read result.
#[repr(C)]
struct Gun {
    x: f32,
    y: f32,
    off: i32,
    raw_x: i32,
    raw_y: i32,
}

/// The players (`WAL_KONAMI_PLAYERS`): 2, a monitor each (the cabinet), or 1: player 1's
/// monitor alone is shown, player 2 cannot join. The game always gets both monitors: with one
/// adapter (its player count), a lone player's CPU rival has no player 2 to run on (the game
/// crashed at the stage start).
static PLAYERS_ON: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(2);

/// The monitors shown side by side: 2, or 1 (player 1's) with 1 player unless
/// `WAL_KONAMI_RIVAL_SCREEN=1` (player 2's monitor shows the CPU rival).
static SCREENS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(2);

/// The pointer (0..1 over the window) on `player`'s 640x480 screen (0..1): with two monitors
/// in the window, each player aims on their own, side by side (player 1 left).
fn to_screen(player: usize, x: f32, y: f32) -> (f32, f32) {
    let shown = SCREENS.load(std::sync::atomic::Ordering::Relaxed) as f32;
    let (left, top, width, height) = crate::two_screens::picture_area();
    ((x - left) / width * shown - player as f32, (y - top) / height)
}

/// The game's gun read (thiscall, `this` the I/O object).
const GUN_READ: usize = 0x51d540;
const GUN_READ_BYTES: [u8; 6] = [0x55, 0x8b, 0xec, 0x83, 0xec, 0x10];

/// The player's pointer on their 640x480 screen; -1, -1 (cover) off it or with `cover`.
extern "thiscall" fn gun_read(_this: *mut c_void, player: i32, out: *mut Gun) {
    let player = (player != 0) as usize;
    let stick = wal_payload_common::input(player);
    let fx = (stick.axis(Axis::LeftX) as f32 + 32768.0) / 65535.0;
    let fy = (stick.axis(Axis::LeftY) as f32 + 32768.0) / 65535.0;
    let (fx, fy) = to_screen(player, fx, fy);
    let off = !(0.0..1.0).contains(&fx) || !(0.0..1.0).contains(&fy) || pressed(player, Native::Cover);
    let gun = if off {
        Gun { x: -1.0, y: -1.0, off: 1, raw_x: 0, raw_y: 0 }
    } else {
        Gun { x: fx * 640.0, y: fy * 480.0, off: 0, raw_x: (fx * 255.0) as i32, raw_y: (fy * 255.0) as i32 }
    };
    if !out.is_null() {
        unsafe { out.write(gun) };
    }
}

/// The boot's SYSTEM DATA CHECK1 (2-1005-0001) compares the CRC of check.a, game.exe and the
/// DLLs with the values in check.a: this iob.dll is not the cabinet's. The files must still be
/// there and readable; any CRC passes.
const CRC_CHECK: usize = 0x536bb3;
const CRC_CHECK_BYTES: [u8; 2] = [0x75, 0x46];

/// The boot's CLOCK check (PLEASE SET THE CLOCK BEFORE OPERATION): the "clock set" flag of the
/// bookkeeping, set by the test menu's CLOCK screen (it sets the PC's time). The PC's clock is
/// the host's: always set.
const CLOCK_CHECK: usize = 0x536cb0;
const CLOCK_CHECK_BYTES: [u8; 5] = [0xa1, 0x38, 0x89, 0x77, 0x00];

/// The boot's GUN CALIBRATION check (PLEASE CALIBRATE GUN): each gun's calibration done (the
/// test menu's GUN CALIBRATION). The aim is the pointer's (`gun_read`): no calibration.
const CALIBRATION_CHECK: usize = 0x536cc0;
const CALIBRATION_CHECK_BYTES: [u8; 6] = [0x8b, 0x0d, 0x58, 0x41, 0x69, 0x00];

/// Writes `new` at `at` when the code there is `expected`.
fn patch(at: usize, expected: &[u8], new: &[u8], what: &str) {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    if unsafe { std::slice::from_raw_parts(at as *const u8, expected.len()) } != expected {
        log!("le3: unexpected code at {at:#x}, {what} not patched");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, new.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(new.as_ptr(), at as *mut u8, new.len());
        VirtualProtect(at as *const c_void, new.len(), old, &mut old);
    }
}

/// This payload is the game's iob.dll, beside gunio.dll.
pub fn detected(module: windows_sys::Win32::Foundation::HINSTANCE) -> bool {
    crate::file_name(module).eq_ignore_ascii_case("iob.dll") && unsafe { !GetModuleHandleA(c"gunio.dll".as_ptr().cast()).is_null() }
}

/// The cabinet's game folder in the game's paths (its data, movies: some opened by DirectShow).
const GAME_FOLDER: &[u8] = b"D:\\D22\\";

/// `D:\D22\...` path strings of the executable made relative (`.\...`): the game runs from
/// its folder.
fn relative_paths() {
    use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
    let base = unsafe { GetModuleHandleA(std::ptr::null()) } as usize;
    let (start, end) = (base + 0x227000, base + 0x386000);
    let image = unsafe { std::slice::from_raw_parts_mut(start as *mut u8, end - start) };
    let mut count = 0;
    let mut i = 0;
    while let Some(at) = image[i..].windows(GAME_FOLDER.len()).position(|w| w == GAME_FOLDER) {
        let at = i + at;
        let len = image[at..].iter().position(|&b| b == 0).unwrap_or(0);
        let mut old = 0;
        unsafe { VirtualProtect(image[at..].as_ptr().cast(), len + 1, PAGE_READWRITE, &mut old) };
        // ".\" and the rest, NUL terminated, in place
        image.copy_within(at + GAME_FOLDER.len()..at + len + 1, at + 2);
        image[at] = b'.';
        image[at + 1] = b'\\';
        unsafe { VirtualProtect(image[at..].as_ptr().cast(), len + 1, old, &mut old) };
        count += 1;
        i = at + 2;
    }
    log!("le3: {count} D:\\D22 paths made relative");
}

pub fn init() {
    relative_paths();
    patch(CRC_CHECK, &CRC_CHECK_BYTES, &[0x90, 0x90], "the files' CRC check");
    // mov eax, 1
    patch(CLOCK_CHECK, &CLOCK_CHECK_BYTES, &[0xb8, 0x01, 0x00, 0x00, 0x00], "the clock check");
    // mov eax, 1; ret 4
    patch(CALIBRATION_CHECK, &CALIBRATION_CHECK_BYTES, &[0xb8, 0x01, 0x00, 0x00, 0x00, 0xc2, 0x04, 0x00], "the calibration check");
    // the game imports both: gunio.dll is mapped when iob's DllMain runs
    let gunio = unsafe { GetModuleHandleA(c"gunio.dll".as_ptr().cast()) };
    for (name, target) in gunio_overrides() {
        match unsafe { GetProcAddress(gunio, name.as_ptr().cast()) } {
            Some(f) => unsafe { crate::f24::jump(f as usize, target) },
            None => log!("le3: gunio {} not found", name.to_string_lossy()),
        }
    }
    if unsafe { std::slice::from_raw_parts(GUN_READ as *const u8, GUN_READ_BYTES.len()) } == GUN_READ_BYTES {
        unsafe { crate::f24::jump(GUN_READ, gun_read as *const () as usize) };
    } else {
        log!("le3: unexpected gun read code, guns aim through the board's calibration");
    }
    let players = if std::env::var("WAL_KONAMI_PLAYERS").is_ok_and(|v| v.trim() == "1") { 1 } else { 2 };
    let rival = std::env::var("WAL_KONAMI_RIVAL_SCREEN").is_ok_and(|v| v.trim() == "1");
    let screens = if players == 2 || rival { 2 } else { 1 };
    PLAYERS_ON.store(players, std::sync::atomic::Ordering::Relaxed);
    SCREENS.store(screens, std::sync::atomic::Ordering::Relaxed);
    if screens == 2 {
        crate::two_screens::init("le3");
    } else {
        crate::two_screens::init_first_only("le3");
    }
    log!("le3: {players} player(s), {screens} screen(s) shown");
    log!("le3: I/O board and gun boards emulated (Lethal Enforcers 3)");
}
