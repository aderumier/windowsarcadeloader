//! KanColle Arcade (Sega, Nu, game id SDBZ): `WAL_SEGANU_KANCOLLE=1`.
//!
//! * `SetWindowFeedbackSetting` (Windows 8's touch feedback, absent from Wine: the game
//!   aborted): accepted.
//! * The game's own ALL.Net client (`alw`, as Project DIVA Arcade's: the RingEdge payload's
//!   `diva.rs`): the shop's router and ALL.Net's hosts are the network's gateway
//!   (`wal_payload_common::dns`), its connections to port 80 the payload's ALL.Net server
//!   (`wal_payload_common::allnet`); the clock setters do nothing (the server's time, refused by
//!   Wine); its card system's AimeDB (`wal_payload_common::aimedb`); the billing server
//!   (`wal_payload_common::billing`: its accounting report, else error 8113); its title server
//!   (`kcserver.rs`). The broadcasts (the shop's
//!   local file server, its client looks for it on the subnet) go to the loopback address,
//!   which the machine does not deliver them to.
//! * The Aime reader (837-15396) on `WAL_SEGANU_KANCOLLE_AIME` (default `COM10`):
//!   `wal_payload_common::sgreader`; player 1's `card` input touches it with the player's card
//!   (`WAL_SEGANU_AIME_CODE`, 20 digits, default the first card).
//! * The touch screen: a 3M MicroTouch controller (`wal_payload_common::touch3m`) on
//!   `WAL_SEGANU_KANCOLLE_TOUCH` (default `COM1`); the common crosshair where it touches.
//! * The LED boards (837-15093-06, `wal_payload_common::led`): the partition LED on
//!   `WAL_SEGANU_KANCOLLE_LED` (default `COM11`), the gear pop LED on
//!   `WAL_SEGANU_KANCOLLE_GEAR_LED` (default `COM13`; the game takes the first serial port off the
//!   main board in the SetupDi list: `WAL_COMPORTS`, `wal_payload_common::comports`), the RFID deck reader (837-15345, `wal_payload_common::deck`: the
//!   printed cards on the deck) on `WAL_SEGANU_KANCOLLE_DECK` (default `COM12`).
//! * The CHC-C310 card printer: `wal_payload_common::chcusb` (`WAL_CHC_PRINTER=1`).

use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::{iat, input, log, sgreader};
use wal_protocol::{Axis, button};

pub fn init() {
    if !std::env::var("WAL_SEGANU_KANCOLLE").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    unsafe { iat::hook("user32.dll", "SetWindowFeedbackSetting", set_window_feedback_setting as *const () as usize) };
    if let Some(o) = unsafe { iat::hook("user32.dll", "RegisterTouchWindow", register_touch_window as *const () as usize) } {
        REGISTER_TOUCH.store(o, Ordering::Relaxed);
    }
    // the image information on the AMFS drive E: (its network delivery, amGfetcher, reads it)
    let version = std::env::var("WAL_SEGANU_GAME_VERSION").ok().and_then(|v| wal_payload_common::icf::version(&v)).unwrap_or(wal_payload_common::icf::GAME);
    let game_id = std::env::var("WAL_SEGANU_GAME_ID").unwrap_or_else(|_| "SDBZ".into());
    let platform = std::env::var("WAL_SEGANU_PLATFORM_ID").unwrap_or_else(|_| "AAV1".into());
    wal_payload_common::icf::ensure(game_id.trim(), platform.trim(), version);
    allnet(crate::kcserver::answer);
    // the card printer (WAL_CHC_PRINTER)
    wal_payload_common::chcusb::init();
    // the LED board (837-15093-06) and the RFID deck reader (837-15345)
    let port = |k: &str, d: &str| std::env::var(k).ok().map(|p| p.trim().to_ascii_uppercase()).filter(|p| p.starts_with("COM")).unwrap_or_else(|| d.into());
    // the gear pop LED board: the first serial port off the main board (a USB serial device in
    // WAL_COMPORTS), found through SetupDi
    wal_payload_common::comports::init();
    wal_payload_common::led::init_pair(&port("WAL_SEGANU_KANCOLLE_LED", "COM11"), &port("WAL_SEGANU_KANCOLLE_GEAR_LED", "COM13"));
    // the touch screen's 3M MicroTouch controller
    wal_payload_common::touch3m::init(&port("WAL_SEGANU_KANCOLLE_TOUCH", "COM1"));
    wal_payload_common::glview::crosshair();
    wal_payload_common::deck::init(&port("WAL_SEGANU_KANCOLLE_DECK", "COM12"));
    // the helm and the telegraph on the JVS board
    wal_payload_common::jvs_board::set_switches_hook(helm);
    wal_payload_common::jvs_board::set_analog_hook(telegraph);
    let aime_port = std::env::var("WAL_SEGANU_KANCOLLE_AIME").ok().map(|p| p.trim().to_ascii_uppercase()).unwrap_or_else(|| "COM10".into());
    if aime_port.starts_with("COM") {
        aime(&aime_port);
    }
    hook_broadcast();
    log!("kancolle: initialized");
}

/// The broadcasts to the loopback address ([`sendto`]).
pub(crate) fn hook_broadcast() {
    let f = sendto as *const () as usize;
    let exe = iat::game_module();
    if let Some(o) = unsafe { iat::hook_ordinal(exe, "WS2_32.dll", 20, f) }.or_else(|| unsafe { iat::hook("ws2_32.dll", "sendto", f) }) {
        SENDTO.store(o, Ordering::Relaxed);
    }
}

unsafe extern "system" fn set_window_feedback_setting(_hwnd: usize, _feedback: u32, _flags: u32, _size: u32, _config: usize) -> i32 {
    1
}

/// The machine's address (the route a datagram would take).
fn local_address() -> Option<std::net::Ipv4Addr> {
    let socket = std::net::UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    socket.connect(("192.0.2.1", 9)).ok()?;
    match socket.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// The game's own ALL.Net client (KanColle Arcade's, Project DIVA Arcade Future Tone's), its
/// title server `title`.
pub(crate) fn allnet(title: wal_payload_common::allnet::Title) {
    wal_payload_common::clock::hook_setters();
    let ip = local_address().unwrap_or(std::net::Ipv4Addr::new(192, 168, 0, 2)).octets();
    wal_payload_common::dns::set_router([ip[0], ip[1], ip[2], 1]);
    wal_payload_common::dns::init();
    wal_payload_common::allnet::redirect_connect(wal_payload_common::allnet::to_server);
    // its title server (the power-on's uri: `/SDBZ/`), the payload's (kcserver.rs)
    wal_payload_common::allnet::set_title(title);
    wal_payload_common::allnet::start();
    wal_payload_common::aimedb::init();
    // its amLib checks in with ALL.Net's billing server itself (else error 8113)
    wal_payload_common::billing::init();
}

static SENDTO: AtomicUsize = AtomicUsize::new(0);

/// `sendto` (ws2_32 ordinal 20): a broadcast (`x.x.x.255`) to the loopback address, same port.
unsafe extern "system" fn sendto(s: usize, buf: *const u8, len: i32, flags: i32, to: *const u8, tolen: i32) -> i32 {
    let real: unsafe extern "system" fn(usize, *const u8, i32, i32, *const u8, i32) -> i32 = unsafe { std::mem::transmute(SENDTO.load(Ordering::Relaxed)) };
    if !to.is_null() && tolen >= 16 {
        let mut a = [0u8; 16];
        a.copy_from_slice(unsafe { std::slice::from_raw_parts(to, 16) });
        if a[0] == 2 && a[7] == 255 {
            a[4..8].copy_from_slice(&[127, 0, 0, 1]);
            return unsafe { real(s, buf, len, flags, a.as_ptr(), tolen) };
        }
    }
    unsafe { real(s, buf, len, flags, to, tolen) }
}

/// Player 1's 20-digit Aime access code (`WAL_SEGANU_AIME_CODE`, default the first card).
pub(crate) fn access_code() -> String {
    let code: String = std::env::var("WAL_SEGANU_AIME_CODE").unwrap_or_default().chars().filter(char::is_ascii_digit).collect();
    if code.len() == 20 { code } else { "50000000000000000001".into() }
}

/// The Aime reader on its port, player 1's card input touching it with the player's card.
fn aime(port: &str) {
    sgreader::init(port, &[iat::game_module()]);
    let code = access_code();
    log!("kancolle: Aime reader on {port}, card {code}");
    std::thread::spawn(move || {
        let mut held = false;
        loop {
            let card = input(0).pressed(button::CARD);
            if card && !held {
                sgreader::insert(&code, Duration::from_secs(3));
            }
            held = card;
            std::thread::sleep(Duration::from_millis(50));
        }
    });
}

static REGISTER_TOUCH: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn register_touch_window(hwnd: usize, flags: u32) -> i32 {
    log!("kancolle: RegisterTouchWindow({hwnd:#x}, {flags:#x})");
    let real: unsafe extern "system" fn(usize, u32) -> i32 = unsafe { std::mem::transmute(REGISTER_TOUCH.load(Ordering::Relaxed)) };
    unsafe { real(hwnd, flags) }
}

/// The helm's position: a count of its steps (it turns without end), 8 bits.
static HELM: AtomicU8 = AtomicU8::new(0);

/// The helm's steps per switch read at full turn (`WAL_SEGANU_KANCOLLE_HELM`, default 24).
fn helm_speed() -> f32 {
    static SPEED: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *SPEED.get_or_init(|| std::env::var("WAL_SEGANU_KANCOLLE_HELM").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(24.0))
}

/// The cabinet's switches: the order button on b2, the helm, turned by player 1's right stick
/// (or left / right; the left stick is the touch pointer, the mouse): its position, Gray-coded,
/// on player 2's switches (the high nibble on up, down, left, right; the low one on buttons 3-6).
fn helm(players: &mut [wal_payload_common::jvs_board::Switches]) {
    let stick = input(0);
    // the order button (JVS button 1) on b2: b1 is the mouse's click, the touch screen's (its
    // press as the order button too kept the touches from skipping the attract)
    if let Some(p1) = players.get_mut(0) {
        p1.0 = (p1.0 & !0x02) | if stick.pressed(button::B2) { 0x02 } else { 0 };
        p1.0 &= !0x01;
    }
    let mut turn = stick.axis(Axis::RightX) as f32 / 32768.0;
    if turn.abs() < 0.15 {
        turn = 0.0;
    }
    if stick.pressed(button::LEFT) {
        turn = -1.0;
    } else if stick.pressed(button::RIGHT) {
        turn = 1.0;
    }
    let step = (turn * helm_speed()).round() as i32 as u8;
    let position = HELM.fetch_add(step, Ordering::Relaxed).wrapping_add(step);
    let gray = position ^ (position >> 1);
    if let Some(p2) = players.get_mut(1) {
        p2.0 = (p2.0 & !0x3C) | (gray >> 4) << 2;
        p2.1 = (p2.1 & !0xF0) | (gray & 0x0F) << 4;
    }
}

/// The telegraph's speed (1-7, 4 at the start).
static SPEED: AtomicU8 = AtomicU8::new(4);
/// The telegraph's last input was held (one step per push).
static SPEED_HELD: AtomicU8 = AtomicU8::new(0);

/// The telegraph (analog channel 0): 7 speeds, each one's value the middle of its area
/// (0000-FFFF calibrated in the test mode's TELEGRAPH SETTING); player 1's right stick up /
/// down (or up / down) steps it.
fn telegraph(channel: usize, value: u16) -> u16 {
    if channel != 0 {
        return value;
    }
    let stick = input(0);
    let y = stick.axis(Axis::RightY);
    let up = y < -16000 || stick.pressed(button::UP);
    let down = y > 16000 || stick.pressed(button::DOWN);
    if SPEED_HELD.swap((up || down) as u8, Ordering::Relaxed) == 0 {
        let speed = SPEED.load(Ordering::Relaxed);
        if up && speed < 7 {
            SPEED.store(speed + 1, Ordering::Relaxed);
        } else if down && speed > 1 {
            SPEED.store(speed - 1, Ordering::Relaxed);
        }
    }
    ((SPEED.load(Ordering::Relaxed) as u32 - 1) * 0xFFFF / 6) as u16
}
