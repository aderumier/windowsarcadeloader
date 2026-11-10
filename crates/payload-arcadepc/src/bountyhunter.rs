//! Bounty Hunter (GCTECH, 2002, `bh.exe`, Power Render 4 on Direct3D 8, 32-bit, UPX-packed):
//! `WAL_ARCADEPC_BOARD=bountyhunter`, x86 build.
//!
//! A striking game: the player hits eight pads (two rows of punch pads, two of foot pads, left
//! and right) where the targets light up on the vertical screen. The cabinet's board on COM1
//! (57600 bauds) sends them in 5-byte packets `xx, cmd, data, xx, sum` (`sum`: the complement of
//! the first four bytes' sum), read once per frame (`0x412ae0`, checked at `0x412c20`):
//!
//! - `4`: the pads, one bit each, latched until the frame's end (`0x412960` clears them): bits
//!   0-3 the left pads, 4-7 the right ones, top to bottom (the game's development keys,
//!   `WM_KEYDOWN` at `0x413c20`, in the same places: numpad 8 / 9, 5 / 6, 2 / 3, down / up):
//!   punch high, punch low, foot high, foot low (the menus: a punch moves, a foot selects).
//!   The pads are struck: each press is sent once (a held button would repeat every frame:
//!   the setup menu's values ran several steps per press).
//! - `1`: a coin (`Tab` on the keyboard), `3`: the setup menu (`F1`), `2`: a service credit
//!   (`F2`). One packet per press.
//!
//! The game writes `FF 06 01 n sum` commands (its lamps): accepted, nothing answered.
//!
//! The board is used when `BountyHunter.GC` (the operator settings of `DataSetting.exe`, read at
//! `0x4141c0`) says so (`+0x4b0`); the dump's file is set for the keyboard: the profile hides it,
//! the game starts with its own defaults (board on) and keeps them in the run directory.
//!
//! - **Packed.** UPX: the program is unpacked after the payload's start (loaded at the stub's
//!   entry point), and the stub fills its import table with `GetProcAddress`: the payload's
//!   `GetProcAddress` gives it the serial emulation's functions. Its first call comes after the
//!   code is unpacked: the payload's code patch is written then.
//! - **Loading.** The engine's allocations go through a leak tracker (`0x475f30` / `0x476110`):
//!   each block is a node of a list, and each free walks the list from its head. With the
//!   game's data loaded (thousands of blocks), the boot's loading took minutes: the two
//!   functions jump to the payload's, the same blocks (the size and two guard words
//!   `0xeeeeeeee` around, zeroed) from the game's C runtime, without the list.
//! - **Keyboard.** The game's window procedure takes the development keys (`WM_KEYDOWN`,
//!   `WM_CHAR`: switches, coins, the setup menu, test settings): skipped (`0x413c32`: jmp to the
//!   default procedure), the real keyboard never reaches the game. Its DirectInput keyboard
//!   (Escape quits) is the common fake one, without keys.
//! - **Free play.** `setup.dat` holds `coins per credit, credits per coin` (`0x415500`), copied
//!   in the game's object (`[0x4cfd50]`, `+0x1e0`): 0 coins plays free (any switch starts, the
//!   title's credit count hidden). The common `WAL_FREEPLAY` sets it once the object exists.
//!
//! Natives: `punch_lh punch_rh punch_ll punch_rl foot_lh foot_rh foot_ll foot_rl coin service
//! test`.

use std::ffi::{CStr, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, input, log, patches, serial};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_READWRITE, VirtualQuery};

const PORT: &str = "COM1";

/// The board's packet commands.
const COIN: u8 = 1;
const SERVICE: u8 = 2;
const SETUP: u8 = 3;
const SWITCHES: u8 = 4;

/// The game's object (its address, once the game loop runs).
const GAME: usize = 0x4cfd50;
/// The object's coins per credit.
const COINS_PER_CREDIT: usize = 0x1e0;
/// Bytes of the unpacked program (the coins check of the attract loop, `0x4139d1`): the
/// unpacker is done with the code once they are there.
const UNPACKED: (usize, [u8; 7]) = (0x4139d1, [0x83, 0xBE, 0xE0, 0x01, 0x00, 0x00, 0x01]);
/// The key messages' branch of the game's window procedure (`je` to `WM_KEYDOWN`), replaced
/// with a jump to its default procedure call (`0x413f08`).
const KEYS_PATCH: &str = "0x13c32:E9D102000090";
/// The leak tracker's allocation and free, and the game's C runtime `malloc` / `free`.
const TRACKED_ALLOC: usize = 0x475f30;
const TRACKED_FREE: usize = 0x476110;
const MALLOC: usize = 0x4a4eaa;
const FREE: usize = 0x4a4b4e;
const GUARD: u32 = 0xeeee_eeee;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// A pad: its bit.
    Switch(u8),
    /// A command packet (coin, setup, service).
    Command(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("punch_lh", Native::Switch(0)),
    ("punch_ll", Native::Switch(1)),
    ("foot_lh", Native::Switch(2)),
    ("foot_ll", Native::Switch(3)),
    ("punch_rh", Native::Switch(4)),
    ("punch_rl", Native::Switch(5)),
    ("foot_rh", Native::Switch(6)),
    ("foot_rl", Native::Switch(7)),
    ("coin", Native::Command(COIN)),
    ("test", Native::Command(SETUP)),
    ("service", Native::Command(SERVICE)),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "punch_lh"),
    ("b2", "punch_rh"),
    ("b3", "punch_ll"),
    ("b4", "punch_rl"),
    ("b5", "foot_lh"),
    ("b6", "foot_rh"),
    ("b7", "foot_ll"),
    ("b8", "foot_rl"),
    ("left", "punch_ll"),
    ("right", "punch_rl"),
    ("up", "punch_lh"),
    ("down", "foot_ll"),
    ("start", "foot_ll"),
    ("coin", "coin"),
    ("test", "test"),
    ("service", "service"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// A board packet.
fn packet(cmd: u8, data: u8) -> [u8; 5] {
    let mut p = [0xFF, cmd, data, 0, 0];
    p[4] = !p[..4].iter().fold(0u8, |s, &b| s.wrapping_add(b));
    p
}

/// The pads held at the last poll.
static PADS_HELD: AtomicU8 = AtomicU8::new(0);
/// The command packets held at the last poll (coin, service, setup: one packet per press).
static HELD: [AtomicBool; 3] = [const { AtomicBool::new(false) }; 3];

/// The board's packets for the switches and commands held.
fn packets(switches: u8, commands: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    if switches != 0 {
        out.extend_from_slice(&packet(SWITCHES, switches));
    }
    for &c in commands {
        out.extend_from_slice(&packet(c, 0));
    }
    out
}

/// What the board sends when the game polls: the new presses.
fn idle() -> Vec<u8> {
    let mut switches = 0u8;
    let mut held = [false; 3];
    map().for_each_pressed(&input(0), |n| match n {
        Native::Switch(bit) => switches |= 1 << bit,
        Native::Command(c) => held[(c - COIN) as usize] = true,
    });
    let switches = switches & !PADS_HELD.swap(switches, Ordering::Relaxed);
    let mut commands = Vec::new();
    for (i, &h) in held.iter().enumerate() {
        if h && !HELD[i].swap(true, Ordering::Relaxed) {
            commands.push(COIN + i as u8);
        } else if !h {
            HELD[i].store(false, Ordering::Relaxed);
        }
    }
    packets(switches, &commands)
}

/// The game's commands (lamps): nothing to answer.
fn handler(_p: &[u8]) -> Vec<u8> {
    Vec::new()
}

fn read<const N: usize>(addr: usize) -> [u8; N] {
    unsafe { std::ptr::read_volatile(addr as *const [u8; N]) }
}

/// Whether `addr` is in committed read-write memory (the game's object, on its stack).
fn writable(addr: usize) -> bool {
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let n = unsafe { VirtualQuery(addr as *const c_void, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) };
    n != 0 && info.State == MEM_COMMIT && info.Protect & (PAGE_READWRITE | PAGE_EXECUTE_READWRITE) != 0
}

/// Free play: waits for the game's object (`[GAME]` holds the packed data until the game sets
/// it: `before`, read before the program runs).
fn free_play(before: usize) {
    loop {
        let game = unsafe { std::ptr::read_volatile(GAME as *const usize) };
        if game != before && game != 0 && writable(game + COINS_PER_CREDIT) {
            unsafe { ((game + COINS_PER_CREDIT) as *mut i32).write_volatile(0) };
            log!("bountyhunter: free play");
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The tracker's allocation (`size, source file, line`): a zeroed block between two guards.
extern "C" fn alloc(size: usize, _file: *const u8, _line: i32) -> *mut u8 {
    let malloc: extern "C" fn(usize) -> *mut u8 = unsafe { std::mem::transmute(MALLOC) };
    let block = malloc(size + 8);
    if block.is_null() {
        return block;
    }
    unsafe {
        std::ptr::write_bytes(block, 0, size + 8);
        block.cast::<u32>().write_unaligned(GUARD);
        block.add(size + 4).cast::<u32>().write_unaligned(GUARD);
        block.add(4)
    }
}

/// The tracker's free (`block, source file, line`).
extern "C" fn free(p: *mut u8, _file: *const u8, _line: i32) {
    if p.is_null() {
        return;
    }
    let free: extern "C" fn(*mut u8) = unsafe { std::mem::transmute(FREE) };
    free(unsafe { p.sub(4) });
}

/// `jmp target` at `at`, as a patch of the executable.
fn jump(at: usize, target: usize) -> String {
    let rel = (target as isize - (at as isize + 5)) as i32;
    format!("{:#x}:E9{}", at - 0x400000, rel.to_le_bytes().iter().map(|b| format!("{b:02X}")).collect::<String>())
}

/// The `GetProcAddress` called before ours (the common DirectInput's, else kernel32's).
static GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static PATCHED: AtomicBool = AtomicBool::new(false);

/// The stub's import resolution: the serial functions are the emulation's; the code patch once
/// the program is unpacked.
unsafe extern "system" fn get_proc_address(module: *mut c_void, name: *const u8) -> *mut c_void {
    if !PATCHED.load(Ordering::Relaxed) && read::<7>(UNPACKED.0) == UNPACKED.1 && !PATCHED.swap(true, Ordering::Relaxed) {
        patches::apply_list(KEYS_PATCH);
        patches::apply_list(&jump(TRACKED_ALLOC, alloc as *const () as usize));
        patches::apply_list(&jump(TRACKED_FREE, free as *const () as usize));
        log!("bountyhunter: program unpacked, key messages skipped, allocations untracked");
        // off: the operator's setup.dat
        if wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") == Some(true) {
            let before = unsafe { std::ptr::read_volatile(GAME as *const usize) };
            std::thread::spawn(move || free_play(before));
        }
    }
    if name as usize > 0xFFFF
        && let Ok(n) = unsafe { CStr::from_ptr(name.cast()) }.to_str()
        && let Some(f) = serial::replacement(n)
    {
        return f as *mut c_void;
    }
    let orig: unsafe extern "system" fn(*mut c_void, *const u8) -> *mut c_void =
        unsafe { std::mem::transmute(GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    unsafe { orig(module, name) }
}

pub fn init() {
    serial::install_streaming(PORT, handler, idle);
    match unsafe { iat::hook("kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) } {
        Some(o) => GET_PROC_ADDRESS.store(o, Ordering::Relaxed),
        None => log!("bountyhunter: the stub does not import GetProcAddress: no board"),
    }
    log!("bountyhunter: board on {PORT}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_sum() {
        // the complement of the first four bytes' sum
        assert_eq!(packet(SWITCHES, 0x11), [0xFF, 0x04, 0x11, 0x00, !0x14u8]);
        assert_eq!(packet(COIN, 0), [0xFF, 0x01, 0x00, 0x00, !0x00u8]);
        assert_eq!(packets(0, &[]), Vec::<u8>::new());
        assert_eq!(packets(0x80, &[COIN]).len(), 10);
    }
}
