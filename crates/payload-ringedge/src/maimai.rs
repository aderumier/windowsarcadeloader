//! maimai GreeN, game id `SBYC` (Sega RingEdge 2, Direct3D 9; the Chinese edition): the devices
//! the base RingEdge payload does not already answer. Enabled by `WAL_RINGEDGE_MAIMAI=1`.
//!
//! - The LED boards: four 837-15070 boards (`E0 dest src len cmd... sum` frames, 115200 baud,
//!   reset, EEPROM reads), two per player, on the first free ports from `COM5` on
//!   (`COM5`-`COM8`): the game only finds a board whose port opens ("1P / 2P LED BD Not Found"
//!   warning else), then resets it and sends its lights (payload-common `led.rs`, a board
//!   without lights).
//! - The touch sensors: one controller for both screens on `COM3` (9600 baud, the game's
//!   `NOSERIAL 0`). The game writes 6-byte ASCII commands: `{HALT}` stops the sensor stream,
//!   `{STAT}` starts it, `{RSET}` resets; while halted it sets each sensor's sensitivity
//!   (`{LAkZ}`: `L` / `R` the player, `A`-`Q` the sensor, `Z` the value; answered `(LAkZ)`) and
//!   reads it back (`{LAth}`, answered `(LAtZ)`: the value set, else "Touch Sensor Error
//!   (INITIALIZE)"). Running, the controller streams 14-byte frames: `(`, player 1's 6 bytes,
//!   player 2's 6 bytes, `)`; each byte holds sensors in its low 5 bits (0x40 set: the game
//!   appends what it reads as a C string, a zero byte cut the frame): bytes 0-3 A1 B1 A2 B2,
//!   A3 B3 A4 B4, A5 B5 A6 B6, A7 B7 A8 B8 C (bits 0-4; the touch sensor test's layout): A the
//!   outer ring, B the inner ring, C the center, each A / B sensor the eighth of the ring facing
//!   button `n` (button 1 right of the top, clockwise). No frame for 6 seconds: "Touch Sensor
//!   Error".
//!   The touches: the touch surface's fingers, else player 1's pointer (the mouse, or the left
//!   stick) while kp0 is held (the profile maps the mouse button to it), on either player's
//!   screen (the window shows both 1080x1920 screens side by side, `WAL_D3D9_ASPECT`); each
//!   screen's circle is its lower 1080x1080 square.
//! - The 8 buttons around each screen: the JVS board's switches right, left, button 2-7 of each
//!   player (the profile's `native_map`), active low: their bits set while released
//!   ([`buttons_released`]; every button reads pressed else). Button 5 is the entry (START).
//! - Player 1's Aime reader on `COM2` (aimelib 3.01, generation 1: `WAL_AIME_READER=tn32`),
//!   the card input touching it with the player's card (`WAL_RINGEDGE_AIME_CODE`).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use wal_payload_common::{input, log, screenshot, serial, sgreader, touches};
use wal_protocol::{Axis, button};

/// The touch controller's commands not complete yet.
static TOUCH_RX: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// The sensor stream stopped (`{HALT}`) until `{STAT}`.
static HALTED: AtomicBool = AtomicBool::new(true);
/// Each player's sensor sensitivities (A-Q), as the game set them.
static RATIOS: Mutex<[[u8; 17]; 2]> = Mutex::new([[0; 17]; 2]);
/// The touch controller's start / stop commands logged (the first ones).
static TOUCH_LOGGED: AtomicUsize = AtomicUsize::new(0);

/// The answers to the game's touch controller commands.
fn touch_packet(data: &[u8]) -> Vec<u8> {
    let mut rx = TOUCH_RX.lock().unwrap();
    rx.extend_from_slice(data);
    let mut out = Vec::new();
    loop {
        let Some(start) = rx.iter().position(|&b| b == b'{') else {
            rx.clear();
            break;
        };
        rx.drain(..start);
        if rx.len() < 6 {
            break;
        }
        let cmd: Vec<u8> = rx.drain(..6).collect();
        // the stream's start and stop (not the 34 sensitivity settings of each start)
        if !matches!(cmd[3], b'k' | b't') && TOUCH_LOGGED.fetch_add(1, Ordering::Relaxed) < 50 {
            log!("maimai: touch {}", String::from_utf8_lossy(&cmd));
        }
        match &cmd[1..5] {
            b"HALT" => HALTED.store(true, Ordering::Relaxed),
            b"STAT" => HALTED.store(false, Ordering::Relaxed),
            b"RSET" => {}
            &[side, letter, op, value] => {
                // a sensor's sensitivity: set (`k`, echoed), read back (`t`: the value set)
                let player = (side == b'R') as usize;
                let sensor = letter.wrapping_sub(b'A') as usize;
                let mut ratios = RATIOS.lock().unwrap();
                let value = match (op, ratios[player].get_mut(sensor)) {
                    (b'k', Some(r)) => {
                        *r = value;
                        value
                    }
                    (b't', Some(r)) => *r,
                    _ => value,
                };
                out.extend([b'(', side, letter, op, value, b')']);
            }
            _ => {}
        }
    }
    out
}

/// The sensor bit touched at `(x, y)` of a screen's circle (fractions of its square, center
/// 0.5): A / B sensor `n` (0-7) in byte `n / 2` at bit `(n % 2) * 2` / the next one, C the fifth
/// bit of byte 3.
fn sensor(x: f32, y: f32) -> Option<u32> {
    let (dx, dy) = (x - 0.5, y - 0.5);
    let r = (dx * dx + dy * dy).sqrt() * 2.0;
    if r > 1.0 {
        return None;
    }
    if r < 0.18 {
        return Some(19);
    }
    // the eighth of the ring, clockwise from the top: A1 / B1 from 0 to 45 degrees
    let angle = dx.atan2(-dy).rem_euclid(std::f32::consts::TAU);
    let n = ((angle / std::f32::consts::FRAC_PI_4) as u32).min(7);
    let inner = (r < 0.52) as u32;
    Some(n / 2 * 5 + n % 2 * 2 + inner)
}

/// Each player's sensors touched now (bit `n`: sensor `n`).
fn sensors() -> [u32; 2] {
    let mut points: Vec<(f32, f32)> = touches().iter().map(|&(x, y)| (x as f32 / 65535.0, y as f32 / 65535.0)).collect();
    if points.is_empty() {
        let p1 = input(0);
        if p1.pressed(button::KP0) {
            points.push((p1.axis_u16(Axis::LeftX) as f32 / 65535.0, p1.axis_u16(Axis::LeftY) as f32 / 65535.0));
        }
    }
    let mut out = [0u32; 2];
    for (sx, sy) in points {
        // the picture: two 1080x1920 screens side by side, each circle its lower square
        let (px, py) = screenshot::to_picture(sx, sy);
        let player = (px >= 0.5) as usize;
        let x = (px * 2.0).fract();
        let y = (py * 1920.0 - 840.0) / 1080.0;
        if let Some(s) = sensor(x, y) {
            out[player] |= 1 << s;
        }
    }
    out
}

/// The running controller's sensor frame, at each poll.
fn touch_stream() -> Vec<u8> {
    if HALTED.load(Ordering::Relaxed) {
        return Vec::new();
    }
    let mut frame = Vec::with_capacity(14);
    frame.push(b'(');
    for bits in sensors() {
        frame.extend((0..6).map(|i| 0x40 | ((bits >> (i * 5)) & 0x1F) as u8));
    }
    frame.push(b')');
    frame
}

/// The JVS switch bits of the 8 buttons (switch byte 1: right, left, button 2; byte 2: buttons
/// 3-7), set while released.
fn buttons_released(players: &mut [wal_payload_common::jvs_board::Switches]) {
    for p in players.iter_mut().take(2) {
        p.0 ^= 0x0D;
        p.1 ^= 0xF8;
    }
}

/// Player 1's 20-digit Aime access code (`WAL_RINGEDGE_AIME_CODE`, default the first card).
fn access_code() -> String {
    let code: String = std::env::var("WAL_RINGEDGE_AIME_CODE").unwrap_or_default().chars().filter(char::is_ascii_digit).collect();
    if code.len() == 20 { code } else { "50000000000000000001".into() }
}

/// The Aime reader on `port` (generation 1, `WAL_AIME_READER=tn32`), player 1's card input
/// touching it with the player's card.
fn aime(port: &str) {
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    sgreader::init(port, &[exe]);
    let code = access_code();
    std::thread::spawn(move || {
        let mut held = false;
        loop {
            let card = input(0).pressed(button::CARD);
            if card && !held {
                sgreader::insert(&code, std::time::Duration::from_secs(3));
            }
            held = card;
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
}

/// The game's devices, when `WAL_RINGEDGE_MAIMAI=1` (after the base payload's hooks).
pub(crate) fn init() {
    if !std::env::var("WAL_RINGEDGE_MAIMAI").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    wal_payload_common::led::init_15070_ports(&["COM5", "COM6", "COM7", "COM8"]);
    serial::install_streaming("COM3", touch_packet, touch_stream);
    wal_payload_common::jvs_board::set_switches_hook(buttons_released);
    aime("COM2");
    log!("maimai: LED boards on COM5-COM8, touch sensors on COM3");
}
