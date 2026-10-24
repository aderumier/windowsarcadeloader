//! The HBHI I/O board (ACIO device 5).
//!
//! `ac_io_hbhi_get_control_status_buffer` fills the game's 0x40-byte status: the board's 15-byte
//! status packet at +4 and what libacio derives from it (+0x14: 8 analog channels of 10 bits,
//! left-aligned; +0x24/+0x25: the 6-bit coin counters of slots 1 and 2; +0x28: the packet
//! count; +0x38: the packet time, which the game watches: unchanged for 20 s is an I/O error).
//!
//! The packet: analog channel n's high byte and its 2 low bits in the next byte's top bits for
//! channels 0-3 (bytes 0-7), whose remaining 6 bits carry the switches (bytes 1 and 3) and the
//! coin counters (5 and 7); byte 8 more switches; byte 9 the low bits of channels 4-7, whose
//! high bytes are 10-13. The game reads controller 1's x/y/z from channels 0-2, controller 2's
//! from 3-5, and its switches as two joysticks (gs-system): the system one (P1) and P2.
//!
//! Native inputs for `native_map`: `test service start trigger button2 coin`.

use std::ffi::c_char;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use wal_payload_common::log;
use wal_payload_common::mapping::ButtonMap;
use wal_protocol::Axis;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Start,
    Trigger,
    Button2,
    Coin,
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("start", Native::Start),
    ("trigger", Native::Trigger),
    ("button2", Native::Button2),
    ("coin", Native::Coin),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("start", "start"),
    ("coin", "coin"),
    ("b1", "trigger"),
    ("b2", "button2"),
];

/// A switch: (packet byte, bit) for player 1 and player 2 (the test menu's INPUT CHECK and
/// gs-system's joystick decoding). The coin is its slot's coin mech switch plus a coin counter
/// step; the trigger is the gun's switch plus its analog pull (z).
fn switch(native: Native, player: usize) -> Option<(usize, u8)> {
    Some(match (native, player) {
        (Native::Test, _) => (1, 0x20),
        (Native::Service, _) => (1, 0x10),
        (Native::Coin, 0) => (1, 0x04),
        (Native::Coin, _) => (1, 0x02),
        (Native::Start, 0) => (8, 0x01),
        (Native::Start, _) => (8, 0x02),
        (Native::Trigger, 0) => (3, 0x02),
        (Native::Trigger, _) => (3, 0x01),
        (Native::Button2, _) => (3, 0x20),
    })
}

/// The guns' potentiometer ranges (min, max of x, y, z) of the game's default calibration
/// (gs_setting::ANALOGVOL): the pointer is scaled onto them, so the guns aim at the whole
/// screen without a calibration (and a test menu calibration with the pointer keeps it).
const GUN_RANGES: [[(u16, u16); 3]; 2] = [
    [(0x4800, 0xe800), (0x0a00, 0x6e00), (0x1800, 0x4800)],
    [(0x2000, 0xc000), (0x2600, 0x8a00), (0x5800, 0x8800)],
];

struct Board {
    maps: [ButtonMap<Native>; 2],
    /// Coin counters of slots 1 and 2 (6 bits on the wire) and whether the coin is held.
    coins: [u8; 2],
    coin_held: [bool; 2],
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);
/// Coins inserted and not yet taken by the game (`ac_io_hbhi_current_coinstock`).
static COIN_STOCK: AtomicU32 = AtomicU32::new(0);
static PACKETS: AtomicU32 = AtomicU32::new(0);
static CLOCK: AtomicU64 = AtomicU64::new(0);

/// The board's current status packet.
fn packet() -> [u8; 15] {
    if crate::gti_club::active() {
        return crate::gti_club::packet();
    }
    if crate::rf3d::active() {
        return crate::rf3d::packet();
    }
    let mut guard = BOARD.lock().unwrap();
    let board = guard.get_or_insert_with(|| Board {
        maps: [ButtonMap::new(NATIVES, DEFAULT_MAP), ButtonMap::with_defaults(NATIVES, DEFAULT_MAP)],
        coins: [0; 2],
        coin_held: [false; 2],
    });
    let mut raw = [0u8; 15];
    // analog channels: (high byte, low 2 bits) of each player's x, y, z
    let mut analog = [0u16; 8];
    for player in 0..2 {
        let stick = wal_payload_common::input(player);
        let mut trigger = false;
        board.maps[player].for_each_pressed(&stick, |n| trigger |= n == Native::Trigger);
        for (i, (min, max)) in GUN_RANGES[player].into_iter().enumerate() {
            // x, y: the pointer (-1..1 left/top to right/bottom; the y pot reads the bottom
            // of the screen at its minimum); z: the trigger pull
            let t = match i {
                0 => (stick.axis(Axis::LeftX) as f32 + 32768.0) / 65535.0,
                1 => (32767.0 - stick.axis(Axis::LeftY) as f32) / 65535.0,
                _ => if trigger { 1.0 } else { 0.0 },
            };
            analog[player * 3 + i] = (min as f32 + t * (max - min) as f32) as u16 & 0xffc0;
        }
        let mut coin = false;
        board.maps[player].for_each_pressed(&stick, |n| {
            if n == Native::Coin {
                coin = true;
            }
            if let Some((byte, bit)) = switch(n, player) {
                raw[byte] |= bit;
            }
        });
        if coin && !board.coin_held[player] {
            coin_inserted();
            board.coins[player] = board.coins[player].wrapping_add(1) & 0x3f;
        }
        board.coin_held[player] = coin;
    }
    for ch in 0..4 {
        raw[ch * 2] = (analog[ch] >> 8) as u8;
        raw[ch * 2 + 1] |= (analog[ch] & 0xc0) as u8;
    }
    raw[5] |= board.coins[0];
    raw[7] |= board.coins[1];
    for ch in 4..8 {
        raw[10 + ch - 4] = (analog[ch] >> 8) as u8;
        raw[9] |= ((analog[ch] & 0xc0) >> (2 * (ch - 4))) as u8;
    }
    raw
}

/// A new status packet is always there (the board answers every poll).
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_update_control_status_buffer() -> bool {
    PACKETS.fetch_add(1, Ordering::Relaxed);
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_get_control_status_buffer(out: *mut u8) {
    if out.is_null() {
        return;
    }
    let raw = packet();
    let mut buf = [0u8; 0x40];
    buf[4..19].copy_from_slice(&raw);
    let analog = [
        u16::from_be_bytes([raw[0], raw[1] & 0xc0]),
        u16::from_be_bytes([raw[2], raw[3] & 0xc0]),
        u16::from_be_bytes([raw[4], raw[5] & 0xc0]),
        u16::from_be_bytes([raw[6], raw[7] & 0xc0]),
        u16::from_be_bytes([raw[10], raw[9] & 0xc0]),
        u16::from_be_bytes([raw[11], (raw[9] & 0x30) << 2]),
        u16::from_be_bytes([raw[12], (raw[9] & 0x0c) << 4]),
        u16::from_be_bytes([raw[13], (raw[9] & 0x03) << 6]),
    ];
    for (i, v) in analog.into_iter().enumerate() {
        buf[0x14 + i * 2..0x16 + i * 2].copy_from_slice(&v.to_le_bytes());
    }
    buf[0x24] = raw[5] & 0x3f;
    buf[0x25] = raw[7] & 0x3f;
    buf[0x28..0x2c].copy_from_slice(&PACKETS.load(Ordering::Relaxed).to_le_bytes());
    let clock = CLOCK.fetch_add(1, Ordering::Relaxed) + 1;
    buf[0x38..0x40].copy_from_slice(&clock.to_le_bytes());
    unsafe { std::ptr::copy_nonoverlapping(buf.as_ptr(), out, buf.len()) };
}

/// A coin went in (a press of a slot's coin control).
pub(crate) fn coin_inserted() {
    COIN_STOCK.fetch_add(1, Ordering::Relaxed);
}

/// The coins the board counted since the game last took them (GTI Club).
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_current_coinstock(_slot: u32, stock: *mut u32) -> bool {
    if !stock.is_null() {
        unsafe { *stock = COIN_STOCK.load(Ordering::Relaxed) };
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_consume_coinstock(_slot: u32, coins: u32) -> bool {
    let _ = COIN_STOCK.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| Some(n.saturating_sub(coins)));
    true
}

// The security plugs (software plug: the game's id, GLH24JAA; system plug: the cabinet's).
// Checks complete at once: `0x12` a plug found, `8` the missing-plug check passed.

const PLUG_FOUND: u32 = 0x12;
const PLUGS_PRESENT: u32 = 8;
static PLUG_STATUS: AtomicU32 = AtomicU32::new(PLUG_FOUND);

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_req_secplug_check_softwareplug(id: *const c_char) -> bool {
    if !id.is_null() {
        log!("hbhi: software plug {} present", unsafe { std::ffi::CStr::from_ptr(id) }.to_string_lossy());
    }
    PLUG_STATUS.store(PLUG_FOUND, Ordering::Relaxed);
    true
}

/// The software plug check of GTI Club (the plug's id, e.g. `GKHGTEAA`).
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_req_secplug_check(id: *const c_char) -> bool {
    ac_io_hbhi_req_secplug_check_softwareplug(id)
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_req_secplug_check_systemplug() -> bool {
    PLUG_STATUS.store(PLUG_FOUND, Ordering::Relaxed);
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_req_secplug_check_isfinished(status: *mut u32) -> bool {
    if !status.is_null() {
        unsafe { *status = PLUG_STATUS.load(Ordering::Relaxed) };
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_req_secplug_missing_check() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_req_secplug_missing_check_isfinished(status: *mut u32) -> bool {
    if !status.is_null() {
        unsafe { *status = PLUGS_PRESENT };
    }
    true
}

/// A plug's id as libacio formats it: `0140` and its 8 bytes in hex (21 bytes with the NUL).
fn plug_id(out: *mut c_char, id: [u8; 8]) -> bool {
    if out.is_null() {
        return false;
    }
    let mut text = String::from("0140");
    for b in id {
        text.push_str(&format!("{b:02X}"));
    }
    let bytes = text.as_bytes();
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), out.cast::<u8>(), bytes.len());
        *out.add(bytes.len()) = 0;
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_get_softwareid(out: *mut c_char) -> bool {
    plug_id(out, *b"GLH24JAA")
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_get_systemid(out: *mut c_char) -> bool {
    plug_id(out, [0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef])
}

// Coin counters, coin blockers and lamps: outputs without a device here.

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_lock_coincounter(_slot: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_unlock_coincounter(_slot: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_control_coin_blocker_open(_slot: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_control_coin_blocker_close(_slot: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_control_lamp_on(_lamp: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_control_lamp_off(_lamp: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_control_lamp_bright(_lamps: u32, _value: u8) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_control_lamp_mode(_mode: u32) -> bool {
    true
}

/// The board's watchdog never fired.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_get_watchdog_status() -> bool {
    true
}

// The rest of the board's functions Road Fighters 3D looks up in its HGTH bus library
// (libacioEx.dll, the payload): never used without the hardware, answered.

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_add_coin(_slot: u32, _coins: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_control_parallel_on(_bits: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_control_parallel_off(_bits: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_control_reset() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_set_framing_err_packet_send_interval(_interval: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_req_volume_control(_a: u32, _b: u32, _c: u32, _d: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_req_volume_control_isfinished(status: *mut u32) -> bool {
    if !status.is_null() {
        unsafe { *status = 0 };
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_set_watchdog_time(_time: u16) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_watchdog_off() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_get_watchdog_time_min() -> u16 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_get_watchdog_time_now() -> u16 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_req_get_control_status() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_req_coin_input_wave() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_get_coin_input_wave_buffer(_out: *mut u8) -> bool {
    false
}

/// The card dispenser: none (every request fails).
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_req_carddispenser_init() -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_req_carddispenser_init_isfinished(_status: *mut u32) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_req_carddispenser_get_status() -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_req_carddispenser_get_status_isfinished(_status: *mut u32) -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_req_carddispenser_disburse() -> bool {
    false
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hbhi_req_carddispenser_disburse_isfinished(_status: *mut u32) -> bool {
    false
}

// The HDXS board (cabinet LEDs): not in this cabinet; its status is empty.

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hdxs_update_control_status_buffer() -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hdxs_get_control_status_buffer(out: *mut u8) {
    if !out.is_null() {
        unsafe { std::ptr::write_bytes(out, 0, 0x40) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hdxs_led_set_pattern(_a: u32, _b: u32, _c: u32, _d: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hdxs_led_set_rgb_mask(_a: u32, _b: u32, _c: u32, _d: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hdxs_led_scroll(_a: u32, _b: u32, _c: u32, _d: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_hdxs_set_framing_err_packet_send_interval(_interval: u32) -> bool {
    true
}
