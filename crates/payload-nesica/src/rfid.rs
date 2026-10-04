//! NESiCA card reader emulation (port of WindowsLoader's RfidEmu).
//!
//! The Taito RFID board sits on a serial port (`COM2`) and speaks JVS framing; the port is
//! emulated by `wal_payload_common::serial`.
//!
//! The card is inserted/removed by the virtual stick `card` input of any player (toggle).
//!
//! Options: `WAL_NESICA_RFID=0` disables it, `WAL_NESICA_RFID_PORT` (default `COM2`),
//! `WAL_NESICA_CARD_ID` (16 digits, default the WindowsLoader card `7020392010281502`).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use wal_payload_common::jvs::{self, Encoder, REPORT_OK};
use wal_payload_common::{log, serial};
use wal_protocol::button;
const BOARD_ID: &[u8] = b"TAITO CORP.;RFID CTRL P.C.B.;Ver1.00;";
const CARD_HEADER: [u8; 8] = [0x04, 0xC2, 0x3D, 0xDA, 0x6F, 0x52, 0x80, 0x00];

static INSERTED: AtomicBool = AtomicBool::new(false);
static CARD: OnceLock<[u8; 0x18]> = OnceLock::new();

fn card_data() -> &'static [u8; 0x18] {
    CARD.get_or_init(|| {
        let id = std::env::var("WAL_NESICA_CARD_ID").unwrap_or_else(|_| "7020392010281502".into());
        let mut data = [b'0'; 0x18];
        data[..8].copy_from_slice(&CARD_HEADER);
        for (d, c) in data[8..].iter_mut().zip(id.bytes()) {
            *d = c;
        }
        data
    })
}

/// Answers one request packet of the game (WindowsLoader `process_stream`).
fn process(packet: &[u8]) -> Vec<u8> {
    let Some(req) = jvs::parse(packet) else { return Vec::new() };
    if !matches!(req.node, 0x00 | 0x01 | 0xFF) {
        return Vec::new();
    }
    let arg = |c: &[u8], i: usize| c.get(i).copied().unwrap_or(0);
    let mut r = Encoder::new();
    let mut cmds = req.commands;
    while !cmds.is_empty() {
        let step = match cmds[0] {
            0xF0 => 2, // bus reset
            0xF1 => {
                r.push(REPORT_OK);
                serial::READY.store(true, Ordering::Relaxed);
                2
            }
            0x01 | 0x03 => {
                r.extend(&[REPORT_OK, 1]);
                2
            }
            0x04 => {
                r.push(REPORT_OK);
                1
            }
            0x05 => {
                r.push(REPORT_OK);
                3
            }
            0x10 => {
                r.push(REPORT_OK);
                r.extend(BOARD_ID);
                r.push(0);
                1
            }
            0x11 => {
                r.extend(&[REPORT_OK, 0x13]); // command format revision
                1
            }
            0x12 => {
                r.extend(&[REPORT_OK, 0x30]); // JVS revision
                1
            }
            0x13 => {
                r.extend(&[REPORT_OK, 0x10]); // communication revision
                1
            }
            0x14 => {
                r.extend(&[REPORT_OK, 1, 7, 0, 8, 0, 0x12, 8, 0, 0, 0]); // features
                1
            }
            0x20 => {
                r.extend(&[REPORT_OK, 0, 0, 0, 0, 0]); // switches
                3
            }
            0x21 => {
                r.extend(&[REPORT_OK, 0, 0, 0, 0]); // coins
                2
            }
            0x26 => {
                // general purpose input: card presence
                let n = arg(cmds, 1) as usize;
                r.push(REPORT_OK);
                let v = if INSERTED.load(Ordering::Relaxed) { 0x19 } else { 0 };
                for _ in 0..n {
                    r.push(v);
                }
                2 + n
            }
            0x2F => 1, // retransmit
            0x30 | 0x31 => {
                r.push(REPORT_OK); // coin decrease / payout
                4
            }
            0x32 => {
                // general purpose output: card data
                let n = arg(cmds, 1) as usize;
                r.push(REPORT_OK);
                if INSERTED.load(Ordering::Relaxed) {
                    r.extend(card_data());
                } else {
                    for _ in 0..n * 0x18 {
                        r.push(0);
                    }
                }
                r.push(REPORT_OK);
                2 + n
            }
            other => {
                log!("rfid: unknown command {other:#04x}");
                r.push(REPORT_OK);
                1
            }
        };
        cmds = &cmds[step.min(cmds.len())..];
    }
    r.finish()
}

pub(crate) fn init() {
    if std::env::var("WAL_NESICA_RFID").is_ok_and(|v| v == "0") {
        return;
    }
    let port = std::env::var("WAL_NESICA_RFID_PORT").unwrap_or_else(|_| "COM2".into());
    serial::install(&port, process);

    // card insert/remove: toggled by the `card` virtual input
    std::thread::spawn(|| {
        let mut held = false;
        loop {
            let pressed = (0..wal_protocol::MAX_PLAYERS).any(|p| wal_payload_common::input(p).pressed(button::CARD));
            if pressed && !held {
                let now = !INSERTED.fetch_xor(true, Ordering::Relaxed);
                log!("rfid: card {}", if now { "inserted" } else { "removed" });
            }
            held = pressed;
            std::thread::sleep(Duration::from_millis(30));
        }
    });
}
