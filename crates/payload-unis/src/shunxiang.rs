//! The UNIS "shunxiang" gun cabinet (Dino Invasion: Unity 5.4, Mono, 64-bit):
//! `WAL_UNIS_BOARD=shunxiang`, the default.
//!
//! The cabinet: an I/O board on COM3 (19200 bauds) and the guns' camera
//! tracker (`LC64.dll`), behind two dongles (`DllBindFunc.dll`, `Sense4.dll`). The game imports
//! them through Mono (`payload-common`'s `mono_imports.rs`), resolved to the payload's:
//!
//! - **Board.** 16-byte frames `55 5A`, the coins inserted per gun (8 bytes, counters the game
//!   compares with the last frame's), the start, trigger and function bits (bytes 10, 11, 12:
//!   bit n gun n; the function keys are the operator's: 0 the menu, 1 the selection), two
//!   zeros and the XOR of the 15 bytes before. The PC's frames (`55 5B`: the guns' blowers)
//!   are ignored.
//! - **Guns.** `LCGetPoint(gun, x, y, brightness)`: the gun's place on the picture, 0..1 from
//!   the bottom left (the camera's y up); the game maps it through its calibration
//!   (`Assets/config/game/hwok_tmp/data1.txt`: its first four points the corners (0, 0),
//!   (1, 0), (0, 1), (1, 1), the aim unchanged). The game draws no gun cursor: the common
//!   crosshair option. The real DLL needs `FTD2XX.dll` (the dump's, in `Plugins`) to load.
//! - **Dongles.** `DllBindFunc`: present and verified, the licence the game expects;
//!   `Sense4`: every call succeeds.
//!
//! Natives: `trigger start coin` (each player), `menu select` (the operator's).

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log, mono_imports, serial};

const PORT: &str = "COM3";
const PLAYERS: usize = 4;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Start,
    Coin,
    /// An operator's function bit.
    Func(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("menu", Native::Func(0)),
    ("select", Native::Func(1)),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("start", "start"),
    ("coin", "coin"),
    ("test", "menu"),
    ("service", "select"),
    ("down", "menu"),
    ("b2", "select"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The coin counters, the coin keys held; the last frame's time.
struct State {
    coins: [u8; PLAYERS],
    coins_held: [bool; PLAYERS],
    last: Instant,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static FRAMES: AtomicU64 = AtomicU64::new(0);

/// A board frame: `55 5A data[13] xor`.
fn frame(data: &[u8; 13]) -> Vec<u8> {
    let mut f = vec![0x55, 0x5A];
    f.extend_from_slice(data);
    f.push(f.iter().fold(0, |a, b| a ^ b));
    f
}

/// The board's frame when the game reads the port (every 16 ms at most).
fn idle() -> Vec<u8> {
    let mut guard = STATE.lock().unwrap();
    let state = guard.get_or_insert_with(|| State { coins: [0; PLAYERS], coins_held: [false; PLAYERS], last: Instant::now() - Duration::from_secs(1) });
    if state.last.elapsed() < Duration::from_millis(16) {
        return Vec::new();
    }
    state.last = Instant::now();
    let mut data = [0u8; 13];
    for player in 0..PLAYERS {
        let mut coin = false;
        map().for_each_pressed(&input(player), |n| match n {
            Native::Trigger => data[9] |= 1 << player,
            Native::Start => data[8] |= 1 << player,
            Native::Coin => coin = true,
            Native::Func(bit) if player == 0 => data[10] |= 1 << bit,
            Native::Func(_) => {}
        });
        if coin && !state.coins_held[player] {
            state.coins[player] = state.coins[player].wrapping_add(1);
        }
        state.coins_held[player] = coin;
        data[player] = state.coins[player];
    }
    if FRAMES.fetch_add(1, Ordering::Relaxed) == 0 {
        log!("shunxiang: first frame to the game");
    }
    frame(&data)
}

/// The PC's frames: the blowers, nothing to answer.
fn handler(_packet: &[u8]) -> Vec<u8> {
    Vec::new()
}

// LC64: the guns' camera tracker

extern "C" fn lc_start(points: i32) {
    log!("shunxiang: LCStart({points})");
}

extern "C" fn lc_none() {}

extern "C" fn lc_contrast() -> f32 {
    1.0
}

extern "C" fn lc_get_point(index: i32, x: *mut f32, y: *mut f32, brightness: *mut f32) -> i32 {
    let (px, py) = usize::try_from(index).ok().filter(|i| *i < PLAYERS).and_then(gun::gun_of_player_in_picture).unwrap_or((-1.0, -1.0));
    // the camera's y up
    unsafe {
        *x = px;
        *y = if py < 0.0 { py } else { 1.0 - py };
        *brightness = 1.0;
    }
    1
}

// DllBindFunc: the dongle the game verifies with a licence

/// The licence `dog_check.DongleVerify` compares.
const LICENCE: [u8; 8] = [95, 215, 208, 249, 248, 165, 248, 66];

extern "C" fn bind_true() -> i32 {
    1
}

extern "C" fn bind_verify(_bytes: *const u8) -> i32 {
    1
}

extern "C" fn bind_licence(bytes: *mut u8) -> i32 {
    unsafe { std::ptr::copy_nonoverlapping(LICENCE.as_ptr(), bytes, LICENCE.len()) };
    1
}

// Sense4: success

extern "C" fn s4_enum(_ctx: *mut c_void, size: *mut u32) -> u32 {
    unsafe { *size = 96 };
    0
}

extern "C" fn s4_ok() -> u32 {
    0
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    let f = |f: usize| Some(f);
    match module {
        "kernel32.dll" => serial::replacement(name),
        "lc64.dll" => match name {
            "LCStart" => f(lc_start as *const () as usize),
            "LCGetPoint" => f(lc_get_point as *const () as usize),
            "LCGetContrast" => f(lc_contrast as *const () as usize),
            _ => f(lc_none as *const () as usize),
        },
        "dllbindfunc.dll" => match name {
            "BIND_IsDongleVerify" => f(bind_verify as *const () as usize),
            "BIND_GetDongleVerifyLic" => f(bind_licence as *const () as usize),
            _ => f(bind_true as *const () as usize),
        },
        "sense4.dll" => match name {
            "S4Enum" => f(s4_enum as *const () as usize),
            _ => f(s4_ok as *const () as usize),
        },
        _ => None,
    }
}

// The scene's gun hardware: the active board manager's data

/// Field offsets: `serial_manage_shoulie`'s arrays and flag, the active manager's static.
struct Board {
    manager: *mut u8,
    coins: usize,
    fire: usize,
    func: usize,
    set: usize,
}

unsafe impl Send for Board {}
unsafe impl Sync for Board {}

static BOARD: OnceLock<Board> = OnceLock::new();

/// The active manager (`serial_manage_shoulie.myserial_manage_shoulie`), null before it starts.
fn manager() -> Option<(&'static Board, *const u8)> {
    let b = BOARD.get()?;
    let obj = unsafe { *(b.manager as *const usize) } as *const u8;
    (!obj.is_null()).then_some((b, obj))
}

/// Where a managed array's elements start (after its object header, bounds and length).
const ARRAY_DATA: usize = 4 * size_of::<usize>();

/// Element `i` of the managed array in the field at `offset` of `obj`.
fn element<T: Copy + Default>(obj: *const u8, offset: usize, i: i32) -> T {
    let array = unsafe { *(obj.add(offset) as *const usize) } as *const u8;
    if array.is_null() || !(0..8).contains(&i) {
        return T::default();
    }
    unsafe { *(array.add(ARRAY_DATA) as *const T).add(i as usize) }
}

extern "C" fn hw_coins(_this: *mut c_void, gun: i32) -> i32 {
    manager().map_or(0, |(b, m)| element::<i32>(m, b.coins, gun))
}

extern "C" fn hw_fire(_this: *mut c_void, gun: i32) -> u8 {
    manager().map_or(0, |(b, m)| element::<u8>(m, b.fire, gun))
}

extern "C" fn hw_func(_this: *mut c_void, key: i32) -> u8 {
    manager().map_or(0, |(b, m)| element::<u8>(m, b.func, key))
}

extern "C" fn hw_read(_this: *mut c_void) -> u8 {
    manager().map_or(0, |(b, m)| unsafe { *m.add(b.set) })
}

/// The game scene's gun hardware (`gun_hardware_shiyu`) reads a separate I/O server's data
/// (`shiyu_gun_work`: a TCP client of 127.0.0.1:5000), `gun_hardware_shunxiang` a scene's own
/// `serial_manage_shoulie`, idle (only the boot scene's, kept across scenes, runs the port):
/// both read the active board manager's coins, triggers, operator's keys and data received.
/// Replaced once the game's domain runs (its board manager started): the methods compiled
/// before are the root domain's.
fn follow_active_board() {
    use wal_payload_common::mono::mono;
    for _ in 0..1200 {
        std::thread::sleep(Duration::from_millis(100));
        let Some(m) = mono() else { continue };
        if !m.attach() {
            continue;
        }
        let image = m.image("Assembly-CSharp");
        let serial = m.class(image, "", "serial_manage_shoulie");
        if serial.is_null() {
            continue;
        }
        // the game's domain running: its board manager started (the port open)
        let manager = m.static_addr(serial, m.field(serial, "myserial_manage_shoulie"));
        if manager.is_null() || unsafe { *(manager as *const usize) } == 0 {
            continue;
        }
        let offset = |f: &str| m.offset(m.field(serial, f));
        let _ = BOARD.set(Board { manager, coins: offset("my_coins"), fire: offset("my_key_gun_fire"), func: offset("my_key_func"), set: offset("my_is_set_jvs_datas") });
        let mut replaced = 0;
        for class in ["gun_hardware_shunxiang", "gun_hardware_shiyu"] {
            let hw = m.class(image, "", class);
            replaced += [
                ("get_coin_count", 1, hw_coins as *const () as usize),
                ("get_gun_status", 1, hw_fire as *const () as usize),
                ("get_key_func", 1, hw_func as *const () as usize),
                ("is_read_serial_data", 0, hw_read as *const () as usize),
            ]
            .into_iter()
            .filter(|(name, args, f)| m.replace(m.method(hw, name, *args), *f))
            .count();
        }
        log!("shunxiang: the scene's gun hardware reads the active board ({replaced}/8)");
        return;
    }
    log!("shunxiang: the gun hardware's class not found");
}

pub fn init() {
    serial::install_streaming_in(PORT, handler, idle, &[]);
    mono_imports::hook(resolve);
    log!("shunxiang: board on {PORT}, guns, dongles");
    std::thread::spawn(follow_active_board);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames() {
        let f = frame(&[0; 13]);
        assert_eq!((f.len(), f[0], f[1], f[15]), (16, 0x55, 0x5A, 0x55 ^ 0x5A));
    }
}
