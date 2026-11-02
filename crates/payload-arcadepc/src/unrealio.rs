//! The "UnrealIO" 32-bit USB I/O board of Maiqiang's cabinets (Polar Wars, `JiDiDaZhan.exe`,
//! Unity 5.6, Mono, 32-bit): `WAL_ARCADEPC_BOARD=unrealio`.
//!
//! The game's `UnrealIO32.CommLogicMain` (the settings' `CommSetting.ini`: `is32Bit`, `isUSB`)
//! talks to the board through `UnrealHid.dll` (`DllImport("UnrealHid")`, cdecl: `CreateInstance`,
//! `Open`, `Close`, `Read`, `Write`, `SetReadTimeout`, `Release`): the payload, installed as that
//! DLL, answers them through Mono's `GetProcAddress` (`mono_imports.rs`). Each exchange is one
//! 32-byte `Write` (PC: `F5`, the outputs, the ticket counts, `sum` of the bytes before it), then
//! one 32-byte `Read` (0 = done; board: `FA`, the bytes below, `sum`):
//!
//! * 1: the operator keys (bit 0 setting / menu down, 1 menu left, 2 ticket / menu right, 3 menu
//!   up);
//! * 2: a coin per player (bit), each answer one coin;
//! * 3, 5: a ticket / a capsule paid per player (bit), each answer one; 4, 6: their dispensers
//!   empty;
//! * 7..22: 16 ADC bytes: the guns' x, y as 16-bit big endian (player 1 bytes 7..10, player 2
//!   11..14);
//! * 23..26: 32 key bits (0, 1 the start keys, 2, 3 the triggers, 4, 5 the ball out sensors,
//!   16..23 the ball motors' faults, 24, 25 the coin switches, 30, 31 the water level).
//!
//! Aim: 0..4080 over the picture, y from the top. The game maps the readings through 11 points
//! per gun (`Save/CorrectionData<n>P.txt`, at 10 %, 30 %, 50 %, 70 % and 90 % of the screen):
//! the profile's files put them at those fractions of 4080, a linear map.
//!
//! Tickets: the PC's byte 8 (low nibble player 1, high player 2) asks tickets, byte 12 a capsule
//! per player (bit): the payload pays them, one every 10 exchanges.
//!
//! The boot's board updater (`IOUpdate_Data`'s firmware) switches the board to its 64-byte
//! bootloader frames: those reads fail, and it gives up after a few seconds.
//!
//! Last stage (Polar Wars): the scenario select (`PS_Selete.StateStart`) locks the fifth
//! (`level[4].Lock(true)`, chains over it), played last. At the board's first call (the game's
//! managed thread, before that method is compiled) its IL is patched to `Lock(false)`, and
//! `PS_SeleteLevel.Lock` (only called there) skips its colliders part: the stage can be shot to
//! pick it, a stage played stays closed (`Init`).
//!
//! Natives: `trigger start coin` (each player), `setting left right up` (the operator's).

use std::sync::{Mutex, OnceLock};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log, mono};

use crate::{demul, mono_imports};

const PLAYERS: usize = 2;
const LEN: usize = 32;
/// The ADC range over the picture (the calibration files' 10 % is 408).
const ADC_SPAN: f32 = 4080.0;
/// Exchanges between two tickets paid.
const TICKET_INTERVAL: u32 = 10;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Start,
    Coin,
    /// An operator key's bit in byte 1.
    Key(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("setting", Native::Key(0)),
    ("left", Native::Key(1)),
    ("right", Native::Key(2)),
    ("up", Native::Key(3)),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("start", "start"),
    ("coin", "coin"),
    ("test", "setting"),
    ("down", "setting"),
    ("left", "left"),
    ("right", "right"),
    ("service", "right"),
    ("up", "up"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

struct Board {
    /// The last PC frame.
    pc: [u8; LEN],
    coin_held: [bool; PLAYERS],
    /// Tickets and capsules still to pay.
    tickets: [u32; PLAYERS],
    capsules: [u32; PLAYERS],
    exchanges: u32,
}

static BOARD: Mutex<Board> =
    Mutex::new(Board { pc: [0; LEN], coin_held: [false; PLAYERS], tickets: [0; PLAYERS], capsules: [0; PLAYERS], exchanges: 0 });

fn sum(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |s, b| s.wrapping_add(*b))
}

/// The ADC reading of `player`'s gun (`y`: the vertical one, from the top).
fn adc(player: usize, y: bool) -> u16 {
    let (px, py) = demul::gun_of_player_in_picture(player).unwrap_or((0.5, 0.5));
    let v = if y { py } else { px };
    (ADC_SPAN * v.clamp(0.0, 1.0)).round() as u16
}

/// The board's answer to the last PC frame.
fn answer(board: &mut Board) -> [u8; LEN] {
    let mut f = [0u8; LEN];
    f[0] = 0xFA;
    let mut keys: u32 = 0;
    for player in 0..PLAYERS {
        let mut coin = false;
        map().for_each_pressed(&input(player), |n| match n {
            Native::Trigger => keys |= 1 << (2 + player),
            Native::Start => keys |= 1 << player,
            Native::Coin => coin = true,
            Native::Key(bit) if player == 0 => f[1] |= 1 << bit,
            Native::Key(_) => {}
        });
        if coin {
            keys |= 1 << (24 + player);
            if !board.coin_held[player] {
                f[2] |= 1 << player;
            }
        }
        board.coin_held[player] = coin;
        let at = 7 + player * 4;
        f[at..at + 2].copy_from_slice(&adc(player, false).to_be_bytes());
        f[at + 2..at + 4].copy_from_slice(&adc(player, true).to_be_bytes());
    }
    f[23..27].copy_from_slice(&keys.to_le_bytes());
    // the tickets and capsules asked (once per PC frame), paid one at a time
    let pc = board.pc;
    board.pc = [0; LEN];
    let clear = pc[6];
    for player in 0..PLAYERS {
        if clear & (1 << player) != 0 {
            board.tickets[player] = 0;
        }
        if clear & (1 << (4 + player)) != 0 {
            board.capsules[player] = 0;
        }
        board.tickets[player] += ((pc[8] >> (4 * player)) & 0x0F) as u32;
        board.capsules[player] += ((pc[12] >> player) & 1) as u32;
    }
    board.exchanges = board.exchanges.wrapping_add(1);
    if board.exchanges % TICKET_INTERVAL == 0 {
        for player in 0..PLAYERS {
            if board.tickets[player] > 0 {
                board.tickets[player] -= 1;
                f[3] |= 1 << player;
            }
            if board.capsules[player] > 0 {
                board.capsules[player] -= 1;
                f[5] |= 1 << player;
            }
        }
    }
    f[LEN - 1] = sum(&f[..LEN - 1]);
    f
}

/// The device object `CreateInstance` returns: the functions ignore it.
static INSTANCE: [u8; 16] = [0; 16];

/// Opens the scenario select's last stage (its IL, before its first call).
fn unlock_last_stage() {
    let Some(m) = mono::mono() else { return };
    let image = m.image("Assembly-CSharp");
    // PS_SeleteLevel.Lock: `ldarg.0 ldfld health ... ArrayEx.ForEach(...) pop` (0x0D..0x2A)
    // becomes nops
    let lock = m.il_code(m.method(m.class(image, "", "PS_SeleteLevel"), "Lock", 1));
    // PS_Selete.StateStart: `ldc.i4.4 ldelem.ref ldc.i4.1 callvirt Lock` -> `ldc.i4.0`
    let start = m.il_code(m.method(m.class(image, "", "PS_Selete"), "StateStart", 0));
    let (Some(lock), Some(start)) = (lock, start) else {
        log!("unrealio: no scenario select to unlock");
        return;
    };
    let colliders = 0x0D..0x2A;
    let at = start.windows(4).position(|w| w == [0x1A, 0x9A, 0x17, 0x6F]);
    match at {
        Some(at) if lock.len() == 0x61 && lock[0x0D..0x0F] == [0x02, 0x7B] && lock[0x29] == 0x26 => {
            lock[colliders].fill(0x00);
            start[at + 2] = 0x16;
            log!("unrealio: last stage unlocked");
        }
        _ => log!("unrealio: scenario select's code not the expected one, last stage locked"),
    }
}

extern "C" fn create_instance(com: *mut *const u8) -> i32 {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(unlock_last_stage);
    if !com.is_null() {
        unsafe { *com = INSTANCE.as_ptr() };
    }
    1
}

extern "C" fn open(_com: *const u8) -> i32 {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| log!("unrealio: board opened"));
    1
}

/// `Close`, `Release`, `SetReadTimeout`: any arguments (cdecl: the caller pops them).
extern "C" fn ignored() {}

extern "C" fn write(_com: *const u8, buffer: *const u8, length: u8) {
    if buffer.is_null() || length as usize != LEN {
        return;
    }
    let data = unsafe { std::slice::from_raw_parts(buffer, LEN) };
    if data[0] == 0xF5 && sum(&data[..LEN - 1]) == data[LEN - 1] {
        BOARD.lock().unwrap().pc.copy_from_slice(data);
    }
}

/// 0 when `buffer` holds the board's frame; the updater's bootloader frames (64 bytes) fail.
extern "C" fn read(_com: *const u8, buffer: *mut u8, length: u8) -> i32 {
    if buffer.is_null() || length as usize != LEN {
        return -1;
    }
    let f = answer(&mut BOARD.lock().unwrap());
    unsafe { std::ptr::copy_nonoverlapping(f.as_ptr(), buffer, LEN) };
    0
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    if module != "unrealhid.dll" {
        return None;
    }
    Some(match name {
        "CreateInstance" => create_instance as *const () as usize,
        "Open" => open as *const () as usize,
        "Read" => read as *const () as usize,
        "Write" => write as *const () as usize,
        _ => ignored as *const () as usize,
    })
}

pub fn init() {
    mono_imports::hook(resolve);
    log!("unrealio: UnrealIO USB board");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames() {
        let mut board = Board { pc: [0; LEN], coin_held: [false; PLAYERS], tickets: [0; PLAYERS], capsules: [0; PLAYERS], exchanges: 0 };
        board.pc[8] = 0x21;
        let f = answer(&mut board);
        assert_eq!(f[0], 0xFA);
        assert_eq!(f[LEN - 1], sum(&f[..LEN - 1]));
        assert_eq!((board.tickets[0], board.tickets[1]), (1, 2));
    }
}
