//! The cabinet's image information on the AMFS drive (`E:\ICF1`, its copy `E:\ICF2`): the
//! installed system and game versions, and the game's id amdaemon reads it from. Dumps without
//! them (House of the Dead: Scarlet Dawn) get one written before amdaemon starts: without it
//! the game id stays unset ("Game ID is invalid", "Disconnect dongle") and the game stops at
//! its first request.
//!
//! AES-128-CBC (no padding: 0x40-byte entries), its key and IV built by amdaemon from three
//! obfuscated 32-byte tables (`key[i] = A[i] ^ B[i+10] ^ C[i+20]`, `iv[i] = A[i+16] ^ B[i+26]
//! ^ C[i+4]`). Plain layout: a 0x40-byte header (CRC-32 of the bytes after it, the size, the
//! entry count, game id, platform id, the XOR of the entries' CRC-32s), then the entries:
//! `02 01 00 00`, the kind (0 system, 1 game, 0x101 patch, 2 option), at 0x20 the version
//! (build, minor, major u16), its date (year u16, month, day, hour, minute, second, 0), the
//! system version it needs.

use aes::Aes128;
use aes::cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};
use wal_payload_common::log;

use crate::registry::Cabinet;

const KEY: [u8; 16] = [0x09, 0xca, 0x5e, 0xfd, 0x30, 0xc9, 0xaa, 0xef, 0x38, 0x04, 0xd0, 0xa7, 0xe3, 0xfa, 0x71, 0x20];
const IV: [u8; 16] = [0xb1, 0x55, 0xc2, 0x2c, 0x2e, 0x7f, 0x04, 0x91, 0xfa, 0x7f, 0x0f, 0xdc, 0x21, 0x7a, 0xff, 0x90];

/// The system version written (0040.01.01, as other ALLS images) and the game's (1.00.00).
const SYSTEM: [u8; 4] = [1, 1, 40, 0];
const GAME: [u8; 4] = [0, 0, 1, 0];
/// 2018-01-01 00:00:00
const DATE: [u8; 8] = [0xe2, 0x07, 1, 1, 0, 0, 0, 0];

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn entry(kind: u32, version: [u8; 4]) -> [u8; 0x40] {
    let mut e = [0u8; 0x40];
    e[0..4].copy_from_slice(&0x0102u32.to_le_bytes());
    e[4..8].copy_from_slice(&kind.to_le_bytes());
    e[0x20..0x24].copy_from_slice(&version);
    e[0x24..0x2c].copy_from_slice(&DATE);
    e[0x2c..0x30].copy_from_slice(&SYSTEM);
    e
}

/// The plain image information: the system, the game.
fn plain(cab: &Cabinet) -> Vec<u8> {
    let entries = [entry(0, SYSTEM), entry(1, GAME)];
    let mut d = vec![0u8; 0x40];
    let size = (0x40 + entries.len() * 0x40) as u32;
    d[4..8].copy_from_slice(&size.to_le_bytes());
    d[0x10..0x18].copy_from_slice(&(entries.len() as u64).to_le_bytes());
    d[0x18..0x1c].iter_mut().zip(cab.game_id.bytes()).for_each(|(d, s)| *d = s);
    d[0x1c..0x1f].iter_mut().zip(cab.platform_id.bytes()).for_each(|(d, s)| *d = s);
    let sums = entries.iter().fold(0, |x, e| x ^ crc32(e));
    d[0x20..0x24].copy_from_slice(&sums.to_le_bytes());
    for e in &entries {
        d.extend_from_slice(e);
    }
    let sum = crc32(&d[4..]);
    d[0..4].copy_from_slice(&sum.to_le_bytes());
    d
}

fn encrypt(data: &[u8]) -> Vec<u8> {
    let cipher = Aes128::new(GenericArray::from_slice(&KEY));
    let mut prev = IV;
    let mut out = Vec::with_capacity(data.len());
    for block in data.chunks(16) {
        let mut b = GenericArray::from([0u8; 16]);
        for i in 0..16 {
            b[i] = block[i] ^ prev[i];
        }
        cipher.encrypt_block(&mut b);
        prev.copy_from_slice(&b);
        out.extend_from_slice(&b);
    }
    out
}

/// Writes `E:\ICF1` and `E:\ICF2` when the dump has none.
pub(crate) fn ensure(cab: &Cabinet) {
    if std::path::Path::new("E:\\ICF1").exists() {
        return;
    }
    let icf = encrypt(&plain(cab));
    for path in ["E:\\ICF1", "E:\\ICF2"] {
        if let Err(e) = std::fs::write(path, &icf) {
            log!("alls: {path}: {e}");
            return;
        }
    }
    log!("alls: E:\\ICF1, ICF2 written (game {}, platform {})", cab.game_id, cab.platform_id);
}
