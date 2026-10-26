//! Wangan Midnight Maximum Tune 5's card terminal (the "Wangan terminal" of the cabinet's
//! store network): the drive cabinet does not start without one. The terminal multicasts its
//! packets to the cabinets (UDP 225.0.0.1:50765); the payload sends them to the cabinet's
//! socket (bound to any address, port 50765) on the loopback instead: the cabinet does not
//! check where they come from or to, and local multicast is often dropped (a firewall, the
//! loopback without multicast). They come from 127.0.0.2: the cabinet keeps one node per
//! address, and its own packets come back from 127.0.0.1 (`wmmt5_net.rs`).
//!
//! A packet: the link group (1), the sender's node (the terminal: 4), the length (u16 LE, the
//! header and the message), a `wm5.protobuf.Message`, then the CRC-32/MPEG-2 of the rest (big
//! endian). The cabinet drops the terminal when no packet comes for a few seconds.
//!
//! The message: a heartbeat (frame number, the terminal serial: its 5th digit, the store
//! group, must be the cabinet's), the store's settings (`batch_setting`: coin costs, free play
//! `WAL_NAMCOES3_FREEPLAY`, closing hours, time zone) every other packet, the information and
//! feature version of the terminal. The cabinet links when the settings' sync time changes
//! (the current time, a new one each second) and their software revision is its own.

use std::net::{Ipv4Addr, UdpSocket};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use wal_payload_common::log;

const PORT: u16 = 50765;
const LINK_GROUP: u8 = 1;
const TERMINAL_NODE: u8 = 4;
const TERMINAL_SERIAL: &str = "272211990002";
/// The cabinet's software revision (a global of the game).
const SOFTWARE_REVISION: usize = 0x1948dbc;

/// Protocol buffer fields.
struct Message(Vec<u8>);

impl Message {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn varint(&mut self, mut v: u64) {
        loop {
            let b = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                self.0.push(b);
                return;
            }
            self.0.push(b | 0x80);
        }
    }

    fn int(mut self, field: u32, v: u64) -> Self {
        self.varint((field << 3) as u64);
        self.varint(v);
        self
    }

    fn bytes(mut self, field: u32, v: &[u8]) -> Self {
        self.varint((field << 3 | 2) as u64);
        self.varint(v.len() as u64);
        self.0.extend_from_slice(v);
        self
    }

    fn message(self, field: u32, m: Message) -> Self {
        self.bytes(field, &m.0)
    }
}

fn crc32_mpeg2(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
        }
    }
    crc
}

/// The terminal's packet number `frame`; with the store settings when `settings`.
fn packet(frame: u32, settings: bool, free_play: bool, revision: u32) -> Vec<u8> {
    // race_car: an empty car of rank 4, as an idle terminal sends
    let mut car = Message::new().bytes(1, b"");
    car = car.int(2, 4);
    for field in 3..=10 {
        car = car.int(field, 0);
    }
    let course = if free_play { Message::new().bytes(11, b"") } else { Message::new() };
    let mut m = Message::new()
        .message(2, car)
        .message(3, course)
        .message(5, Message::new().int(1, frame as u64).bytes(2, TERMINAL_SERIAL.as_bytes()).int(3, 0));
    if free_play {
        // batch_setting_state
        m = m.int(6, 3);
    }
    if settings {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        // coin chute, buy card, game, continue, full course costs, free play, wins and
        // remains, event mode and distance, closing type, closing hours (96: none),
        // software revision, sync time, event mode count, time zone, daylight saving
        let mut s = Message::new();
        for (field, v) in [(1, 1), (2, 3), (3, 2), (4, 2), (5, 4), (6, free_play as u64), (7, 1), (8, 0), (9, 0), (10, 2)] {
            s = s.int(field, v);
        }
        for field in 11..=18 {
            s = s.int(field, 96);
        }
        for (field, v) in [(19, revision as u64), (20, now), (21, 4), (22, 36), (23, 0)] {
            s = s.int(field, v);
        }
        m = m.message(8, s);
    }
    // information: system mode 1, sync id -1 (zigzag 1)
    m = m.message(9, Message::new().int(1, 1).int(2, 1).int(3, 0).int(4, 0));
    // feature version 100, May 2014
    m = m.message(10, Message::new().int(1, 100).int(2, 2014).int(3, 5).int(4, 0).int(5, 0));

    let len = (4 + m.0.len()) as u16;
    let mut out = vec![LINK_GROUP, TERMINAL_NODE];
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&m.0);
    let crc = crc32_mpeg2(&out);
    out.extend_from_slice(&crc.to_be_bytes());
    out
}

pub(crate) fn start(base: usize) {
    let free_play = std::env::var("WAL_NAMCOES3_FREEPLAY").map_or(true, |v| v != "0");
    std::thread::spawn(move || {
        let socket = match UdpSocket::bind((Ipv4Addr::new(127, 0, 0, 2), 0)) {
            Ok(s) => s,
            Err(e) => {
                log!("wmmt5 terminal: socket: {e}");
                return;
            }
        };
        log!("wmmt5 terminal: sending to {}:{PORT} (free play {free_play})", Ipv4Addr::LOCALHOST);
        let mut frame = 0u32;
        loop {
            frame = frame.wrapping_add(1);
            // the cabinet's software revision (its batch settings must carry the same)
            let revision = unsafe { *((base + SOFTWARE_REVISION) as *const u32) };
            if let Err(e) = socket.send_to(&packet(frame, frame % 2 == 0, free_play, revision), (Ipv4Addr::LOCALHOST, PORT)) {
                if frame < 5 {
                    log!("wmmt5 terminal: send: {e}");
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_of_a_captured_packet() {
        // a terminal heartbeat (frame 0x12), its CRC as the terminal sends it
        let mut p = packet(0x12, false, true, 0);
        let crc = p.split_off(p.len() - 4);
        assert_eq!(crc, crc32_mpeg2(&p).to_be_bytes());
        assert_eq!(p.len(), 75);
        assert_eq!(&crc, &[0xec, 0x72, 0x00, 0x41]);
    }
}
