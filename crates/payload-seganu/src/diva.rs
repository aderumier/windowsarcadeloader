//! Hatsune Miku Project DIVA Arcade Future Tone (Sega, Nu, game id SBZV): `WAL_SEGANU_DIVA=1`.
//! Its devices per segatools' `divahook` (public domain):
//!
//! * The game's own ALL.Net client, title server (payload-common `divaserver.rs`, ARTEMiS's
//!   Future Tone server), AimeDB and billing, its local file server found by broadcast: as
//!   KanColle Arcade's (`kancolle.rs`).
//! * The 4 hit buttons (triangle, square, cross, circle), start, test and service on the JVS
//!   board (837-14572), no custom wiring.
//! * The touch screen: the Elo 2701 controller (payload-common `elo.rs`, `WAL_RINGEDGE_ELO`).
//! * The Aime reader (TN32MSEC003S, generation 1: `WAL_AIME_READER=tn32`) on
//!   `WAL_SEGANU_DIVA_AIME` (default `COM10`): `wal_payload_common::sgreader`; player 1's `card`
//!   input touches it with the player's card (`WAL_SEGANU_AIME_CODE`), `WAL_SEGANU_AUTO_CARD=1`:
//!   after start or a touch, the card is on the reader when the game polls it.
//! * The touch slider (837-15275, [`slider`]) on `WAL_SEGANU_DIVA_SLIDER` (default `COM11`).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::{input, log, serial, sgreader};
use wal_protocol::{Axis, StickState, button};

pub fn init() {
    if !std::env::var("WAL_SEGANU_DIVA").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    // the image information on the AMFS drive E: (its network delivery, amGfetcher, reads it)
    let version = std::env::var("WAL_SEGANU_GAME_VERSION").ok().and_then(|v| wal_payload_common::icf::version(&v)).unwrap_or(wal_payload_common::icf::GAME);
    let game_id = std::env::var("WAL_SEGANU_GAME_ID").unwrap_or_else(|_| "SBZV".into());
    let platform = std::env::var("WAL_SEGANU_PLATFORM_ID").unwrap_or_else(|_| "AAV0".into());
    wal_payload_common::icf::ensure(game_id.trim(), platform.trim(), version);
    wal_payload_common::divaserver::set_future_tone();
    crate::kancolle::allnet(wal_payload_common::divaserver::answer);
    crate::kancolle::hook_broadcast();
    let port = |k: &str, d: &str| std::env::var(k).ok().map(|p| p.trim().to_ascii_uppercase()).unwrap_or_else(|| d.into());
    let aime_port = port("WAL_SEGANU_DIVA_AIME", "COM10");
    if aime_port.starts_with("COM") {
        aime(&aime_port);
    }
    // the touch screen's Elo controller (WAL_RINGEDGE_ELO), the pointer drawn over the picture
    wal_payload_common::elo::init();
    let slider_port = port("WAL_SEGANU_DIVA_SLIDER", "COM11");
    if slider_port.starts_with("COM") {
        serial::install_streaming(&slider_port, slider, slider_scan);
        log!("diva: touch slider on {slider_port}");
    }
    log!("diva: initialized");
}

/// The Aime reader on its port, player 1's card input touching it with the player's card.
fn aime(port: &str) {
    sgreader::init(port, &[wal_payload_common::iat::game_module()]);
    let code = crate::kancolle::access_code();
    log!("diva: Aime reader on {port}, card {code}");
    if std::env::var("WAL_SEGANU_AUTO_CARD").is_ok_and(|v| v.trim() == "1") {
        // after start, or a touch (a game started on the touch screen)
        sgreader::card_on_poll_after_press(&code, Duration::from_millis(0), |p1| p1.pressed(button::START) || p1.pressed(button::B8));
    }
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

const SYNC: u8 = 0xFF;
const ESC: u8 = 0xFD;
/// The slider's touch cells, from the left.
const CELLS: usize = 32;

/// The game asked the slider to stream its cells.
static SCANNING: AtomicBool = AtomicBool::new(false);
static RX: Mutex<Vec<u8>> = Mutex::new(Vec::new());
/// The last report's time and the hands' automatic slides ([`Slides`]).
static STATE: Mutex<(Option<Instant>, Slides)> = Mutex::new((None, Slides { left: None, right: None, step: None }));
/// One report every 2 ms at most.
const SCAN_PERIOD: Duration = Duration::from_millis(2);
/// An automatic slide's speed: one cell every 30 ms (segatools' divaio).
const SLIDE_STEP: Duration = Duration::from_millis(30);

/// The hands sliding by themselves while their input is held, from the middle outwards (the
/// arrows' directions: the left hand to the left, the right one to the right), each one's cell;
/// past the end, a few steps untouched before it starts again from the middle (else the game
/// reads a long slide back).
struct Slides {
    left: Option<i32>,
    right: Option<i32>,
    step: Option<Instant>,
}

/// The steps a hand stays off past the end of the slider.
const SLIDE_GAP: i32 = 4;

/// A slider frame: sync, command, payload length, payload, the sum of them all negated (the
/// bytes after the sync escaped: `FD`, the byte minus 1).
fn slider_frame(cmd: u8, payload: &[u8]) -> Vec<u8> {
    let mut plain = vec![SYNC, cmd, payload.len() as u8];
    plain.extend_from_slice(payload);
    let sum = plain.iter().fold(0u8, |s, b| s.wrapping_add(*b));
    plain.push(sum.wrapping_neg());
    let mut out = vec![SYNC];
    for &b in &plain[1..] {
        if b == SYNC || b == ESC {
            out.extend([ESC, b - 1]);
        } else {
            out.push(b);
        }
    }
    out
}

/// The next whole request in `rx` (command, payload; its sum checked), removed from it.
fn next_request(rx: &mut Vec<u8>) -> Option<(u8, Vec<u8>)> {
    loop {
        let start = rx.iter().position(|&b| b == SYNC)?;
        rx.drain(..start);
        let mut frame = vec![SYNC];
        let mut escape = false;
        let mut used = None;
        for (i, &b) in rx.iter().enumerate().skip(1) {
            if b == SYNC {
                used = Some((i, false));
                break;
            }
            if b == ESC {
                escape = true;
                continue;
            }
            frame.push(if escape { b.wrapping_add(1) } else { b });
            escape = false;
            if frame.len() >= 3 && frame.len() == frame[2] as usize + 4 {
                used = Some((i + 1, true));
                break;
            }
        }
        let (n, whole) = used?;
        rx.drain(..n);
        if whole && frame.iter().fold(0u8, |s, b| s.wrapping_add(*b)) == 0 {
            return Some((frame[1], frame[3..frame.len() - 1].to_vec()));
        }
    }
}

/// The touch slider 837-15275 (segatools' `divahook/slider.c`): reset (answered with sync
/// `FF`), board info (number, firmware), scan start (not answered: the cells streamed, see
/// [`slider_scan`]) and stop, the lights (not answered), commands 09 and 0A (answered empty).
fn slider(data: &[u8]) -> Vec<u8> {
    let mut rx = RX.lock().unwrap();
    rx.extend_from_slice(data);
    let mut out = Vec::new();
    while let Some((cmd, _payload)) = next_request(&mut rx) {
        match cmd {
            0x10 => {
                log!("diva: slider reset");
                SCANNING.store(false, Ordering::Relaxed);
                out.extend(slider_frame(cmd, &[]));
            }
            0xF0 => {
                let mut version = [0u8; 32];
                version[..16].copy_from_slice(b"15275   \xA006687\xFF\x90");
                out.extend(slider_frame(cmd, &version));
            }
            0x03 => {
                log!("diva: slider scan started");
                SCANNING.store(true, Ordering::Relaxed);
            }
            0x04 => {
                SCANNING.store(false, Ordering::Relaxed);
                out.extend(slider_frame(cmd, &[]));
            }
            0x09 | 0x0A => out.extend(slider_frame(cmd, &[])),
            0x02 => {}
            _ => log!("diva: slider command {cmd:#04x}"),
        }
    }
    out
}

/// The cells player 1 touches, from the left: the hands' automatic slides ([`Slides`]), the
/// left one while b5 (L1), the left trigger (the brake axis) or the right stick to the left is
/// held, the right one while b6 (R1), the right trigger (the gas axis) or the right stick to the
/// right is. (The touch surface's fingers are the touch screen's.)
fn cells(p1: &StickState, slides: &mut Slides) -> [bool; CELLS] {
    let mut cells = [false; CELLS];
    let rx = p1.axis(Axis::RightX);
    let left = p1.pressed(button::B5) || p1.axis(Axis::Brake) > 16384 || rx < -16384;
    let right = p1.pressed(button::B6) || p1.axis(Axis::Accel) > 16384 || rx > 16384;
    let half = CELLS as i32 / 2;
    let now = Instant::now();
    let step = slides.step.is_none_or(|t| now.duration_since(t) >= SLIDE_STEP);
    if step {
        slides.step = Some(now);
    }
    // each hand: its cell after this step, from the middle outwards, `dir` its direction
    for (held, hand, dir) in [(left, &mut slides.left, -1), (right, &mut slides.right, 1)] {
        if !held {
            *hand = None;
            continue;
        }
        let start = if dir < 0 { half - 1 } else { half };
        let mut cell = hand.unwrap_or(start);
        if step && hand.is_some() {
            cell += dir;
            if !(-SLIDE_GAP..CELLS as i32 + SLIDE_GAP).contains(&cell) {
                cell = start;
            }
        }
        *hand = Some(cell);
        if (0..CELLS as i32).contains(&cell) {
            cells[cell as usize] = true;
        }
    }
    cells
}

/// The cells' report while scanning: 32 pressures from the left (20 touched, segatools' value).
fn slider_scan() -> Vec<u8> {
    if !SCANNING.load(Ordering::Relaxed) {
        return Vec::new();
    }
    let mut state = STATE.lock().unwrap();
    if state.0.is_some_and(|t| t.elapsed() < SCAN_PERIOD) {
        return Vec::new();
    }
    state.0 = Some(Instant::now());
    let cells = cells(&input(0), &mut state.1);
    let pressure: Vec<u8> = cells.iter().map(|&c| if c { 20 } else { 0 }).collect();
    slider_frame(0x01, &pressure)
}
