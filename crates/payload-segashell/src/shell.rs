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

const SHELL_SETCREDIT: u16 = 0x41;

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

fn packet(id: u16, data: &[u8]) -> Vec<u8> {
    let mut p = b"TFM".to_vec();
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&(data.len() as u16).to_be_bytes());
    p.extend_from_slice(&[0; 4]);
    p.extend_from_slice(data);
    p
}

fn send(id: u16, data: &[u8]) {
    if let Some(stream) = GAME.lock().unwrap().as_mut() {
        let _ = stream.write_all(&packet(id, data));
    }
}

fn serve(mut stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    log!("shell: game connected");
    *GAME.lock().unwrap() = stream.try_clone().ok();
    send(SHELL_SETCREDIT, &[*CREDITS.lock().unwrap()]);
    loop {
        let mut header = [0u8; 11];
        if stream.read_exact(&mut header).is_err() || &header[..3] != b"TFM" {
            break;
        }
        let id = u16::from_be_bytes([header[3], header[4]]);
        let mut data = vec![0u8; u16::from_be_bytes([header[5], header[6]]) as usize];
        if stream.read_exact(&mut data).is_err() {
            break;
        }
        let name = GAME_MSG.get(id as usize).copied().unwrap_or("?");
        // keepalives, per-shot events and the lamps (0x1d) every frame or so: not logged
        if !matches!(id, 0x00 | 0x0b..=0x0f | 0x1d) {
            log!("shell: game {id:#04x} {name} {data:02X?}");
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
                send(SHELL_SETCREDIT, &[credits]);
            }
            *h = down;
        }
    }
}
