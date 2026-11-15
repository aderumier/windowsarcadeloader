//! The system's backup amdaemon keeps on the AMFS drive (`E:\sysfile.dat`, 0x6000 bytes): the
//! payload sets its DIP switches and free play there before amdaemon reads it (segatools'
//! `platform/system.c`, public domain). Written by amdaemon at its first start: the values
//! apply from the next one.
//!
//! 512-byte blocks, each a CRC-32 of its bytes 4..512 first: the credit block at 0x0000 (copy at
//! 0x3000; free play at +10), the DIP switch block at 0x2800 (copy at 0x5800; the switches
//! at +8, bit 0 = DIP 1).
//!
//! amdaemon writes its configuration's credit settings over the backup's at start (a game's
//! `config_common.json` with `"credit": {"config": {"freeplay": false}}`: O.N.G.E.K.I.'s free play
//! undone): the free play setting is also written into the profile's `config_wal.json` (the
//! configuration file named so on amdaemon's command line, read last).

use wal_payload_common::log;

const PATH: &str = "E:\\sysfile.dat";
const SIZE: usize = 0x6000;
const BLOCK: usize = 512;

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

/// Sets the byte at `offset` of the block at `at` and its copy at `copy`, their sums updated.
fn set(file: &mut [u8], at: usize, copy: usize, offset: usize, value: u8) {
    let block = &mut file[at..at + BLOCK];
    block[offset] = value;
    let sum = crc32(&block[4..]);
    block[..4].copy_from_slice(&sum.to_le_bytes());
    let block = file[at..at + BLOCK].to_vec();
    file[copy..copy + BLOCK].copy_from_slice(&block);
}

/// The free play setting in the profile's `config_wal.json` (amdaemon's configuration read
/// last): its `credit.config.freeplay`.
fn config_free_play(free_play: bool) {
    use wal_payload_common::json::Json;
    let Some(path) = std::env::args().find(|a| a.to_ascii_lowercase().ends_with("config_wal.json")) else { return };
    let Some(mut doc) = std::fs::read_to_string(&path).ok().and_then(|t| Json::parse(&t)).filter(|d| matches!(d, Json::Obj(_))) else {
        log!("alls: {path}: unreadable, its free play unchanged");
        return;
    };
    let mut credit = doc.get("credit").cloned().unwrap_or(Json::obj());
    let mut config = credit.get("config").cloned().unwrap_or(Json::obj());
    if config.get("freeplay") == Some(&Json::Bool(free_play)) {
        return;
    }
    config.set("freeplay", free_play);
    credit.set("config", config);
    doc.set("credit", credit);
    match std::fs::write(&path, doc.to_string()) {
        Ok(()) => log!("alls: {path}: free play {free_play}"),
        Err(e) => log!("alls: {path}: {e}"),
    }
}

pub(crate) fn apply() {
    config_free_play(wal_payload_common::free_play("WAL_ALLS_FREEPLAY").unwrap_or(true));
    let Ok(mut file) = std::fs::read(PATH) else {
        log!("alls: no {PATH} yet (amdaemon's first start): DIP switches and free play from the next one");
        return;
    };
    if file.len() != SIZE {
        log!("alls: {PATH}: {} bytes, left as is", file.len());
        return;
    }
    let dipsw = std::env::var("WAL_ALLS_DIPSW").ok().and_then(|v| u8::from_str_radix(v.trim(), 16).ok()).unwrap_or(0x01);
    let free_play = wal_payload_common::free_play("WAL_ALLS_FREEPLAY").unwrap_or(true);
    let before = file.clone();
    set(&mut file, 0x0000, 0x3000, 10, free_play as u8);
    set(&mut file, 0x2800, 0x5800, 8, dipsw);
    if file != before {
        if let Err(e) = std::fs::write(PATH, &file) {
            log!("alls: {PATH}: {e}");
            return;
        }
    }
    log!("alls: system DIP switches {dipsw:02x}, free play {free_play}");
}
