//! The Shell's TCP service (127.0.0.1:18888) the game connects to.
//!
//! Packets both ways: `TFM`, the command id (2 bytes, big endian), the data length (2 bytes, big
//! endian), 4 zero bytes, the data. Game commands 0x00-0x1c (`GAME_MSG_*`: keepalive, launched,
//! attract, start pressed, game start, shots, recoil, game end...), Shell commands 0x40-0x44
//! (`SHELL_MSG_SET_SKU`, `SETCREDIT` (data: the credit count), `SECURITY_AUTHENTICATE`,
//! `RESET`, `EXIT`).
//!
//! The Shell owns the credits: coins (the virtual sticks' coin buttons) add one and send the
//! new count.
//!
//! Showdown (`WAL_SEGASHELL_PROTOCOL=dsw`, an earlier version): magic `DSW`, one numbering for
//! both ways (game 0x00-0x0c, `GAME_MSG_*`: nop, ack, launched, assets loaded, credit used,
//! game start / end / continue, security challenge, event, state, shutdown; Shell 0x0d-0x1a,
//! `SHELL_MSG_*`: `SETCREDIT` 0x0d (data: the credit count), `SECURITY_AUTHENTICATE`, `EXIT`,
//! `SET_SKU`, keypad, bonus codes, motion base, QR code, spoofed button). The game waits for
//! `SET_SKU` (the cabinet's configuration) after its `LAUNCHED_OK`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::time::Duration;

use wal_payload_common::log;
use wal_protocol::button;

const PORT: u16 = 18888;

const GAME_MSG: [&str; 0x1d] = [
    "KEEPALIVE",
    "LAUNCHED_OK",
    "ASSETS_LOADED",
    "ATTRACT_START",
    "ATTRACT_LOGO",
    "ATTRACT_VIDEO",
    "START_PRESSED",
    "GAME_START",
    "PLAYERS_IN_GAME",
    "STAGE_LOADING",
    "STAGE_START",
    "CURRENT_SCORE",
    "CURRENT_HEALTH",
    "SHOT_FIRED",
    "ENEMY_HIT",
    "TRIGGER_RECOIL",
    "STAGE_COMPLETED",
    "STAGE_RESULTS",
    "MISSION_SELECT_ENTERED",
    "MISSION_SELECT_CHOSEN",
    "PLAY_END",
    "GAME_END",
    "CONTINUE_OFFERED",
    "CONTINUE_CHOSEN",
    "HIGHSCORE_ACHIEVED",
    "HIGHSCORE_ENTRY",
    "INITIAL_CONFIRMED",
    "SECURITY_CHALLENGE",
    "SHUTDOWN",
];

const DSW_MSG: [&str; 0x1c] = [
    "NOP",
    "ACK",
    "?",
    "LAUNCHED_OK",
    "ASSETS_LOADED",
    "CREDITUSED",
    "GAMESTART",
    "GAMEEND",
    "GAMECONTINUE",
    "SECURITY_CHALLENGE",
    "EVENT",
    "STATE",
    "GAMESHUTDOWN",
    "SETCREDIT",
    "SECURITY_AUTHENTICATE",
    "EXIT",
    "SET_SKU",
    "KEYPAD_PRESSED",
    "BONUS_CODE",
    "BONUS_CODE_VALID",
    "BONUS_CODE_INVALID",
    "BIT_CODE",
    "SHELL_BONUS_CODE",
    "?",
    "MOTIONBASE_ENABLED",
    "QRCODE_READY",
    "SPOOF_BUTTON",
    "KEYBOARD_SPOOF",
];

/// The Shell's protocol version: its packets' magic, its message names, `SETCREDIT`.
struct Protocol {
    magic: &'static [u8; 3],
    names: &'static [&'static str],
    set_credit: u16,
    /// Messages not logged (keepalives, events every frame or so).
    quiet: &'static [u16],
    /// The game's launched message, answered with its cabinet configuration (`SET_SKU`).
    launched: Option<u16>,
}

const TFM: Protocol = Protocol { magic: b"TFM", names: &GAME_MSG, set_credit: 0x41, quiet: &[0x00, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x1d], launched: None };
const DSW: Protocol = Protocol { magic: b"DSW", names: &DSW_MSG, set_credit: 0x0d, quiet: &[0x00], launched: Some(0x03) };

/// DSW `SET_SKU` data (`WAL_SEGASHELL_SKU`, hex): 0 ?, 1 surround audio, 2 motion base,
/// 3 seat belt, 4 age rating shown, 5 external screen, 6 Blue Ripple logo shown, 7 -, 8-11
/// screen preset (u32: 0 legacy, 1 standard, 2 super deluxe), 12-15 default view (u32: 0
/// bonnet, 1 chase).
fn sku() -> Vec<u8> {
    let hex = std::env::var("WAL_SEGASHELL_SKU").unwrap_or_else(|_| "00000000000000000100000001000000".into());
    let hex: String = hex.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    (0..hex.len() / 2).filter_map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()).collect()
}

fn protocol() -> &'static Protocol {
    static P: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *P.get_or_init(|| std::env::var("WAL_SEGASHELL_PROTOCOL").is_ok_and(|v| v.trim().eq_ignore_ascii_case("dsw"))) { &DSW } else { &TFM }
}

/// The game's connection, for the coin thread's credit updates.
static GAME: Mutex<Option<TcpStream>> = Mutex::new(None);
static CREDITS: Mutex<u8> = Mutex::new(0);

pub(crate) fn start() {
    let listener = match TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(l) => l,
        Err(e) => {
            log!("shell: cannot listen on {PORT}: {e}");
            return;
        }
    };
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || serve(stream));
        }
    });
    std::thread::spawn(coins);
    log!("shell: listening on {PORT}");
}

fn packet(id: u16, seq: [u8; 4], data: &[u8]) -> Vec<u8> {
    let mut p = protocol().magic.to_vec();
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&(data.len() as u16).to_be_bytes());
    p.extend_from_slice(&seq);
    p.extend_from_slice(data);
    p
}

fn send(id: u16, data: &[u8]) {
    if let Some(stream) = GAME.lock().unwrap().as_mut() {
        let _ = stream.write_all(&packet(id, [0; 4], data));
    }
}

fn serve(mut stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    log!("shell: game connected");
    *GAME.lock().unwrap() = stream.try_clone().ok();
    send(protocol().set_credit, &[*CREDITS.lock().unwrap()]);
    loop {
        let mut header = [0u8; 11];
        if stream.read_exact(&mut header).is_err() {
            break;
        }
        if &header[..3] != protocol().magic {
            log!("shell: unknown packet {header:02X?}");
            break;
        }
        let id = u16::from_be_bytes([header[3], header[4]]);
        let mut data = vec![0u8; u16::from_be_bytes([header[5], header[6]]) as usize];
        if stream.read_exact(&mut data).is_err() {
            break;
        }
        let name = protocol().names.get(id as usize).copied().unwrap_or("?");
        // keepalives, per-shot events and the lamps every frame or so: not logged
        if !protocol().quiet.contains(&id) {
            log!("shell: game {id:#04x} {name} {:02X?} {data:02X?}", &header[7..]);
        }
        // DSW: the game launched, the cabinet's configuration (SET_SKU, without it the game
        // quits after 5 seconds), then the credits
        if protocol().launched == Some(id) {
            let sku = sku();
            log!("shell: cabinet configuration {sku:02X?}");
            let _ = stream.write_all(&packet(0x10, [0; 4], &sku));
            let _ = stream.write_all(&packet(protocol().set_credit, [0; 4], &[*CREDITS.lock().unwrap()]));
        }
    }
    *GAME.lock().unwrap() = None;
    log!("shell: game disconnected");
}

/// Coins: a press of any player's coin button adds a credit.
fn coins() {
    let mut held = [false; 4];
    loop {
        std::thread::sleep(Duration::from_millis(20));
        for (p, h) in held.iter_mut().enumerate() {
            let down = wal_payload_common::input(p).pressed(button::COIN);
            if down && !*h {
                let credits = {
                    let mut c = CREDITS.lock().unwrap();
                    *c = c.saturating_add(1);
                    *c
                };
                log!("shell: coin, {credits} credit(s)");
                send(protocol().set_credit, &[credits]);
            }
            *h = down;
        }
    }
}
