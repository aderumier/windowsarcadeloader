//! Global VR USBIO board: the `CUSBIO` class exported by `USBIOExtreme.dll`.
//!
//! The game keeps an 8-byte `CUSBIO` object: `{u32 device count, record *records}`. `Init`
//! fills it and returns non-zero when a board is found (else the game falls back to
//! DirectInput guns), `Update(i)` refreshes the 0x34-byte input record `i`, which the game
//! reads directly. Far Cry Paradise Lost only reads record 0, one board for both guns:
//!
//! | offset | content |
//! |---|---|
//! | 0x06 / 0x07 | gun 1 Y / X, 8 bits (0 on both = gun missing) |
//! | 0x08 / 0x09 | gun 2 Y / X |
//! | 0x0E | bit 0/1 gun 1/2 grenade, bit 2/3 panel buttons 1/2 |
//! | 0x0F | bit 0/1 panel buttons 3/4, bit 2/3 gun 1/2 trigger |
//! | 0x10 / 0x14 | coin counters 1 / 2 (u32, total since boot) |
//!
//! The methods are exported under their MSVC names by `USBIOExtreme.def` (thiscall: `this`
//! in ECX).
//!
//! The gun axes go through the game's calibration (`arcade_calibration.txt`: min, max for gun
//! 1 X, gun 1 Y, gun 2 X, gun 2 Y): the axes are sent as 1..=255 (0 would read as a missing
//! gun), so the profile installs a calibration for that range.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use wal_payload_common::{log, mapping::ButtonMap};
use wal_protocol::{Axis, StickState};

const RECORD_SIZE: usize = 0x34;
const DEVICES: usize = 1;

/// Native inputs of a player's gun and of the cabinet panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Native {
    Trigger,
    Grenade,
    /// The player's start: panel button 1 (player 1) or 2 (player 2).
    Start,
    /// Panel button 3 (player 1) or 4 (player 2).
    Action,
    /// Panel button 1-4, whatever the player.
    Panel(u8),
    Coin,
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("grenade", Native::Grenade),
    ("start", Native::Start),
    ("action", Native::Action),
    ("panel1", Native::Panel(1)),
    ("panel2", Native::Panel(2)),
    ("panel3", Native::Panel(3)),
    ("panel4", Native::Panel(4)),
    ("coin", Native::Coin),
];

/// Virtual stick -> USBIO, overridden by the profile `native_map`.
const DEFAULT_MAP: &[(&str, &str)] =
    &[("b1", "trigger"), ("b2", "grenade"), ("start", "start"), ("b3", "action"), ("coin", "coin")];

static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
/// Input records handed to the game (leaked: they live as long as the game).
static RECORDS: OnceLock<usize> = OnceLock::new();
static COINS: [AtomicU32; 2] = [const { AtomicU32::new(0) }; 2];
static COIN_HELD: [AtomicBool; 2] = [const { AtomicBool::new(false) }; 2];
static LAST: [AtomicU32; 2] = [const { AtomicU32::new(u32::MAX) }; 2];

pub(crate) fn init() {
    map();
    // The board counts the coins by itself: the game does not poll it while loading.
    std::thread::spawn(|| {
        loop {
            for player in 0..2 {
                let mut coin = false;
                map().for_each_pressed(&wal_payload_common::input(player), |n| coin |= n == Native::Coin);
                if coin != COIN_HELD[player].swap(coin, Ordering::Relaxed) && coin {
                    let total = COINS[player].fetch_add(1, Ordering::Relaxed) + 1;
                    log!("usbio: coin {} ({total})", player + 1);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    });
}

fn map() -> &'static ButtonMap<Native> {
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP))
}

fn records() -> *mut u8 {
    *RECORDS.get_or_init(|| Box::leak(vec![0u8; RECORD_SIZE * DEVICES].into_boxed_slice()).as_mut_ptr() as usize)
        as *mut u8
}

/// Virtual stick axis (-32768..=32767) -> USBIO gun axis, 1..=255.
fn gun_axis(v: i16) -> u8 {
    (1 + (v as i32 + 32768) * 254 / 65535) as u8
}

/// Builds record 0 from players 1 and 2.
fn fill(rec: &mut [u8; RECORD_SIZE]) {
    let map = map();
    let (mut e, mut f) = (0u8, 0u8);
    for player in 0..2 {
        let stick: StickState = wal_payload_common::input(player);
        rec[6 + player * 2] = gun_axis(stick.axis(Axis::LeftY));
        rec[7 + player * 2] = gun_axis(stick.axis(Axis::LeftX));
        let bit = player as u8;
        map.for_each_pressed(&stick, |n| match n {
            Native::Trigger => f |= 0x04 << bit,
            Native::Grenade => e |= 0x01 << bit,
            Native::Start => e |= 0x04 << bit,
            Native::Action => f |= 0x01 << bit,
            Native::Panel(p @ 1..=2) => e |= 0x04 << (p - 1),
            Native::Panel(p) => f |= 0x01 << (p - 3),
            Native::Coin => {}
        });
    }
    rec[0x0E] = e;
    rec[0x0F] = f;
    // the game counts the coins from the counter increments
    rec[0x10..0x14].copy_from_slice(&COINS[0].load(Ordering::Relaxed).to_le_bytes());
    rec[0x14..0x18].copy_from_slice(&COINS[1].load(Ordering::Relaxed).to_le_bytes());
    // board status seen by the real driver once a board is open
    rec[0x28..0x2C].copy_from_slice(&2u32.to_le_bytes());

    let buttons = (e as u32) | (f as u32) << 8;
    if LAST[0].swap(buttons, Ordering::Relaxed) != buttons {
        log!("usbio: buttons 0x0E={e:#04x} 0x0F={f:#04x}");
    }
}

/// `CUSBIO::CUSBIO()`
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn cusbio_new(this: *mut u32) -> *mut u32 {
    unsafe {
        *this = 0;
        *this.add(1) = 0;
    }
    this
}

/// `CUSBIO::~CUSBIO()`
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn cusbio_drop(_this: *mut u32) {}

/// `int CUSBIO::CUSBIO_Init()`: number of boards found.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn cusbio_init(this: *mut u32) -> i32 {
    let rec = records();
    unsafe {
        *this = DEVICES as u32;
        *this.add(1) = rec as u32;
    }
    log!("usbio: init, {DEVICES} board, records at {rec:p}");
    DEVICES as i32
}

/// `void CUSBIO::CUSBIO_Update(int device)`
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn cusbio_update(this: *mut u32, device: i32) {
    if device != 0 {
        return;
    }
    let rec = unsafe { *this.add(1) } as *mut [u8; RECORD_SIZE];
    if !rec.is_null() {
        fill(unsafe { &mut *rec });
    }
}

/// `char *CUSBIO::CUSBIO_GetLastError(int device)`: NULL, no error.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn cusbio_last_error(_this: *mut u32, _device: i32) -> *const u8 {
    std::ptr::null()
}

/// `void CUSBIO::CUSBIO_Close()`
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn cusbio_close(_this: *mut u32) {
    log!("usbio: close");
}
