//! Luigi's Mansion Arcade's I/O board on COM1 (115200 bauds, `game.bat` flashes its firmware
//! `LMAIO.bin`): both vacuum guns' aims and buttons, the coins, the cabinet's switches; the
//! game's lamps, solenoids and motors (`WAL_SEGANU_LMAIO=COM1`).
//!
//! ASCII packets. The game sends one every 28 ms:
//! `/s\n\nCO:<T|F>,<n>\nSW:..\nSO:..\nMO:..\nST:..\n/e\n` (`CO:T,<n>`: the `n` coins counted
//! taken off the board's counter, repeated until the board reports none; coins inserted
//! meanwhile are reported after its `CO:F`). The board answers lines the game looks for in what it read
//! (`strstr`, `sscanf`), up to an ETX byte (0x03):
//! - `G1:<a1>,<b1>,<a2>,<b2>` (hex) the guns: `a` the x reading (12 bits) and the buttons
//!   (0x1000 the player's start button, 0x2000 action, 0x4000 trigger, 0x8000 off the screen), `b` the y reading (12
//!   bits, from the bottom) and 0x8000 (a valid reading, player 1's taken for both)
//! - `CO:<c>,<n>` the coins counted, `SW:<w1>,<w2>,<w3>` (hex) the switches (the first word's
//!   0x4: test), `ST:<a>,<b>,<c>` the vacuum guns' board status (not 0: error 3002)
//! - `:V,<version>` the firmware's version
//!
//! The game maps a reading to its 1920x1080 picture with the guns' calibration, in its board
//! object (`[[exe + WAL_SEGANU_LMAIO_OBJECT]]`): `+0x5c` x min, `+0x60` y min, `+0x64` x max,
//! `+0x68` y max: x = (a - 256 - min) / (max - min) * 1920, y = (1535 - b - 256 - min) /
//! (max - min) * 1080. Set to 0..1024 at each answer: the readings span the picture exactly.
//!
//! From the virtual sticks: the gun aims (`lx`, `ly`), b1 the trigger, b2 the action button,
//! start, b4 points the gun off the screen; coin adds a coin;
//! `WAL_SEGANU_LMAIO_SW`: the switches' bits, `<player><button>=<word>:<mask>,...` (e.g.
//! `1start=0:4`: player 1's start button, mask 4 of the first word).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use wal_payload_common::{gun, input, log, serial};
use wal_protocol::button as b;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

const SPAN: f32 = 1024.0;

static OBJECT: AtomicUsize = AtomicUsize::new(0);
/// Coins inserted, not taken by the game yet.
static COINS: AtomicU32 = AtomicU32::new(0);
/// The game is taking coins (`CO:T`): taken once, none reported until its `CO:F`.
static TAKING: AtomicBool = AtomicBool::new(false);
/// Players' buttons at the last answer (coin edges).
static LAST: Mutex<[u32; 2]> = Mutex::new([0; 2]);
/// Switch bits: (player, button, word, mask).
static SWITCHES: Mutex<Vec<(usize, u32, usize, u32)>> = Mutex::new(Vec::new());

pub(crate) fn init() {
    let Ok(port) = std::env::var("WAL_SEGANU_LMAIO") else { return };
    let port = port.trim().to_string();
    if let Some(rva) = std::env::var("WAL_SEGANU_LMAIO_OBJECT").ok().and_then(|v| usize::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok()) {
        OBJECT.store(rva, Ordering::Relaxed);
    }
    let mut switches = SWITCHES.lock().unwrap();
    for item in std::env::var("WAL_SEGANU_LMAIO_SW").unwrap_or_default().split(',').filter(|s| !s.trim().is_empty()) {
        let parsed = item.split_once('=').and_then(|(name, bit)| {
            let name = name.trim();
            let player = name.get(..1)?.parse::<usize>().ok()?.checked_sub(1).filter(|&p| p < 2)?;
            let button = b::NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(&name[1..]))?.1;
            let (word, mask) = bit.split_once(':')?;
            Some((player, button, word.trim().parse::<usize>().ok()?.min(2), u32::from_str_radix(mask.trim().trim_start_matches("0x"), 16).ok()?))
        });
        match parsed {
            Some(s) => switches.push(s),
            None => log!("lmaio: ignoring switch '{item}'"),
        }
    }
    drop(switches);
    serial::install(&port, packet);
    log!("lmaio: I/O board on {port}");
}

/// The game's packet: the coins it took, then the board's state.
fn packet(data: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(data);
    match text.split("CO:T,").nth(1).and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next()).and_then(|n| n.parse::<u32>().ok()) {
        Some(n) => {
            if !TAKING.swap(true, Ordering::Relaxed) {
                let _ = COINS.try_update(Ordering::Relaxed, Ordering::Relaxed, |c| Some(c.saturating_sub(n)));
            }
        }
        None if text.contains("CO:F") => TAKING.store(false, Ordering::Relaxed),
        None => {}
    }
    state()
}

/// The guns' calibration set to the readings' span.
fn calibrate() {
    let rva = OBJECT.load(Ordering::Relaxed);
    if rva == 0 {
        return;
    }
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let object = unsafe { ((base + rva) as *const usize).read() };
    if object == 0 {
        return;
    }
    let object = unsafe { (object as *const usize).read() };
    if object == 0 {
        return;
    }
    for (offset, value) in [(0x5c, 0), (0x60, 0), (0x64, SPAN as i32), (0x68, SPAN as i32)] {
        unsafe { ((object + offset) as *mut i32).write_unaligned(value) };
    }
}

fn state() -> Vec<u8> {
    calibrate();
    let mut guns = [(0u32, 0u32); 2];
    let mut words = [0u32; 3];
    let mut last = LAST.lock().unwrap();
    for (p, gun_state) in guns.iter_mut().enumerate() {
        let buttons = input(p).buttons;
        if buttons & !last[p] & b::COIN != 0 {
            COINS.fetch_add(1, Ordering::Relaxed);
        }
        last[p] = buttons;
        let aim = gun::gun_of_player_in_picture(p).filter(|_| buttons & b::B4 == 0);
        let (x, y) = aim.map_or((0.0, 0.0), |(x, y)| (x.clamp(0.0, 1.0) * SPAN, y.clamp(0.0, 1.0) * SPAN));
        let mut a = (256.0 + x) as u32 & 0xfff;
        let by = (1535.0 - 256.0 - y) as u32 & 0xfff;
        if aim.is_none() {
            a |= 0x8000;
        }
        for (button, bit) in [(b::B1, 0x4000), (b::B2, 0x2000), (b::START, 0x1000)] {
            if buttons & button != 0 {
                a |= bit;
            }
        }
        *gun_state = (a, by | 0x8000);
        for &(player, button, word, mask) in SWITCHES.lock().unwrap().iter() {
            if player == p && buttons & button != 0 {
                words[word] |= mask;
            }
        }
    }
    drop(last);
    let coins = if TAKING.load(Ordering::Relaxed) { 0 } else { COINS.load(Ordering::Relaxed) };
    format!(
        "/s\nG1:{:x},{:x},{:x},{:x}\nCO:F,{coins}\nSW:{:x},{:x},{:x}\nSO:0,0\nMO:0,0\nST:0,0,0\n:V,0100\n/e\n\x03",
        guns[0].0, guns[0].1, guns[1].0, guns[1].1, words[0], words[1], words[2]
    )
    .into_bytes()
}
