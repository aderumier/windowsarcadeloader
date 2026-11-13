//! The cabinet's Sega Shell over TCP (Daytona Championship USA): coins, credits, security.
//!
//! The game connects once, when it starts (`soc::ShellCommImpl`, `0x582930`), to the shell at
//! 127.0.0.1:18888 (`WAL_EUROPAR_SHELL_TCP=<port>`). A message is an 11-byte header, `"DYT"`, the
//! message number (u16 big-endian), the data length (u16 big-endian), 4 zero bytes, then the
//! data. Message numbers (the names table at `0x152fbc0`):
//!
//! * game -> shell: 0 keepalive, 1 launched, 2 assets loaded, 3 attract start, 6 start pressed,
//!   ..., 36 game over, 42 security challenge (4 bytes, every 10 s), 43 shutdown
//! * shell -> game (`0x582370`): 44 the cabinet SKU, 45 the credit count (1 byte), 46 the answer
//!   to the challenge (4 bytes), 48 exit, 49..51 party mode
//!
//! The New Season Edition waits for the SKU before it starts (`0x4dd18b`, its handler `0x5844e1`
//! takes any): the payload sends it when the game says it is launched (1). The first edition
//! ignores it.
//!
//! The answer is checked by `0x582500` (a scramble of the 4 bytes compared with the challenge);
//! more than 4 challenges without a valid answer and the game stops (`0x5b9470`). The profile's
//! code patch makes that check accept any answer: the payload answers each challenge.
//!
//! Credits: the payload counts player 1's coins and sends the count; each game started (6) takes
//! one. In free play it sends 99 and takes none. Not before the game has loaded (2 assets loaded,
//! 3 attract start): it connects at its very start, and a credit count then (`0x48cee0`, its coin
//! object not made yet) corrupts its heap.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use wal_payload_common::log;
use wal_protocol::button;

const FREE_PLAY_CREDITS: u32 = 99;

const MSG_LAUNCHED_OK: u16 = 1;
const MSG_ASSETS_LOADED: u16 = 2;
const MSG_ATTRACT_START: u16 = 3;
const MSG_START_PRESSED: u16 = 6;
const MSG_SECURITY_CHALLENGE: u16 = 42;
const MSG_SET_SKU: u16 = 44;
const MSG_SET_CREDIT: u16 = 45;
const MSG_SECURITY_AUTHENTICATE: u16 = 46;

const NAMES: [&str; 55] = [
    "keepalive", "launched ok", "assets loaded", "attract start", "attract logo", "attract video",
    "start pressed", "mode entered", "mode selected", "waiting for challengers", "entry closed",
    "players in game", "track select entered", "track select changed", "track select chosen",
    "car select entered", "car select changed", "car select chosen", "transmission select entered",
    "transmission select changed", "transmission select chosen", "track loading", "rolling start",
    "countdown", "current positions", "wall impact", "player impact", "overtake status",
    "pit status", "time extended", "time out", "gear shift", "lap completed", "race leader",
    "race over", "final results", "game over", "highscore achieved", "highscore entry",
    "highscore value", "initial current", "initial confirmed", "security challenge", "shutdown",
    "set sku", "set credit", "security authenticate", "reset", "exit", "party mode request",
    "party mode start", "party mode end", "party mode acknowledge", "continue offered",
    "continue chosen",
];

static GAME: Mutex<Option<TcpStream>> = Mutex::new(None);
static CREDITS: AtomicU32 = AtomicU32::new(0);
static FREE_PLAY: AtomicBool = AtomicBool::new(false);
/// The game has loaded: credit counts accepted.
static READY: AtomicBool = AtomicBool::new(false);

fn name(id: u16) -> &'static str {
    NAMES.get(id as usize).copied().unwrap_or("?")
}

fn send(id: u16, data: &[u8]) {
    let mut msg = vec![b'D', b'Y', b'T'];
    msg.extend(id.to_be_bytes());
    msg.extend((data.len() as u16).to_be_bytes());
    msg.extend([0; 4]);
    msg.extend(data);
    if let Some(game) = GAME.lock().unwrap().as_mut() {
        if let Err(e) = game.write_all(&msg) {
            log!("shell: {} to the game: {e}", name(id));
        }
    }
}

fn credits() -> u32 {
    if FREE_PLAY.load(Ordering::Relaxed) { FREE_PLAY_CREDITS } else { CREDITS.load(Ordering::Relaxed) }
}

fn send_credits() {
    if !READY.load(Ordering::Relaxed) {
        return;
    }
    let credits = credits();
    log!("shell: {credits} credits");
    send(MSG_SET_CREDIT, &[credits as u8]);
}

/// The game's messages, from one connection.
fn serve(mut game: TcpStream) {
    let _ = game.set_nodelay(true);
    *GAME.lock().unwrap() = game.try_clone().ok();
    let mut header = [0u8; 11];
    loop {
        if game.read_exact(&mut header).is_err() {
            break;
        }
        if &header[..3] != b"DYT" {
            log!("shell: bad header {header:02x?}");
            break;
        }
        let id = u16::from_be_bytes([header[3], header[4]]);
        let mut data = vec![0u8; u16::from_be_bytes([header[5], header[6]]) as usize];
        if game.read_exact(&mut data).is_err() {
            break;
        }
        match id {
            0 => {}
            MSG_LAUNCHED_OK => {
                log!("shell: game message {id} ({}), the cabinet SKU sent", name(id));
                send(MSG_SET_SKU, &[0, 0]);
            }
            MSG_ASSETS_LOADED | MSG_ATTRACT_START => {
                log!("shell: game message {id} ({})", name(id));
                if !READY.swap(true, Ordering::Relaxed) {
                    send_credits();
                }
            }
            MSG_SECURITY_CHALLENGE => send(MSG_SECURITY_AUTHENTICATE, &data),
            MSG_START_PRESSED => {
                log!("shell: game message {id} ({}) {data:02x?}", name(id));
                if !FREE_PLAY.load(Ordering::Relaxed) {
                    let _ = CREDITS.try_update(Ordering::Relaxed, Ordering::Relaxed, |c| c.checked_sub(1));
                }
                send_credits();
            }
            _ => log!("shell: game message {id} ({}) {data:02x?}", name(id)),
        }
    }
    log!("shell: the game disconnected");
    *GAME.lock().unwrap() = None;
}

/// The game's free play setting (`settings.rs`).
pub(crate) fn set_free_play(on: bool) {
    FREE_PLAY.store(on, Ordering::Relaxed);
}

/// `WAL_EUROPAR_SHELL_TCP=<port>`: listening before the game starts (it connects once).
pub(crate) fn init() -> bool {
    let Some(port) = std::env::var("WAL_EUROPAR_SHELL_TCP").ok().and_then(|v| v.trim().parse::<u16>().ok()) else {
        return false;
    };
    if let Some(on) = wal_payload_common::free_play("WAL_EUROPAR_FREEPLAY") {
        set_free_play(on);
    }
    log!("shell: TCP port {port}, free play {}", FREE_PLAY.load(Ordering::Relaxed));
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => {
            std::thread::spawn(move || {
                for game in listener.incoming().flatten() {
                    log!("shell: the game connected");
                    std::thread::spawn(move || serve(game));
                }
            });
        }
        Err(e) => log!("shell: TCP port {port}: {e}"),
    }
    // the coin switch
    std::thread::spawn(|| {
        let mut held = false;
        loop {
            let coin = wal_payload_common::input(0).pressed(button::COIN);
            if coin && !held {
                if FREE_PLAY.load(Ordering::Relaxed) {
                    log!("shell: coin (free play)");
                } else {
                    CREDITS.try_update(Ordering::Relaxed, Ordering::Relaxed, |c| Some((c + 1).min(FREE_PLAY_CREDITS))).ok();
                    send_credits();
                }
            }
            held = coin;
            std::thread::sleep(Duration::from_millis(5));
        }
    });
    true
}
