//! Banapass card reader (NFC, PN532 frames) on a serial port: ported from bananatools'
//! `board/bpreader.c` (public domain, https://gitea.tendokyu.moe/TeamTofuShop/bananatools).
//!
//! The host writes `55` (wake up), `00 00 FF 00 FF 00` (ACK) and commands
//! `00 00 FF len lcs D4 cmd data... dcs 00`; each command gets one reply
//! `00 00 FF len lcs D5 cmd+1 data... dcs 00`.
//!
//! A card ([`insert`], Wangan Midnight Maximum Tune 5) answers the Mifare poll
//! (`4A 01 00`: target 1, ATQA 0004, SAK 08, its UID) and the Mifare commands of
//! InDataExchange (`40 01 cmd`): authentication with the NBGI key A fails (status 1), with key
//! B it succeeds, then the reads of blocks 0..2 (`banapass.rs`). The FeliCa poll finds none.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use wal_payload_common::{log, serial};

use crate::banapass::Card;

/// The card on the reader, until when.
static CARD: Mutex<Option<(&'static Card, Instant)>> = Mutex::new(None);

/// Puts `card` on the reader for `time`.
pub(crate) fn insert(card: &'static Card, time: Duration) {
    *CARD.lock().unwrap() = Some((card, Instant::now() + time));
}

fn card() -> Option<&'static Card> {
    CARD.lock().unwrap().filter(|(_, until)| Instant::now() < *until).map(|(c, _)| c)
}

/// The player's card: `WAL_NAMCOES3_ACCESS_CODE` (20 digits), else a card made once and kept
/// in `F:\banapass.txt`.
pub(crate) fn player_card() -> &'static Card {
    player_card_in("F:\\banapass.txt")
}

/// [`player_card`], the card made once kept in the file `path`.
pub(crate) fn player_card_in(path: &str) -> &'static Card {
    let configured = std::env::var("WAL_NAMCOES3_ACCESS_CODE").ok().filter(|c| !c.trim().is_empty());
    let saved = std::fs::read_to_string(path).ok();
    let card = configured.or(saved).and_then(|c| Card::from_access_code(c.trim())).unwrap_or_else(|| {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
        let card = Card::generate(seed);
        if let Err(e) = std::fs::write(path, &card.access_code) {
            log!("bpreader: {path}: {e}");
        }
        card
    });
    log!("bpreader: Banapass card {}", card.access_code);
    Box::leak(Box::new(card))
}

/// Puts `card` on the reader while player 1's `card` button is held, and 3 seconds after.
pub(crate) fn card_button(card: &'static Card) {
    std::thread::spawn(move || loop {
        if wal_payload_common::input(0).pressed(wal_protocol::button::CARD) {
            insert(card, Duration::from_secs(3));
        }
        std::thread::sleep(Duration::from_millis(50));
    });
}

/// Puts `card` on the reader a second after player 1 presses `start`, for 8 seconds (the game
/// asks for the card once the credit is started).
pub(crate) fn card_on_start(card: &'static Card) {
    card_on_start_for(card, Duration::from_secs(8));
}

/// [`card_on_start`], the card staying `time` on the reader.
pub(crate) fn card_on_start_for(card: &'static Card, time: Duration) {
    card_on_button(card, wal_protocol::button::START, time);
}

/// Puts `card` on the reader a second after player 1 presses `button`, for `time` (Pokkén
/// Tournament reads the card between the coin and start).
pub(crate) fn card_on_button(card: &'static Card, button: u32, time: Duration) {
    std::thread::spawn(move || {
        let mut held = false;
        loop {
            let pressed = wal_payload_common::input(0).pressed(button);
            if pressed && !held {
                std::thread::sleep(Duration::from_secs(1));
                insert(card, time);
                log!("bpreader: card inserted automatically");
            }
            held = pressed;
            std::thread::sleep(Duration::from_millis(50));
        }
    });
}

/// The card [`card_on_poll_after`] puts on the reader at the game's polls, until it is read.
static ARMED: Mutex<Option<&'static Card>> = Mutex::new(None);

/// When [`card_on_poll_after`]'s card was last read: the button does not put it back for a
/// game's length (start also dismisses the game's messages after the card is read).
static READ_AT: Mutex<Option<Instant>> = Mutex::new(None);
const REARM_AFTER: Duration = Duration::from_secs(180);

/// After player 1 presses `button`, puts `card` on the reader (3 seconds) whenever the game looks
/// for a card, until the game reads it (Pokkén Tournament: start, its card question answered
/// yes, the card prompt).
pub(crate) fn card_on_poll_after(card: &'static Card, button: u32) {
    std::thread::spawn(move || {
        let mut held = false;
        loop {
            let pressed = wal_payload_common::input(0).pressed(button);
            if pressed && !held && READ_AT.lock().unwrap().is_none_or(|t| t.elapsed() >= REARM_AFTER) {
                *ARMED.lock().unwrap() = Some(card);
            }
            held = pressed;
            std::thread::sleep(Duration::from_millis(50));
        }
    });
}

pub(crate) fn init(port: &str, modules: &[usize]) {
    serial::install_in(port, process, modules);
    log!("bpreader: card reader on {port}");
}

/// One reply frame: `D5 code data`.
fn frame(code: u8, data: &[u8]) -> Vec<u8> {
    let len = (data.len() + 2) as u8;
    let mut out = vec![0x00, 0x00, 0xFF, len, len.wrapping_neg(), 0xD5, code];
    out.extend_from_slice(data);
    let sum = out[5..].iter().fold(0u8, |s, b| s.wrapping_add(*b));
    out.extend_from_slice(&[sum.wrapping_neg(), 0x00]);
    out
}

fn process(data: &[u8]) -> Vec<u8> {
    // wake up, ACK, or anything else without a command
    if data.len() < 7 || data[0] == 0x55 || data[3] == 0x00 || data[5] != 0xD4 {
        return Vec::new();
    }
    let cmd = data[6];
    let reply = match cmd {
        // ReadRegister: the answers to the reader's own checks
        0x06 if data.get(8) == Some(&0x1C) => frame(0x07, &[0xFF, 0x3F, 0x0E, 0xF1, 0xFF, 0x3F, 0x0E, 0xF1]),
        0x06 => frame(0x07, &[0xDC, 0xF4, 0x3F, 0x11, 0x4D, 0x85, 0x61, 0xF1, 0x26, 0x6A, 0x87]),
        // WriteRegister
        0x08 => frame(0x09, &[0x00]),
        // ReadGPIO
        0x0C => frame(0x0D, &[0x00, 0x06, 0x00]),
        // WriteGPIO (lamp, beep), SetParameters, reset, RFConfiguration
        0x0E | 0x12 | 0x18 | 0x32 => frame(cmd + 1, &[]),
        // InListPassiveTarget: the Mifare poll (baud rate 00) finds the card, else none
        0x4A => match (data.get(8), {
            if let Some(armed) = *ARMED.lock().unwrap()
                && card().is_none()
            {
                insert(armed, Duration::from_secs(3));
            }
            card()
        }) {
            (Some(0x00), Some(c)) => {
                let mut target = vec![0x01, 0x01, 0x00, 0x04, 0x08, 0x04];
                target.extend_from_slice(&c.uid());
                frame(0x4B, &target)
            }
            _ => frame(0x4B, &[0x00]),
        },
        // InDataExchange with target 1: Mifare authentication (60 key A, 61 key B), read (30)
        0x40 => match (data.get(8), data.get(9), card()) {
            (_, _, None) => frame(0x41, &[0x01]),
            (Some(0x60), ..) => frame(0x41, &[0x01]),
            (Some(0x61), ..) => frame(0x41, &[0x00]),
            (Some(0x30), Some(&n), Some(c)) => {
                if ARMED.lock().unwrap().take().is_some() {
                    *READ_AT.lock().unwrap() = Some(Instant::now());
                    log!("bpreader: card read (inserted automatically)");
                }
                let mut out = vec![0x00];
                out.extend_from_slice(&c.block(n as usize));
                frame(0x41, &out)
            }
            _ => frame(0x41, &[0x00]),
        },
        0x44 | 0x52 => frame(cmd + 1, &[0x01, 0x00]),
        0x54 => frame(0x55, &[0x00]),
        _ => {
            log!("bpreader: unknown command {cmd:02x} {data:02X?}");
            frame(cmd + 1, &[])
        }
    };
    if card().is_some() && std::env::var("WAL_NAMCOES3_CARD_LOG").is_ok_and(|v| v == "1") {
        log!("bpreader: {:02X?} -> {:02X?}", &data[5..data.len().saturating_sub(2)], &reply[5..reply.len().saturating_sub(2)]);
    }
    reply
}
