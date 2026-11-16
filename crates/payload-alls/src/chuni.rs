//! CHUNITHM (Sega, Nu 1.1; CHUNITHM STAR, game id SDBT): a game with amLib linked in and no
//! amdaemon, its devices answered in its own process (segatools' chunihook, public domain):
//!
//! - COM1: the 837-15330 ground slider, 32 touch sensors (16 keys, an upper and a lower sensor
//!   each) the game asks to stream (`slider_frame`)
//! - the JVS I/O board's player 1 and 2 switches 5-7: the six air sensors (infrared beams above
//!   the slider, from the lowest: 0 player 1's btn5, 1 player 2's btn5, 2 player 1's btn6...),
//!   on while their beam is clear; a raised hand breaks them from the lowest up
//! - COM10, COM11: the 837-15093-06 LED boards (payload-common `led.rs`)
//! - the title server (`chuni/server.rs`), answered in ALL.Net's server
//! - the backup's settings: the cabinet its store's distribution server (`distribution_server`)
//! - free play in amLib's credit record (`WAL_ALLS_FREEPLAY`, else `WAL_FREEPLAY`, default on)
//! - the billing server (payload-common `billing.rs`)
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

/// The mouse's place in the picture while its left button (`kp0`) is held: the profile's
/// `input.gun` maps it to the pedals (the sticks touch the slider).
fn mouse_touch(p1: &StickState) -> Option<(f32, f32)> {
    if !p1.pressed(button::KP0) {
        return None;
    }
    wal_payload_common::gun::pedals_in_picture(0)
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
    // the mouse (NEW!!'s profile: on the pedals, its left button `kp0`): below the picture's top
    // third, the key under it while the button is held
    if let Some((x, y)) = mouse_touch(p1)
        && y >= 1.0 / 3.0
    {
        keys[((x * 16.0) as usize).min(15)] = true;
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
        || wal_payload_common::touches().iter().any(|&(_, y)| y < AIR_ZONE)
        || mouse_touch(&p1).is_some_and(|(_, y)| y < 1.0 / 3.0);
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
pub(crate) const CREDIT: [u64; 2] = [0x40, 0x840];
pub(crate) const CREDIT_SIZE: usize = 0x50;

pub(crate) fn free_play() -> bool {
    wal_payload_common::free_play("WAL_ALLS_FREEPLAY").unwrap_or(true)
}

/// A credit record given the free play setting, when it is one (its sum checked).
pub(crate) fn set_free_play(record: &mut [u8]) -> bool {
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

/// NEW!!'s settings record in its backup (`Y:\\SDHD\\appfile.dat`, kept by amdaemon): its
/// CRC-32 over the 0x5FC bytes after it, `DHDS`, then the settings, the cabinet group at +0x80
/// (0 off, 1 group A...), the group role, continue, music select time, advertise volume.
const CHUSAN_SETTINGS_SIZE: usize = 0x600;
const CHUSAN_GROUP: usize = 0x80;

/// The backup's settings made a lone cabinet's as amdaemon reads or writes them (no cabinet
/// group: a fresh backup's default is group A, whose power-on waits forever for the group's
/// reference cabinet).
fn lone_cabinet(data: &mut [u8]) {
    let mut from = 0;
    while let Some(pos) = data[from..].windows(4).position(|w| w == b"DHDS") {
        let tag = from + pos;
        from = tag + 4;
        let Some(at) = tag.checked_sub(4) else { continue };
        if at + CHUSAN_SETTINGS_SIZE > data.len() {
            break;
        }
        let r = &mut data[at..at + CHUSAN_SETTINGS_SIZE];
        if r[CHUSAN_GROUP] == 0 || u32::from_le_bytes(r[..4].try_into().unwrap()) != crc32(&r[4..]) {
            continue;
        }
        r[CHUSAN_GROUP] = 0;
        let crc = crc32(&r[4..]);
        r[..4].copy_from_slice(&crc.to_le_bytes());
        log!("chuni: the backup's settings: no cabinet group");
    }
}

/// The settings' first bytes (NEW!!'s defaults: 32 volumes at 20, then 7), where the record's
/// body starts in the game's memory.
const CHUSAN_SETTINGS_START: [u8; 33] = {
    let mut s = [0x14u8; 33];
    s[32] = 7;
    s
};

/// A fresh backup's settings, in the game's memory: its defaults (group A) are kept there until
/// the game saves them (in globals of the program), and its power-on waits for group A's
/// reference cabinet meanwhile. The program's writable pages are searched every 20 ms (before the
/// game reads the group from them) for the settings' body (their first bytes, then the group 1-8,
/// continue on or off, the volume 0-20), the group set off (its CRC-32 before the body,
/// when there is one, made again), for the boot's first 15 minutes.
fn lone_cabinet_in_game() {
    use windows_sys::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_READWRITE, PAGE_WRITECOPY, VirtualQuery};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    const BODY: usize = 0x460;
    const GROUP: usize = CHUSAN_GROUP - 8;
    std::thread::spawn(|| {
        let started = Instant::now();
        let mut chunk = Vec::new();
        let image = wal_payload_common::iat::game_module();
        // the image's size (its PE header's SizeOfImage)
        let end = unsafe {
            let pe = image + *((image + 0x3c) as *const u32) as usize;
            image + *((pe + 0x50) as *const u32) as usize
        };
        while started.elapsed() < Duration::from_secs(15 * 60) {
            std::thread::sleep(Duration::from_millis(20));
            let mut addr = image;
            let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
            while addr < end && unsafe { VirtualQuery(addr as *const _, &mut info, std::mem::size_of_val(&info)) } != 0 {
                let (base, size) = (info.BaseAddress as usize, info.RegionSize);
                if info.State == MEM_COMMIT && matches!(info.Protect, PAGE_READWRITE | PAGE_WRITECOPY) && size >= BODY + 4 {
                    chunk.resize(size, 0);
                    if unsafe { ReadProcessMemory(GetCurrentProcess(), base as *const _, chunk.as_mut_ptr().cast(), size, std::ptr::null_mut()) } != 0 {
                        let mut i = 4;
                        while i + BODY <= size {
                            if chunk[i] == 0x14 && chunk[i + 32] == 7 && chunk[i..i + 33] == CHUSAN_SETTINGS_START {
                                let body = &chunk[i..i + BODY];
                                if (1..=8).contains(&body[GROUP]) && body[GROUP + 2] <= 1 && body[GROUP + 4] <= 20 {
                                    let mut fixed = body.to_vec();
                                    fixed[GROUP] = 0;
                                    let had_crc = u32::from_le_bytes(chunk[i - 4..i].try_into().unwrap()) == crc32(body);
                                    let mut record = if had_crc { crc32(&fixed).to_le_bytes().to_vec() } else { chunk[i - 4..i].to_vec() };
                                    record.extend_from_slice(&fixed);
                                    if unsafe { WriteProcessMemory(GetCurrentProcess(), (base + i - 4) as *const _, record.as_ptr().cast(), record.len(), std::ptr::null_mut()) } != 0 {
                                        log!("chuni: the settings in memory at {:#x}: no cabinet group (CRC {had_crc})", base + i);
                                    }
                                }
                                i += 33;
                            } else {
                                i += 1;
                            }
                        }
                    }
                }
                addr = base + size;
            }
        }
    });
}

/// CHUNITHM SUPERSTAR's and NEW!!'s game (amdaemon has the IO4 board, its air sensors, and the
/// Aime reader): the slider on COM1, the LED boards on COM2 / COM3 (a CVT cabinet).
pub(crate) fn init_chusan() {
    serial::install_streaming("COM1", slider, slider_scan);
    wal_payload_common::led::init_pair("COM2", "COM3");
    // a red dot where the mouse is (the common crosshair option, `WAL_CROSSHAIR`)
    wal_payload_common::crosshair::init();
    if std::env::var("WAL_ALLS_GAME_ID").is_ok_and(|v| v.trim() == "SDHD") {
        lone_cabinet_in_game();
    }
    log!("chuni: slider on COM1, LED boards on COM2 / COM3");
}

/// CHUNITHM SUPERSTAR's and NEW!!'s amdaemon: the title server and the billing server beside
/// its ALL.Net server (the game's data from its working directory: `..\data\A000`), no cabinet
/// group in the backup (`lone_cabinet`).
pub(crate) fn init_daemon() {
    server::init();
    if std::env::var("WAL_ALLS_GAME_ID").is_ok_and(|v| v.trim() == "SDHD") {
        crate::daemon::set_backup_filter(lone_cabinet);
    }
}

/// The game's devices, before the system's hooks (`daemon.rs`, which pass the serial calls on
/// to these).
pub(crate) fn init() {
    distribution_server();
    alib_debug();
    wal_payload_common::amex::set_eeprom_write_filter(eeprom_write);
    serial::install_streaming("COM1", slider, slider_scan);
    wal_payload_common::led::init_pair("COM10", "COM11");
    jvs_board::set_switches_hook(switches);
    server::init();
    log!("chuni: slider on COM1, LED boards on COM10 / COM11, air sensors on the JVS switches");
}
