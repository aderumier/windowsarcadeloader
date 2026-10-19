//! Wahlap USB I/O board: the `UsbIO_*` functions exported by `usb_io.dll` (cdecl, MSVC names
//! in `usb_io.def`).
//!
//! `UsbIO_InitSystem(0)` opens the board (1, else -3 when missing: the game then reads the
//! keyboard and XInput). `UsbIO_GetInputdata(&rec)` fills a 12-byte record:
//!
//! | offset | content |
//! |---|---|
//! | 0x00 | u32 `a`: switches, bits 0x01 0x04 0x08 0x10 |
//! | 0x04 | u32 `b`: bit 0x01, 0x04 (4th analog below the boost threshold), 0x08, 0x10 |
//! | 0x08..0x0B | analogs 0-3, 8 bits: wheel, gas, brake, 4th (boost lever) |
//!
//! The game (`0x49f790`) turns it into its button word `a | (b & 0x18) << 2 | (b & 4) << 6 |
//! (b & 1) << 9` and the analogs into `value / 256`. Its bindings (`CInput::AddButtonPad`):
//! race 0x01 / 0x02 (start, never set by the real board) / 0x04 (the coin switch: credits);
//! menus up 0x20, down 0x40,
//! enter 0x02, back 0x100; 0x100 + 0x20 held then released = the operator menu.
//! The natives below are named after those game bits.
//!
//! Coin counter (`coin_counter`, not credited by this build): one 8-bit counter on the board, `UsbIO_ReadSysCoins(slot)` returns it (any slot),
//! `UsbIO_DecCoin(slot, n)` takes `n` off (a credit used), `UsbIO_CleanCoin` empties it (at
//! boot). The board's memory (`UserControlMemoryRead/Write`) is not implemented by the real
//! driver either (0: the game keeps its data in files).
//!
//! Steering motor (`UsbIO_SetSteeringStatu`), road and car state: force feedback, `ffb.rs`.

use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

use wal_payload_common::{log, mapping::ButtonMap};
use wal_protocol::{Axis, StickState};

/// Native inputs: game button bits, plus the coin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Native {
    /// Game button word bit.
    Bit(u32),
    Coin,
    /// A key of the game's own PC keyboard (virtual key code), see `keys.rs`.
    Key(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("btn1", Native::Bit(0x001)),
    ("start", Native::Bit(0x002)),
    // the coin switch (user: credits)
    ("coin", Native::Bit(0x004)),
    ("btn4", Native::Bit(0x008)),
    ("btn5", Native::Bit(0x010)),
    ("up", Native::Bit(0x020)),
    ("down", Native::Bit(0x040)),
    ("boost", Native::Bit(0x100)),
    ("btn10", Native::Bit(0x200)),
    ("coin_counter", Native::Coin),
    // the same bits by value (Storm Rider's bindings differ)
    ("bit01", Native::Bit(0x001)),
    ("bit02", Native::Bit(0x002)),
    ("bit04", Native::Bit(0x004)),
    ("bit08", Native::Bit(0x008)),
    ("bit10", Native::Bit(0x010)),
    ("bit20", Native::Bit(0x020)),
    ("bit40", Native::Bit(0x040)),
    ("bit100", Native::Bit(0x100)),
    ("bit200", Native::Bit(0x200)),
    ("key_enter", Native::Key(0x0D)),
    ("key_space", Native::Key(0x20)),
    ("key_backspace", Native::Key(0x08)),
    ("key_left", Native::Key(0x25)),
    ("key_up", Native::Key(0x26)),
    ("key_right", Native::Key(0x27)),
    ("key_down", Native::Key(0x28)),
    ("key_b", Native::Key(0x42)),
    ("key_e", Native::Key(0x45)),
    ("key_q", Native::Key(0x51)),
    ("key_w", Native::Key(0x57)),
];

/// Virtual stick -> board, overridden by the profile `native_map`.
const DEFAULT_MAP: &[(&str, &str)] = &[
    // the game's own Enter starts (the board's start bit is never set by the real board)
    ("start", "start+key_enter"),
    ("b1", "boost"),
    ("b2", "btn1"),
    ("b4", "btn4"),
    ("b5", "btn5"),
    ("b6", "btn10"),
    ("up", "up"),
    ("down", "down"),
    ("test", "up+boost"),
    ("coin", "coin"),
];

static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
static COINS: AtomicU32 = AtomicU32::new(0);
static COIN_HELD: AtomicBool = AtomicBool::new(false);
/// Game keys held, by virtual key code.
static KEYS_HELD: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static LAST_BUTTONS: AtomicU32 = AtomicU32::new(u32::MAX);
static STEERING: [AtomicI32; 1] = [const { AtomicI32::new(i32::MIN) }; 1];

pub(crate) fn init() {
    map();
    // the board counts the coins by itself
    std::thread::spawn(|| {
        loop {
            let mut coin = false;
            let mut keys = Vec::new();
            map().for_each_pressed(&wal_payload_common::input(0), |n| match n {
                Native::Coin => coin = true,
                Native::Key(vk) if !keys.contains(&vk) => keys.push(vk),
                _ => {}
            });
            let mut held = KEYS_HELD.lock().unwrap();
            for &vk in keys.iter().filter(|vk| !held.contains(vk)) {
                crate::keys::key(vk, true);
            }
            for &vk in held.iter().filter(|vk| !keys.contains(vk)) {
                crate::keys::key(vk, false);
            }
            *held = keys;
            drop(held);
            if coin != COIN_HELD.swap(coin, Ordering::Relaxed) && coin {
                let total = COINS.try_update(Ordering::Relaxed, Ordering::Relaxed, |c| Some((c + 1).min(255))).unwrap() + 1;
                log!("usbio: coin ({total})");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    });
}

fn map() -> &'static ButtonMap<Native> {
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP))
}

/// Game button word -> the board's `a` and `b` words.
fn encode(bits: u32) -> (u32, u32) {
    let a = bits & 0x1f;
    let b = (bits >> 2 & 0x18) | (bits >> 6 & 0x04) | (bits >> 9 & 0x01);
    (a, b)
}

/// Game button bits pressed by player 1 (`Native::Bit`), logged on change.
pub(crate) fn pressed_bits() -> u32 {
    let stick: StickState = wal_payload_common::input(0);
    let mut bits = 0;
    map().for_each_pressed(&stick, |n| {
        if let Native::Bit(b) = n {
            bits |= b
        }
    });
    if LAST_BUTTONS.swap(bits, Ordering::Relaxed) != bits {
        log!("wahlap: buttons {bits:#05x}");
    }
    bits
}

/// The board's coin counter (8 bits, emulated: coins added by `coin_counter`).
pub(crate) fn coins() -> u32 {
    COINS.load(Ordering::Relaxed)
}

pub(crate) fn take_coins(n: u32) -> u32 {
    let left = COINS.try_update(Ordering::Relaxed, Ordering::Relaxed, |c| Some(c.saturating_sub(n))).unwrap();
    left.saturating_sub(n)
}

pub(crate) fn clear_coins() {
    COINS.store(0, Ordering::Relaxed);
}

/// Game button bits -> the board's `a` and `b` words.
pub(crate) fn split(bits: u32) -> (u32, u32) {
    encode(bits)
}

/// The input record from player 1.
fn record() -> [u8; 12] {
    let stick: StickState = wal_payload_common::input(0);
    let bits = pressed_bits();
    let (a, b) = encode(bits);
    let mut rec = [0u8; 12];
    rec[0..4].copy_from_slice(&a.to_le_bytes());
    rec[4..8].copy_from_slice(&b.to_le_bytes());
    rec[8] = stick.axis_u8(Axis::LeftX);
    rec[9] = stick.axis_u8(Axis::Accel);
    rec[10] = stick.axis_u8(Axis::Brake);
    // the boost lever: released high, pressed under the game's threshold
    rec[11] = if bits & 0x100 != 0 { 0 } else { 255 };
    rec
}

#[unsafe(no_mangle)]
pub extern "C" fn usbio_init_system(device: i32) -> i32 {
    log!("usbio: InitSystem({device})");
    if device == 0 { 1 } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn usbio_exit_system() {
    log!("usbio: ExitSystem");
}

#[unsafe(no_mangle)]
pub extern "C" fn usbio_check_connection() -> bool {
    true
}

/// `int UsbIO_GetInputdata(UsbIOBoardInputData_s &)`: 1, the record filled.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usbio_get_input_data(rec: *mut [u8; 12]) -> i32 {
    if rec.is_null() {
        return 0;
    }
    unsafe { rec.write_unaligned(record()) };
    1
}

/// `int UsbIO_ReadSysCoins(unsigned slot)`: the board's coin counter.
#[unsafe(no_mangle)]
pub extern "C" fn usbio_read_sys_coins(_slot: u32) -> i32 {
    coins() as i32
}

/// `int UsbIO_DecCoin(unsigned short slot, unsigned short n)`
#[unsafe(no_mangle)]
pub extern "C" fn usbio_dec_coin(slot: u16, n: u16) -> i32 {
    log!("usbio: DecCoin({slot}, {n}): {} left", take_coins(n as u32));
    0
}

/// `int UsbIO_CleanCoin(unsigned short slot)`
#[unsafe(no_mangle)]
pub extern "C" fn usbio_clean_coin(_slot: u16) -> i32 {
    clear_coins();
    0
}

/// `int UsbIO_SetSteeringStatu(UsbIOBoardSteeringStatu &)`: the steering motor's state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usbio_set_steering_status(status: *const [i32; 6]) -> i32 {
    if status.is_null() {
        return 0;
    }
    let s = unsafe { status.read_unaligned() };
    if STEERING[0].swap(s[0], Ordering::Relaxed) != s[0] {
        log!("usbio: steering mode {s:?}");
    }
    crate::ffb::steering(s[2]);
    1
}

/// `void UsbIO_OutputCarStatuData(UsbIOBoardCarStatuData_s &)`: {speed 0..=1, gear, ..} (floats).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn usbio_car_status(status: *const f32) {
    if !status.is_null() {
        crate::ffb::speed(unsafe { status.read_unaligned() });
    }
}

/// `void UsbIO_SetRoadType(int)`: 0 road, 1 grass, 2 rough ground, 3 rumble strip.
#[unsafe(no_mangle)]
pub extern "C" fn usbio_set_road_type(t: i32) {
    crate::ffb::road_type(t);
}

#[unsafe(no_mangle)]
pub extern "C" fn usbio_set_motor_volt(volt: i32) {
    let _ = volt; // the motor voltage the test menu sweeps: unused
}

#[unsafe(no_mangle)]
pub extern "C" fn usbio_set_steer_center(center: i32) {
    log!("usbio: SetSteerCenter({center})");
}

#[unsafe(no_mangle)]
pub extern "C" fn usbio_board_id() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn usbio_version_h() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn usbio_version_l() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbio_zero() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn usbio_one() -> i32 {
    1
}

/// Outputs not emulated (lamps, seat, road effects): ignored.
#[unsafe(no_mangle)]
pub extern "C" fn usbio_ignore() {}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's conversion of the board words (`0x49f790`).
    fn game_word(a: u32, b: u32) -> u32 {
        a | (b & 0x18) << 2 | (b & 4) << 6 | (b & 1) << 9
    }

    #[test]
    fn encodes_game_bits() {
        for bit in [0x1, 0x2, 0x4, 0x8, 0x10, 0x20, 0x40, 0x100, 0x200] {
            let (a, b) = encode(bit);
            assert_eq!(game_word(a, b), bit, "bit {bit:#x}");
        }
    }
}

