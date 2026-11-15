//! The cabinet's shell over TCP for Men In Black and Mission: Impossible (SEGA Amusements,
//! Unity): coins, credits, security challenges. The games share the shell's protocol and
//! security key; their header and operation numbers differ ([`Protocol`]).
//!
//! The game (`MIB.MIBClient`, its `ServerIP` 127.0.0.1) connects to the shell at port 18888 and
//! quits when it is not connected (`ProcessMessage`, every 0.15 s). A message is an 11-byte
//! header, `"MIB"`, the operation (u16 big-endian), the data length (u16 big-endian), a sequence
//! number (u32 big-endian), then the data; each message received is acknowledged (`ACK`, the
//! sequence number). Operations (`MIB.MIBOpCode`):
//!
//! * game -> shell: 0 keepalive, 1 ack, 6 security challenge (4 bytes, every 10 s), 101 launched,
//!   104 assets loaded, 200 attract, 301 start pressed (the player), 303 players in game, 504
//!   shot fired, 511 recoil, 600 tickets won, 700 play end, 704 continue, 705 game end...
//! * shell -> game: 66 credits (player 0 = the common count, the count, part credit, maximum),
//!   67 the answer to a challenge (4 bytes)
//!
//! The answer is checked by `MIBCore.AuthenticatedData` (a keyed scramble of the 4 bytes
//! compared with the challenge); more than 4 challenges without a valid answer and the game
//! quits. The payload answers with the bytes that scramble into the challenge ([`answer`]).
//!
//! Credits: the game reads its free play and costs from `C:\Sega\ShellData\ShellData.ini`
//! (`[Credit]`); the shell counts the coins. The payload counts player 1's and player 2's
//! coins in the common count and sends it; each player started (301) takes one (the game waits
//! for the lower count before another player can join). In free play the game ignores counts.
//!
//! Mission: Impossible (`ShellManager`, the same address): header `"MIG"`, the sequence number
//! always 0, nothing acknowledged. Game -> shell: 0 keepalive, 1 launched, 2 assets loaded, 3
//! attract, 6 start pressed (the player; also a continue), 13 shot fired, 15 recoil, 27 security
//! challenge (`AuthenticatedData`, the same scramble and key)...; shell -> game: 65 credits
//! (count, part credit, part credits per credit: 0 keeps the game's), 66 the answer, 68 quit.
//! The game takes its credits itself (`CreditManager.Spend`) and the count it is sent replaces
//! its own; it is sent at attract (the credit manager does not exist yet at launch).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

use wal_payload_common::log;
use wal_protocol::button;

const PORT: u16 = 18888;
const MAX_CREDITS: u32 = 99;

/// A game's messages.
pub(crate) struct Protocol {
    header: &'static [u8; 3],
    /// The acknowledgement each message received gets.
    ack: Option<u16>,
    challenge: u16,
    authenticate: u16,
    set_credits: u16,
    credits_data: fn(u8) -> Vec<u8>,
    /// The game's message after which it takes credit counts.
    ready: u16,
    start_pressed: u16,
    /// Frequent messages, not logged: keepalives, shots, recoil, hits.
    quiet: &'static [u16],
    /// The stage select entered, its stage chosen ([`STAGE_SELECT`]).
    stage_select: Option<(u16, u16)>,
}

/// When the game's stage select was entered (milliseconds since the shell started), 0 when
/// not shown.
pub(crate) static STAGE_SELECT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Milliseconds since the shell started.
pub(crate) fn now_ms() -> u64 {
    STARTED.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64 + 1
}

pub(crate) const MIB: Protocol = Protocol {
    header: b"MIB",
    ack: Some(1),
    challenge: 6,
    authenticate: 67,
    set_credits: 66,
    credits_data: |c| vec![0, c, 0, MAX_CREDITS as u8],
    ready: 101,
    start_pressed: 301,
    quiet: &[0, 1, 504, 505, 506, 508, 511, 513],
    stage_select: None,
};

pub(crate) const MIG: Protocol = Protocol {
    header: b"MIG",
    ack: None,
    challenge: 27,
    authenticate: 66,
    set_credits: 65,
    credits_data: |c| vec![c, 0, 0],
    ready: 3,
    start_pressed: 6,
    quiet: &[0, 11, 12, 13, 14, 15, 29, 38],
    stage_select: Some((33, 34)),
};

static PROTOCOL: std::sync::OnceLock<&Protocol> = std::sync::OnceLock::new();

fn protocol() -> &'static Protocol {
    PROTOCOL.get().unwrap_or(&&MIB)
}

static GAME: Mutex<Option<TcpStream>> = Mutex::new(None);
static SEQ: AtomicU32 = AtomicU32::new(0);
static CREDITS: AtomicU32 = AtomicU32::new(0);
static FREE_PLAY: AtomicBool = AtomicBool::new(false);

/// `MIBCore.SECURITY_KEY`, `DECRYPT_ARRAY`.
const KEY: [u8; 8] = [134, 236, 144, 241, 223, 198, 90, 119];
const DECRYPT: [u8; 10] = [64, 8, 16, 4, 1, 2, 4, 2, 32, 1];

/// One of `AuthenticatedData`'s 12 rounds (`round` 11 down to 0): the 4 bytes shifted left as one
/// big-endian number, each byte's carry flipped by a key bit, a parity bit of the bytes shifted in.
fn round(round: u8, data: [u8; 4]) -> [u8; 4] {
    let mut out = data;
    let mut carry = data[0] >> 7;
    let mut index = KEY[1] as usize;
    for byte in data {
        if index > 9 {
            index = 0;
        }
        if byte & DECRYPT[index] != 0 {
            carry ^= 1;
        }
        index += 1;
    }
    let key_bits = KEY[5] ^ 0x63;
    let mut mask: u8 = 1 << ((round + 2) & 7);
    for i in (0..4).rev() {
        if mask == 0 {
            mask = 0x80;
        }
        let next = (data[i] >> 7) ^ u8::from(key_bits & mask != 0);
        out[i] = (data[i] << 1).wrapping_add(carry);
        carry = next;
        mask >>= 1;
    }
    out
}

/// `MIBCore.AuthenticatedData`'s scramble of an answer, which must give the challenge.
fn scramble(mut data: [u8; 4]) -> [u8; 4] {
    let first = (KEY[1] << 4).wrapping_add(KEY[3]);
    data.iter_mut().for_each(|b| *b ^= first);
    for r in (0..12).rev() {
        data = round(r, data);
    }
    let second = !(KEY[0] << 4).wrapping_add(KEY[4]);
    for (i, b) in data.iter_mut().enumerate() {
        if KEY[3] & (1 << (i & 3)) != 0 {
            *b = b.reverse_bits();
        }
        *b ^= second;
    }
    data.iter_mut().for_each(|b| *b ^= KEY[7]);
    data
}

/// The answer to a challenge: [`scramble`] undone. A round keeps every bit but the first byte's
/// top one: its input is the output shifted back, the top bits tried until the round gives the
/// output again.
fn answer(challenge: [u8; 4]) -> [u8; 4] {
    let mut data = challenge;
    data.iter_mut().for_each(|b| *b ^= KEY[7]);
    let second = !(KEY[0] << 4).wrapping_add(KEY[4]);
    for (i, b) in data.iter_mut().enumerate() {
        *b ^= second;
        if KEY[3] & (1 << (i & 3)) != 0 {
            *b = b.reverse_bits();
        }
    }
    for r in 0..12 {
        let target = data;
        data = (0..16u8)
            .map(|tops| std::array::from_fn(|i| (target[i] >> 1) | ((tops >> i) & 1) << 7))
            .find(|&input| round(r, input) == target)
            .unwrap_or(target);
    }
    let first = (KEY[1] << 4).wrapping_add(KEY[3]);
    data.iter_mut().for_each(|b| *b ^= first);
    data
}

fn send(op: u16, data: &[u8]) {
    let mut msg = protocol().header.to_vec();
    msg.extend(op.to_be_bytes());
    msg.extend((data.len() as u16).to_be_bytes());
    msg.extend((SEQ.fetch_add(1, Ordering::Relaxed) + 1).to_be_bytes());
    msg.extend(data);
    if let Some(game) = GAME.lock().unwrap().as_mut()
        && let Err(e) = game.write_all(&msg)
    {
        log!("shell: operation {op} to the game: {e}");
    }
}

fn send_credits() {
    let credits = CREDITS.load(Ordering::Relaxed);
    log!("shell: {credits} credits");
    send(protocol().set_credits, &(protocol().credits_data)(credits as u8));
}

/// The game's messages, from one connection.
fn serve(mut game: TcpStream) {
    let p = protocol();
    let _ = game.set_nodelay(true);
    *GAME.lock().unwrap() = game.try_clone().ok();
    let mut header = [0u8; 11];
    loop {
        if game.read_exact(&mut header).is_err() {
            break;
        }
        if &header[..3] != p.header {
            log!("shell: bad header {header:02x?}");
            break;
        }
        let op = u16::from_be_bytes([header[3], header[4]]);
        let seq = &header[7..11];
        let mut data = vec![0u8; u16::from_be_bytes([header[5], header[6]]) as usize];
        if game.read_exact(&mut data).is_err() {
            break;
        }
        if let Some(ack) = p.ack
            && op != ack
        {
            send(ack, seq);
        }
        match op {
            _ if op == p.challenge => match <[u8; 4]>::try_from(data.as_slice()) {
                Ok(challenge) => send(p.authenticate, &answer(challenge)),
                Err(_) => log!("shell: challenge of {} bytes", data.len()),
            },
            _ if op == p.ready => {
                log!("shell: game ready ({op}), free play {}", FREE_PLAY.load(Ordering::Relaxed));
                send_credits();
            }
            _ if op == p.start_pressed => {
                log!("shell: start pressed {data:02x?}");
                if !FREE_PLAY.load(Ordering::Relaxed) {
                    let _ = CREDITS.try_update(Ordering::Relaxed, Ordering::Relaxed, |c| c.checked_sub(1));
                    send_credits();
                }
            }
            _ if p.stage_select.is_some_and(|(entered, chosen)| op == entered || op == chosen) => {
                let entered = p.stage_select.is_some_and(|(e, _)| op == e);
                STAGE_SELECT.store(if entered { now_ms() } else { 0 }, Ordering::Relaxed);
                log!("shell: stage select {}", if entered { "entered".to_string() } else { format!("chose {data:02x?}") });
            }
            _ if p.quiet.contains(&op) => {}
            _ => log!("shell: game operation {op} {data:02x?}"),
        }
    }
    log!("shell: the game disconnected");
    *GAME.lock().unwrap() = None;
}

/// Listens before the game starts; counts the coins.
pub(crate) fn init(protocol: &'static Protocol, free_play: bool) {
    let _ = PROTOCOL.set(protocol);
    FREE_PLAY.store(free_play, Ordering::Relaxed);
    match TcpListener::bind(("127.0.0.1", PORT)) {
        Ok(listener) => {
            std::thread::spawn(move || {
                for game in listener.incoming().flatten() {
                    log!("shell: the game connected");
                    std::thread::spawn(move || serve(game));
                }
            });
        }
        Err(e) => log!("shell: TCP port {PORT}: {e}"),
    }
    std::thread::spawn(|| {
        let mut held = [false; 2];
        loop {
            for (player, held) in held.iter_mut().enumerate() {
                let coin = wal_payload_common::input(player).pressed(button::COIN);
                if coin && !*held && !FREE_PLAY.load(Ordering::Relaxed) {
                    CREDITS.try_update(Ordering::Relaxed, Ordering::Relaxed, |c| Some((c + 1).min(MAX_CREDITS))).ok();
                    send_credits();
                }
                *held = coin;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_scramble_into_challenges() {
        let mut x: u32 = 0x1234_5678;
        for _ in 0..2000 {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let c = x.to_le_bytes();
            // the game's challenges: two random bytes, then xors of them
            let challenge = [c[0], c[1], c[0] ^ c[1], c[1] ^ (c[0] ^ c[1])];
            assert_eq!(scramble(answer(challenge)), challenge);
            assert_eq!(scramble(answer(c)), c);
        }
    }
}
