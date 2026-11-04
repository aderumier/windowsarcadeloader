//! The Kaboom Adrio "Hot Wheels" boards of Hot Wheels: Kings of the Road:
//! `KaboomAdrioHotWheels64.dll`, their serial boards' library, replaced (cdecl, every function a
//! `KABOOM_DLL_ERROR` byte, 0 no error, the board's id first).
//!
//! - **Boards.** Three boards (ids 0-2), two players each (a steering wheel with its motor, a
//!   pedal, a keypad, a coin door), on the ports listed (COM3 to COM5, `steering.rs`'s list:
//!   `KaboomManager` takes one a board from `SerialPort.GetPortNames`, the machine's own ports
//!   hidden). `KaboomAdrioHotWheels` opens each with `InitLib(board, port,
//!   keyboard, coin, ticket, steering callbacks)` on the first port answering
//!   `GetTargetVersion` 9 (revision 1143, else it flashes the firmware) and `GetPlayerId` (its
//!   first player's global id: 2 * board).
//! - **Callbacks.** Static, shared by the boards: each carries a global player id. Keypad:
//!   `(player, 0, keys)` (operator 1, down 2, select 4, up 8, accel 16); coins: `(first player,
//!   0, count 1, count 2)`, the two players' running totals (the game credits their increase);
//!   steering: both players' `(player, init in progress, init completed, calibrated, max left,
//!   centre, max right, wheel, pedal)` (the flags as BOOLs). The game calls nothing every frame:
//!   a payload thread (started at the first `InitLib`, Mono loaded) attached to the game's Mono
//!   domain calls them every 10 ms (the steering continuously, the others on a change).
//! - **Wheel and pedal.** Wheel 0..254 around 127 (calibrated 0, 127, 254). Pedal: released
//!   high, pressed low (the game's default calibration 131 released, 55 full; under 110
//!   "pressing", 25 and under a stuck pedal): 140 released to 50 full.
//! - **Players.** The launcher's four players are boards 0 and 1 (players 5 and 6 idle): the
//!   wheel on `lx`, the pedal on `accel` (or the `gas` native, full); the operator's keypad on
//!   player 1. The game's boost is the pedal's double press (Rewired's `PedalDown`, 0.75 of the
//!   pedal, twice within `GameConstants.doublePressSpeed` 0.5 s, the car on the ground): the
//!   `boost` native plays it on the pedal.
//! - **Force feedback.** Each wheel's motor is a position servo: `SetSteeringParameters(board,
//!   wheel, target, power 0..100, offset)` drives it towards the target, `DisengageSteeringWheel`
//!   frees it. The game rests it at the centre at the steering's full power (a centring spring),
//!   shakes it between both ends for its bumps (walls, hazards, coins, cars: a power and a time
//!   each) and sways it between 64 and 192 in attract mode. Played on the player's wheel
//!   (`output::FFB_*`): a target at the centre as the centring spring of that power, any other as
//!   a constant force towards it (its distance from the wheel's position, from `lx`, times 4, at
//!   most the power), every 10 ms; `WAL_HOTWHEELS_FFB` scales it (percent, default 100, 0 off).
//!   The bumps' power is their own times the remote settings' `steeringBumpMultipler`.
//! - **Remote settings.** The online `RemoteSettingsData` (`RemoteSettingsManager.Instance
//!   .m_Save.m_Data`) all zero when they never came: no boost (`speedBoost`), no respawn
//!   (`respawnOnDeath`), no bumps... `RemoteSettingsData.Reset` (the game's defaults) run on it
//!   (Mono's embedding API), checked every second.
//! - **Tickets.** Two feeders a board: `SetTicketFeederTicketToFeedCount(board, feeder, n)`
//!   targets, `GetTicketFeederCount` fed / empty (a ticket every 100 ms, never empty).
//! - **Webcams.** The cabinet's cameras take the players' pictures (`SBK.FaceDetection
//!   .FDManager`) when the operator's `TakePicture` setting is on: then without a camera
//!   playing, "CAMERA(S) MALFUNCTION" at boot. `OperatorUtil.get_TakePicture`'s IL (`ldstr
//!   "TakePicture" call GetBool ret`) becomes nops then `ldc.i4.0 ret` (no pictures, the operator's
//!   setting ignored), before its first call.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::log;
use wal_payload_common::mono::{self, mono};
use wal_protocol::Axis;

use crate::{Native, held};

const NO_ERROR: u8 = 0;
const ERROR: u8 = 1;
const BOARDS: usize = 3;
const VERSION: u16 = 9;
const REVISION: u16 = 1143;

const KEY_OPERATOR: u16 = 1;
const KEY_DOWN: u16 = 2;
const KEY_SELECT: u16 = 4;
const KEY_UP: u16 = 8;
const KEY_ACCEL: u16 = 16;

const WHEEL_LEFT: u8 = 0;
const WHEEL_CENTRE: u8 = 127;
const WHEEL_RIGHT: u8 = 254;
const PEDAL_RELEASED: f32 = 140.0;
const PEDAL_FULL: f32 = 50.0;

type KeyboardEvent = unsafe extern "C" fn(player: u8, padding: u8, keys: u16);
type CoinEvent = unsafe extern "C" fn(player: u8, padding: u8, count1: u16, count2: u16);
type SteeringEvent = unsafe extern "C" fn(
    u8, i32, i32, i32, u8, u8, u8, u8, u8, //
    u8, i32, i32, i32, u8, u8, u8, u8, u8,
);

static KEYBOARD: AtomicUsize = AtomicUsize::new(0);
static COIN: AtomicUsize = AtomicUsize::new(0);
static STEERING: AtomicUsize = AtomicUsize::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);

struct Board {
    open: bool,
    /// The two players' coin totals.
    coins: [u16; 2],
    to_feed: [u16; 2],
    fed: [u16; 2],
}

/// Each wheel's servo command (target, power 0..100), none when disengaged.
static SERVO: Mutex<[Option<(u8, u8)>; 2 * BOARDS]> = Mutex::new([None; 2 * BOARDS]);

/// `WAL_HOTWHEELS_FFB` set (the profile's force feedback strength): the game is Hot Wheels.
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAL_HOTWHEELS_FFB").is_ok())
}

/// `WAL_HOTWHEELS_FFB`: the force feedback's strength, percent.
fn ffb_strength() -> i32 {
    static STRENGTH: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    *STRENGTH.get_or_init(|| std::env::var("WAL_HOTWHEELS_FFB").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(100).clamp(0, 200))
}

/// A player's wheel forces (spring, constant) from its servo command.
fn forces(global: usize) -> (i32, i32) {
    let Some((target, power)) = SERVO.lock().unwrap()[global] else { return (0, 0) };
    let full = i32::from(power.min(100)) * wal_protocol::output::FFB_MAX / 100 * ffb_strength() / 100;
    if target == WHEEL_CENTRE {
        return (full, 0);
    }
    let (wheel, _) = wheel_pedal(global);
    let error = (f32::from(target) - f32::from(wheel)) / f32::from(WHEEL_CENTRE);
    let push = (error * 4.0).clamp(-1.0, 1.0);
    (0, (push * full as f32).round() as i32)
}

static BOARDS_STATE: Mutex<[Board; BOARDS]> = Mutex::new([const { Board { open: false, coins: [0; 2], to_feed: [0; 2], fed: [0; 2] } }; BOARDS]);

/// The launcher's player of a global player, if any.
fn launcher_player(global: usize) -> Option<usize> {
    (global < wal_protocol::MAX_PLAYERS).then_some(global)
}

/// A player's wheel (0..254) and pedal (released high).
fn wheel_pedal(global: usize) -> (u8, u8) {
    let Some(p) = launcher_player(global) else { return (WHEEL_CENTRE, PEDAL_RELEASED as u8) };
    let stick = wal_payload_common::input(p);
    let x = stick.axis(Axis::LeftX) as f32 / 32767.0;
    let wheel = (WHEEL_CENTRE as f32 + x.clamp(-1.0, 1.0) * WHEEL_CENTRE as f32).round() as u8;
    let mut gas = (stick.axis(Axis::Accel).max(0) as f32 / 32767.0).clamp(0.0, 1.0);
    if held(p, Native::Gas) {
        gas = 1.0;
    }
    if let Some(pressed) = boost(p) {
        gas = if pressed { 1.0 } else { 0.0 };
    }
    let pedal = (PEDAL_RELEASED - gas * (PEDAL_RELEASED - PEDAL_FULL)).round() as u8;
    (wheel.min(WHEEL_RIGHT), pedal)
}

/// The boost's pedal, by steps of [`BOOST_STEP`]: released, pressed, released, pressed (two
/// presses 120 ms apart: the game's double press within 0.5 s), then the player's own.
const BOOST_STEP: Duration = Duration::from_millis(60);

/// A player's boost button, its pressed edge starting the pedal's sequence: during it, whether
/// the pedal is pressed.
fn boost(p: usize) -> Option<bool> {
    static STATE: Mutex<[(bool, Option<Instant>); wal_protocol::MAX_PLAYERS]> = Mutex::new([(false, None); wal_protocol::MAX_PLAYERS]);
    let mut state = STATE.lock().unwrap();
    let (was, start) = &mut state[p];
    let now = held(p, Native::Boost);
    if now && !*was {
        *start = Some(Instant::now());
    }
    *was = now;
    let step = start.map(|t| t.elapsed().as_millis() / BOOST_STEP.as_millis())?;
    if step >= 4 {
        *start = None;
        return None;
    }
    Some(step % 2 == 1)
}

/// A player's keypad: the operator's keys on player 1, accel with the pedal pressed.
fn keys(global: usize) -> u16 {
    let mut k = 0;
    if global == 0 {
        for (native, bit) in [(Native::Operator, KEY_OPERATOR), (Native::Down, KEY_DOWN), (Native::Select, KEY_SELECT), (Native::Up, KEY_UP)] {
            if held(0, native) {
                k |= bit;
            }
        }
    }
    if wheel_pedal(global).1 < 110 {
        k |= KEY_ACCEL;
    }
    k
}

/// The remote settings (`RemoteSettingsManager.Instance.m_Save.m_Data`) never received (all
/// zero, the bump multiplier 0): the game's defaults, `RemoteSettingsData.Reset` run on
/// them again.
fn remote_settings() {
    let Some(m) = mono() else { return };
    let image = m.image("Assembly-CSharp");
    let singleton = m.class_of_type_name(image, "SBK.Singleton`1[[RemoteSettingsManager, Assembly-CSharp]]");
    let instance = m.static_addr(singleton, m.field(singleton, "_instance"));
    let (manager, save_class, data_class) = (m.class(image, "", "RemoteSettingsManager"), m.class(image, "", "RemoteSettingsSave"), m.class(image, "", "RemoteSettingsData"));
    let (save, data, multiplier) = (m.field(manager, "m_Save"), m.field(save_class, "m_Data"), m.field(data_class, "steeringBumpMultipler"));
    let reset = m.method(data_class, "Reset", 0);
    if instance.is_null() || save.is_null() || data.is_null() || multiplier.is_null() || reset.is_null() {
        return;
    }
    unsafe {
        let obj = (instance as *const usize).read();
        let save_obj = if obj != 0 { ((obj + m.offset(save)) as *const usize).read() } else { 0 };
        let data_obj = if save_obj != 0 { ((save_obj + m.offset(data)) as *const usize).read() } else { 0 };
        if data_obj == 0 {
            return;
        }
        let value = (data_obj + m.offset(multiplier)) as *const f32;
        if *value != 0.0 {
            return;
        }
        let ok = m.invoke(reset, data_obj as mono::P, &mut []).is_some();
        log!("hotwheels: no remote settings: the game's defaults ({}, ffb bump multiplier {})", if ok { "ok" } else { "failed" }, *value);
    }
}

fn board_thread() {
    if !mono().is_some_and(|m| m.attach()) {
        log!("hotwheels: cannot attach to Mono: no inputs");
        return;
    }
    let mut last_keys = [u16::MAX; 2 * BOARDS];
    let mut coin_held = [false; 2 * BOARDS];
    let mut tick = 0u32;
    let mut sent = [(i32::MIN, i32::MIN); wal_protocol::MAX_PLAYERS];
    loop {
        std::thread::sleep(Duration::from_millis(10));
        tick += 1;
        // the remote settings checked every second (the game may replace them)
        if tick.is_multiple_of(100) {
            remote_settings();
        }
        // the wheels' forces, on change
        for (player, last) in sent.iter_mut().enumerate() {
            let (spring, constant) = forces(player);
            if last.0 != spring {
                wal_payload_common::send_output(player as u8, wal_protocol::output::FFB_SPRING, spring);
            }
            if last.1 != constant {
                wal_payload_common::send_output(player as u8, wal_protocol::output::FFB_CONSTANT, constant);
            }
            *last = (spring, constant);
        }
        let open: Vec<usize> = BOARDS_STATE.lock().unwrap().iter().enumerate().filter(|(_, b)| b.open).map(|(i, _)| i).collect();
        for board in open {
            let first = 2 * board;
            // the coins
            let mut coined = None;
            for local in 0..2 {
                let global = first + local;
                let now = launcher_player(global).is_some_and(|p| held(p, Native::Coin));
                if now && !coin_held[global] {
                    let mut b = BOARDS_STATE.lock().unwrap();
                    b[board].coins[local] = b[board].coins[local].wrapping_add(1);
                    coined = Some(b[board].coins);
                    log!("hotwheels: coin, player {}", global + 1);
                }
                coin_held[global] = now;
            }
            if let Some([c1, c2]) = coined {
                let f = COIN.load(Ordering::Relaxed);
                if f != 0 {
                    unsafe { std::mem::transmute::<usize, CoinEvent>(f)(first as u8, 0, c1, c2) };
                }
            }
            // the keypads
            for global in [first, first + 1] {
                let k = keys(global);
                if k != last_keys[global] {
                    last_keys[global] = k;
                    let f = KEYBOARD.load(Ordering::Relaxed);
                    if f != 0 {
                        unsafe { std::mem::transmute::<usize, KeyboardEvent>(f)(global as u8, 0, k) };
                    }
                }
            }
            // the wheels and pedals
            let f = STEERING.load(Ordering::Relaxed);
            if f != 0 {
                let (wa, pa) = wheel_pedal(first);
                let (wb, pb) = wheel_pedal(first + 1);
                unsafe {
                    std::mem::transmute::<usize, SteeringEvent>(f)(
                        first as u8, 0, 1, 1, WHEEL_LEFT, WHEEL_CENTRE, WHEEL_RIGHT, wa, pa, //
                        first as u8 + 1, 0, 1, 1, WHEEL_LEFT, WHEEL_CENTRE, WHEEL_RIGHT, wb, pb,
                    )
                };
            }
            // a ticket every 100 ms
            if tick.is_multiple_of(10) {
                let mut b = BOARDS_STATE.lock().unwrap();
                for feeder in 0..2 {
                    if b[board].fed[feeder] != b[board].to_feed[feeder] {
                        b[board].fed[feeder] = b[board].fed[feeder].wrapping_add(1);
                    }
                }
            }
        }
    }
}

unsafe fn write<T>(p: *mut T, v: T) {
    if !p.is_null() {
        unsafe { p.write(v) };
    }
}

fn board_ok(board: i32) -> Option<usize> {
    usize::try_from(board).ok().filter(|&b| b < BOARDS)
}

/// The functions without results: no error.
extern "C" fn ok() -> u8 {
    NO_ERROR
}

extern "C" fn init_lib(board: i32, port: i32, keyboard: usize, coin: usize, _ticket: usize, steering: usize) -> u8 {
    let Some(b) = board_ok(board) else { return ERROR };
    log!("hotwheels: InitLib(board {board}, COM{port}): players {} and {}", 2 * b + 1, 2 * b + 2);
    KEYBOARD.store(keyboard, Ordering::Relaxed);
    COIN.store(coin, Ordering::Relaxed);
    STEERING.store(steering, Ordering::Relaxed);
    BOARDS_STATE.lock().unwrap()[b].open = true;
    if !STARTED.swap(true, Ordering::Relaxed) {
        std::thread::spawn(board_thread);
    }
    NO_ERROR
}

extern "C" fn close_lib(board: i32) {
    if let Some(b) = board_ok(board) {
        BOARDS_STATE.lock().unwrap()[b].open = false;
    }
}

extern "C" fn get_target_version(_board: i32, version: *mut [u16; 2]) -> u8 {
    unsafe { write(version, [VERSION, REVISION]) };
    NO_ERROR
}

/// `GetPlayerId`: the board's first player (global, from 0).
extern "C" fn get_player_id(board: i32, id: *mut u16) -> u8 {
    let Some(b) = board_ok(board) else { return ERROR };
    unsafe { write(id, 2 * b as u16) };
    NO_ERROR
}

extern "C" fn get_keypad_count(_board: i32, out: *mut [u16; 2]) -> u8 {
    unsafe { write(out, [0; 2]) };
    NO_ERROR
}

extern "C" fn get_ticket_to_feed(board: i32, feeder: i32, count: *mut u16) -> u8 {
    let n = board_ok(board).zip(usize::try_from(feeder).ok().filter(|&f| f < 2)).map_or(0, |(b, f)| BOARDS_STATE.lock().unwrap()[b].to_feed[f]);
    unsafe { write(count, n) };
    NO_ERROR
}

extern "C" fn set_ticket_to_feed(board: i32, feeder: i32, count: u16) -> u8 {
    if let (Some(b), Ok(f @ 0..=1)) = (board_ok(board), usize::try_from(feeder)) {
        log!("hotwheels: board {board} feeder {} to {count} tickets", f + 1);
        BOARDS_STATE.lock().unwrap()[b].to_feed[f] = count;
    }
    NO_ERROR
}

extern "C" fn get_ticket_feeder_count(board: i32, c1: *mut u16, e1: *mut u16, c2: *mut u16, e2: *mut u16) -> u8 {
    let fed = board_ok(board).map_or([0; 2], |b| BOARDS_STATE.lock().unwrap()[b].fed);
    unsafe {
        write(c1, fed[0]);
        write(e1, 0);
        write(c2, fed[1]);
        write(e2, 0);
    }
    NO_ERROR
}

extern "C" fn reset_ticket_feeder(board: i32) -> u8 {
    if let Some(b) = board_ok(board) {
        let mut s = BOARDS_STATE.lock().unwrap();
        s[b].to_feed = [0; 2];
        s[b].fed = [0; 2];
    }
    NO_ERROR
}

extern "C" fn get_current_coin_count(board: i32, c1: *mut u16, c2: *mut u16) -> u8 {
    let coins = board_ok(board).map_or([0; 2], |b| BOARDS_STATE.lock().unwrap()[b].coins);
    unsafe {
        write(c1, coins[0]);
        write(c2, coins[1]);
    }
    NO_ERROR
}

extern "C" fn set_coin_count(board: i32, c1: u16, c2: u16) -> u8 {
    if let Some(b) = board_ok(board) {
        BOARDS_STATE.lock().unwrap()[b].coins = [c1, c2];
    }
    NO_ERROR
}

#[repr(C)]
struct SteeringInfo {
    init_in_progress: u8,
    init_completed: u8,
    init_succeed: u8,
    max_left: u8,
    centre: u8,
    max_right: u8,
    position: u8,
    pedal: u8,
    power: i16,
    power_offset: i16,
}

/// `GetSteeringStatus(board, steering, out info)`: calibrated.
extern "C" fn get_steering_status(board: i32, steering: u8, out: *mut SteeringInfo) -> u8 {
    let global = board_ok(board).map_or(0, |b| 2 * b) + (steering as usize).min(1);
    let (position, pedal) = wheel_pedal(global);
    unsafe {
        write(out, SteeringInfo { init_in_progress: 0, init_completed: 1, init_succeed: 1, max_left: WHEEL_LEFT, centre: WHEEL_CENTRE, max_right: WHEEL_RIGHT, position, pedal, power: 0, power_offset: 0 })
    };
    NO_ERROR
}

/// `SetSteeringParameters(board, wheel, target, power, offset)`: the servo's command.
extern "C" fn set_steering_parameters(board: i32, wheel: u8, target: u8, power: u8, _offset: i8) -> u8 {
    if let Some(b) = board_ok(board).filter(|_| wheel < 2) {
        let global = 2 * b + wheel as usize;
        let mut servo = SERVO.lock().unwrap();
        if servo[global] != Some((target, power)) {
            log!("hotwheels: ffb player {}: wheel to {target}, power {power}", global + 1);
        }
        servo[global] = Some((target, power));
    }
    NO_ERROR
}

extern "C" fn disengage_steering_wheel(board: i32, wheel: u8) -> u8 {
    if let Some(b) = board_ok(board).filter(|_| wheel < 2) {
        SERVO.lock().unwrap()[2 * b + wheel as usize] = None;
    }
    NO_ERROR
}

extern "C" fn get_event_mask(_board: i32, out: *mut u16) -> u8 {
    unsafe { write(out, 0xFFFF) };
    NO_ERROR
}

extern "C" fn get_steering_position(_board: i32) -> u8 {
    WHEEL_CENTRE
}

extern "C" fn read_input(_board: i32, out: *mut u16) -> u8 {
    unsafe { write(out, 0) };
    NO_ERROR
}

extern "C" fn get_ir_detector_count(_board: i32, _output: u16, count: *mut u16) -> u8 {
    unsafe { write(count, 0) };
    NO_ERROR
}

/// At the game's first `DllImport`s (Assembly-CSharp loaded, its operator settings not read
/// yet): no webcam pictures, once (nothing for a game without `FDManager`).
pub fn init() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    let Some(m) = mono() else { return };
    let image = m.image("Assembly-CSharp");
    if image.is_null() {
        return;
    }
    DONE.store(true, Ordering::Relaxed);
    if m.class(image, "SBK.FaceDetection", "FDManager").is_null() {
        return;
    }
    match m.il_code(m.method(m.class(image, "SBK.BaseArcade", "OperatorUtil"), "get_TakePicture", 0)) {
        Some(il) if il.len() == 11 && il[0] == 0x72 && il[5] == 0x28 && il[10] == 0x2A => {
            // nops, ldc.i4.0 ret (Mono decodes the whole body: no bytes left after the ret)
            il.fill(0x00);
            il[9..].copy_from_slice(&[0x16, 0x2A]);
            log!("hotwheels: webcam pictures off (no camera check)");
        }
        _ => log!("hotwheels: OperatorUtil.get_TakePicture not the expected one: webcams checked"),
    }
}

/// The boards' functions.
pub fn resolve(name: &str) -> Option<usize> {
    let f = match name {
        "InitLib" => init_lib as *const () as usize,
        "CloseLib" => close_lib as *const () as usize,
        "GetTargetVersion" => get_target_version as *const () as usize,
        "GetPlayerId" => get_player_id as *const () as usize,
        "GetKeypadCount" => get_keypad_count as *const () as usize,
        "GetTicketFeederTicketToFeedCount" => get_ticket_to_feed as *const () as usize,
        "SetTicketFeederTicketToFeedCount" => set_ticket_to_feed as *const () as usize,
        "GetTicketFeederCount" => get_ticket_feeder_count as *const () as usize,
        "ResetTicketFeeder" => reset_ticket_feeder as *const () as usize,
        "GetCurrentCoinCount" => get_current_coin_count as *const () as usize,
        "SetCoinCount" => set_coin_count as *const () as usize,
        "GetSteeringStatus" => get_steering_status as *const () as usize,
        "GetEventMask" => get_event_mask as *const () as usize,
        "GetSteeringPosition" => get_steering_position as *const () as usize,
        "ReadInput" => read_input as *const () as usize,
        "GetIRDetectorCount" => get_ir_detector_count as *const () as usize,
        "SetSteeringParameters" => set_steering_parameters as *const () as usize,
        "DisengageSteeringWheel" => disengage_steering_wheel as *const () as usize,
        // programming, steering commands and force, outputs, LED display, diagnostics, IR
        "EnterProgramming" | "InitSteeringKickStart" | "SetEventMask" | "SetSteeringForce" | "SetOutput"
        | "ClearOutput" | "SetExtendedOutput" | "ClearExtendedOutput" | "SetDiagnosticState" | "SetIRDetectorCount" | "StartIRDetector" | "SetLedDisplay" => {
            ok as *const () as usize
        }
        _ => return None,
    };
    Some(f)
}
