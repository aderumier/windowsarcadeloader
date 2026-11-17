//! Gunslinger Stratos 2 (Square Enix / Taito, NESiCAxLive, Type X3, 2015: `game.exe`, 64-bit,
//! Direct3D 11), `WAL_NESICA_GAME=gunslinger`. One player, two guns ("G devices"), each with a
//! trigger and a lever (a stick, pushed in too); the right one has the jump button.
//!
//! * The guns' 2D sensor CPU boards (error 10000004 without them): the game's gun library (the
//!   functions of `ohmicX2dll.dll`, built into the executable) opens COM1 (left gun) and COM3
//!   (right gun), 38400 baud, odd parity, and writes to COM1 only; both boards answer. Each answer
//!   ends with a checksum byte making the sum of its bytes 0xFF.
//!   - `04`: the firmware version (version, at least 0x32), then `14` or `18` (the test menu's
//!     G device board ID 0 / 1, echoed): COM1 only, at start.
//!   - `08`, every frame: where the gun points, 11 bytes: bytes 0-1 X (10 bits, from the left),
//!     bytes 2-3 Y (10 bits, from the bottom), byte 8 flags (0x80 the gun off the screen, 0x20
//!     the trigger), byte 9 0x75 (else the game retries, then fails).
//!   - `1C`, every frame: the gun's 10 light sensors (the test menu's "sensor" cells), bits from
//!     byte 0 bit 7: all of them see the screen while the gun points at it.
//!
//!   The test menu's aim adjustment (照準調整) maps each gun's X/Y at 10 %, 50 % and 90 % of the
//!   screen; its default (and an adjustment made aiming the mouse at its marks) is the screen's
//!   fraction as sent here.
//! * FastIO 0x4120 (the G device test): bit 16 left lever pushed, 17 right lever pushed, 18 side
//!   style, 19 jump, 20 tandem style (the guns joined); 0x10 ENTER, 0x08 SELECT, 0x40 test, 0x04
//!   service. 0x4128: the levers, one byte per axis (0x80 at rest; 0xFF left, 0xFF up): left X,
//!   right X, left Y, right Y. Coins: 0x4140 as a counter (`WAL_FASTIO_COIN=counter`).
//!
//! Player 1's controls ([`switches`], [`analogs`], [`aim`]): the left stick (or the d-pad) is
//! the left lever, the right stick the right lever; the gun or mouse (`accel` / `brake` across
//! the screen, the profile's `input.gun`) aims both guns. b1 left trigger, b2 right trigger, b7
//! both triggers, b3 jump, b4 / b5 left / right lever pushed, b6 side style, b8 tandem style,
//! start ENTER. Two lightguns (the cabinet's way): once player 2's gun moved it is the right gun,
//! each gun's b7 (its trigger) fires its own gun, player 2's b3 / b4 are jump and the right lever
//! pushed (the right gun's buttons).

use wal_payload_common::{gun, log, serial};
use wal_protocol::{Axis, StickState, button};

pub(crate) fn detected() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAL_NESICA_GAME").is_ok_and(|v| v.trim().eq_ignore_ascii_case("gunslinger")))
}

pub(crate) fn init() {
    serial::install("COM1", board1);
    serial::install("COM3", board2);
    log!("gunslinger: 2D sensor boards on COM1, COM3");
    std::thread::spawn(settings);
}

/// The game's system settings object (its pointer at +0x21D0758, set by its constructor at
/// +0x31A083: `mov [rip+...], rcx`).
const SETTINGS: usize = 0x21D0758;
const SETTINGS_STORE: usize = 0x31A083;
const SETTINGS_STORE_BYTES: [u8; 7] = [0x48, 0x89, 0x0D, 0xCE, 0x66, 0xEB, 0x01];

/// The settings kept: time mode off (+0xBC), free play (+0xF0) from the common option.
fn settings() {
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    if unsafe { ((exe + SETTINGS_STORE) as *const [u8; 7]).read_unaligned() } != SETTINGS_STORE_BYTES {
        log!("gunslinger: unknown executable: settings left alone");
        return;
    }
    let free = wal_payload_common::free_play("WAL_NESICA_FREEPLAY");
    let mut logged = false;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let obj = unsafe { *((exe + SETTINGS) as *const usize) };
        if obj == 0 {
            continue;
        }
        let field = |off: usize| (obj + off) as *mut u32;
        unsafe {
            if !logged {
                logged = true;
                log!("gunslinger: settings time mode {} free play {}", *field(0xBC), *field(0xF0));
            }
            if *field(0xBC) != 0 {
                *field(0xBC) = 0;
            }
            if let Some(on) = free
                && *field(0xF0) != on as u32
            {
                *field(0xF0) = on as u32;
            }
        }
    }
}

/// The firmware version the boards report.
const VERSION: u8 = 0x40;

/// `bytes` and the checksum byte making their sum 0xFF.
fn with_checksum(mut bytes: Vec<u8>) -> Vec<u8> {
    let sum = bytes.iter().fold(0u8, |s, b| s.wrapping_add(*b));
    bytes.push(0xFFu8.wrapping_sub(sum));
    bytes
}

/// The answer of gun `gun` (0 left, 1 right) to `command`.
fn answer(gun: usize, command: u8) -> Vec<u8> {
    match command {
        0x04 => with_checksum(vec![VERSION]),
        0x14 | 0x18 => with_checksum(vec![command]),
        0x08 => {
            let aim = aim(gun);
            let (x, y) = aim.unwrap_or((0.5, 0.5));
            let to10 = |v: f32| (v.clamp(0.0, 1.0) * 1023.0).round() as u16;
            // Y from the bottom
            let (x, y) = (to10(x), 1023 - to10(y));
            let mut flags = 0;
            if aim.is_none() {
                flags |= 0x80;
            }
            if trigger(gun) {
                flags |= 0x20;
            }
            with_checksum(vec![(x >> 8) as u8, x as u8, (y >> 8) as u8, y as u8, 0, 0, 0, 0, flags, 0x75])
        }
        0x1C => {
            let seen = if aim(gun).is_some() { 0xFF } else { 0 };
            with_checksum(vec![seen, seen & 0xC0, 0, 0, 0, 0, 0, 0, 0, 0])
        }
        _ => {
            log!("gunslinger: gun {gun} command {command:#04x} unknown");
            Vec::new()
        }
    }
}

/// COM1: the left gun's board; the right one's (COM3) answers the same polls.
fn board1(packet: &[u8]) -> Vec<u8> {
    let mut reply = Vec::new();
    for &command in packet {
        reply.extend(answer(0, command));
        if matches!(command, 0x08 | 0x1C) {
            serial::push("COM3", &answer(1, command));
        }
    }
    reply
}

/// COM3: the game never writes there.
fn board2(packet: &[u8]) -> Vec<u8> {
    packet.iter().flat_map(|&c| answer(1, c)).collect()
}

/// Whether `player`'s gun or mouse moved (its `accel` / `brake`, 0 until then).
fn gun_moved(p: &StickState) -> bool {
    p.axis(Axis::Accel) != 0 || p.axis(Axis::Brake) != 0
}

/// Where gun `gun` (0 left, 1 right) points in the game's picture (fractions from its top left),
/// `None` off the picture: player 1's gun or mouse (the picture's center until it moved), player
/// 2's for the right gun once it moved.
fn aim(gun: usize) -> Option<(f32, f32)> {
    let player = if gun == 1 && gun_moved(&wal_payload_common::input(1)) { 1 } else { 0 };
    if !gun_moved(&wal_payload_common::input(player)) {
        return Some((0.5, 0.5));
    }
    gun::pedals_in_picture(player)
}

/// Whether player 2's gun aims the right gun (it moved): two lightguns, one per hand.
fn two_guns() -> bool {
    gun_moved(&wal_payload_common::input(1))
}

/// Whether gun `gun`'s trigger is pulled: b1 / b2, b7 both (with two lightguns, each gun's own
/// b7: player 1 the left gun, player 2 the right one).
fn trigger(gun: usize) -> bool {
    let p = wal_payload_common::input(0);
    let own = if gun == 0 { button::B1 } else { button::B2 };
    if p.pressed(own) {
        return true;
    }
    if !two_guns() {
        return p.pressed(button::B7);
    }
    wal_payload_common::input(gun).pressed(button::B7)
}

/// FastIO 0x4120 (player 1's controls; test and service from any player).
pub(crate) fn switches() -> u32 {
    let p = wal_payload_common::input(0);
    let bits = [
        (button::START, 0x10),
        (button::B4, 1 << 16),
        (button::B5, 1 << 17),
        (button::B6, 1 << 18),
        (button::B3, 1 << 19),
        (button::B8, 1 << 20),
    ];
    let mut v = bits.iter().filter(|(b, _)| p.pressed(*b)).fold(0, |v, (_, bit)| v | bit);
    // two lightguns: the right gun's buttons are player 2's (jump, its lever pushed)
    if two_guns() {
        let p2 = wal_payload_common::input(1);
        for (b, bit) in [(button::B3, 1 << 19), (button::B4, 1 << 17)] {
            if p2.pressed(b) {
                v |= bit;
            }
        }
    }
    for player in 0..wal_protocol::MAX_PLAYERS {
        let s = wal_payload_common::input(player);
        if s.pressed(button::TEST) {
            v |= 0x40;
        }
        if s.pressed(button::SERVICE) {
            v |= 0x04;
        }
    }
    v
}

/// FastIO 0x4128: the levers (one byte per axis, 0x80 at rest, 0xFF left / up): left X, right X,
/// left Y, right Y.
pub(crate) fn analogs() -> u32 {
    let p = wal_payload_common::input(0);
    let byte = |v: i16| (128 - (v as i32 >> 8)).clamp(0, 255) as u32;
    // the d-pad: the left lever at full tilt
    let dpad = |neg: u32, pos: u32, v: i16| {
        if p.pressed(neg) {
            i16::MIN
        } else if p.pressed(pos) {
            i16::MAX
        } else {
            v
        }
    };
    let lx = dpad(button::LEFT, button::RIGHT, p.axis(Axis::LeftX));
    let ly = dpad(button::UP, button::DOWN, p.axis(Axis::LeftY));
    byte(lx) | byte(p.axis(Axis::RightX)) << 8 | byte(ly) << 16 | byte(p.axis(Axis::RightY)) << 24
}


