//! maimai GreeN, game id `SBYC` (Sega RingEdge 2, Direct3D 9; the Chinese edition), and maimai
//! FiNALE, `SDEY` (the same cabinet): the devices the base RingEdge payload does not already
//! answer. Enabled by `WAL_RINGEDGE_MAIMAI=1`.
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
//!   Error". `WAL_RINGEDGE_MAIMAI_TOUCH_RUN=1`: streaming from the start (FiNALE looks for the
//!   frames before any command: "Touch Sensor Not Found", error 3102, else).
//!   The touches: the touch surface's fingers, else player 1's pointer (the mouse, or the left
//!   stick) while kp0 is held (the profile maps the mouse button to it), on either player's
//!   screen (the window shows both 1080x1920 screens side by side, `WAL_D3D9_ASPECT`); each
//!   screen's circle is its lower 1080x1080 square. And each player's right stick on that
//!   player's circle (its direction the ring's eighth, its push the ring: a pad's slides).
//! - The 8 buttons around each screen: the JVS board's switches right, left, button 2-7 of each
//!   player (the profile's `native_map`), active low: their bits set while released
//!   ([`buttons_released`]; every button reads pressed else). Button 5 is the entry (START).
//! - The Aime readers on `COM2` (aimelib 3.01, generation 1: `WAL_AIME_READER=tn32`; player 2's
//!   beside player 1's: `WAL_AIME_READER_2P=1`), each player's card input touching its reader
//!   with the player's card (`WAL_RINGEDGE_AIME_CODE`, `WAL_RINGEDGE_AIME_CODE_2P`;
//!   `WAL_RINGEDGE_AUTO_CARD=1`: touched by itself after the player's entry button).

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

/// `WAL_RINGEDGE_MAIMAI_SCREEN=p1`: only player 1's screen shown.
fn p1_only() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAL_RINGEDGE_MAIMAI_SCREEN").is_ok_and(|v| v.trim().eq_ignore_ascii_case("p1")))
}

/// `WAL_RINGEDGE_MAIMAI_SCREEN=p1` (before the Direct3D 9 shims read their settings): player 1's
/// portrait screen alone, centered with its aspect (the back buffer's left 1080x1920:
/// `WAL_D3D9_SOURCE`, `WAL_D3D9_ASPECT` 9:16), `WAL_RINGEDGE_MAIMAI_BEZEL`'s image around it
/// (`WAL_D3D9_BEZEL`, black else); the touches all player 1's.
pub(crate) fn screen_env() {
    if !std::env::var("WAL_RINGEDGE_MAIMAI").is_ok_and(|v| v.trim() == "1") || !p1_only() {
        return;
    }
    // SAFETY: DllMain, before the payload's threads start
    unsafe {
        std::env::set_var("WAL_D3D9_SOURCE", "0,0,1080,1920");
        std::env::set_var("WAL_D3D9_ASPECT", "9:16");
        if let Ok(bezel) = std::env::var("WAL_RINGEDGE_MAIMAI_BEZEL")
            && !bezel.trim().is_empty()
        {
            std::env::set_var("WAL_D3D9_BEZEL", bezel.trim());
        }
    }
}

/// The launcher's touch surface is a touchpad (`WAL_TOUCH_SURFACE`, read at init: not inside the
/// serial hooks).
fn touchpad() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAL_TOUCH_SURFACE").is_ok_and(|v| v.trim() == "touchpad"))
}

/// Each player's sensors touched now (bit `n`: sensor `n`).
fn sensors() -> [u32; 2] {
    let mut out = [0u32; 2];
    let fingers: Vec<(f32, f32)> = touches().iter().map(|&(x, y)| (x as f32 / 65535.0, y as f32 / 65535.0)).collect();
    // a touchpad (`WAL_TOUCH_SURFACE=touchpad`, the launcher's) is the round screen itself, not
    // the picture: the whole pad player 1's circle with `WAL_RINGEDGE_MAIMAI_SCREEN=p1`, else
    // its left half player 1's, its right half player 2's
    let touchpad = touchpad();
    let mut points = Vec::new();
    for &(tx, ty) in &fingers {
        if !touchpad {
            points.push((tx, ty));
            continue;
        }
        let (player, x) = if p1_only() { (0, tx) } else if tx < 0.5 { (0, tx * 2.0) } else { (1, tx * 2.0 - 1.0) };
        if let Some(s) = sensor(x, ty) {
            out[player] |= 1 << s;
        }
    }
    if fingers.is_empty() {
        let p1 = input(0);
        if p1.pressed(button::KP0) {
            points.push((p1.axis_u16(Axis::LeftX) as f32 / 65535.0, p1.axis_u16(Axis::LeftY) as f32 / 65535.0));
        }
    }
    for (sx, sy) in points {
        // the picture: two 1080x1920 screens side by side (player 1's alone with
        // `WAL_RINGEDGE_MAIMAI_SCREEN=p1`), each circle its lower square
        let (px, py) = screenshot::to_picture(sx, sy);
        let (player, x) = if p1_only() { (0, px) } else { ((px >= 0.5) as usize, (px * 2.0).fract()) };
        let y = (py * 1920.0 - 840.0) / 1080.0;
        if let Some(s) = sensor(x, y) {
            out[player] |= 1 << s;
        }
    }
    // each player's right stick on that player's circle (a pad's slides): its direction the
    // eighth of the ring, pushed fully the outer ring (A), halfway the inner one (B), its dead
    // zone no touch; turning it from a button's direction to another traces a slide
    for (player, bits) in out.iter_mut().enumerate() {
        let state = input(player);
        let (rx, ry) = (state.axis(Axis::RightX) as f32 / 32767.0, state.axis(Axis::RightY) as f32 / 32767.0);
        let m = (rx * rx + ry * ry).sqrt();
        if m > 0.3 {
            let r = m.min(1.0) * 0.42 / m;
            if let Some(s) = sensor(0.5 + rx * r, 0.5 + ry * r) {
                *bits |= 1 << s;
            }
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

/// A player's 20-digit Aime access code (`WAL_RINGEDGE_AIME_CODE`, player 2
/// `WAL_RINGEDGE_AIME_CODE_2P`; default the first / second card).
fn access_code(player: usize) -> String {
    let var = if player == 0 { "WAL_RINGEDGE_AIME_CODE" } else { "WAL_RINGEDGE_AIME_CODE_2P" };
    let code: String = std::env::var(var).unwrap_or_default().chars().filter(char::is_ascii_digit).collect();
    if code.len() == 20 { code } else { format!("5000000000000000000{}", player + 1) }
}

/// A player's card on its reader for 3 s (player 2: the reader beside player 1's).
fn put_card(player: usize, code: &str) {
    let time = std::time::Duration::from_secs(3);
    if player == 0 { sgreader::insert(code, time) } else { sgreader::insert_2p(code, time) }
}

/// The Aime readers on `port` (generation 1, `WAL_AIME_READER=tn32`; player 2's beside player
/// 1's with `WAL_AIME_READER_2P=1`), each player's card input touching its reader with the
/// player's card.
fn aime(port: &str) {
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    sgreader::init(port, &[exe]);
    let players = if std::env::var("WAL_AIME_READER_2P").is_ok_and(|v| v.trim() == "1") { 2 } else { 1 };
    let auto = std::env::var("WAL_RINGEDGE_AUTO_CARD").is_ok_and(|v| v.trim() == "1");
    for player in 0..players {
        let code = access_code(player);
        // the Aime id the AimeDB gives the card: the player's session (logged in, its automatic
        // card waits)
        let user = wal_payload_common::aimedb::aime_id(&code).to_string();
        std::thread::spawn(move || {
            let (mut card_held, mut entry_held) = (false, false);
            let mut due: Option<std::time::Instant> = None;
            loop {
                let state = input(player);
                let card = state.pressed(button::CARD);
                if card && !card_held {
                    put_card(player, &code);
                }
                card_held = card;
                // `WAL_RINGEDGE_AUTO_CARD=1`: 3 s after the player's button 5 (the attract's
                // entry; on the entry screen the guest play), the card on the reader (the entry
                // screen's card prompt), once the boot is over (the game settings came from the
                // title server) and while the player is not logged in: again at each press, a
                // card read during the boot or the attract does nothing
                let entry = state.pressed(button::B5);
                let ready = auto && crate::maimai_server::GAME_SETTING_SENT.load(Ordering::Relaxed) && !crate::maimai_server::player_session(&user);
                if entry && !entry_held && ready && due.is_none() {
                    due = Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
                }
                entry_held = entry;
                if due.is_some_and(|d| std::time::Instant::now() >= d) {
                    due = None;
                    if !crate::maimai_server::player_session(&user) {
                        log!("maimai: player {}'s automatic card on the reader", player + 1);
                        put_card(player, &code);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
    }
}

static EXIT_PROCESS: AtomicUsize = AtomicUsize::new(0);
/// When player 1's test switch was last pressed (seconds since the payload's start), plus one.
static TEST_PRESSED_AT: AtomicUsize = AtomicUsize::new(0);
static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
/// The test menu's arguments (`WAL_RINGEDGE_MAIMAI_TEST_ARGS`).
static TEST_ARGS: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// `ExitProcess`: maimai FiNALE leaves on the test switch, the cabinet then starting its test
/// menu (`maimai.exe gametest`, gametest.bat), and the test menu leaves when exited, the cabinet
/// starting the game again (game.bat): the launcher starts it again with those arguments.
unsafe extern "system" fn exit_process(code: u32) {
    let args = TEST_ARGS.get().map_or("", |a| a.as_str());
    let in_test_menu = std::env::args().skip(1).any(|a| a.eq_ignore_ascii_case(args));
    let now = START.get().map_or(0, |s| s.elapsed().as_secs() as usize);
    let pressed = TEST_PRESSED_AT.load(Ordering::Relaxed);
    if in_test_menu {
        log!("maimai: the test menu left: the game started again");
        wal_payload_common::request_relaunch("");
    } else if pressed != 0 && now + 1 - pressed <= 30 {
        log!("maimai: the game left on the test switch: started again with '{args}'");
        wal_payload_common::request_relaunch(args);
    }
    let real: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(EXIT_PROCESS.load(Ordering::Relaxed)) };
    unsafe { real(code) }
}

/// The test menu's relaunch ([`exit_process`]), when `WAL_RINGEDGE_MAIMAI_TEST_ARGS` is set.
fn test_relaunch() {
    let Ok(args) = std::env::var("WAL_RINGEDGE_MAIMAI_TEST_ARGS") else { return };
    let _ = TEST_ARGS.set(args.trim().to_string());
    let _ = START.set(std::time::Instant::now());
    let f = exit_process as *const () as usize;
    // the game's own calls and its C runtime's exit
    let mut hooked = unsafe { wal_payload_common::iat::hook("kernel32.dll", "ExitProcess", f) };
    let crt = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(c"msvcr90.dll".as_ptr().cast()) } as usize;
    if crt != 0 {
        hooked = unsafe { wal_payload_common::iat::hook_module(crt, "kernel32.dll", "ExitProcess", f) }.or(hooked);
    }
    match hooked {
        Some(o) => EXIT_PROCESS.store(o, Ordering::Relaxed),
        None => return log!("maimai: ExitProcess not hooked: no test menu relaunch"),
    }
    std::thread::spawn(|| {
        loop {
            if input(0).pressed(button::TEST) {
                TEST_PRESSED_AT.store(START.get().map_or(0, |s| s.elapsed().as_secs() as usize) + 1, Ordering::Relaxed);
            }
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
    // read now (the touch stream runs in the serial hooks)
    touchpad();
    p1_only();
    // the stream running from the start (FiNALE's check at boot)
    if std::env::var("WAL_RINGEDGE_MAIMAI_TOUCH_RUN").is_ok_and(|v| v.trim() == "1") {
        HALTED.store(false, Ordering::Relaxed);
    }
    serial::install_streaming("COM3", touch_packet, touch_stream);
    wal_payload_common::jvs_board::set_switches_hook(buttons_released);
    aime("COM2");
    test_relaunch();
    log!("maimai: LED boards on COM5-COM8, touch sensors on COM3");
}
