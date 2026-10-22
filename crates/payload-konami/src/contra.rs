//! Contra: Evolution (`AMContra.exe`): the payload is its `libacio.dll`, whose functions it imports
//! under their MSVC names (`libacio.def`) and never calls. The cabinet reaches the game in other
//! ways, answered here:
//!
//! * The players' controls are the game's DirectInput keyboard keys: CButtonMan maps DIK codes
//!   to actions with a table in the executable (`KEY_TABLE`). The fake DirectInput's system
//!   keyboard presses each action's keys from the virtual sticks.
//! * The coin slots are on the control panel on COM1 (115200 8N1): the game writes `0x90` (its
//!   start), the panel sends 2-byte events `0xB0 code` (a coin of slot 1: code 0x0a, slot 2: 0x09).
//! * The operator clock: the game keeps a "clock set" flag (setting 0x36) next to its offset from
//!   the PC clock, written by the test menu's clock screen; unset is boot error 5-2503 "The clock
//!   is un-setting up". The settings getter reports it set: the game uses the PC clock.
//!
//! Native inputs for `native_map`: `start up down left right shot jump select test service coin`
//! (players 1 and 2; the test menu's INPUT CHECK names them).

use std::sync::Mutex;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{dinput, log, serial};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// CButtonMan's keyboard table: {DIK code, action} pairs (`.rdata`).
const KEY_TABLE: usize = 0x55cea8;
const KEY_TABLE_END: usize = 0x55cfd4;

/// The settings getter `int settings_get(int index)` (`return settings[index]`) and its bytes:
/// `mov eax, [esp+4]; mov ecx, [0x5e1ccc]`.
const SETTINGS_GET: usize = 0x463a50;
const SETTINGS_GET_BYTES: [u8; 10] = [0x8b, 0x44, 0x24, 0x04, 0x8b, 0x0d, 0xcc, 0x1c, 0x5e, 0x00];
const SETTINGS: usize = 0x5e1ccc;
/// The clock-set flag's setting.
const CLOCK_SET: i32 = 0x36;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Start,
    Up,
    Down,
    Left,
    Right,
    /// The weapon select button (CButtonMan's first button).
    Select,
    Shot,
    Jump,
    Test,
    Service,
    Coin,
}

const NATIVES: &[(&str, Native)] = &[
    ("start", Native::Start),
    ("up", Native::Up),
    ("down", Native::Down),
    ("left", Native::Left),
    ("right", Native::Right),
    ("shot", Native::Shot),
    ("jump", Native::Jump),
    ("select", Native::Select),
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("start", "start"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "shot"),
    ("b2", "jump"),
    ("b3", "select"),
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
];

/// CButtonMan's action of a player's input (even: player 1, odd: player 2; test and service
/// are the cabinet's).
fn action(native: Native, player: usize) -> Option<u32> {
    let p = player as u32;
    Some(match native {
        Native::Start => p,
        Native::Left => 2 + p,
        Native::Right => 4 + p,
        Native::Down => 6 + p,
        Native::Up => 8 + p,
        Native::Select => 10 + p,
        Native::Shot => 12 + p,
        Native::Jump => 14 + p,
        Native::Test => 20,
        Native::Service => 21,
        Native::Coin => return None,
    })
}

struct Panel {
    maps: [ButtonMap<Native>; 2],
    /// The DIK codes of each action (from the game's table).
    keys: Vec<(u32, u8)>,
    coin_held: [bool; 2],
}

static PANEL: Mutex<Option<Panel>> = Mutex::new(None);

pub fn detected() -> bool {
    crate::file_name(std::ptr::null_mut()).eq_ignore_ascii_case("AMContra.exe")
}

pub fn init() {
    let table = KEY_TABLE as *const [u32; 2];
    let keys: Vec<(u32, u8)> = (0..(KEY_TABLE_END - KEY_TABLE) / 8)
        .map(|i| unsafe { *table.add(i) })
        .filter(|&[dik, _]| dik != 0 && dik < 256)
        .map(|[dik, action]| (action, dik as u8))
        .collect();
    if keys.first() != Some(&(8, 0x11)) {
        log!("contra: unexpected keyboard table, controls not mapped");
        return;
    }
    *PANEL.lock().unwrap() = Some(Panel {
        maps: [ButtonMap::new(NATIVES, DEFAULT_MAP), ButtonMap::with_defaults(NATIVES, DEFAULT_MAP)],
        keys,
        coin_held: [false; 2],
    });
    dinput::set_keyboard(keyboard);
    serial::install_streaming("COM1", panel_write, panel_events);
    clock_init();
    log!("contra: keyboard from the virtual sticks, COM1 control panel, clock set");
}

/// The system keyboard's keys: every key of the actions pressed.
fn keyboard(state: &mut [u8; 256]) {
    let guard = PANEL.lock().unwrap();
    let Some(panel) = guard.as_ref() else { return };
    for (player, map) in panel.maps.iter().enumerate() {
        map.for_each_pressed(&wal_payload_common::input(player), |n| {
            if let Some(a) = action(n, player) {
                for &(_, dik) in panel.keys.iter().filter(|(k, _)| *k == a) {
                    state[dik as usize] = 0x80;
                }
            }
        });
    }
}

/// The game's writes to the panel (`0x90`, its start): nothing to answer.
fn panel_write(_packet: &[u8]) -> Vec<u8> {
    Vec::new()
}

/// The panel's events while the game polls it: a coin of a slot per press of its coin control.
fn panel_events() -> Vec<u8> {
    const COIN_CODES: [u8; 2] = [0x0a, 0x09];
    let mut guard = PANEL.lock().unwrap();
    let Some(panel) = guard.as_mut() else { return Vec::new() };
    let mut events = Vec::new();
    for player in 0..2 {
        let mut coin = false;
        panel.maps[player].for_each_pressed(&wal_payload_common::input(player), |n| coin |= n == Native::Coin);
        if coin && !panel.coin_held[player] {
            log!("contra: coin (slot {})", player + 1);
            events.extend([0xb0, COIN_CODES[player]]);
        }
        panel.coin_held[player] = coin;
    }
    events
}

/// The settings getter, with the clock-set flag on.
extern "C" fn settings_get(index: i32) -> i32 {
    if index == CLOCK_SET {
        return 1;
    }
    unsafe { *(*(SETTINGS as *const *const i32)).offset(index as isize) }
}

fn clock_init() {
    let at = SETTINGS_GET as *mut u8;
    if unsafe { std::slice::from_raw_parts(at, SETTINGS_GET_BYTES.len()) } != SETTINGS_GET_BYTES {
        log!("contra: unexpected settings getter, clock left unset");
        return;
    }
    let rel = (settings_get as *const () as usize).wrapping_sub(SETTINGS_GET + 5) as u32;
    let mut old = 0;
    unsafe {
        VirtualProtect(at.cast(), 5, PAGE_EXECUTE_READWRITE, &mut old);
        *at = 0xe9;
        at.add(1).cast::<u32>().write_unaligned(rel);
        VirtualProtect(at.cast(), 5, old, &mut old);
    }
}
