//! Namco JVS I/O board (`namco ltd.;NA-JV`), on the serial port the game's I/O library opens
//! (`WAJVOpen("COM3")`, `WAJVCom*`).
//!
//! Battle Pod controls: flight stick (analog 2 / 3), throttle (analog 1), trigger and weapon
//! buttons on the stick, view button, start, and the operator buttons. Switch bytes of player
//! 1: byte 0 start 0x80, service 0x40, menu up 0x20, menu down 0x10, enter 0x02; byte 1 view
//! 0x80; byte 2 weapon 0x01, trigger 0x02. System byte: test 0x80.
//!
//! Lost Land Adventure (`lla`): player 1's byte 0 as Battle Pod's (service, up, down, enter),
//! plus the operator panel 0x01 (on with the test switch: the launcher only reads the test
//! switch with the panel open), byte 1 start 1 0x80, start 2 0x40 (player 2's `start`); both
//! players' coins in slot 1.
//! The guns are the game's own serial device (`lla.rs`).
//!
//! Wangan Midnight Maximum Tune 5 (`wmmt5`): the wheel (analog 0), accelerator (1) and brake
//! (2); see [`WMMT5_SWITCHES`] for the buttons and the shift lever's four switches (the gate
//! of a 6-speed H pattern: 1 up-left, 2 down-left, 3 up, 4 down, 5 up-right, 6 down-right).
//! `gear_up` / `gear_down` move a sequential gear through the H pattern (neutral below 1st).
//!
//! The cabinet's test switch latches: `test` toggles it (`WAL_NAMCOES3_TEST_TOGGLE=0`: held).
//! While it is on, up / down also move the test menu cursor. The stick and throttle follow
//! the analog axes; the d-pad and the `accelerate` / `brake` buttons move them gradually.

use std::sync::Mutex;
use std::sync::atomic::Ordering;

use wal_payload_common::jvs::Encoder;
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{log, serial};
use wal_protocol::{Axis, StickState, button};

const IDENTIFIER: &[u8] = b"namco ltd.;NA-JV;Ver4.00;JPN,Multipurpose.\0";
const REVISION: u8 = 0x31;
const SYNC: u8 = 0xE0;
const MARK: u8 = 0xD0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Native {
    Test,
    Service,
    Coin,
    Start,
    Trigger,
    Weapon,
    View,
    Enter,
    MenuUp,
    MenuDown,
    Accelerate,
    Brake,
    Interrupt,
    LeverUp,
    LeverDown,
    LeverLeft,
    LeverRight,
    GearUp,
    GearDown,
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("start", Native::Start),
    ("trigger", Native::Trigger),
    ("weapon", Native::Weapon),
    ("view", Native::View),
    ("enter", Native::Enter),
    ("menuup", Native::MenuUp),
    ("menudown", Native::MenuDown),
    ("accelerate", Native::Accelerate),
    ("brake", Native::Brake),
    ("interrupt", Native::Interrupt),
    ("lever_up", Native::LeverUp),
    ("lever_down", Native::LeverDown),
    ("lever_left", Native::LeverLeft),
    ("lever_right", Native::LeverRight),
    ("gear_up", Native::GearUp),
    ("gear_down", Native::GearDown),
];

/// Virtual stick -> board; profiles override it with `native_map`.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("b1", "trigger"),
    ("b2", "weapon"),
    ("b3", "view"),
    ("b4", "enter"),
    ("b5", "menuup"),
    ("b6", "menudown"),
    ("b7", "accelerate"),
    ("b8", "brake"),
];

/// Lost Land Adventure: the guns' buttons are the game's; the test menu also moves with the
/// d-pad while the test switch is on.
const LLA_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("b2", "enter"),
];

/// Wangan Midnight Maximum Tune 5: the d-pad is the shift lever, b3 / b4 shift up / down.
const WMMT5_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "view"),
    ("b1", "view"),
    ("b2", "interrupt"),
    ("b3", "gear_up"),
    ("b4", "gear_down"),
    ("b5", "menuup"),
    ("b6", "menudown"),
    ("b7", "enter"),
    ("up", "lever_up"),
    ("down", "lever_down"),
    ("left", "lever_left"),
    ("right", "lever_right"),
];

/// Wangan Midnight Maximum Tune 5's switches: native input, byte of player 1, bit.
const WMMT5_SWITCHES: &[(Native, usize, u8)] = &[
    (Native::Service, 0, 0x40),
    (Native::MenuUp, 0, 0x20),
    (Native::MenuDown, 0, 0x10),
    (Native::Enter, 0, 0x02),
    (Native::View, 1, 0x02),
    (Native::Interrupt, 1, 0x01),
];
/// The shift lever's switches: up, down, left, right (byte 1).
const WMMT5_LEVER: [u8; 4] = [0x80, 0x40, 0x20, 0x10];
/// The H pattern's gears: lever switches (up, down, left, right) of 1st..6th.
const GEARS: [[bool; 4]; 6] = [
    [true, false, true, false],
    [false, true, true, false],
    [true, false, false, false],
    [false, true, false, false],
    [true, false, false, true],
    [false, true, false, true],
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Game {
    BattlePod,
    Lla,
    Wmmt5,
}

/// Per poll, how far the d-pad and the throttle buttons move the axes.
const RAMP: i32 = 16;
/// Flight stick deadzone (virtual stick units), and the throttle's on the right stick.
const STICK_DEADZONE: i32 = 2500;
const THROTTLE_DEADZONE: i32 = 8689;

struct Board {
    map: ButtonMap<Native>,
    /// Player 2's map (Lost Land Adventure's start 2 and coin).
    map2: ButtonMap<Native>,
    game: Game,
    /// Raw bytes written by the game, up to a complete packet.
    pending: Vec<u8>,
    coins: i32,
    coin_held: bool,
    test_on: bool,
    test_held: bool,
    test_toggle: bool,
    reverse_y: bool,
    reverse_throttle: bool,
    /// Digital positions of x, y, throttle (0x80 = center).
    ramp: [i32; 3],
    /// Wangan Midnight Maximum Tune 5: the sequential gear (0 neutral), the gear buttons held.
    gear: usize,
    gear_held: (bool, bool),
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);

/// Installs the board on the serial calls of `modules`.
pub(crate) fn init(modules: &[usize]) {
    let flag = |name: &str, default: bool| std::env::var(name).map_or(default, |v| v != "0");
    let game = if crate::lla::is_lla() {
        Game::Lla
    } else if crate::wmmt5::is_wmmt5() {
        Game::Wmmt5
    } else {
        Game::BattlePod
    };
    let defaults = match game {
        Game::Lla => LLA_MAP,
        Game::Wmmt5 => WMMT5_MAP,
        Game::BattlePod => DEFAULT_MAP,
    };
    *BOARD.lock().unwrap() = Some(Board {
        map: ButtonMap::new(NATIVES, defaults),
        map2: ButtonMap::with_defaults(NATIVES, defaults),
        game,
        pending: Vec::new(),
        coins: 0,
        coin_held: false,
        test_on: false,
        test_held: false,
        test_toggle: flag("WAL_NAMCOES3_TEST_TOGGLE", true),
        reverse_y: flag("WAL_NAMCOES3_REVERSE_Y", false),
        reverse_throttle: flag("WAL_NAMCOES3_REVERSE_THROTTLE", false),
        ramp: [0x80; 3],
        gear: 0,
        gear_held: (false, false),
    });
    // sense line: present, then addressed
    serial::MODEM_STATUS[0].store(0x10, Ordering::Relaxed);
    serial::MODEM_STATUS[1].store(0x30, Ordering::Relaxed);
    let port = std::env::var("WAL_NAMCOES3_JVS_PORT").unwrap_or_else(|_| "COM3".into());
    serial::install_in(&port, process, modules);
    log!("jvs: Namco I/O board on {port}");
}

/// Board state for one poll.
struct Inputs {
    test: bool,
    sw: [u8; 3],
    /// Analog channels 0..8 (high byte).
    analog: [u8; 8],
}

fn toward(cur: i32, neg: bool, pos: bool) -> i32 {
    let target = match (neg, pos) {
        (true, false) => 0,
        (false, true) => 255,
        _ => 0x80,
    };
    if cur < target { (cur + RAMP).min(target) } else { (cur - RAMP).max(target) }
}

fn poll(board: &mut Board) -> Inputs {
    let stick: StickState = wal_payload_common::input(0);
    let mut n = [false; NATIVES.len()];
    board.map.for_each_pressed(&stick, |p| n[NATIVES.iter().position(|(_, q)| *q == p).unwrap()] = true);
    let on = |p: Native| n[NATIVES.iter().position(|(_, q)| *q == p).unwrap()];

    if board.test_toggle {
        if on(Native::Test) && !board.test_held {
            board.test_on = !board.test_on;
            log!("jvs: test switch {}", if board.test_on { "on" } else { "off" });
        }
        board.test_held = on(Native::Test);
    } else {
        board.test_on = on(Native::Test);
    }
    // player 2 (Lost Land Adventure): start 2, coin
    let (mut start2, mut coin2) = (false, false);
    if board.game == Game::Lla {
        let stick2: StickState = wal_payload_common::input(1);
        board.map2.for_each_pressed(&stick2, |p| match p {
            Native::Start => start2 = true,
            Native::Coin => coin2 = true,
            _ => {}
        });
    }
    let coin = on(Native::Coin) || coin2;
    if board.coin_held && !coin {
        board.coins += 1;
    }
    board.coin_held = coin;

    let (up, down) = (stick.pressed(button::UP), stick.pressed(button::DOWN));
    let menu_up = on(Native::MenuUp) || board.test_on && up;
    let menu_down = on(Native::MenuDown) || board.test_on && down;
    if board.game == Game::Wmmt5 {
        return wmmt5(board, &stick, &on, menu_up, menu_down);
    }
    let mut sw = [0u8; 3];
    // Lost Land Adventure: the starts are in byte 1
    let start = on(Native::Start) && board.game != Game::Lla;
    for (bit, pressed) in [(0x80, start), (0x40, on(Native::Service)), (0x20, menu_up), (0x10, menu_down), (0x02, on(Native::Enter))] {
        if pressed {
            sw[0] |= bit;
        }
    }
    if board.game == Game::Lla {
        // the launcher reads the test switch only with the operator panel open (0x01)
        if board.test_on {
            sw[0] |= 0x01;
        }
        for (bit, pressed) in [(0x80, on(Native::Start)), (0x40, start2)] {
            if pressed {
                sw[1] |= bit;
            }
        }
        return Inputs { test: board.test_on, sw, analog: [0x80; 8] };
    }
    if on(Native::View) {
        sw[1] |= 0x80;
    }
    if on(Native::Weapon) {
        sw[2] |= 0x01;
    }
    if on(Native::Trigger) {
        sw[2] |= 0x02;
    }

    // d-pad / throttle buttons, else the analog axes. Up (d-pad or stick) is a high y, like
    // pushing the cabinet's flight stick forward. Throttle: 0x80 idle, up accelerates.
    let r = &mut board.ramp;
    r[0] = toward(r[0], stick.pressed(button::LEFT), stick.pressed(button::RIGHT));
    r[1] = toward(r[1], down, up);
    r[2] = toward(r[2], on(Native::Brake), on(Native::Accelerate));
    // small stick offsets read as centered
    let centered = |a: Axis| (stick.axis(a) as i32).abs() < STICK_DEADZONE;
    let x = if r[0] != 0x80 || centered(Axis::LeftX) { r[0] as u8 } else { stick.axis_u8(Axis::LeftX) };
    let mut y = if r[1] != 0x80 || centered(Axis::LeftY) { r[1] as u8 } else { 255 - stick.axis_u8(Axis::LeftY) };
    let mut throttle = if r[2] != 0x80 {
        r[2] as u8
    } else {
        let ry = stick.axis(Axis::RightY) as i32;
        let ry = if ry.abs() < THROTTLE_DEADZONE { 0 } else { ry };
        let pedals = stick.axis_u8(Axis::Accel) as i32 - stick.axis_u8(Axis::Brake) as i32;
        (0x80 + pedals / 2 - ry / 256).clamp(0, 255) as u8
    };
    if board.reverse_y {
        y = !y;
    }
    if board.reverse_throttle {
        throttle = !throttle;
    }
    let mut analog = [0x80; 8];
    analog[1..4].copy_from_slice(&[throttle, x, y]);
    Inputs { test: board.test_on, sw, analog }
}

/// Wangan Midnight Maximum Tune 5's poll: buttons, shift lever, wheel and pedals.
fn wmmt5(board: &mut Board, stick: &StickState, on: &dyn Fn(Native) -> bool, menu_up: bool, menu_down: bool) -> Inputs {
    let mut sw = [0u8; 3];
    for &(native, byte, bit) in WMMT5_SWITCHES {
        let pressed = match native {
            Native::MenuUp => menu_up,
            Native::MenuDown => menu_down,
            n => on(n),
        };
        if pressed {
            sw[byte] |= bit;
        }
    }
    // the lever moved by hand (outside the test menu, where up / down move its cursor)
    let lever = [Native::LeverUp, Native::LeverDown, Native::LeverLeft, Native::LeverRight].map(|n| on(n) && !board.test_on);
    if lever.iter().any(|l| *l) {
        // the sequential gear starts again from neutral
        board.gear = 0;
    }
    let (gear_up, gear_down) = (on(Native::GearUp), on(Native::GearDown));
    if gear_up && !board.gear_held.0 && board.gear < GEARS.len() {
        board.gear += 1;
    }
    if gear_down && !board.gear_held.1 && board.gear > 0 {
        board.gear -= 1;
    }
    board.gear_held = (gear_up, gear_down);
    let switches = if board.gear > 0 { GEARS[board.gear - 1] } else { lever };
    for (pressed, bit) in switches.into_iter().zip(WMMT5_LEVER) {
        if pressed {
            sw[1] |= bit;
        }
    }
    let mut analog = [0x80; 8];
    analog[0] = stick.axis_u8(Axis::LeftX);
    analog[1] = stick.axis_u8(Axis::Accel);
    analog[2] = stick.axis_u8(Axis::Brake);
    Inputs { test: board.test_on, sw, analog }
}

/// Serial handler: raw bytes in, replies to the complete packets out.
fn process(data: &[u8]) -> Vec<u8> {
    let mut guard = BOARD.lock().unwrap();
    let Some(board) = guard.as_mut() else { return Vec::new() };
    let mut out = Vec::new();
    for &b in data {
        if b == SYNC {
            board.pending.clear();
        } else if board.pending.is_empty() {
            continue;
        }
        board.pending.push(b);
        // unescaped: node, size, data..., sum (size counts data + sum)
        let mut packet = Vec::with_capacity(board.pending.len());
        let mut escaped = false;
        for &c in &board.pending[1..] {
            if escaped {
                packet.push(c.wrapping_add(1));
                escaped = false;
            } else if c == MARK {
                escaped = true;
            } else {
                packet.push(c);
            }
        }
        if packet.len() == 2 && packet[1] == 0 {
            // size counts the checksum, so 0 is a corrupt packet: drop it
            log!("jvs: dropped zero-size packet {:02X?}", board.pending);
            board.pending.clear();
        } else if packet.len() >= 2 && packet.len() == packet[1] as usize + 2 {
            board.pending.clear();
            out.extend(packet_reply(board, &packet));
        }
    }
    out
}

/// One packet `node size commands... sum`.
fn packet_reply(board: &mut Board, p: &[u8]) -> Vec<u8> {
    let (node, cmds) = (p[0], &p[2..p.len() - 1]);
    if cmds.first() == Some(&0xF0) {
        serial::READY.store(false, Ordering::Relaxed);
        return Vec::new();
    }
    if node != 0xFF && node != 0x01 {
        return Vec::new();
    }
    let io = poll(board);
    let mut rep = Vec::new();
    let mut i = 0;
    while i < cmds.len() {
        let arg = |k: usize| cmds.get(i + k).copied().unwrap_or(0) as usize;
        rep.push(0x01); // report OK
        i += match cmds[i] {
            0xF1 => {
                serial::READY.store(true, Ordering::Relaxed);
                2
            }
            0x10 => {
                rep.extend_from_slice(IDENTIFIER);
                1
            }
            0x11..=0x13 => {
                rep.push(REVISION);
                1
            }
            0x14 => {
                // 1 player x 24 switches, 2 coin slots, 8 analogs (10 bits), 18 outputs
                rep.extend_from_slice(&[0x01, 2, 0x18, 0, 0x02, 2, 0, 0, 0x03, 8, 0x0A, 0, 0x12, 0x14, 0, 0, 0x00]);
                1
            }
            0x15 => cmds[i..].iter().position(|b| *b == 0).map_or(cmds.len() - i, |z| z + 1),
            0x20 => {
                rep.push(if io.test { 0x80 } else { 0 });
                for player in 0..arg(1) {
                    for k in 0..arg(2) {
                        rep.push(if player == 0 { io.sw.get(k).copied().unwrap_or(0) } else { 0 });
                    }
                }
                3
            }
            0x21 => {
                for slot in 0..arg(1) {
                    let c = if slot == 0 { board.coins } else { 0 };
                    rep.extend_from_slice(&[((c >> 8) & 0x3F) as u8, c as u8]);
                }
                2
            }
            0x22 => {
                for ch in 0..arg(1) {
                    let v = io.analog.get(ch).copied().unwrap_or(0x80);
                    rep.extend_from_slice(&[v, 0]);
                }
                2
            }
            0x26 => {
                rep.extend(std::iter::repeat_n(0, arg(1)));
                2
            }
            0x2E => {
                rep.extend_from_slice(&[0; 4]);
                2
            }
            0x2F => 1,
            c @ (0x30 | 0x31) => {
                let count = (arg(2) << 8 | arg(3)) as i32;
                if arg(1) == 1 {
                    board.coins = (board.coins + if c == 0x30 { -count } else { count }).max(0);
                }
                4
            }
            0x32 | 0x34 => 2 + arg(1),
            0x33 => 2 + 2 * arg(1),
            0x35 | 0x36 => 4,
            0x37 | 0x38 => 3,
            // Namco specific
            0x70 => match arg(1) {
                0x18 => {
                    rep.push(1);
                    cmds.len() - i
                }
                0x05 => {
                    rep.push(1);
                    arg(2).max(1)
                }
                0x03 => {
                    rep.push(0);
                    4
                }
                0x15 | 0x16 => {
                    rep.push(1);
                    4
                }
                sub => {
                    log!("jvs: unknown Namco command 70 {sub:02x}");
                    rep.push(1);
                    cmds.len() - i
                }
            },
            0x78..=0x80 => 15,
            c => {
                log!("jvs: unknown command {c:02x}");
                *rep.last_mut().unwrap() = 0x02; // parameter error
                cmds.len() - i
            }
        };
    }
    let mut e = Encoder::new();
    e.extend(&rep);
    e.finish()
}
