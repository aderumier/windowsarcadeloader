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
//!
//! Gunslinger Stratos 3 (2017, the same cabinet and code base): the same devices, plus the
//! headset amplifier on COM4 ([`amplifier`]); its settings fields differ ([`SETTINGS`]).

use wal_payload_common::{gun, log, serial};
use wal_protocol::{Axis, StickState, button};

pub(crate) fn detected() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAL_NESICA_GAME").is_ok_and(|v| v.trim().eq_ignore_ascii_case("gunslinger")))
}

pub(crate) fn init() {
    serial::install("COM1", board1);
    serial::install("COM3", board2);
    // Gunslinger Stratos 3: the headset amplifier (only opened by that game)
    serial::install("COM4", amplifier);
    log!("gunslinger: 2D sensor boards on COM1, COM3");
    std::thread::spawn(settings);
}

/// The game's system settings object, per executable: the RVA of its constructor's store of
/// its pointer (`mov [rip+...], rcx`), that instruction, the pointer's RVA, the time mode field
/// (kept off) and the free play field.
struct Settings {
    store: usize,
    bytes: [u8; 7],
    pointer: usize,
    time_mode: Option<usize>,
    free_play: usize,
}

const SETTINGS: [Settings; 2] = [
    // Gunslinger Stratos 2 (2.11 JPN): time mode +0xBC (the factory Monday-Thursday schedule
    // forces coins), free play +0xF0
    Settings {
        store: 0x31A083,
        bytes: [0x48, 0x89, 0x0D, 0xCE, 0x66, 0xEB, 0x01],
        pointer: 0x21D0758,
        time_mode: Some(0xBC),
        free_play: 0xF0,
    },
    // Gunslinger Stratos 3 (2.46 JPN): free play +0x100 (the credit control's free play is
    // +0x100 or +0xBC)
    Settings {
        store: 0x3F0903,
        bytes: [0x48, 0x89, 0x0D, 0xFE, 0xB3, 0xD1, 0x02],
        pointer: 0x310BD08,
        time_mode: None,
        free_play: 0x100,
    },
];

/// The settings kept: time mode off, free play from the common option.
fn settings() {
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    let Some(game) =
        SETTINGS.iter().find(|g| unsafe { ((exe + g.store) as *const [u8; 7]).read_unaligned() } == g.bytes)
    else {
        log!("gunslinger: unknown executable: settings left alone");
        return;
    };
    let free = wal_payload_common::free_play("WAL_NESICA_FREEPLAY");
    let mut logged = false;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let obj = unsafe { *((exe + game.pointer) as *const usize) };
        if obj == 0 {
            continue;
        }
        let field = |off: usize| (obj + off) as *mut u32;
        unsafe {
            if !logged {
                logged = true;
                let time_mode = game.time_mode.map(|off| *field(off));
                log!("gunslinger: settings time mode {time_mode:?} free play {}", *field(game.free_play));
            }
            if let Some(off) = game.time_mode
                && *field(off) != 0
            {
                *field(off) = 0;
            }
            if let Some(on) = free
                && *field(game.free_play) != on as u32
            {
                *field(game.free_play) = on as u32;
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

/// The headset amplifier's levels (Gunslinger Stratos 3's voice chat): volume, headphone
/// volume, microphone gain, microphone on.
static AMPLIFIER: std::sync::Mutex<[u8; 4]> = std::sync::Mutex::new([20, 15, 15, 0]);

/// COM4 (Gunslinger Stratos 3, 9600 baud): the headset amplifier, text commands ending with
/// LF, answers ending with CR. `GNA` its name, `GVE` its version, `GAP` its levels
/// (`VNddHVddMGddHS0dMS0d`: volume at most 40, headphone volume and microphone gain at most 31,
/// headset plugged, microphone on), `SVNdd` / `SHVdd` / `SMGdd` set them, `SMR0` / `SMR1` the
/// microphone; the setters' answer is any 3 characters.
fn amplifier(packet: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(packet);
    let command = text.trim_end();
    let mut levels = AMPLIFIER.lock().unwrap();
    let level = |max: u8| command.get(3..5).and_then(|v| v.parse::<u8>().ok()).map(|v| v.min(max));
    let answer = match command.get(..3).unwrap_or("") {
        "GNA" => "WAL HEADSET AMP".to_string(),
        "GVE" => "1.00".to_string(),
        "GAP" => {
            let [vn, hv, mg, mic] = *levels;
            format!("VN{vn:02}HV{hv:02}MG{mg:02}HS00MS0{mic}")
        }
        set @ ("SVN" | "SHV" | "SMG" | "SMR") => {
            match set {
                "SVN" => levels[0] = level(40).unwrap_or(levels[0]),
                "SHV" => levels[1] = level(31).unwrap_or(levels[1]),
                "SMG" => levels[2] = level(31).unwrap_or(levels[2]),
                _ => levels[3] = (command.get(3..4) == Some("1")) as u8,
            }
            set.to_string()
        }
        _ => {
            log!("gunslinger: amplifier command {command:?} unknown");
            return Vec::new();
        }
    };
    let mut reply = answer.into_bytes();
    reply.push(b'\r');
    reply
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


