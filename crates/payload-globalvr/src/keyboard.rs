//! The game's own keyboard: only the operator keys, driven by the virtual sticks.
//!
//! Far Cry Paradise Lost reads the PC keyboard through DirectInput (one `GetDeviceState` of the
//! 256 key states per frame, at `0x7abaf3`). That keyboard is the cabinet's operator keys, but
//! this build also keeps the console game's controls and debug keys on it: QWERTY WASD walk
//! the player off his spot, G god mode, K kill self, C crashes the game... The call goes
//! through [`stub`], which clears the key states and sets only the operator keys from the
//! virtual sticks:
//!
//! | virtual stick (any player) | game key | test menu |
//! |---|---|---|
//! | test | 5 | open, select |
//! | service | 6 | service credit |
//! | up / down | 9 / 0 | move up / down |
//!
//! The game also takes debug keys from its window messages (F1-F5 difficulty...):
//! `PeekMessageA` / `GetMessageA` turn keyboard messages into `WM_NULL`.

use std::ffi::c_void;

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use wal_protocol::button;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// `push edx (states); push 0x100; push eax (device); call [ecx + 0x24]` (GetDeviceState).
const SITE: usize = 0x3abaec;
const ORIGINAL: [u8; 10] = [0x52, 0x68, 0x00, 0x01, 0x00, 0x00, 0x50, 0xFF, 0x51, 0x24];

const DIK_5: usize = 0x06;
const DIK_6: usize = 0x07;
const DIK_9: usize = 0x0A;
const DIK_0: usize = 0x0B;
const KEYS: [(u32, usize); 4] =
    [(button::TEST, DIK_5), (button::SERVICE, DIK_6), (button::UP, DIK_9), (button::DOWN, DIK_0)];

// Called in place of the 10 bytes at SITE with eax = device, ecx = its vtable, edx = states;
// returns GetDeviceState's HRESULT in eax.
core::arch::global_asm!(
    ".globl {stub}",
    "{stub}:",
    "push edx",
    "push edx",
    "push 0x100",
    "push eax",
    "call dword ptr [ecx + 0x24]",
    "pop edx",
    "push eax",
    "push edx",
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
    states.fill(0);
    for player in 0..2 {
        let stick = wal_payload_common::input(player);
        for (button, key) in KEYS {
            if stick.pressed(button) {
                states[key] = 0x80;
            }
        }
    }
}

const WM_KEYFIRST: u32 = 0x100;
const WM_KEYLAST: u32 = 0x109;

static PEEK: AtomicUsize = AtomicUsize::new(0);
static GET: AtomicUsize = AtomicUsize::new(0);

type PeekFn = unsafe extern "system" fn(*mut u32, usize, u32, u32, u32) -> i32;
type GetFn = unsafe extern "system" fn(*mut u32, usize, u32, u32) -> i32;

/// `MSG.message` (after `hwnd`) of a keyboard message becomes `WM_NULL`.
fn drop_key(msg: *mut u32) {
    let message = unsafe { msg.add(1) };
    if (WM_KEYFIRST..=WM_KEYLAST).contains(&unsafe { *message }) {
        unsafe { *message = 0 };
    }
}

unsafe extern "system" fn peek_message(msg: *mut u32, hwnd: usize, min: u32, max: u32, remove: u32) -> i32 {
    let real: PeekFn = unsafe { std::mem::transmute(PEEK.load(Ordering::Relaxed)) };
    let r = unsafe { real(msg, hwnd, min, max, remove) };
    if r != 0 {
        drop_key(msg);
    }
    r
}

unsafe extern "system" fn get_message(msg: *mut u32, hwnd: usize, min: u32, max: u32) -> i32 {
    let real: GetFn = unsafe { std::mem::transmute(GET.load(Ordering::Relaxed)) };
    let r = unsafe { real(msg, hwnd, min, max) };
    if r > 0 {
        drop_key(msg);
    }
    r
}

pub(crate) fn init() {
    if let Some(o) = unsafe { iat::hook("user32.dll", "PeekMessageA", peek_message as *const () as usize) } {
        PEEK.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { iat::hook("user32.dll", "GetMessageA", get_message as *const () as usize) } {
        GET.store(o, Ordering::Relaxed);
    }
    let at = unsafe { GetModuleHandleW(std::ptr::null()) } as usize + SITE;
    let code = unsafe { std::slice::from_raw_parts_mut(at as *mut u8, ORIGINAL.len()) };
    if code != ORIGINAL {
        log!("keyboard: GetDeviceState call not found, game keyboard left alone");
        return;
    }
    let mut old = 0;
    unsafe { VirtualProtect(at as *const c_void, ORIGINAL.len(), PAGE_EXECUTE_READWRITE, &mut old) };
    code[0] = 0xE8;
    code[1..5].copy_from_slice(&((stub as *const () as usize).wrapping_sub(at + 5) as i32).to_le_bytes());
    code[5..].fill(0x90);
    unsafe { VirtualProtect(at as *const c_void, ORIGINAL.len(), old, &mut old) };
    log!("keyboard: game keyboard limited to the operator keys");
}
