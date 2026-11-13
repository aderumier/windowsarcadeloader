//! Go Go Drummer Babe (a Chinese HGE-engine drum medal / redemption game: `mygame.exe`,
//! Haaf's Game Engine on Direct3D 8, `bass.dll` audio, 640x480, 32-bit): its cabinet I/O,
//! emulated by the payload (`WAL_ARCADEPC_BOARD=gogodrummer`, x86 build).
//!
//! **Input is the HGE keyboard** (`0x71ebd0` reads `Input_GetKeyState`; HGE reads
//! `GetKeyboardState`, which the payload answers, the real keyboard and mouse filtered):
//! - **`'5'` `'6'` `'7'` `'8'`** are the four drum sensors, left to right (`DAT_008009d7`..`da`).
//! - **`'1'`** adds a credit (`DAT_00800a69` -> `NowCoin`+1), **`'2'` `'3'`** the other operator
//!   signals. Free play is the game's `Set.ini [FreePlay] Val` (1: no coin needed).
//!
//! **The board.** The game still opens its serial board on `\\.\COM`+port (any number, `COM*`);
//! the payload is that port, silent — its writes (lamps, ticket dispense) are accepted and
//! dropped, and its monitor thread's `WaitCommEvent` simply stays pending (the input comes
//! through the keyboard, so nothing is pushed and nothing double-counts).
//!
//! Natives: `drum_l`, `drum_ml`, `drum_mr`, `drum_r` (the four sensors), `coin`, `service`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_key};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log, serial};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

/// Any COM port the game opens (`\\.\COM`+number): kept as a silent device so the open and the
/// game's writes succeed (the input is the keyboard, not this board).
const PORT: &str = "COM*";

/// The game's input keys (the number keys HGE reads).
const VK_1: u16 = 0x31;
const VK_2: u16 = 0x32;
const VK_3: u16 = 0x33;
const VK_5: u16 = 0x35;
const VK_6: u16 = 0x36;
const VK_7: u16 = 0x37;
const VK_8: u16 = 0x38;

/// Virtual input -> game key. The four drums are `'5'`..`'8'`, the coin / credit `'1'`, the
/// operator signals `'2'` / `'3'`.
const NATIVES: &[(&str, u16)] = &[
    ("drum_l", VK_5),
    ("drum_ml", VK_6),
    ("drum_mr", VK_7),
    ("drum_r", VK_8),
    ("coin", VK_1),
    ("service", VK_2),
    ("test", VK_3),
];

const DEFAULTS: &[(&str, &str)] = &[
    // the four drums, left to right, on the gamepad shoulders L1 / L2 / R1 / R2 (the standard
    // gamepad mapping makes those the virtual buttons b4 / b8 / b3 / b7)
    ("b4", "drum_l"),  // L1
    ("b8", "drum_ml"), // L2
    ("b3", "drum_mr"), // R1
    ("b7", "drum_r"),  // R2
    // the stick directions hit the matching drums too (a keyboard's arrows)
    ("left", "drum_l"),
    ("up", "drum_ml"),
    ("down", "drum_mr"),
    ("right", "drum_r"),
    ("coin", "coin"),
    ("service", "service"),
    ("test", "test"),
];

fn map() -> &'static ButtonMap<u16> {
    static MAP: OnceLock<ButtonMap<u16>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The keys held (bit n: virtual key n), for the game's `GetKeyboardState` polls.
static HELD: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

pub fn key_held(vk: i32) -> bool {
    (0..256).contains(&vk) && HELD[vk as usize / 64].load(Ordering::Relaxed) & 1 << (vk % 64) != 0
}

fn input_thread() {
    let mut was = [0u64; 4];
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = gun::find_window();
        let stick = input(0);
        let mut keys = [0u64; 4];
        map().for_each_pressed(&stick, |vk| keys[vk as usize / 64] |= 1 << (vk % 64));
        for (held, k) in HELD.iter().zip(keys) {
            held.store(k, Ordering::Relaxed);
        }
        // also post the key messages (HGE updates its key state from the window queue too)
        if !hwnd.is_null() {
            for vk in 0..256u16 {
                let (now, before) = (keys[vk as usize / 64] >> (vk % 64) & 1 != 0, was[vk as usize / 64] >> (vk % 64) & 1 != 0);
                if now != before {
                    post_key(hwnd as HWND, vk, now);
                }
            }
        }
        was = keys;
    }
}

pub fn init() {
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    // HGE reads GetKeyboardState: the payload answers it with the board's keys; the real
    // keyboard and mouse never reach the game
    wal_payload_common::input_filter::init(exe, key_held);
    own_mouse(exe);
    // the cabinet's serial board: a silent device (the open and the game's writes succeed)
    serial::install(PORT, |_| Vec::new());
    std::thread::spawn(input_thread);
    log!("gogodrummer: inputs from the virtual sticks as the HGE keys, COM* a silent board");
}
