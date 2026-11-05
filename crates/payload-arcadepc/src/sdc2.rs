//! Wasteland Racers 2071's force feedback wheel board (`SDC2.dll`, `WAL_ARCADEPC_BOARD=sdc2`): the
//! payload is that library (`payload_install`, x86 build only; exports in `sdc2.def`).
//!
//! `Arcade.exe` loads it when `Inputs.xml`'s `UseForceFeedbackWheel` is 1 and reads the board
//! through cdecl calls each frame:
//!
//! * `Init(port)` 0 (-1: "Force feedback board disconnected"), then `IsConnected` 1 at each
//!   update (0: the same message, the game ends);
//! * `GetAxisData(axis)`: an axis of the board as a float, `(raw - 32767) / 32767` clamped to
//!   -1..1: 0 the wheel (right positive, used as is), 1 the gas and 2 the brake, which the game
//!   reads as `1 - value` scaled by `Inputs.xml`'s calibration (`MinAxisY`..`MaxAxisY`, the
//!   profile's 1..2: released 0, pressed -1);
//! * `GetDigitalIn()`: the 16 buttons, a bit at 0 = pressed, `Inputs.xml`'s `Button0..15`:
//!   0 boost, 1 camera, 2..5 cheats 1..4, 7 gear, 8 coin, 11 maintenance (the operator's
//!   menu), 12 stop;
//! * `GetCoin`, `SetCoin`, `ClearCoin`: the board's coin counter, the credits left (the coin
//!   button adds one; free play's buttons set it to a game's price, the start takes it);
//! * `SetForce(force)`, `SetSteeringPos(position)`: the wheel's motor, a position servo (its
//!   strength, the position it holds the wheel at), played as a constant force toward that
//!   position; `SetDGain`, `SetDigitalOut` (lamps): ignored.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log};
use std::time::Duration;

use wal_protocol::Axis;
use wal_protocol::output::{FFB_CONSTANT, FFB_MAX};

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Boost,
    Camera,
    Cheat1,
    Cheat2,
    Cheat3,
    Cheat4,
    Gear,
    Coin,
    Maintenance,
    Stop,
    Gas,
    Brake,
    SteerLeft,
    SteerRight,
}

/// The `GetDigitalIn` bit of a button.
fn button_bit(n: Native) -> Option<u16> {
    Some(match n {
        Native::Boost => 1 << 0,
        Native::Camera => 1 << 1,
        Native::Cheat1 => 1 << 2,
        Native::Cheat2 => 1 << 3,
        Native::Cheat3 => 1 << 4,
        Native::Cheat4 => 1 << 5,
        Native::Gear => 1 << 7,
        Native::Coin => 1 << 8,
        Native::Maintenance => 1 << 11,
        Native::Stop => 1 << 12,
        Native::Gas | Native::Brake | Native::SteerLeft | Native::SteerRight => return None,
    })
}

const NATIVES: &[(&str, Native)] = &[
    ("boost", Native::Boost),
    ("camera", Native::Camera),
    ("cheat1", Native::Cheat1),
    ("cheat2", Native::Cheat2),
    ("cheat3", Native::Cheat3),
    ("cheat4", Native::Cheat4),
    ("gear", Native::Gear),
    ("coin", Native::Coin),
    ("maintenance", Native::Maintenance),
    ("stop", Native::Stop),
    ("gas", Native::Gas),
    ("brake", Native::Brake),
    ("steer_left", Native::SteerLeft),
    ("steer_right", Native::SteerRight),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("coin", "coin"),
    ("start", "boost"),
    ("test", "maintenance"),
    ("service", "stop"),
    ("b1", "gas"),
    ("b2", "brake"),
    ("b3", "boost"),
    ("b4", "camera"),
    ("b5", "gear"),
    ("up", "gas"),
    ("down", "brake"),
    ("left", "steer_left"),
    ("right", "steer_right"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The board's state now: buttons (active high), wheel, gas, brake (-32767..32767, 0..32767).
fn state() -> (u16, i32, i32, i32) {
    let stick = input(0);
    let (mut buttons, mut gas, mut brake, mut left, mut right) = (0u16, false, false, false, false);
    map().for_each_pressed(&stick, |n| match n {
        Native::Gas => gas = true,
        Native::Brake => brake = true,
        Native::SteerLeft => left = true,
        Native::SteerRight => right = true,
        n => buttons |= button_bit(n).unwrap_or(0),
    });
    let wheel = match (left, right) {
        (true, false) => -32767,
        (false, true) => 32767,
        _ => (stick.axis(Axis::LeftX) as i32).max(-32767),
    };
    let pedal = |held: bool, axis: Axis| if held { 32767 } else { (stick.axis(axis) as i32).clamp(0, 32767) };
    (buttons, wheel, pedal(gas, Axis::Accel), pedal(brake, Axis::Brake))
}

#[unsafe(no_mangle)]
pub extern "C" fn sdc2_init(port: i32) -> i32 {
    log!("sdc2: Init(COM{port}), board emulated");
    static FFB: std::sync::Once = std::sync::Once::new();
    FFB.call_once(|| {
        std::thread::spawn(ffb_thread);
    });
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn sdc2_none() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn sdc2_none1(_a: i32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn sdc2_connected() -> i32 {
    1
}

/// `GetAxisData`: the wheel (0), the gas (1) and the brake (2) pressed toward -1.
#[unsafe(no_mangle)]
pub extern "C" fn sdc2_axis(axis: i32) -> f32 {
    let (_, wheel, gas, brake) = state();
    let v = match axis {
        0 => wheel,
        1 => -gas,
        2 => -brake,
        _ => 0,
    };
    v as f32 / 32767.0
}

/// `GetDigitalIn`: the buttons, active low.
#[unsafe(no_mangle)]
pub extern "C" fn sdc2_digital_in() -> u32 {
    let buttons = state().0;
    count_coin(buttons);
    (!buttons) as u32
}

/// The board's coin counter (credits left: the game sets it to the price of a game in free play
/// and takes a game's price from it at the start), and whether the coin button was held.
static COINS: AtomicI32 = AtomicI32::new(0);
static COIN_HELD: AtomicBool = AtomicBool::new(false);

/// A coin button press adds a coin to the counter.
fn count_coin(buttons: u16) {
    let held = buttons & button_bit(Native::Coin).unwrap_or(0) != 0;
    if held && !COIN_HELD.swap(held, Ordering::Relaxed) {
        let coins = COINS.fetch_add(1, Ordering::Relaxed) + 1;
        log!("sdc2: coin, {coins} coin(s)");
    } else {
        COIN_HELD.store(held, Ordering::Relaxed);
    }
}

/// `GetCoin(slot)`: the coins counted.
#[unsafe(no_mangle)]
pub extern "C" fn sdc2_get_coin(_slot: i32) -> i32 {
    count_coin(state().0);
    COINS.load(Ordering::Relaxed)
}

/// `SetCoin(slot, coins)`: the counter set by the game.
#[unsafe(no_mangle)]
pub extern "C" fn sdc2_set_coin(slot: i32, coins: i32) -> i32 {
    if COINS.swap(coins, Ordering::Relaxed) != coins {
        log!("sdc2: SetCoin({}) {coins}", slot as i16);
    }
    0
}

/// `ClearCoin(slot)`: the counter emptied.
#[unsafe(no_mangle)]
pub extern "C" fn sdc2_clear_coin(_slot: i32) -> i32 {
    COINS.store(0, Ordering::Relaxed);
    0
}

/// The motor's strength (0..=32767) and the position it holds the wheel at (the axis' units,
/// -32767..=32767), as the game last set them.
static FORCE: AtomicI32 = AtomicI32::new(0);
static POSITION: AtomicI32 = AtomicI32::new(0);

/// `SetForce`: the motor's strength.
#[unsafe(no_mangle)]
pub extern "C" fn sdc2_set_force(force: i32) -> i32 {
    FORCE.store((force as i16 as i32).max(0), Ordering::Relaxed);
    0
}

/// `SetSteeringPos`: the position the motor holds the wheel at.
#[unsafe(no_mangle)]
pub extern "C" fn sdc2_set_steering_pos(position: i32) -> i32 {
    POSITION.store(position as i16 as i32, Ordering::Relaxed);
    0
}

/// The wheel's distance from the motor's position at which it pushes with its whole strength.
const FULL_FORCE_DISTANCE: i32 = 8192;

/// The motor as a constant force (-FFB_MAX left ..= FFB_MAX right): toward its position from
/// the wheel's, in proportion to the distance up to `FULL_FORCE_DISTANCE`, at its strength. The
/// game's effects move that position (the road's cracks: +-4500 every 27 ms, a shake).
fn constant_force() -> i32 {
    let (_, wheel, _, _) = state();
    let distance = (POSITION.load(Ordering::Relaxed) - wheel).clamp(-FULL_FORCE_DISTANCE, FULL_FORCE_DISTANCE);
    let strength = FORCE.load(Ordering::Relaxed).min(32767) as i64;
    (distance as i64 * strength * FFB_MAX as i64 / (FULL_FORCE_DISTANCE as i64 * 32767)) as i32
}

/// Plays the motor on player 1's wheel, on change, every 10 ms.
fn ffb_thread() {
    let mut sent = i32::MIN;
    loop {
        std::thread::sleep(Duration::from_millis(10));
        let constant = constant_force();
        // changes under 1% not sent (the wheel's reading moves a little all the time)
        if (constant - sent).abs() >= FFB_MAX / 100 || (constant == 0 && sent != 0) {
            wal_payload_common::send_output(0, FFB_CONSTANT, constant);
            sent = constant;
        }
    }
}
