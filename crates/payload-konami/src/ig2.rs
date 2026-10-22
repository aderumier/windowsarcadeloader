//! Gashaaaan Refill! (IG2): the cabinet I/O through `ig2ACLib.dll`.
//!
//! gasya.exe loads ig2ACLib (`LoadLibraryA` + `GetProcAddress` of its 53 functions): the
//! library boots AVS and drives two serial boards, the I/O board (`CIg2ACLibIOIob`: switches,
//! coins, lamps, the ball supply motors, the security plug) and the camera board
//! (`CIg2ACLibIOBallio`: where the thrown balls hit the screens). Both boards have a
//! no-hardware mode (their boot's argument 1: boot done, security checks passed, nothing read):
//! it is forced, and the input functions are answered from the virtual sticks instead.
//!
//! - Buttons (`ig2ACLibIOGetButton`, 24 bytes): byte 9 TEST, 10 SERVICE, 11 the coin mech,
//!   12-15 the start buttons of players 1-4 (the test menu's INPUT CHECK).
//! - Coins (`ig2ACLibIOCoinGetIncrease`): coins inserted since the last call, which the game
//!   hands to the BM-System (`bms.rs`).
//! - Balls (`ig2ACLibIOGetBallPosSensorPosNum` / `Pos`): where balls hit, in the game's
//!   1280x480 picture (the two 640x480 screens side by side): a player's pointer (gun or
//!   mouse) and its throw button, one ball per press, more while it is held.
//!
//! Also ig2ACLib's master volume setup through the winmm mixer (Wine's has no such line: the
//! SPEAKER SETTING error) is skipped, and gasya's "clock not set" boot warning (the operator
//! sets it once in the test menu; the PC's clock is right) and "screen not adjusted" one (the
//! test menu's camera calibration: the hits here are already in the picture's coordinates).
//!
//! Native inputs for `native_map`: `test service coin start throw`.

use std::ffi::{CStr, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log};
use wal_protocol::{Axis, MAX_PLAYERS};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
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

/// The game's picture: what ig2ACLib's ball positions are in (`ig2ACLibScreenSetWidth/Height`).
const WIDTH: f32 = 1280.0;
const HEIGHT: f32 = 480.0;
/// A held throw button throws again after this.
const REPEAT: Duration = Duration::from_millis(150);
/// Hits reported per update at most (the game reads 16).
const MAX_HITS: usize = 16;

struct Io {
    maps: Vec<ButtonMap<Native>>,
    coin_held: bool,
    coins: u32,
    /// When each player's held throw button threw last.
    thrown: [Option<Instant>; MAX_PLAYERS],
    /// The hits of the current update (`PosNum` takes them, `Pos` reads them).
    hits: Vec<(i16, i16)>,
}

static IO: Mutex<Option<Io>> = Mutex::new(None);

fn with_io<R>(f: impl FnOnce(&mut Io) -> R) -> R {
    let mut guard = IO.lock().unwrap();
    let io = guard.get_or_insert_with(|| Io {
        maps: (0..MAX_PLAYERS)
            .map(|p| if p == 0 { ButtonMap::new(NATIVES, DEFAULT_MAP) } else { ButtonMap::with_defaults(NATIVES, DEFAULT_MAP) })
            .collect(),
        coin_held: false,
        coins: 0,
        thrown: [None; MAX_PLAYERS],
        hits: Vec::new(),
    });
    f(io)
}

/// The natives pressed by any player, and each player's own.
fn pressed(io: &Io) -> [Vec<Native>; MAX_PLAYERS] {
    std::array::from_fn(|p| {
        let mut v = Vec::new();
        io.maps[p].for_each_pressed(&wal_payload_common::input(p), |n| v.push(n));
        v
    })
}

// ig2ACLib functions answered here (cdecl, 0 = success).

unsafe extern "C" fn get_button(out: *mut u8) -> i32 {
    if out.is_null() {
        return -1;
    }
    let mut buf = [0u8; 24];
    with_io(|io| {
        for (player, natives) in pressed(io).iter().enumerate() {
            for n in natives {
                match n {
                    Native::Test => buf[9] = 1,
                    Native::Service => buf[10] = 1,
                    Native::Coin => buf[11] = 1,
                    Native::Start => buf[12 + player] = 1,
                    Native::Throw => {}
                }
            }
        }
    });
    unsafe { std::ptr::copy_nonoverlapping(buf.as_ptr(), out, buf.len()) };
    0
}

unsafe extern "C" fn coin_get_increase(out: *mut u32) -> i32 {
    let coins = with_io(|io| {
        let coin = pressed(io).iter().flatten().any(|n| *n == Native::Coin);
        if coin && !io.coin_held {
            io.coins += 1;
        }
        io.coin_held = coin;
        std::mem::take(&mut io.coins)
    });
    if !out.is_null() {
        unsafe { *out = coins };
    }
    0
}

unsafe extern "C" fn coin_consume(_count: u32) -> i32 {
    0
}

/// The balls thrown since the last update: one per throw press (another every `REPEAT` while
/// held), at the player's pointer.
unsafe extern "C" fn ball_pos_num(out: *mut i32) -> i32 {
    let n = with_io(|io| {
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
            let x = (stick.axis(Axis::LeftX) as f32 + 32768.0) / 65535.0 * WIDTH;
            let y = (stick.axis(Axis::LeftY) as f32 + 32768.0) / 65535.0 * HEIGHT;
            let hit = ((x as i16).clamp(0, WIDTH as i16 - 1), (y as i16).clamp(0, HEIGHT as i16 - 1));
            if io.hits.len() < MAX_HITS {
                io.hits.push(hit);
            }
        }
        io.hits.len()
    });
    if !out.is_null() {
        unsafe { *out = n as i32 };
    }
    0
}

unsafe extern "C" fn ball_pos(index: i32, out: *mut [i16; 2]) -> i32 {
    let hit = with_io(|io| io.hits.get(index as usize).copied());
    if out.is_null() {
        return -1;
    }
    let (x, y) = hit.unwrap_or((0, 0));
    unsafe { out.write_unaligned([x, y]) };
    if hit.is_some() {
        log!("ig2: ball hit at {x},{y}");
    }
    0
}

/// The ig2ACLib functions answered here.
fn override_for(name: &[u8]) -> Option<usize> {
    Some(match name {
        b"ig2ACLibIOGetButton" => get_button as *const () as usize,
        b"ig2ACLibIOCoinGetIncrease" => coin_get_increase as *const () as usize,
        b"ig2ACLibIOCoinConsume" => coin_consume as *const () as usize,
        b"ig2ACLibIOGetBallPosSensorPosNum" => ball_pos_num as *const () as usize,
        b"ig2ACLibIOGetBallPosSensorPos" => ball_pos as *const () as usize,
        _ => return None,
    })
}

/// Writes `new` over `expected` at `at`, when the code is the expected one.
unsafe fn patch(what: &str, at: usize, expected: &[u8], new: &[u8]) {
    let current = unsafe { std::slice::from_raw_parts(at as *const u8, expected.len()) };
    if current != expected {
        log!("ig2: {what}: code differs ({current:02x?}), not patched");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, new.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(new.as_ptr(), at as *mut u8, new.len());
        VirtualProtect(at as *const c_void, new.len(), old, &mut old);
    }
    log!("ig2: {what}");
}

/// ig2ACLib.dll (`090216J`), patched once loaded.
unsafe fn patch_aclib(base: usize) {
    unsafe {
        // the master volume through the mixer: `xor eax, eax; ret` (done)
        patch("ig2ACLib: speaker volume setup skipped", base + 0xfd60, &[0x55, 0x8b, 0xec, 0x83, 0xe4, 0xf8], &[0x31, 0xc0, 0xc3]);
        // the boards' boot(mode): `mov eax, [esp+4]` -> `push 1; pop eax; nop`
        patch("ig2ACLib: I/O board without hardware", base + 0x3500, &[0x8b, 0x44, 0x24, 0x04, 0x83, 0xf8, 0x01], &[0x6a, 0x01, 0x58, 0x90]);
        patch("ig2ACLib: camera board without hardware", base + 0x2570, &[0x8b, 0x44, 0x24, 0x04, 0xba, 0x01], &[0x6a, 0x01, 0x58, 0x90]);
    }
}

static ORIG_GET_PROC: AtomicUsize = AtomicUsize::new(0);
static ACLIB_PATCHED: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn get_proc_address(module: *mut c_void, name: *const u8) -> *const c_void {
    let orig: unsafe extern "system" fn(*mut c_void, *const u8) -> *const c_void =
        unsafe { std::mem::transmute(ORIG_GET_PROC.load(Ordering::Relaxed)) };
    let proc = unsafe { orig(module, name) };
    if name as usize <= 0xffff {
        return proc;
    }
    let name = unsafe { CStr::from_ptr(name.cast()) }.to_bytes();
    if !name.starts_with(b"ig2ACLib") {
        return proc;
    }
    if !ACLIB_PATCHED.swap(true, Ordering::Relaxed) {
        unsafe { patch_aclib(module as usize) };
    }
    match override_for(name) {
        Some(f) => f as *const c_void,
        None => proc,
    }
}

pub fn init() {
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    unsafe {
        // gasya's boot check of the settings' "clock set" flag: `jne` -> `jmp`
        patch("gasya: clock-not-set warning skipped", exe + 0x2d4fc, &[0x80, 0x3d, 0x80, 0xdd, 0x6a, 0x00, 0x00, 0x75, 0x0a], &[0x80, 0x3d, 0x80, 0xdd, 0x6a, 0x00, 0x00, 0xeb]);
        // and of its "screen adjusted" flag (the test menu's camera calibration, ADJUST SCREEN)
        patch("gasya: screen-not-adjusted warning skipped", exe + 0x2d52c, &[0x80, 0x3d, 0x81, 0xdd, 0x6a, 0x00, 0x00, 0x75, 0x0a], &[0x80, 0x3d, 0x81, 0xdd, 0x6a, 0x00, 0x00, 0xeb]);
        match iat::hook("kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) {
            Some(orig) => ORIG_GET_PROC.store(orig, Ordering::Relaxed),
            None => log!("ig2: GetProcAddress not imported, ig2ACLib not hooked"),
        }
    }
    log!("ig2: cabinet I/O from the virtual sticks (ig2ACLib)");
}
