//! Tatsunoko vs. Capcom: Cross Generation of Heroes (Capcom, 2008, the Wii-based RVA-001
//! arcade board) in a Dolphin build with the board's devices (`Dolphin.exe -b -n
//! 0000000100000002`: the game installed in the NAND as the system menu, `User\` beside the
//! executable, `portable.txt`): `WAL_ARCADEPC_BOARD=dolphin`.
//!
//! **Inputs.** Dolphin's RVA JVS I/O reads two GameCube pads: Start, A / B / X / Y shots 1 to 4,
//! the main stick or d-pad the lever, L each player's coin, pad 1's C-stick right the test
//! switch, a C-stick up the player's service switch. Dolphin reads the PC keyboard only through
//! DirectInput (`DINPUT8.dll`, its `DInput/0/Keyboard Mouse` device), the pads bound to keys in
//! the profile's `User\Config\GCPadNew.ini`: the common fake DirectInput (`WAL_DINPUT_DISABLE`)
//! gives it a system keyboard whose keys come from both players' virtual sticks
//! ([`dinput::set_keyboard`]), so the real keyboard never reaches it. The profile's
//! `Hotkeys.ini` binds no hotkey (Dolphin's defaults: F1-F8 load states, Esc stops).
//!
//! **Free play.** The game's SYSTEM CONFIGURATION (FREE PLAY) is byte 0x806 of the board's SRAM
//! (`User\Arcade\SRAM_TatsunokoVSCapcom.bin`, 32 KB, Dolphin's file opened when the game
//! boots): set from the common `WAL_FREEPLAY` before. The file's first two bytes (big endian)
//! are the sum of the others less 0x1700: they move with the byte. A dump without the file gets
//! the game's defaults (free play off) at its first boot, the setting at the next.
//!
//! Natives per player (`p2_` for player 2): `up`, `down`, `left`, `right`, `a`, `b`, `x`, `y`,
//! `start`, `coin`, `service`; player 1's `test`.

use std::sync::OnceLock;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{dinput, input, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;

/// The SRAM's FREE PLAY byte (0 off, 1 on).
const FREE_PLAY: usize = 0x806;
const SRAM_SIZE: usize = 0x8000;

/// DirectInput key codes (DIK_*) of the profile's `GCPadNew.ini`.
const NATIVES: &[(&str, u8)] = &[
    ("up", 0xC8),
    ("down", 0xD0),
    ("left", 0xCB),
    ("right", 0xCD),
    ("a", 0x10),       // Q
    ("b", 0x11),       // W
    ("x", 0x12),       // E
    ("y", 0x13),       // R
    ("start", 0x14),   // T
    ("coin", 0x06),    // 5: L
    ("test", 0x0A),    // 9: C-stick right
    ("service", 0x0B), // 0: C-stick up
    ("p2_up", 0x17),   // I
    ("p2_down", 0x25), // K
    ("p2_left", 0x24), // J
    ("p2_right", 0x26), // L
    ("p2_a", 0x1E),    // A
    ("p2_b", 0x1F),    // S
    ("p2_x", 0x20),    // D
    ("p2_y", 0x21),    // F
    ("p2_start", 0x22), // G
    ("p2_coin", 0x07), // 6: L
    ("p2_service", 0x09), // 8: C-stick up
];

/// The game's Light / Medium / Heavy / Partner on shots 1 to 4 (A, B, X, Y): the pad's left
/// face button, top, R1 and right one (the dump's own layout), the other two faces Light and
/// Partner too.
const DEFAULTS: &[(&str, &str)] = &[
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "a"),
    ("b2", "b"),
    ("b3", "x"),
    ("b6", "y"),
    ("b5", "a"),
    ("b4", "y"),
    ("start", "start"),
    ("coin", "coin"),
    ("service", "service"),
    ("test", "test"),
];

const DEFAULTS_P2: &[(&str, &str)] = &[
    ("up", "p2_up"),
    ("down", "p2_down"),
    ("left", "p2_left"),
    ("right", "p2_right"),
    ("b1", "p2_a"),
    ("b2", "p2_b"),
    ("b3", "p2_x"),
    ("b6", "p2_y"),
    ("b5", "p2_a"),
    ("b4", "p2_y"),
    ("start", "p2_start"),
    ("coin", "p2_coin"),
    ("service", "p2_service"),
    ("test", "test"),
];

fn maps() -> &'static [ButtonMap<u8>; 2] {
    static MAPS: OnceLock<[ButtonMap<u8>; 2]> = OnceLock::new();
    MAPS.get_or_init(|| [ButtonMap::new(NATIVES, DEFAULTS), ButtonMap::with_defaults(NATIVES, DEFAULTS_P2)])
}

/// The DirectInput keyboard's keys: both players' sticks.
fn keyboard(keys: &mut [u8; 256]) {
    for (player, map) in maps().iter().enumerate() {
        map.for_each_pressed(&input(player), |dik| keys[dik as usize] = 0x80);
    }
}

/// `<executable's folder>\User\Arcade\SRAM_TatsunokoVSCapcom.bin`.
fn sram_path() -> Option<std::path::PathBuf> {
    let mut buf = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let exe = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    Some(exe.parent()?.join("User").join("Arcade").join("SRAM_TatsunokoVSCapcom.bin"))
}

/// The SRAM's FREE PLAY byte set to `free`, its sum moved with it.
fn set_free_play(free: bool) {
    let Some(path) = sram_path() else { return };
    let mut sram = match std::fs::read(&path) {
        Ok(d) if d.len() == SRAM_SIZE => d,
        Ok(d) => return log!("dolphin: {} is {} bytes, free play left as it is", path.display(), d.len()),
        Err(e) => return log!("dolphin: {}: {e}: free play is the game's default (off) until it writes it", path.display()),
    };
    let value = free as u8;
    if sram[FREE_PLAY] == value {
        return log!("dolphin: free play {} already", if free { "on" } else { "off" });
    }
    let sum = u16::from_be_bytes([sram[0], sram[1]]).wrapping_add(value as u16).wrapping_sub(sram[FREE_PLAY] as u16);
    sram[FREE_PLAY] = value;
    sram[..2].copy_from_slice(&sum.to_be_bytes());
    match std::fs::write(&path, &sram) {
        Ok(()) => log!("dolphin: free play {}", if free { "on" } else { "off" }),
        Err(e) => log!("dolphin: {} not written: {e}", path.display()),
    }
}

pub fn init() {
    if let Some(free) = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") {
        set_free_play(free);
    }
    dinput::set_keyboard(keyboard);
    log!("dolphin: DirectInput keyboard from players 1 and 2");
}
