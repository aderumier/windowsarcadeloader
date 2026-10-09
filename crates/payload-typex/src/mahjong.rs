//! Mahjong panel of Taito Type X mahjong cabinets, read by the game as a keyboard.
//!
//! `WAL_TYPEX_MAHJONG=hot-gimmick-5` (Taisen Hot Gimmick 5). The game reads its panel through
//! DirectInput: one `GetDeviceState` of the 256 key states per frame, with its own keys (tiles
//! A-N on `1`-`0` `-` `Q` `W` `E`, start and the calls on `O` `Y` `T` `R` `U` `I`); only the
//! coins and the service/test switches come from the JVS board. The call goes through
//! [`stub`], which rebuilds the key states:
//!
//! * tiles A-N: the PC keyboard's `A`-`N` keys (MAME's mahjong layout);
//! * start and the calls: player 1's virtual stick, so pads drive them too (the launcher's
//!   keyboard puts b1-b5 on left Ctrl, left Alt, Space, left Shift, Z: MAME's kan, pon, chi,
//!   reach, ron).
//!
//! Every other key of the game's keyboard is cleared.

use std::ffi::c_void;

use wal_payload_common::log;
use wal_protocol::button;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// `push edi (states); push eax (0x100); push edx (device); call [ecx + 0x24]`
/// (GetDeviceState).
const SITE: usize = 0x50ac7;
const ORIGINAL: [u8; 6] = [0x57, 0x50, 0x52, 0xFF, 0x51, 0x24];

/// PC keyboard `A`-`N` (DIK codes) -> the game's tile keys.
const TILES: [(usize, usize); 14] = [
    (0x1E, 0x02), // A -> 1
    (0x30, 0x03), // B -> 2
    (0x2E, 0x04), // C -> 3
    (0x20, 0x05), // D -> 4
    (0x12, 0x06), // E -> 5
    (0x21, 0x07), // F -> 6
    (0x22, 0x08), // G -> 7
    (0x23, 0x09), // H -> 8
    (0x17, 0x0A), // I -> 9
    (0x24, 0x0B), // J -> 0
    (0x25, 0x0C), // K -> -
    (0x26, 0x10), // L -> Q
    (0x32, 0x11), // M -> W
    (0x31, 0x12), // N -> E
];

/// Player 1's virtual stick -> the game's keys.
const BUTTONS: [(u32, usize); 6] = [
    (button::START, 0x18), // O
    (button::B1, 0x15),    // Y
    (button::B2, 0x14),    // T
    (button::B3, 0x13),    // R
    (button::B4, 0x16),    // U
    (button::B5, 0x17),    // I
];

// Called in place of the 6 bytes at SITE with edi = states, eax = their size, edx = device,
// ecx = its vtable; returns GetDeviceState's HRESULT in eax (edi is kept by the call).
core::arch::global_asm!(
    ".globl {stub}",
    "{stub}:",
    "push edi",
    "push eax",
    "push edx",
    "call dword ptr [ecx + 0x24]",
    "push eax",
    "push edi",
    "call {filter}",
    "add esp, 4",
    "pop eax",
    "ret",
    stub = sym stub,
    filter = sym filter,
);

unsafe extern "C" {
    fn stub();
}

extern "C" fn filter(states: *mut u8) {
    let states = unsafe { std::slice::from_raw_parts_mut(states, 256) };
    let keyboard: [u8; 256] = states.try_into().unwrap();
    states.fill(0);
    for (pc, game) in TILES {
        states[game] |= keyboard[pc] & 0x80;
    }
    let stick = wal_payload_common::input(0);
    for (b, game) in BUTTONS {
        if stick.pressed(b) {
            states[game] = 0x80;
        }
    }
}

pub(crate) fn init() {
    let Ok(game) = std::env::var("WAL_TYPEX_MAHJONG") else { return };
    if game.trim() != "hot-gimmick-5" {
        log!("mahjong: unknown game {game:?}");
        return;
    }
    let at = unsafe { GetModuleHandleW(std::ptr::null()) } as usize + SITE;
    let code = unsafe { std::slice::from_raw_parts_mut(at as *mut u8, ORIGINAL.len()) };
    if code != ORIGINAL {
        log!("mahjong: GetDeviceState call not found, keyboard left alone");
        return;
    }
    let mut old = 0;
    unsafe { VirtualProtect(at as *const c_void, ORIGINAL.len(), PAGE_EXECUTE_READWRITE, &mut old) };
    code[0] = 0xE8;
    code[1..5].copy_from_slice(&((stub as *const () as usize).wrapping_sub(at + 5) as i32).to_le_bytes());
    code[5] = 0x90;
    unsafe { VirtualProtect(at as *const c_void, ORIGINAL.len(), old, &mut old) };
    log!("mahjong: panel on the game's keyboard (tiles A-N, start and calls from player 1)");
}
