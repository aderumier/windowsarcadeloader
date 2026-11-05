//! Wicked Tuna (UNIS's four-player fishing cabinet, Unity 2018, Mono, 64-bit):
//! `WAL_UNIS_BOARD=wickedtuna`.
//!
//! The cabinet: one serial I/O board (COM 2..10, 115200 bauds) for the four rods. The game
//! (`IOBoardControler`) talks to it through `iobaordCom.dll`'s `exIO_Initiate`,
//! `exIO_SendCmd` (26 bytes: lights, the reels' and rods' resistance, the shakers, tickets)
//! and `exIO_ReadData` (48 bytes): its `DllImport`s are the payload's (`payload-common`'s
//! `mono_imports.rs`).
//!
//! - **Board data.** Four 12-byte records, one per rod: byte 0 `A1` + the player, byte 2 the
//!   buttons (bits 0..2 the reel's position: each change one turn of the handle; 3 the hook
//!   button: cast, confirm; 4 a coin, counted on each read it is set: one read per press; 5 repair, 6 service,
//!   7 test: the operator's), bytes 3, 4 the rod's direction (12 bits, high nibble first: 0 turned
//!   right, 4095 left), bytes 5, 6 its angle (12 bits: raised, the fight's "pull"), 7..9 the tickets left,
//!   byte 11 the record's check (the complement of the sum of bytes 0..10).
//! - **The rods on light guns.** The gun's x is the rod's direction (where the hook falls, the
//!   rod turned), its y the rod's angle (aimed up: raised), the trigger the hook button, a held
//!   button the reel (the handle turned at the board's pace). `WAL_UNIS_TUNA_AIM=column` (the
//!   default): each gun's x across its player's quarter of the picture (four guns, each at its
//!   player's water); `screen`: across the whole picture (pads, a mouse: the stick's centre
//!   the rod's).
//! - **Calibration.** The game maps both through the operator menu's per-player ranges (`logs/
//!   Data.dat`'s `AdMin` / `AdMax` / `AdLeft` / `AdRight`): the profile's are 0..4095.
//!
//! - **Sound.** Each player's effects go to a speaker of the cabinet's (Bass streams with a
//!   speaker flag: players 1, 2 the front left and right, 3, 4 the rear left and right). A
//!   stream BASS refuses (a stereo device without rear speakers) is opened again on the front
//!   speaker of its side: every player heard.
//!
//! Natives: `hook reel coin` (each rod), `test service repair` (the operator's).

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log, mono_imports};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

const PLAYERS: usize = 4;
const RECORD: usize = 12;
const DATA: usize = PLAYERS * RECORD;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Hook,
    Reel,
    Coin,
    /// An operator's button: its bit of byte 2.
    Operator(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("hook", Native::Hook),
    ("reel", Native::Reel),
    ("coin", Native::Coin),
    ("test", Native::Operator(7)),
    ("service", Native::Operator(6)),
    ("repair", Native::Operator(5)),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "hook"),
    ("start", "hook"),
    ("b2", "reel"),
    ("coin", "coin"),
    // the operator menu: test opens it and selects, service and repair move
    ("test", "test"),
    ("service", "service"),
    ("down", "service"),
    ("up", "repair"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// `WAL_UNIS_TUNA_AIM`: the gun's x across its player's column (true) or the whole picture.
fn column_aim() -> bool {
    static COLUMN: OnceLock<bool> = OnceLock::new();
    *COLUMN.get_or_init(|| std::env::var("WAL_UNIS_TUNA_AIM").map_or(true, |v| v.trim() != "screen"))
}

/// The rod's direction and angle (0..4095) for a gun at `(x, y)` (fractions of the picture
/// from its top left): x across the player's column or the picture (the board's direction
/// from the right: 0 the rod turned right), y up.
fn rod(player: usize, (x, y): (f32, f32), column: bool) -> (u16, u16) {
    let x = if column { (x * PLAYERS as f32 - player as f32).clamp(0.0, 1.0) } else { x };
    (((1.0 - x) * 4095.0) as u16, ((1.0 - y) * 4095.0) as u16)
}

struct Rods {
    /// Each rod's direction and angle, kept while its gun points off the picture.
    rod: [(u16, u16); PLAYERS],
    /// Each reel's position (0..7).
    reel: [u8; PLAYERS],
    /// Each coin key held at the last read (a press is one read's coin).
    coin: [bool; PLAYERS],
}

static RODS: Mutex<Rods> = Mutex::new(Rods { rod: [(2048, 2048); PLAYERS], reel: [0; PLAYERS], coin: [false; PLAYERS] });

/// The board's data, one record per rod.
fn data() -> [u8; DATA] {
    let mut d = [0u8; DATA];
    let mut rods = RODS.lock().unwrap();
    for player in 0..PLAYERS {
        let r = &mut d[player * RECORD..][..RECORD];
        r[0] = 0xA1 + player as u8;
        let (mut reeling, mut coin) = (false, false);
        map().for_each_pressed(&input(player), |n| match n {
            Native::Hook => r[2] |= 0x08,
            Native::Reel => reeling = true,
            Native::Coin => coin = true,
            Native::Operator(bit) if player == 0 => r[2] |= 1 << bit,
            Native::Operator(_) => {}
        });
        if coin && !rods.coin[player] {
            r[2] |= 0x10;
        }
        rods.coin[player] = coin;
        if reeling {
            rods.reel[player] = (rods.reel[player] + 1) & 7;
        }
        r[2] |= rods.reel[player];
        if let Some(at) = gun::gun_of_player_in_picture(player) {
            rods.rod[player] = rod(player, at, column_aim());
        }
        let (dir, angle) = rods.rod[player];
        r[3..5].copy_from_slice(&dir.to_be_bytes());
        r[5..7].copy_from_slice(&angle.to_be_bytes());
        r[11] = !r[..11].iter().fold(0u8, |s, b| s.wrapping_add(*b));
    }
    d
}

unsafe extern "C" fn initiate(port: u32, baud: u32) -> i32 {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if !LOGGED.swap(true, Ordering::Relaxed) {
        log!("wickedtuna: board on COM{port}, {baud} bauds");
    }
    1
}

unsafe extern "C" fn read_data(out: *mut u8, len: i32) {
    if out.is_null() || len <= 0 {
        return;
    }
    let d = data();
    let n = (len as usize).min(DATA);
    unsafe { std::ptr::copy_nonoverlapping(d.as_ptr(), out, n) };
}

unsafe extern "C" fn send_cmd(_data: *const u8, _len: i32) {}

unsafe extern "C" fn nothing() {}

/// BASS's speaker assignment: `BASS_SPEAKER_N(n)` (bits 24..27, 1 the front pair, 2 the rear),
/// its left or right speaker (bits 28, 29).
const SPEAKER_PAIR: u32 = 0x0F00_0000;
const SPEAKER_FRONT: u32 = 0x0100_0000;

type StreamCreateFile = unsafe extern "system" fn(i32, *const c_void, u64, u64, u32) -> u32;

fn bass(name: &core::ffi::CStr) -> usize {
    let module = unsafe { GetModuleHandleW(windows_sys::w!("bass.dll")) };
    unsafe { GetProcAddress(module, name.as_ptr().cast()) }.map_or(0, |f| f as usize)
}

/// `BASS_StreamCreateFile`: a stream on a speaker the device lacks opened on the front pair's
/// speaker of its side.
unsafe extern "system" fn stream_create_file(mem: i32, file: *const c_void, offset: u64, length: u64, flags: u32) -> u32 {
    let real = bass(c"BASS_StreamCreateFile");
    if real == 0 {
        return 0;
    }
    let real: StreamCreateFile = unsafe { std::mem::transmute(real) };
    let stream = unsafe { real(mem, file, offset, length, flags) };
    let pair = flags & SPEAKER_PAIR;
    if stream != 0 || pair == 0 || pair == SPEAKER_FRONT {
        return stream;
    }
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if !LOGGED.swap(true, Ordering::Relaxed) {
        let code = bass(c"BASS_ErrorGetCode");
        let code = if code == 0 { -1 } else { unsafe { std::mem::transmute::<usize, unsafe extern "system" fn() -> i32>(code)() } };
        log!("wickedtuna: speaker {:#x} refused (BASS error {code}): its streams on the front speakers", pair >> 24);
    }
    unsafe { real(mem, file, offset, length, flags & !SPEAKER_PAIR | SPEAKER_FRONT) }
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    let f = |f: *const c_void| Some(f as usize);
    match (module, name) {
        ("iobaordcom.dll", "exIO_Initiate") => f(initiate as *const c_void),
        ("iobaordcom.dll", "exIO_ReadData") => f(read_data as *const c_void),
        ("iobaordcom.dll", "exIO_SendCmd") => f(send_cmd as *const c_void),
        ("iobaordcom.dll", "exIO_Shutdown" | "exRelease") => f(nothing as *const c_void),
        ("bass.dll", "BASS_StreamCreateFile") => f(stream_create_file as *const c_void),
        _ => None,
    }
}

pub fn init() {
    mono_imports::hook(resolve);
    log!("wickedtuna: four rods' serial board, aim {}", if column_aim() { "per column" } else { "across the picture" });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rod_column() {
        // player 2's column is the picture's third quarter
        assert_eq!(rod(2, (0.625, 0.0), true), (2047, 4095));
        assert_eq!(rod(2, (0.1, 1.0), true), (4095, 0));
        assert_eq!(rod(1, (0.5, 1.0), true), (0, 0));
        assert_eq!(rod(0, (0.5, 0.5), false), (2047, 2047));
    }

    #[test]
    fn record_check() {
        let d = data();
        for r in d.chunks(RECORD) {
            assert_eq!(r[..11].iter().fold(0u8, |s, b| s.wrapping_add(*b)).wrapping_add(r[11]), 0xFF);
        }
    }
}
