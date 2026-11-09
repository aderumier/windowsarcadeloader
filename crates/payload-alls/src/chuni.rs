//! CHUNITHM (Sega, Nu 1.1; CHUNITHM STAR, game id SDBT): a game with amLib linked in and no
//! amdaemon, its devices answered in its own process (segatools' chunihook, public domain):
//!
//! - COM1: the 837-15330 ground slider, 32 touch sensors (16 keys, an upper and a lower sensor
//!   each) the game asks to stream (`slider_frame`)
//! - the JVS I/O board's player 1 and 2 switches 5-7: the six air sensors (infrared beams above
//!   the slider, from the lowest: 0 player 1's btn5, 1 player 2's btn5, 2 player 1's btn6...),
//!   on while their beam is clear; a raised hand breaks them from the lowest up
//! - COM10, COM11: the 837-15093-06 LED boards (`led.rs`)
//! - the title server (`chuni/server.rs`), answered in ALL.Net's server
//! - the backup's settings: the cabinet its store's distribution server (`distribution_server`)
//! - free play in amLib's credit record (`WAL_ALLS_FREEPLAY`, else `WAL_FREEPLAY`, default on)
//! - the billing server (`billing.rs`)
//! - COM12: the Aime reader, in `aimeReaderHost.exe` (a program of its own, the game reading the
//!   cards from it through shared memory)
//!
//! The controls (player 1's virtual stick): the left stick touches the left half of the slider
//! (keys 1-8 from the left, picked by the stick's direction from left through up to right), the
//! right stick the right half (keys 9-16); the d-pad (left, down, up, right) and buttons 1, 5,
//! 2, 6 (a pad's X, A, Y, B) touch keys two by two from the left (1-2 ... 15-16), as the
//! keyboard of segatools does; buttons 3, 4, 7, 8 (the shoulders) raise the hand through the air
//! sensors. A touchpad or touchscreen (the profile's `input.touch`): each finger touches the key
//! under it (16 across the surface), a finger in the surface's top third raises the hand.

mod server;

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::{jvs_board, log, serial};
use wal_protocol::{Axis, StickState, button};

const SYNC: u8 = 0xFF;
const ESC: u8 = 0xFD;

/// The game asked the slider to stream its sensors.
static SCANNING: AtomicBool = AtomicBool::new(false);
static RX: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static LAST_SCAN: Mutex<Option<Instant>> = Mutex::new(None);
/// One sensor report every 2 ms at most (the board's ~1 kHz).
const SCAN_PERIOD: Duration = Duration::from_millis(2);

/// A slider frame: sync, command, payload length, payload, the sum of them all negated (the
/// bytes after the sync escaped: `FD`, the byte minus 1).
fn slider_frame(cmd: u8, payload: &[u8]) -> Vec<u8> {
    let mut plain = vec![SYNC, cmd, payload.len() as u8];
    plain.extend_from_slice(payload);
    let sum = plain.iter().fold(0u8, |s, b| s.wrapping_add(*b));
    plain.push(sum.wrapping_neg());
    let mut out = vec![SYNC];
    for &b in &plain[1..] {
        if b == SYNC || b == ESC {
            out.extend([ESC, b - 1]);
        } else {
            out.push(b);
        }
    }
    out
}

/// The next whole request in `rx` (command, payload; its sum checked), removed from it.
fn next_request(rx: &mut Vec<u8>) -> Option<(u8, Vec<u8>)> {
    loop {
        let start = rx.iter().position(|&b| b == SYNC)?;
        rx.drain(..start);
        let mut frame = vec![SYNC];
        let mut escape = false;
        let mut used = None;
        for (i, &b) in rx.iter().enumerate().skip(1) {
            if b == SYNC {
                used = Some((i, false));
                break;
            }
            if b == ESC {
                escape = true;
                continue;
            }
            frame.push(if escape { b.wrapping_add(1) } else { b });
            escape = false;
            if frame.len() >= 3 && frame.len() == frame[2] as usize + 4 {
                used = Some((i + 1, true));
                break;
            }
        }
        let (n, whole) = used?;
        rx.drain(..n);
        if whole && frame.iter().fold(0u8, |s, b| s.wrapping_add(*b)) == 0 {
            return Some((frame[1], frame[3..frame.len() - 1].to_vec()));
        }
    }
}

fn slider(data: &[u8]) -> Vec<u8> {
    let mut rx = RX.lock().unwrap();
    rx.extend_from_slice(data);
    let mut out = Vec::new();
    while let Some((cmd, _payload)) = next_request(&mut rx) {
        match cmd {
            // reset
            0x10 => {
                log!("chuni: slider reset");
                SCANNING.store(false, Ordering::Relaxed);
                out.extend(slider_frame(cmd, &[]));
            }
            // board info: number, firmware (segatools' values), 32 bytes
            0xF0 => {
                let mut version = [0u8; 32];
                version[..16].copy_from_slice(b"15330   \xA006712\xFF\x90");
                out.extend(slider_frame(cmd, &version));
            }
            // auto scan start (not answered), stop
            0x03 => {
                log!("chuni: slider scan started");
                SCANNING.store(true, Ordering::Relaxed);
            }
            0x04 => {
                SCANNING.store(false, Ordering::Relaxed);
                out.extend(slider_frame(cmd, &[]));
            }
            // the slider's lights (not answered)
            0x02 => {}
            _ => log!("chuni: slider command {cmd:#04x}"),
        }
    }
    out
}

/// Player 1's touched keys, from the left (0-15).
fn keys(p1: &StickState) -> [bool; 16] {
    let mut keys = [false; 16];
    // each stick on its half: its direction, from left (key 0) through up to right (key 7)
    for (first, (x, y)) in [(0, (Axis::LeftX, Axis::LeftY)), (8, (Axis::RightX, Axis::RightY))] {
        let (x, y) = (p1.axis(x) as f32 / 32767.0, p1.axis(y) as f32 / 32767.0);
        if x * x + y * y >= 0.5 * 0.5 {
            let from_left = std::f32::consts::PI - (-y).atan2(x).abs();
            keys[first + ((from_left / std::f32::consts::PI * 8.0) as usize).min(7)] = true;
        }
    }
    // the touch surface's fingers (the launcher's `input.touch`): below its top third, the key
    // under each one
    for (x, y) in wal_payload_common::touches() {
        if y >= AIR_ZONE {
            keys[(x as usize * 16 / 65536).min(15)] = true;
        }
    }
    let pairs = [button::LEFT, button::DOWN, button::UP, button::RIGHT, button::B1, button::B5, button::B2, button::B6];
    for (n, b) in pairs.into_iter().enumerate() {
        if p1.pressed(b) {
            keys[2 * n] = true;
            keys[2 * n + 1] = true;
        }
    }
    keys
}

/// The sensors' report while scanning: 32 pressures, numbered from the right (key 16's lower
/// and upper sensors first).
fn slider_scan() -> Vec<u8> {
    if !SCANNING.load(Ordering::Relaxed) {
        return Vec::new();
    }
    let mut last = LAST_SCAN.lock().unwrap();
    if last.is_some_and(|t| t.elapsed() < SCAN_PERIOD) {
        return Vec::new();
    }
    *last = Some(Instant::now());
    let keys = keys(&wal_payload_common::input(0));
    let mut pressure = [0u8; 32];
    for from_left in (0..16).filter(|&k| keys[k]) {
        let from_right = 15 - from_left;
        pressure[2 * from_right] = 128;
        pressure[2 * from_right + 1] = 128;
    }
    slider_frame(0x01, &pressure)
}

/// The touch surface's top third: a finger there raises the hand (the air sensors).
const AIR_ZONE: u16 = 21845;

/// The hand's height over the air sensors (0: none broken, 6: all).
static HAND: AtomicU8 = AtomicU8::new(0);

/// Each beam's switch (player 1's, player 2's switch byte 2), from the lowest beam.
const BEAMS: [(u8, u8); 6] = [(0x20, 0), (0, 0x20), (0x10, 0), (0, 0x10), (0x08, 0), (0, 0x08)];

/// The hand's height after one more read of the air sensors: it rises (or falls) one beam per
/// read, as segatools'.
pub(crate) fn hand() -> u8 {
    let p1 = wal_payload_common::input(0);
    let raised = [button::B3, button::B4, button::B7, button::B8].into_iter().any(|b| p1.pressed(b))
        || wal_payload_common::touches().iter().any(|&(_, y)| y < AIR_ZONE);
    let hand = HAND.load(Ordering::Relaxed);
    let hand = if raised { (hand + 1).min(6) } else { hand.saturating_sub(1) };
    HAND.store(hand, Ordering::Relaxed);
    hand
}

/// The JVS switches the game reads: test, service and the air sensors only (the other
/// buttons touch the slider).
fn switches(players: &mut [jvs_board::Switches]) {
    let hand = hand();
    for p in players.iter_mut().take(2) {
        // service stays (switch byte 1's 0x40)
        p.0 &= 0x40;
        p.1 = 0;
        p.4 = 0;
    }
    for (i, &(b1, b2)) in BEAMS.iter().enumerate() {
        if hand as usize <= i {
            players[0].1 |= b1;
            if let Some(p) = players.get_mut(1) {
                p.1 |= b2;
            }
        }
    }
}

/// The game's settings record in the AMEX EEPROM (at 0x10F0, its copy at 0x1558, 0x468 bytes:
/// CRC-32 of the bytes after it, `TBDS`...): the cabinet the distribution server (+0x80 = 0;
/// the default 1, a client, waits for a server cabinet on the LAN) without a cabinet group
/// (+0x81 = 0), as segatools' guide sets them in the test menu. Set in the backup at start and
/// in the game's writes of the record (a blank backup gets its defaults).
const SETTINGS: [u64; 2] = [0x10F0, 0x1558];
const SETTINGS_SIZE: usize = 0x468;

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

/// A settings record made the distribution server's, when it is one (its id and sum checked).
fn serve_distribution(record: &mut [u8]) -> bool {
    let valid = &record[4..8] == b"TBDS" && u32::from_le_bytes(record[..4].try_into().unwrap()) == crc32(&record[4..]);
    if valid && (record[0x80] != 0 || record[0x81] != 0) {
        record[0x80] = 0;
        record[0x81] = 0;
        let crc = crc32(&record[4..]);
        record[..4].copy_from_slice(&crc.to_le_bytes());
        return true;
    }
    false
}

/// amLib's credit record (backup record 1: at 0x40, its copy at 0x840, 0x50 bytes: CRC-32 of the
/// bytes after it), free play at +0x0A: `WAL_ALLS_FREEPLAY`, else the common `WAL_FREEPLAY`
/// (default on). Set in the backup at start and in the game's writes (a blank backup's first).
const CREDIT: [u64; 2] = [0x40, 0x840];
const CREDIT_SIZE: usize = 0x50;

fn free_play() -> bool {
    wal_payload_common::free_play("WAL_ALLS_FREEPLAY").unwrap_or(true)
}

/// A credit record given the free play setting, when it is one (its sum checked).
fn set_free_play(record: &mut [u8]) -> bool {
    let valid = u32::from_le_bytes(record[..4].try_into().unwrap()) == crc32(&record[4..]) && record.iter().any(|&b| b != 0);
    if valid && record[0x0A] != free_play() as u8 {
        record[0x0A] = free_play() as u8;
        let crc = crc32(&record[4..]);
        record[..4].copy_from_slice(&crc.to_le_bytes());
        return true;
    }
    false
}

/// The game's EEPROM writes: a whole settings record made the distribution server's, a credit
/// record given the free play setting.
fn eeprom_write(offset: u64, data: &mut [u8]) {
    if std::env::var("WAL_CHUNI_DEBUG").is_ok() && offset < 0x19C0 && offset + data.len() as u64 > 0x10F0 {
        log!("chuni: EEPROM write {offset:#x} +{:#x}", data.len());
    }
    for at in SETTINGS {
        let Some(start) = at.checked_sub(offset).map(|s| s as usize) else { continue };
        if let Some(record) = data.get_mut(start..start + SETTINGS_SIZE)
            && serve_distribution(record)
        {
            log!("chuni: the settings written at {at:#x}: the distribution server");
        }
    }
    for at in CREDIT {
        let Some(start) = at.checked_sub(offset).map(|s| s as usize) else { continue };
        if let Some(record) = data.get_mut(start..start + CREDIT_SIZE)
            && set_free_play(record)
        {
            log!("chuni: the credit record written at {at:#x}: free play {}", free_play());
        }
    }
}

/// The game's id record (at 0x1F0, its copy at 0x9F0, 0x30 bytes: CRC-32, 0, `SDBT`, 1).
const ID: [usize; 2] = [0x1F0, 0x9F0];
const ID_SIZE: usize = 0x30;

/// A settings record with the game's defaults, the distribution server's.
fn default_settings() -> Vec<u8> {
    let mut r = vec![0u8; SETTINGS_SIZE];
    r[4..8].copy_from_slice(b"TBDS");
    r[8..0x28].fill(0x14);
    r[0x28] = 0x07;
    r[0x70..0x78].fill(0x06);
    // delivery (server), group (off), group role, continue, music select time, advertise volume
    r[0x80..0x86].copy_from_slice(&[0, 0, 1, 1, 1, 4]);
    let crc = crc32(&r[4..]);
    r[..4].copy_from_slice(&crc.to_le_bytes());
    r
}

/// The settings in the backup at start. A blank backup (no id record) gets the game's id and
/// its default settings, the distribution server's (else its first session sets up the backup
/// as a client's, and waits for a server).
fn distribution_server() {
    const PATH: &str = "Y:\\amex\\EEPROM.bin";
    let mut eeprom = std::fs::read(PATH).unwrap_or_default();
    eeprom.resize(eeprom.len().max(0x2000), 0);
    let mut changed = false;
    let set_up = ID.iter().any(|&at| {
        let r = &eeprom[at..at + ID_SIZE];
        &r[8..12] == b"SDBT" && u32::from_le_bytes(r[..4].try_into().unwrap()) == crc32(&r[4..])
    });
    if !set_up {
        let mut id = vec![0u8; ID_SIZE];
        id[8..12].copy_from_slice(b"SDBT");
        id[12] = 1;
        let crc = crc32(&id[4..]);
        id[..4].copy_from_slice(&crc.to_le_bytes());
        for at in ID {
            eeprom[at..at + ID_SIZE].copy_from_slice(&id);
        }
        for at in SETTINGS.map(|a| a as usize) {
            eeprom[at..at + SETTINGS_SIZE].copy_from_slice(&default_settings());
        }
        log!("chuni: a blank backup: the game's id and default settings written");
        changed = true;
    }
    for at in SETTINGS.map(|a| a as usize) {
        changed |= serve_distribution(&mut eeprom[at..at + SETTINGS_SIZE]);
    }
    for at in CREDIT.map(|a| a as usize) {
        changed |= set_free_play(&mut eeprom[at..at + CREDIT_SIZE]);
    }
    if changed {
        let _ = std::fs::create_dir_all("Y:\\amex");
        match std::fs::write(PATH, &eeprom) {
            Ok(()) => log!("chuni: the cabinet is the distribution server, no cabinet group; free play {}", free_play()),
            Err(e) => log!("chuni: {PATH}: {e}"),
        }
    }
}

/// The ALL.Net library's log line (its log callback's argument: the formatted line).
extern "C" fn alib_log(line: *const std::ffi::c_char) {
    if !line.is_null() {
        log!("alib: {}", unsafe { std::ffi::CStr::from_ptr(line) }.to_string_lossy().trim_end());
    }
}

/// `WAL_CHUNI_DEBUG`: the ALL.Net library's log in the payload log (CHUNITHM STAR 1.20 only, its
/// globals: level 0x1953920, categories 0x1953928, callback 0x195392C), kept set.
fn alib_debug() {
    if std::env::var("WAL_CHUNI_DEBUG").is_err() {
        return;
    }
    let base = wal_payload_common::iat::game_module();
    std::thread::spawn(move || {
        loop {
            unsafe {
                *((base + 0x195_3920) as *mut u32) = 5;
                *((base + 0x195_3928) as *mut u32) = u32::MAX;
                *((base + 0x195_392C) as *mut usize) = alib_log as *const () as usize;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    });
}

/// CHUNITHM SUPERSTAR's game (amdaemon has the IO4 board, its air sensors, and the Aime reader):
/// the slider on COM1, the LED boards on COM2 / COM3 (a CVT cabinet).
pub(crate) fn init_chusan() {
    serial::install_streaming("COM1", slider, slider_scan);
    crate::led::init_pair("COM2", "COM3");
    log!("chuni: slider on COM1, LED boards on COM2 / COM3");
}

/// CHUNITHM SUPERSTAR's amdaemon: the title server and the billing server beside its ALL.Net
/// server (the game's data from its working directory, `App\amdaemon`: `..\data\A000`).
pub(crate) fn init_daemon() {
    server::init();
}

/// The game's devices, before the system's hooks (`daemon.rs`, which pass the serial calls on
/// to these).
pub(crate) fn init() {
    distribution_server();
    alib_debug();
    wal_payload_common::amex::set_eeprom_write_filter(eeprom_write);
    serial::install_streaming("COM1", slider, slider_scan);
    crate::led::init_pair("COM10", "COM11");
    jvs_board::set_switches_hook(switches);
    server::init();
    log!("chuni: slider on COM1, LED boards on COM10 / COM11, air sensors on the JVS switches");
}
