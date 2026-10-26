//! Banapass card reader (NFC, PN532 frames) on a serial port: ported from bananatools'
//! `board/bpreader.c` (public domain, https://gitea.tendokyu.moe/TeamTofuShop/bananatools).
//!
//! The host writes `55` (wake up), `00 00 FF 00 FF 00` (ACK) and commands
//! `00 00 FF len lcs D4 cmd data... dcs 00`; each command gets one reply
//! `00 00 FF len lcs D5 cmd+1 data... dcs 00`. No card is ever on the reader.

use wal_payload_common::{log, serial};

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
        // InListPassiveTarget: no card
        0x4A => frame(0x4B, &[0x00, 0x00, 0x00]),
        0x44 | 0x52 => frame(cmd + 1, &[0x01, 0x00]),
        0x54 => frame(0x55, &[0x00]),
        _ => {
            log!("bpreader: unknown command {cmd:02x} {data:02X?}");
            frame(cmd + 1, &[])
        }
    };
    reply
}
