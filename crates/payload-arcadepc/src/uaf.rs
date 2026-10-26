//! The "UAF" cabinet framework (Hot Shots Fire Rescue: Unity 2019.2, IL2CPP, 32-bit):
//! `WAL_ARCADEPC_BOARD=uaf`.
//!
//! `GameAssembly.dll` resolves the game's `DllImport`s with `GetProcAddress`, hooked
//! (`payload-common`'s `mono_imports.rs`):
//!
//! - **IO board** (`io_board.dll`): `openIOBoard` 0 (opened), `readIOBoard` the board's 32-byte
//!   report, `writeIOBoard` (lamps, motors, pumps, tickets) accepted. The game reads an input
//!   as a bit of a report byte, the places its scene gives (`UAF_ControlManager`, read live):
//!   byte 2 the operator's test 0x20, service 0x40, repair 0x80; byte 3 the coins of players
//!   1 and 2 (0x01, 0x02), their start keys (0x40, 0x80), the water tank's level switches (0x04 low, 0x10 normal, 0x08
//!   high: filled, 0x14);
//!   byte 29 the coins of players 3 and 4 (0x01, 0x02), the four cannons' triggers (0x04
//!   0x08 0x10 0x20), the start keys of players 3 and 4 (0x40, 0x80).
//! - **SuperDog dongle** (`api_dsp_windows.dll`, stdcall): logged in, its data file (65524)
//!   zeros: the licence perpetual (`CheckDog`).
//! - **Aim.** The cannons' potentiometers (report bytes 8..24) go through a 13-point grid
//!   calibration; `UAF_ControlManager.MoveGunController(player, current, out target, ...)`
//!   (RVA 0x42E910, the 2022-11-23 v3.3 `GameAssembly.dll`) is replaced by the payload's: the
//!   target is the cannon's place on the picture in the game's centered pixels (x -960..960,
//!   y -540..540 up).
//!
//! Natives: `trigger start coin` (each player), `test service repair` (the operator's).

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log, mono_imports};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

const PLAYERS: usize = 4;
const REPORT: usize = 32;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Start,
    Coin,
    /// An operator's key: its bit of byte 2.
    Key(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("test", Native::Key(0x20)),
    ("service", Native::Key(0x40)),
    ("repair", Native::Key(0x80)),
];

const DEFAULTS: &[(&str, &str)] = &[("b1", "trigger"), ("start", "start"), ("coin", "coin"), ("test", "test"), ("service", "service")];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// Each player's (coin, start, trigger) places: (byte, bit).
const PLACES: [[(usize, u8); 3]; PLAYERS] = [
    [(3, 0x01), (3, 0x40), (29, 0x04)],
    [(3, 0x02), (3, 0x80), (29, 0x08)],
    [(29, 0x01), (29, 0x40), (29, 0x10)],
    [(29, 0x02), (29, 0x80), (29, 0x20)],
];

/// The IO board's report: the inputs now.
fn report() -> [u8; REPORT] {
    let mut r = [0u8; REPORT];
    // the water tank filled: above its low (0x04) and normal (0x10) levels, under its high (0x08)
    r[3] |= 0x14;
    for (player, places) in PLACES.iter().enumerate() {
        map().for_each_pressed(&input(player), |n| match n {
            Native::Coin => r[places[0].0] |= places[0].1,
            Native::Start => r[places[1].0] |= places[1].1,
            Native::Trigger => r[places[2].0] |= places[2].1,
            Native::Key(key) if player == 0 => r[2] |= key,
            Native::Key(_) => {}
        });
    }
    r
}

static READS: AtomicU64 = AtomicU64::new(0);

extern "C" fn open_board() -> i32 {
    log!("uaf: IO board opened");
    0
}

extern "C" fn none() -> i32 {
    0
}

/// `readIOBoard(byte[] data, length)`: the array marshaled as its elements.
extern "C" fn read_board(data: *mut u8, length: i32) -> i32 {
    let r = report();
    let n = (length.max(0) as usize).min(REPORT);
    unsafe {
        std::ptr::write_bytes(data, 0, length.max(0) as usize);
        std::ptr::copy_nonoverlapping(r.as_ptr(), data, n);
    }
    if READS.fetch_add(1, Ordering::Relaxed) == 0 {
        log!("uaf: first report to the game ({length} bytes)");
    }
    length
}

extern "C" fn write_board(_data: *const u8, length: i32) -> i32 {
    length
}

// The SuperDog dongle: stdcall (`DllImport`'s default), the callee pops its arguments

extern "system" fn dog_login(_feature: i32, _scope: *const c_void, _vendor: *const c_void, handle: *mut i32) -> i32 {
    unsafe { *handle = 1 };
    0
}
extern "system" fn dog_logout(_handle: i32) -> i32 {
    0
}
extern "system" fn dog_read(_handle: i32, _file: i32, _offset: i32, length: i32, buffer: *mut u8) -> i32 {
    unsafe { std::ptr::write_bytes(buffer, 0, length.max(0) as usize) };
    0
}
extern "system" fn dog_write(_handle: i32, _file: i32, _offset: i32, _length: i32, _buffer: *const u8) -> i32 {
    0
}
extern "system" fn dog_get_size(_handle: i32, _file: i32, size: *mut i32) -> i32 {
    unsafe { *size = 256 };
    0
}
extern "system" fn dog_encrypt(_handle: i32, _data: *mut u8, _length: i32) -> i32 {
    0
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    let f = |f: usize| Some(f);
    match module {
        "io_board.dll" => match name {
            "openIOBoard" => f(open_board as *const () as usize),
            "readIOBoard" => f(read_board as *const () as usize),
            "writeIOBoard" => f(write_board as *const () as usize),
            _ => f(none as *const () as usize),
        },
        "api_dsp_windows.dll" => match name {
            "dog_login_scope" => f(dog_login as *const () as usize),
            "dog_logout" => f(dog_logout as *const () as usize),
            "dog_read" => f(dog_read as *const () as usize),
            "dog_write" => f(dog_write as *const () as usize),
            "dog_get_size" => f(dog_get_size as *const () as usize),
            "dog_encrypt" => f(dog_encrypt as *const () as usize),
            _ => None,
        },
        _ => None,
    }
}

// The aim

/// `UAF_ControlManager.MoveGunController` and its prologue in the known `GameAssembly.dll`.
const MOVE_GUN: usize = 0x42E910;
const MOVE_GUN_PROLOGUE: [u8; 6] = [0x55, 0x8B, 0xEC, 0x83, 0xE4, 0xF8];

/// Each cannon's last target (x, y bits), kept while it points off the picture.
static TARGETS: [AtomicU64; PLAYERS] = [const { AtomicU64::new(0) }; PLAYERS];

/// `bool MoveGunController(this, int player, Vector3 current, out Vector2 target, float speed,
/// int gun, MethodInfo*)`.
#[allow(clippy::too_many_arguments)]
extern "C" fn move_gun(this: *mut u8, player: i32, _cx: f32, _cy: f32, _cz: f32, target: *mut [f32; 2], _speed: f32, _gun: i32, _method: usize) -> u8 {
    let Some(slot) = usize::try_from(player).ok().filter(|p| *p < PLAYERS) else {
        return 0;
    };
    if let Some((x, y)) = gun::gun_of_player_in_picture(slot) {
        let t = [(x - 0.5) * 1920.0, (0.5 - y) * 1080.0];
        TARGETS[slot].store((t[0].to_bits() as u64) << 32 | t[1].to_bits() as u64, Ordering::Relaxed);
    }
    let bits = TARGETS[slot].load(Ordering::Relaxed);
    unsafe {
        *target = [f32::from_bits((bits >> 32) as u32), f32::from_bits(bits as u32)];
        // the game's "moved" flag (`flag`)
        *this.add(0x1D4) = 1;
    }
    1
}

/// Jumps `MoveGunController` to the payload's, once `GameAssembly.dll` is loaded.
fn replace_move_gun() {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    for _ in 0..20000 {
        let base = unsafe { GetModuleHandleW(windows_sys::w!("GameAssembly.dll")) } as usize;
        if base == 0 {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        let code = (base + MOVE_GUN) as *mut u8;
        if unsafe { std::slice::from_raw_parts(code, MOVE_GUN_PROLOGUE.len()) } != MOVE_GUN_PROLOGUE {
            log!("uaf: unknown GameAssembly.dll: the aim not replaced");
            return;
        }
        let rel = (move_gun as *const () as usize).wrapping_sub(code as usize + 5) as u32;
        let mut jump = [0xE9u8, 0, 0, 0, 0];
        jump[1..].copy_from_slice(&rel.to_le_bytes());
        let mut old = 0;
        unsafe {
            VirtualProtect(code.cast(), jump.len(), PAGE_EXECUTE_READWRITE, &mut old);
            std::ptr::copy_nonoverlapping(jump.as_ptr(), code, jump.len());
            VirtualProtect(code.cast(), jump.len(), old, &mut old);
        }
        log!("uaf: the cannons' aim is the payload's");
        return;
    }
}

pub fn init() {
    mono_imports::hook(resolve);
    std::thread::spawn(replace_move_gun);
    log!("uaf: IO board, SuperDog, aim");
}
