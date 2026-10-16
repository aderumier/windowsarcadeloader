//! The cabinet's I/O board ("SAE board"): a USB game controller the game reads through
//! DirectInput (the first joystick, `DIJOYSTATE`). `CSAEBoard::Update` (thiscall, no argument)
//! copies its state into a record the game's input code reads; `WAL_SEGASHELL_BOARD=<rva>`
//! replaces it with [`update`], which fills the record from the virtual sticks.
//!
//! Record (Transformers: Shadows Rising):
//! * `+0x14`: index of the current button set (0/1, flipped on each update; the other one is
//!   the previous state, for presses).
//! * `+0x08 + set * 6 + n`: board button `n` (0-5), bit 7 = pressed. P1: 0 start, 2 trigger;
//!   P2: 1 start, 3 trigger; 4/5 the cabinet's lever, pushed front / pulled back.
//! * `+0x16`/`+0x18`: P1 gun X/Y, `+0x1a`/`+0x1c`: P2 gun X/Y, 0-255, both reversed (255
//!   left / top).
//!
//! Sticks: B1 trigger, START start, UP / DOWN the lever (either player).
//!
//! The game only reads buttons 4 and 5 while the board is active (a DirectInput device): a
//! profile patch makes `CSAEBoard::IsActive` return true.

use wal_payload_common::log;
use wal_protocol::{Axis, StickState, button};

pub(crate) fn init() {
    let Some(rva) = std::env::var("WAL_SEGASHELL_BOARD").ok().and_then(|v| usize::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok())
    else {
        return;
    };
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    unsafe { jump_to(base + rva, update as *const () as usize) };
    log!("board: update at +{rva:#x} replaced");
}

/// Gun axis, -32768..32767 (left/top..right/bottom), as the board's 0-255, reversed (255 left
/// / top).
fn gun(stick: &StickState, axis: Axis) -> u16 {
    255 - ((stick.axis(axis) as i32 + 32768) >> 8) as u16
}

unsafe extern "thiscall" fn update(this: *mut u8) {
    let sticks = [0, 1].map(wal_payload_common::input);
    let on = |s: &StickState, b: u32| if s.pressed(b) { 0x80u8 } else { 0 };
    let buttons = [
        on(&sticks[0], button::START),
        on(&sticks[1], button::START),
        on(&sticks[0], button::B1),
        on(&sticks[1], button::B1),
        on(&sticks[0], button::UP) | on(&sticks[1], button::UP),
        on(&sticks[0], button::DOWN) | on(&sticks[1], button::DOWN),
    ];
    unsafe {
        let set = 1 - (*this.add(0x14)).min(1);
        *this.add(0x14) = set;
        std::ptr::copy_nonoverlapping(buttons.as_ptr(), this.add(0x08 + set as usize * 6), 6);
        for (i, v) in [gun(&sticks[0], Axis::LeftX), gun(&sticks[0], Axis::LeftY), gun(&sticks[1], Axis::LeftX), gun(&sticks[1], Axis::LeftY)].iter().enumerate() {
            std::ptr::write_unaligned(this.add(0x16 + i * 2) as *mut u16, *v);
        }
    }
}

unsafe fn jump_to(at: usize, stub: usize) {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const _, 5, PAGE_EXECUTE_READWRITE, &mut old);
        *(at as *mut u8) = 0xE9;
        std::ptr::write_unaligned((at + 1) as *mut i32, stub.wrapping_sub(at + 5) as i32);
        VirtualProtect(at as *const _, 5, old, &mut old);
    }
}
