//! Namco JVS I/O board (`namco ltd.;NA-JV`), on the serial port the game's I/O library opens
//! (`WAJVOpen("COM3")`, `WAJVCom*`).
//!
//! Mario Kart uses this board both for controls and the STR steering board's boot handshake.
//!
//! The cabinet's test switch latches: `test` toggles it (`WAL_NAMCOES3_TEST_TOGGLE=0`: held).
//! While it is on, up / down also move the test menu cursor. Analog channels 0, 1 and 2
//! are steering, accelerator and brake. Pedals use the cabinet's default calibration range.

use std::sync::Mutex;
use std::sync::atomic::Ordering;

use wal_payload_common::jvs::Encoder;
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{log, serial};
use wal_protocol::{Axis, StickState, button};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentProcess;

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
];

/// Virtual stick -> board; profiles override it with `native_map`.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("b1", "trigger"),
    ("b2", "weapon"),
    ("b3", "brake"),
    ("b4", "enter"),
    ("b5", "menuup"),
    ("b6", "menudown"),
    ("b7", "menuup"),
    ("b8", "menudown"),
];

/// Per poll, how far digital steering moves the wheel.
const RAMP: i32 = 16;

struct Board {
    map: ButtonMap<Native>,
    /// Raw bytes written by the game, up to a complete packet.
    pending: Vec<u8>,
    coins: i32,
    coin_held: bool,
    test_on: bool,
    test_held: bool,
    test_toggle: bool,
    /// Digital steering position (0x80 = center).
    steering: i32,
    /// Driver state last answered on the STR stream; idle state re-arms the next check.
    str_served: i32,
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);

/// Installs the board on the serial calls of `modules`.
pub(crate) fn init(modules: &[usize]) {
    let flag = |name: &str, default: bool| std::env::var(name).map_or(default, |v| v != "0");
    *BOARD.lock().unwrap() = Some(Board {
        map: ButtonMap::new(NATIVES, DEFAULT_MAP),
        pending: Vec::new(),
        coins: 0,
        coin_held: false,
        test_on: false,
        test_held: false,
        test_toggle: flag("WAL_NAMCOES3_TEST_TOGGLE", true),
        steering: 0x80,
        str_served: -1,
    });
    // JVS SENSE/CTS is clear until the host assigns an address, then asserted.
    serial::MODEM_STATUS[0].store(0, Ordering::Relaxed);
    serial::MODEM_STATUS[1].store(0x10, Ordering::Relaxed);
    let ports = std::env::var("WAL_NAMCOES3_JVS_PORT").unwrap_or_else(|_| "COM3".into());
    for port in ports.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        serial::install_in(port, process, modules);
        log!("jvs: Namco I/O board on {port}");
    }
}

/// The STR driver sets this state to 0..4 for each boot check and 6 while idle.
/// Return -2 when the game-owned pointer is unreadable so the transport can use
/// the finite fallback sequence instead of treating a missing read as idle.
fn str_reader_state() -> i32 {
    if super::game_version() != Some(super::MkdxVersion::V11022) { return -2; }
    let base = unsafe { GetModuleHandleW(std::ptr::null()) as usize };
    let mut object = 0u32;
    let ok = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(), (base + 0x00E423D4) as *const _,
            (&mut object as *mut u32).cast(), 4, std::ptr::null_mut(),
        )
    };
    if ok == 0 || object == 0 { return -2; }
    let mut state = 0u32;
    let ok = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(), (object as usize + 4) as *const _,
            (&mut state as *mut u32).cast(), 4, std::ptr::null_mut(),
        )
    };
    if ok == 0 { -2 } else if state == 6 { -1 } else if state > 4 { -2 } else { state as i32 }
}

fn str_record(step: usize) -> Vec<u8> {
    let len = [7, 7, 0x16, 0x0A, 7][step];
    let mut data = vec![0; len];
    data[0] = 0x02; // STX
    data[1] = b'A';
    data[4] = b'1'; // boot check status
    if step == 3 {
        data[5] = 0x38; // card dispenser present / not sold out
        data[6] = b'1';
    }
    if len > 1 { data[len - 1] = 0x03; } // ETX
    data
}

/// Board state for one poll.
struct Inputs {
    test: bool,
    sw: [u8; 3],
    analog: [u16; 3],
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
    let coin = on(Native::Coin);
    if board.coin_held && !coin {
        board.coins += 1;
    }
    board.coin_held = coin;

    let (up, down) = (stick.pressed(button::UP), stick.pressed(button::DOWN));
    let menu_up = on(Native::MenuUp) || board.test_on && up;
    let menu_down = on(Native::MenuDown) || board.test_on && down;
    let mut sw = [0u8; 3];
    for (bit, pressed) in [(0x80, on(Native::Start)), (0x40, on(Native::Service)), (0x20, menu_up), (0x10, menu_down), (0x02, on(Native::Enter))] {
        if pressed {
            sw[0] |= bit;
        }
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

    board.steering = toward(board.steering, stick.pressed(button::LEFT), stick.pressed(button::RIGHT));
    let steering = if board.steering != 0x80 {
        board.steering as u16 * 257
    } else {
        stick.axis_u16(Axis::LeftX)
    };
    // v1.10.22's default Test Mode calibration (0x5e8ca0) gives both pedals
    // 0xa000 released and 0xd000 fully pressed. Keep their ranges independent:
    // the old flight-stick layout put steering on the brake channel and left
    // channel 0 fixed at center, so the game's handle value never moved.
    let pedal = |axis: Axis, pressed: bool| -> u16 {
        let value = if pressed { u16::MAX } else { stick.axis_u16(axis) };
        (0xa000 + u32::from(value) * 0x3000 / 65535) as u16
    };
    let analog = [steering, pedal(Axis::Accel, on(Native::Accelerate)), pedal(Axis::Brake, on(Native::Brake))];
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
        if packet.len() >= 2 && packet.len() == packet[1] as usize + 2 {
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
                rep.extend_from_slice(&[0x01, 2, 0x18, 0, 0x02, 2, 0, 0, 0x03, 8, 0x10, 0, 0x12, 0x14, 0, 0, 0x00]);
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
                    let v = io.analog.get(ch as usize).copied().unwrap_or(0x8000);
                    rep.extend_from_slice(&v.to_be_bytes());
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
                    // Acknowledge one vendor-write byte. The STR board uses a
                    // separate read poll to clock its finite boot handshake.
                    rep.push(0);
                    // The game batches several independent 0x70 writes into
                    // one JVS packet. Each group needs its own report/data
                    // pair; stopping at the first group leaves the rest of
                    // the reply short and makes the I/O self-test fail.
                    cmds[i + 1..].iter().position(|b| *b == 0x70)
                        .map_or(cmds.len() - i, |n| n + 1)
                }
                0x05 => {
                    rep.push(1);
                    arg(2).max(1)
                }
                0x03 => {
                    rep.push(0);
                    4
                }
                0x15 => {
                    let form = arg(3) as u8;
                    if form == 0x20 {
                        let state = str_reader_state();
                        if state == -1 {
                            board.str_served = -1;
                            rep.push(0); // no STR record while the driver is idle
                        } else {
                            // The memory probe is version/build dependent. If it
                            // cannot read the driver's state, still answer the
                            // link check with the finite five-record boot stream.
                            let step = if state >= 0 {
                                state
                            } else if board.str_served < 4 {
                                board.str_served + 1
                            } else {
                                -1
                            };
                            if step < 0 || step == board.str_served {
                                rep.push(0);
                                i += 4;
                                continue;
                            }
                            let record = str_record(step as usize);
                            rep.push(record.len() as u8);
                            rep.extend_from_slice(&record);
                            board.str_served = step;
                            log!("jvs: STR boot record {} ({} bytes{})", step, record.len(), if state == -2 { ", fallback" } else { "" });
                        }
                    } else {
                        rep.push(0); // form A is a presence probe
                    }
                    4
                }
                0x16 => {
                    board.str_served = -1;
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
