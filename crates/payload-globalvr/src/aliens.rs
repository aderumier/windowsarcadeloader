//! Aliens Extermination (v1.03 US): guns and panel buttons from the virtual sticks,
//! `WAL_GLOBALVR_ALIENS=1`.
//!
//! The game reads its GVRIO board over HID itself (no `USBIOExtreme.dll`), once per frame and
//! player, in `0x448810(record, player)`:
//!
//! * `0x42ee60(gun, record)`: the gun (GVRIO HID) -> `record[0]` buttons (0x10 trigger, 0x20
//!   special weapon, 0x40 flamethrower; DemulShooter sends its off screen trigger there),
//!   `record + 0x1ec` / `+ 0x1f0` X / Y (0..65535), returns 1. Replaced by [`gun`]. The player
//!   records are at `0x596acbc` and `0x596afd4` (DemulShooter's `Game_GvrAliens.cs`: same
//!   addresses, which pokes them from outside after NOPing the writes of this function).
//!   The game reloads on a shot off screen: a position outside its calibrated range.
//!   `offscreen` (a lightgun pointing off the screen) sends such a position, `reload` (mice,
//!   pads) also pulls the trigger.
//! * `0x42f790(player, record)`: the panel buttons, player 1's record only, from the PC
//!   keyboard (`GetAsyncKeyState`, a table of 11 keys and bits at `0x85edf0`, numeric keypad
//!   keys at `0x85ee48` in another setting). Its `GetAsyncKeyState` call (`0x42f7c7`) goes to
//!   [`key_state`]: the keys of the table's bits from the virtual sticks.
//!
//! | key | bit | game | native (default virtual stick) |
//! |---|---|---|---|
//! | 1 / 2 | 0x10000 / 0x20000 | start players 1 / 2 | `start` (start) |
//! | 8 | 0x10000000 | held (timed) | `key8` |
//! | F1 / F2 | 0x100000 / 0x200000 | test menu, select in it | `keyf1` (test, service) / `keyf2` (b4) |
//! | 9 / 0 | 0x400000 / 0x800000 | volume up / down, test menu up / down (repeat) | `key9` (up) / `key0` (down) |
//! | left Ctrl / Shift / Alt / Win | 0x1000000 << n | | `keyctrl`, `keyshift`, `keyalt`, `keywin` |
//!
//! Coins are not keys: the GVRIO thread queues them, a count per coin slot (4 slots, 0x18
//! bytes each at `0x5279f80`) the panel function adds as credits (`0x4134a0`). `coin` of
//! player N adds one to slot N.

use std::sync::OnceLock;

use wal_payload_common::{log, mapping::ButtonMap};
use wal_protocol::{Axis, StickState};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// Function and the first bytes expected there (other build: left alone).
const GUN: (usize, &[u8]) = (0x42ee60, &[0x8b, 0x44, 0x24, 0x04, 0x50, 0xe8]);
/// `call [GetAsyncKeyState]` of the panel function.
const KEY_CALL: (usize, &[u8]) = (0x42f7c7, &[0xff, 0x15, 0xb0, 0x92, 0x98, 0x05]);
/// Key tables: (virtual key, bit) pairs, 8 bytes each.
const KEY_TABLES: [usize; 2] = [0x85edf0, 0x85ee48];
const KEYS: usize = 11;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Native {
    Trigger,
    Action,
    Flame,
    Offscreen,
    Reload,
    Start,
    Coin,
    /// A key of the panel table, by its bit.
    Key(u32),
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("special", Native::Action),
    ("flame", Native::Flame),
    ("offscreen", Native::Offscreen),
    ("reload", Native::Reload),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("key8", Native::Key(0x10000000)),
    ("keyf1", Native::Key(0x100000)),
    ("keyf2", Native::Key(0x200000)),
    ("key9", Native::Key(0x400000)),
    ("key0", Native::Key(0x800000)),
    ("keyctrl", Native::Key(0x1000000)),
    ("keyshift", Native::Key(0x2000000)),
    ("keyalt", Native::Key(0x4000000)),
    ("keywin", Native::Key(0x8000000)),
];

/// Virtual stick -> game, overridden by the profile `native_map`.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("b2", "special"),
    ("b3", "flame"),
    ("b5", "reload"),
    ("b6", "offscreen"),
    ("start", "start"),
    ("coin", "coin"),
    ("test", "keyf1"),
    ("service", "keyf1"),
    ("b4", "keyf2"),
    ("up", "key9"),
    ("down", "key0"),
];

/// Coin queue of the GVRIO thread: a count per slot, 0x18 bytes apart.
const COIN_QUEUE: usize = 0x5279f80;
static COIN_HELD: [std::sync::atomic::AtomicBool; 2] = [const { std::sync::atomic::AtomicBool::new(false) }; 2];

static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();

fn map() -> &'static ButtonMap<Native> {
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP))
}

pub(crate) fn init() {
    if std::env::var("WAL_GLOBALVR_ALIENS").map_or(true, |v| v != "1") {
        return;
    }
    map();
    let ok = redirect(GUN, gun as *const () as usize) && call(KEY_CALL, key_state as *const () as usize);
    log!("aliens: {}", if ok { "guns and panel from the virtual sticks" } else { "not this build, left alone" });
}

/// Replaces the function at `site` by `mov eax, target; jmp eax`, if its bytes are the expected ones.
fn redirect((site, expected): (usize, &[u8]), target: usize) -> bool {
    let at = site as *mut u8;
    if unsafe { std::slice::from_raw_parts(at, expected.len()) } != expected {
        log!("aliens: unexpected code at {site:#x}");
        return false;
    }
    let mut code = [0xB8, 0, 0, 0, 0, 0xFF, 0xE0];
    code[1..5].copy_from_slice(&(target as u32).to_le_bytes());
    let mut old = 0;
    unsafe {
        VirtualProtect(at as _, code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), at, code.len());
        VirtualProtect(at as _, code.len(), old, &mut old);
    }
    true
}

/// Replaces the 6-byte indirect call at `site` by `call target; nop`.
fn call((site, expected): (usize, &[u8]), target: usize) -> bool {
    let at = site as *mut u8;
    if unsafe { std::slice::from_raw_parts(at, expected.len()) } != expected {
        log!("aliens: unexpected code at {site:#x}");
        return false;
    }
    let mut code = [0xE8, 0, 0, 0, 0, 0x90];
    code[1..5].copy_from_slice(&(target.wrapping_sub(site + 5) as u32).to_le_bytes());
    let mut old = 0;
    unsafe {
        VirtualProtect(at as _, code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), at, code.len());
        VirtualProtect(at as _, code.len(), old, &mut old);
    }
    true
}

/// Virtual stick axis (-32768..=32767) -> 0..65535.
fn axis(v: i16) -> i32 {
    v as i32 + 32768
}

/// `int gun(int gun, u32 *record)` (cdecl): the gun's buttons and position, 1 (gun present).
unsafe extern "C" fn gun(index: i32, record: *mut u32) -> i32 {
    if !(0..2).contains(&index) || record.is_null() {
        return 0;
    }
    let stick: StickState = wal_payload_common::input(index as usize);
    let mut buttons = 0;
    let mut offscreen = false;
    map().for_each_pressed(&stick, |n| match n {
        Native::Trigger => buttons |= 0x10,
        Native::Action => buttons |= 0x20,
        Native::Flame => buttons |= 0x40,
        Native::Offscreen => offscreen = true,
        Native::Reload => {
            offscreen = true;
            buttons |= 0x10;
        }
        _ => {}
    });
    // far outside the calibrated range (0..65535 spans the screen)
    let (x, y) = if offscreen { (-65536, -65536) } else { (axis(stick.axis(Axis::LeftX)), axis(stick.axis(Axis::LeftY))) };
    unsafe {
        *record = buttons;
        *(record as *mut u8).add(0x1ec).cast::<i32>() = x;
        *(record as *mut u8).add(0x1f0).cast::<i32>() = y;
    }
    1
}

/// `GetAsyncKeyState(vk)` of the panel function: 0x8000 when the bit of `vk` in the game's
/// key tables is pressed on the virtual sticks.
unsafe extern "system" fn key_state(vk: i32) -> i16 {
    let bit = KEY_TABLES.iter().flat_map(|t| (0..KEYS).map(move |i| t + 8 * i)).find_map(|e| unsafe {
        (*(e as *const u8) as i32 == vk).then(|| *((e + 4) as *const u32))
    });
    let Some(bit) = bit else { return 0 };
    let mut pressed = 0u32;
    for p in 0..2 {
        let mut coin = false;
        map().for_each_pressed(&wal_payload_common::input(p), |n| match n {
            Native::Start => pressed |= 0x10000 << p,
            Native::Key(b) => pressed |= b,
            Native::Coin => coin = true,
            _ => {}
        });
        // a coin on the press: one more in the player's slot (called in the panel function,
        // the thread that empties the queue)
        if coin != COIN_HELD[p].swap(coin, std::sync::atomic::Ordering::Relaxed) && coin {
            unsafe { *((COIN_QUEUE + 0x18 * p) as *mut u32) += 1 };
            log!("aliens: coin {}", p + 1);
        }
    }
    if pressed & bit != 0 { i16::MIN } else { 0 }
}
