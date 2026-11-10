//! Pop Moto (`fightmoto.exe`, 3D GameStudio engine `acknex.dll`, Direct3D 9, 32-bit):
//! `WAL_ARCADEPC_BOARD=popmoto`, x86 build.
//!
//! - **Board.** The game's scripts call the cabinet's USB board library (`USB_board.dll`, an
//!   engine plugin: cdecl functions by name): the payload is that library (`payload_install`,
//!   the dump's community replacement put aside). `getJoy_x(0)` the handlebar (0..255, 128
//!   centered), `getJoy_x(1)` / `getJoy_y` the throttle lever (128 + accelerator - brake),
//!   `isButtonDown(n)` the keys (0 start, 1 item, 2 test), `getCoinSum` 1 for each coin
//!   inserted. The ticket, counter and lamp functions do nothing.
//! - **Engine patches.** `A6_DLLAPI` (the engine's plugin entry, called when it loads the
//!   library) patches the game's compiled scripts (their code at the engine's
//!   `[acknex.dll+0x21ab1c]`): two 3-byte instructions at +0x5283c and +0x17ef7f (from the
//!   community library), and makes the network library's connection (`GS_XSocket_CS.dll`'s
//!   `x_connect_to_sever`) return at once: one cabinet, no server.
//! - **Free play.** The common `WAL_FREEPLAY` sets the game's own (`config.ini`, `[Config]`
//!   `gamemode`: 1 free, the screens' FREE MODE; 0 the coin settings) at start, the file's
//!   other bytes kept (its GBK comments).
//!
//! Natives: `start item test coin accel brake left right`; the left stick steers, the pedals
//! throttle and brake.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log};
use wal_protocol::Axis;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Start,
    Item,
    Test,
    Coin,
    Accel,
    Brake,
    Left,
    Right,
}

const NATIVES: &[(&str, Native)] = &[
    ("start", Native::Start),
    ("item", Native::Item),
    ("test", Native::Test),
    ("coin", Native::Coin),
    ("accel", Native::Accel),
    ("brake", Native::Brake),
    ("left", Native::Left),
    ("right", Native::Right),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("start", "start"),
    ("b3", "item"),
    ("test", "test"),
    ("coin", "coin"),
    ("b1", "accel"),
    ("b2", "brake"),
    ("left", "left"),
    ("right", "right"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The natives held by player 1.
fn held() -> [bool; NATIVES.len()] {
    let mut held = [false; NATIVES.len()];
    map().for_each_pressed(&input(0), |n| held[n as usize] = true);
    held
}

/// The handlebar: the left stick, or full lock with the left / right natives (0..255).
fn handlebar() -> i32 {
    let on = held();
    let lx = input(0).axis(Axis::LeftX) as i32;
    let lx = if on[Native::Left as usize] {
        -32767
    } else if on[Native::Right as usize] {
        32767
    } else {
        lx
    };
    (128 + lx * 127 / 32767).clamp(1, 255)
}

/// The throttle lever: 128 at rest, up with the accelerator, down with the brake (0..255).
fn throttle() -> i32 {
    let on = held();
    let stick = input(0);
    let accel = if on[Native::Accel as usize] { 32767 } else { stick.axis(Axis::Accel).max(0) as i32 };
    let brake = if on[Native::Brake as usize] { 32767 } else { stick.axis(Axis::Brake).max(0) as i32 };
    (128 + accel * 127 / 32767 - brake * 127 / 32767).clamp(1, 255)
}

static COIN_HELD: AtomicBool = AtomicBool::new(false);

/// Writes `bytes` at `at` when it holds `expect` (a build check); whether it did.
unsafe fn patch(at: usize, expect: Option<&[u8]>, bytes: &[u8]) -> bool {
    let p = at as *mut u8;
    let current = unsafe { std::slice::from_raw_parts(p, bytes.len()) }.to_vec();
    if expect.is_some_and(|e| e != current.as_slice()) {
        log!("popmoto: {at:#x} holds {current:02x?}, not {expect:02x?}: not patched");
        return false;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(p.cast(), bytes.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        VirtualProtect(p.cast(), bytes.len(), old, &mut old);
    }
    log!("popmoto: {at:#x} {current:02x?} -> {bytes:02x?}");
    true
}

/// The engine's plugin entry: the game patches (see the module's notes); the plugin SDK
/// version the engine expects.
#[unsafe(no_mangle)]
pub extern "C" fn A6_DLLAPI(_a: i32) -> i32 {
    unsafe {
        let socket = LoadLibraryA(c"GS_XSocket_CS.dll".as_ptr().cast());
        if let Some(connect) = GetProcAddress(socket, c"x_connect_to_sever".as_ptr().cast()) {
            patch(connect as usize, None, &[0xC3]);
        }
        let acknex = GetModuleHandleA(c"acknex.dll".as_ptr().cast()) as usize;
        if acknex != 0 {
            let scripts = *((acknex + 0x21ab1c) as *const usize);
            log!("popmoto: scripts at {scripts:#x}");
            if scripts != 0 {
                patch(scripts + 0x5283c, None, &[0x90; 3]);
                patch(scripts + 0x17ef7f, None, &[0x90; 3]);
            }
        }
    }
    630
}

#[unsafe(no_mangle)]
pub extern "C" fn usb_board_init() -> i32 {
    log!("popmoto: usb_board_init");
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn getIsDeviceOnline() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn reConnectUSBDevice() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn getJoy_x(axis: i32) -> i32 {
    if axis == 0 { handlebar() } else { throttle() }
}

#[unsafe(no_mangle)]
pub extern "C" fn getJoy_y(_a: i32) -> i32 {
    throttle()
}

/// The keys: 0 start, 1 item, 2 test.
#[unsafe(no_mangle)]
pub extern "C" fn isButtonDown(key: i32) -> i32 {
    let on = held();
    let native = match key {
        0 => Native::Start,
        1 => Native::Item,
        2 => Native::Test,
        _ => return 0,
    };
    on[native as usize] as i32
}

/// 1 for each coin inserted (at the press).
#[unsafe(no_mangle)]
pub extern "C" fn getCoinSum(_a: i32) -> i32 {
    let coin = held()[Native::Coin as usize];
    (!COIN_HELD.swap(coin, Ordering::Relaxed) && coin) as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn getCRC(_a: i32, _b: i32, _c: i32) -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn respondByUser(_a: i32, _b: i32, _c: i32, _d: i32) -> i32 {
    1
}

macro_rules! zero {
    ($($name:ident($($arg:ident),*);)*) => {
        $(
            #[unsafe(no_mangle)]
            pub extern "C" fn $name($(_: $arg),*) -> i32 {
                0
            }
        )*
    };
}

zero! {
    addCoin(i32, i32);
    addOutTicketSum(i32, i32);
    addSumToCounter(i32, i32);
    decreaseCoin(i32, i32);
    getInBit(i32);
    getIsCoinIn(i32);
    getIsSpare(i32);
    getOutTicketSum(i32);
    getRegular(i32);
    getWorkState(i32);
    getWorkStateCounter(i32);
    setOutBit(i32, i32);
    setOutTicketSum(i32, i32);
    setStop(i32);
    setStopToCounter(i32);
    setTicketType(i32, i32);
    setWork(i32);
    setWorkToCounter(i32);
}

/// `config.ini` with its first `gamemode=` (the `[Config]` section's, before `[default]`'s)
/// set to `value`; `None` when it has none or already holds it.
fn with_game_mode(ini: &[u8], value: u8) -> Option<Vec<u8>> {
    const KEY: &[u8] = b"\ngamemode=";
    let at = ini.windows(KEY.len()).position(|w| w == KEY)? + KEY.len();
    if ini.get(at) == Some(&value) {
        return None;
    }
    let end = ini[at..].iter().position(|&b| b == b'\r' || b == b'\n').map_or(ini.len(), |n| at + n);
    let mut out = ini[..at].to_vec();
    out.push(value);
    out.extend_from_slice(&ini[end..]);
    Some(out)
}

pub fn init() {
    let free_play = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY");
    if let (Some(free), Some(dir)) = (free_play, std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf()))) {
        let path = dir.join("config.ini");
        match std::fs::read(&path).map(|ini| with_game_mode(&ini, if free { b'1' } else { b'0' })) {
            Ok(Some(ini)) => match std::fs::write(&path, ini) {
                Ok(()) => log!("popmoto: config.ini gamemode {}", free as u8),
                Err(e) => log!("popmoto: config.ini not written: {e}"),
            },
            Ok(None) => {}
            Err(e) => log!("popmoto: config.ini not read: {e}"),
        }
    }
    log!("popmoto: USB_board.dll, free play {free_play:?}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_mode() {
        let ini = b"[Config]\r\nlife=8\r\ngamemode=0\r\n[default]\r\ngamemode=0\r\n";
        assert_eq!(with_game_mode(ini, b'1').unwrap(), b"[Config]\r\nlife=8\r\ngamemode=1\r\n[default]\r\ngamemode=0\r\n");
        assert_eq!(with_game_mode(ini, b'0'), None);
        assert_eq!(with_game_mode(b"[Config]\r\n", b'1'), None);
    }
}
