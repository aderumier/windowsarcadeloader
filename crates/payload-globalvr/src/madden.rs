//! EA SPORTS Madden NFL Football Season 2 (Global VR): the frontend (`shell.exe`) and the game it
//! starts (`mainapp.exe`).
//!
//! Both read the cabinet's two sticks and buttons through `MaddenInputRaw.dll`: the
//! `GVRInputRaw` functions (cdecl) on the caller's state; the programs import five
//! (`Init(hwnd, state, game)`, `Update(state)`, `Cleanup(state)`, `HandleDeviceChanged`,
//! `SetPauseLight`): the payload, installed under its name, points their imports to these.
//!
//! The state (0x60 bytes, the caller's):
//!
//! | offset | content |
//! |---|---|
//! | 0x00 / 0x08 | QueryPerformanceCounter of this / the previous update |
//! | 0x10 | QueryPerformanceFrequency |
//! | 0x18 | driver: 2 the USBIO board |
//! | 0x1C | boards (up to 16) |
//! | 0x20 | pointers to their records |
//!
//! A board's record (0x28 bytes) holds both players' controls: the even controller (player 1)
//! the low 16 bits of the buttons and the first stick, the odd one (player 2) the high 16 bits
//! and the second stick:
//!
//! | offset | content |
//! |---|---|
//! | 0x00 / 0x04 | buttons / previous buttons (after the program's `input.ini` table) |
//! | 0x0C / 0x0E | i16 player 1 stick X / Y, -127 ..= 127 |
//! | 0x16 / 0x1C | i16 player 2 stick X / Y |
//! | 0x24 | board id |
//!
//! Each player's buttons, as the `input.ini` tables map the board's: 0x1 / 0x2 / 0x4 / 0x8 the
//! four buttons, start 0x10 (the frontend) or 0x200 (the game); 0x80000000 the operator button.
//!
//! Both also read the HASP dongle (its API linked in), and ask the cabinet's monitor programs
//! (Hercules: coins, coin sound, stall) over shared memory messages, waiting for the answers: the
//! game waited for the coin monitor every frame and stayed on its EA SPORTS loading logo. Their
//! client functions are replaced, the coins kept in Blazing Angels' coin bank (`blazing.rs`).

use std::sync::atomic::{AtomicU32, Ordering};

use wal_payload_common::{iat, log};
use wal_protocol::{Axis, button};
use windows_sys::Win32::Foundation::{CloseHandle, HWND, RECT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, SW_HIDE, SW_SHOW, ShowWindow,
};

const DRIVER_USBIO: u32 = 2;
const RECORD_SIZE: usize = 0x28;

/// The board's record, the programs read it through the state's pointer.
static mut RECORD: [u8; RECORD_SIZE] = [0; RECORD_SIZE];
static BUTTONS: AtomicU32 = AtomicU32::new(0);

/// Virtual stick buttons and a player's controller bits.
const PLAYER_BUTTONS: [(u32, u32); 9] = [
    (button::B1, 0x1),
    (button::B2, 0x2),
    (button::B3, 0x4),
    (button::B4, 0x8),
    (button::START, 0x10),
    (button::B5, 0x20),
    (button::B6, 0x40),
    (button::B7, 0x80),
    (button::B8, 0x100),
];
/// A player's start bit: 0x10, the game's (its `input.ini`) 0x200.
static START_BIT: AtomicU32 = AtomicU32::new(0x10);
/// The board's operator button.
const OPERATOR: u32 = 0x8000_0000;

unsafe fn put16(p: *mut u8, offset: usize, v: i16) {
    unsafe { (p.add(offset) as *mut i16).write_unaligned(v) };
}

unsafe fn put32(p: *mut u8, offset: usize, v: u32) {
    unsafe { (p.add(offset) as *mut u32).write_unaligned(v) };
}

/// A stick axis as the board's: -127 ..= 127.
fn stick(v: i16) -> i16 {
    (v as i32 * 127 / 32767).clamp(-127, 127) as i16
}

fn fill() {
    let record = &raw mut RECORD as *mut u8;
    let mut buttons = 0u32;
    for player in 0..2 {
        let s = wal_payload_common::input(player);
        let mut bits = 0u32;
        for (b, bit) in PLAYER_BUTTONS {
            if s.pressed(b) {
                bits |= if b == button::START { START_BIT.load(Ordering::Relaxed) } else { bit };
            }
        }
        buttons |= bits << (16 * player);
        if s.pressed(button::TEST) {
            buttons |= OPERATOR;
        }
        // the digital directions (keyboard arrows, a pad's d-pad) push the stick fully over
        let digital = |minus, plus, axis| match (s.pressed(minus), s.pressed(plus)) {
            (true, false) => -i16::MAX,
            (false, true) => i16::MAX,
            _ => s.axis(axis),
        };
        let (x, y) = if player == 0 { (0x0C, 0x0E) } else { (0x16, 0x1C) };
        unsafe {
            put16(record, x, stick(digital(button::LEFT, button::RIGHT, Axis::LeftX)));
            put16(record, y, stick(digital(button::UP, button::DOWN, Axis::LeftY)));
        }
    }
    let previous = BUTTONS.swap(buttons, Ordering::Relaxed);
    unsafe {
        put32(record, 0x00, buttons);
        put32(record, 0x04, previous);
    }
}

fn timestamps(state: *mut u8) {
    unsafe {
        (state.add(0x08) as *mut i64).write_unaligned((state as *const i64).read_unaligned());
        QueryPerformanceCounter(state as *mut i64);
    }
}

/// `GVRInputRawInit(hwnd, state, game)`: the board found, 0.
unsafe extern "C" fn raw_init(_hwnd: usize, state: *mut u8, _game: *const u8) -> i32 {
    if state.is_null() {
        return 2;
    }
    unsafe {
        std::ptr::write_bytes(state, 0, 0x60);
        QueryPerformanceFrequency(state.add(0x10) as *mut i64);
        QueryPerformanceCounter(state as *mut i64);
        put32(state, 0x18, DRIVER_USBIO);
        put32(state, 0x1C, 1);
        put32(state, 0x20, &raw mut RECORD as usize as u32);
        put32(&raw mut RECORD as *mut u8, 0x24, 1);
    }
    fill();
    log!("madden: controls (one board, two players)");
    0
}

/// `GVRInputRawUpdate(state)`: 0, the controls read.
unsafe extern "C" fn update(state: *mut u8) -> i32 {
    if state.is_null() {
        return 3;
    }
    timestamps(state);
    fill();
    0
}

unsafe extern "C" fn cleanup(_state: *mut u8) {}

unsafe extern "C" fn device_changed(_event: usize, _state: *mut u8) {}

unsafe extern "C" fn pause_light(_state: *mut u8, _on: i32) {}

/// The `GVRInputRaw` imports of the program pointed to the payload's.
fn hook_input(exe: usize) {
    let hooks: [(&str, usize); 5] = [
        ("GVRInputRawInit", raw_init as *const () as usize),
        ("GVRInputRawUpdate", update as *const () as usize),
        ("GVRInputRawCleanup", cleanup as *const () as usize),
        ("GVRInputRawHandleDeviceChanged", device_changed as *const () as usize),
        ("GVRInputRawSetPauseLight", pause_light as *const () as usize),
    ];
    for (name, hook) in hooks {
        if unsafe { iat::hook_module(exe, "MaddenInputRaw.dll", name, hook) }.is_none() {
            log!("madden: {name} not imported");
        }
    }
}

/// The HASP4 API linked into both programs (`hasp(service, seed, lpt, pw1, pw2, &p1, &p2, &p3,
/// &p4)`, cdecl): the frontend reads the 112-byte memory (`CheckHDLock`), the game compares it
/// too and exits at once without it.
const HASP: usize = 0x401000;
/// The dongle's memory: game name (0..6), version (bytes 6 and 8: 2.0), country (13..15),
/// cabinet type (19, low 4 bits; 15 skips the registration program's modem call).
const DONGLE: &[u8] = b"MADDEN2.0\0\0\0\0US\0\0\0\0\x0f";

unsafe extern "C" fn hasp(service: i32, _seed: i32, _lpt: i32, _pw1: i32, _pw2: i32, p1: *mut i32, p2: *mut i32, p3: *mut i32, p4: *mut i32) {
    unsafe {
        match service {
            // IsHasp: present
            1 => *p1 = 1,
            // HaspStatus: memory size 1 (112 bytes), type 1, port 1
            5 => {
                *p1 = 1;
                *p2 = 1;
                *p3 = 1;
            }
            // ReadBlock(start word p1, words p2, buffer p4)
            50 => {
                let start = (*p1).max(0) as usize * 2;
                let len = ((*p2).max(0) as usize * 2).min(112usize.saturating_sub(start));
                let buffer = *p4 as usize as *mut u8;
                if !buffer.is_null() {
                    for i in 0..len {
                        *buffer.add(i) = DONGLE.get(start + i).copied().unwrap_or(0);
                    }
                }
                *p3 = 0;
            }
            _ => {
                log!("madden: hasp service {service}");
                *p3 = 0;
            }
        }
    }
}

/// A monitor message sent, ok (cdecl, its arguments unused).
extern "C" fn sent() -> i32 {
    0
}

/// The frontend's clients of the coin monitor (`GVRCoinMonitor`), as Blazing Angels': the coins
/// available (0x7da), service coins (0x7d5), coins taken (0x7d4).
const FRONTEND_CLIENTS: [(usize, *const ()); 3] = [
    (0x473420, crate::blazing::coins as *const ()),
    (0x473490, crate::blazing::service_coins as *const ()),
    (0x4733b0, crate::blazing::take as *const ()),
];

/// The game's clients of the same coin monitor, of the coin sound (`GVRCoinSound`) and of the
/// stall monitor (`GVRStallMonitor`).
const GAME_CLIENTS: [(usize, *const ()); 7] = [
    (0x92a4e0, crate::blazing::coins as *const ()),
    (0x92a6c0, crate::blazing::service_coins as *const ()),
    (0x92a470, crate::blazing::take as *const ()),
    // the coin sound, the coin mech counter (0x7d6)
    (0x92a550, sent as *const ()),
    (0x92a5b0, sent as *const ()),
    // the stall monitor's messages (0x7d1, 0x7d2)
    (0x929490, sent as *const ()),
    (0x9294f0, sent as *const ()),
];

/// A `jmp` from `at` to `to`.
fn jump(at: usize, to: usize) {
    unsafe {
        let mut old = 0;
        VirtualProtect(at as *const _, 5, PAGE_EXECUTE_READWRITE, &mut old);
        (at as *mut u8).write(0xE9);
        ((at + 1) as *mut i32).write_unaligned(to.wrapping_sub(at + 5) as i32);
        VirtualProtect(at as *const _, 5, old, &mut old);
    }
}

/// The visible top-level windows: whether the game (`mainapp.exe`, another process) shows one,
/// and this process' largest.
#[derive(Default)]
struct Windows {
    game: bool,
    own: usize,
    own_area: i64,
}

unsafe extern "system" fn each_window(hwnd: HWND, data: isize) -> i32 {
    let found = unsafe { &mut *(data as *mut Windows) };
    if unsafe { IsWindowVisible(hwnd) } == 0 {
        return 1;
    }
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == unsafe { GetCurrentProcessId() } {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetWindowRect(hwnd, &mut r) };
        let area = (r.right - r.left) as i64 * (r.bottom - r.top) as i64;
        if area > found.own_area {
            (found.own, found.own_area) = (hwnd as usize, area);
        }
        return 1;
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if !process.is_null() {
        let mut buf = [0u16; 260];
        let mut n = buf.len() as u32;
        if unsafe { QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut n) } != 0 {
            let path = String::from_utf16_lossy(&buf[..n as usize]).to_ascii_lowercase();
            found.game |= path.ends_with("\\mainapp.exe");
        }
        unsafe { CloseHandle(process) };
    }
    1
}

/// The frontend's screen-sized window stayed above the game's fullscreen window (the window
/// manager keeps a fullscreen window on top): hidden while the game shows a window, shown again
/// after.
fn frontend_hidden_during_game() {
    std::thread::spawn(|| {
        let mut hidden = 0usize;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let mut found = Windows::default();
            unsafe { EnumWindows(Some(each_window), &mut found as *mut Windows as isize) };
            if found.game && hidden == 0 && found.own != 0 {
                hidden = found.own;
                unsafe { ShowWindow(hidden as _, SW_HIDE) };
                log!("madden: frontend window {hidden:#x} hidden during the game");
            } else if !found.game && hidden != 0 {
                unsafe { ShowWindow(hidden as _, SW_SHOW) };
                log!("madden: frontend window {hidden:#x} shown again");
                hidden = 0;
            }
        }
    });
}

pub(crate) fn init() {
    let name = crate::frontend_fit::exe_name();
    if !matches!(name.as_str(), "shell.exe" | "mainapp.exe")
        || unsafe { iat::hook_addr("MaddenInputRaw.dll", "GVRInputRawInit") }.is_none()
    {
        return;
    }
    hook_input(unsafe { GetModuleHandleW(std::ptr::null()) } as usize);
    jump(HASP, hasp as *const () as usize);
    if name == "mainapp.exe" {
        START_BIT.store(0x200, Ordering::Relaxed);
        for (at, to) in GAME_CLIENTS {
            jump(at, to as usize);
        }
    } else {
        // the frontend's window sized to the screen (its 800x600 picture stretched, the
        // profile's WAL_WINDOW_SIZE): not the game it starts, which sets its own fullscreen mode
        unsafe { std::env::remove_var("WAL_WINDOW_SIZE") };
        frontend_hidden_during_game();
        for (at, to) in FRONTEND_CLIENTS {
            jump(at, to as usize);
        }
    }
    crate::blazing::coin_thread();
    log!("madden: {name}: controls, dongle and cabinet monitors answered");
}
