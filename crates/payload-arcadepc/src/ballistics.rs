//! Ballistics (Grin, 2001, arcade version; `Ballistics.exe`, Direct3D 8, 32-bit):
//! `WAL_ARCADEPC_BOARD=ballistics`, x86 build.
//!
//! - **Controls.** The cabinet's controls are a USB "HAPP UGCI(tm) USB Driving GameFrame
//!   v2.02": the input setup (`0x42c920`) looks for a DirectInput joystick (device type joystick
//!   or gamepad: the enumeration's callback `0x44bc30` skips driving devices) of that name, else
//!   throws "Could not find HAPP UGCI..." and quits; it binds its axes by their names' first
//!   letter (`X`: pitch and roll right, the steering; `Y`: roll left, the throttle, full at the
//!   top of its range) and its buttons by name (`Button 0` select and thrust, `1` / `7` flip,
//!   `2` brake, `3` throttle, `4` roll right, `9` booster; all but 4 also the menus' "any
//!   button"), next to a few keys of a DirectInput keyboard, which must exist ("Could not find
//!   Keyboard input device!"). `Common\Bindings.txt` (the PC version's keyboard and mouse) is
//!   read too, its devices absent. The common fake DirectInput's wheel is that joystick
//!   (`WAL_DINPUT_WHEEL*`: its name, joystick type, buttons named from 0, the system keyboard
//!   and mouse listed); the keyboard's keys stay up.
//! - **Coins.** The cabinet's coin program posts the registered message `BALLISTICS ARCADE
//!   COINDROP` to the game's window (class `Diesel default rendering window`), lParam the count
//!   of coins since it started: the window procedure (`0x432fc0`) adds the difference to the
//!   coin counter (`0x41af90`), a credit each `Price` coins (`Common\Options.txt`). The
//!   payload posts it at each coin press (the readme's Escape key is not read by this build).
//!   Free play (the common `WAL_FREEPLAY`): the context's price (`[0x51adbc] + 0x1c0`, loaded
//!   from `Options.txt`) made 0, the game's own free play (the attract's loop `0x455589`: a
//!   button pressed without credits gives one, `0x41b130`).
//! - **Restart.** After `Restart Time` (3600 s, `+0x1b0`) the attract's loop broadcasts `SET
//!   OUTPUT` and quits (the cabinet's `start.bat` started it again): the time is made a year.
//! - **Display settings.** `display_settings.txt` holds the adapter and mode chosen in the
//!   settings dialog; when it does not match the adapter (or is missing) the boot shows the
//!   dialog (`DialogBoxParamA`, template 101): the payload's dialog procedure presses OK once
//!   the game's has set it up (its defaults: full screen 800x600, 32-bit), the game writes the
//!   file and goes on.
//! - **Motion seat.** `dbox.dll` (D-Box, `LoadLibraryA` at `0x41ad4f`, its functions optional)
//!   is not loaded: its `programInit` dies on its absent COM server.
//!
//! Natives: none (the joystick's buttons are the profile's `WAL_DINPUT_WHEEL_BUTTONS`); `coin`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::{dinput, iat, input, log};
use wal_protocol::button;

/// The coin counter's message (the cabinet's coin program posts it to the game's window, the
/// counter of every coin since its start in lParam) and the game's window class.
const COIN_DROP: &[u8] = b"BALLISTICS ARCADE COINDROP\0";
const WINDOW_CLASS: &[u8] = b"Diesel default rendering window\0";

/// The game's context (a pointer, relative to the executable) and its `Common\Options.txt`
/// settings: `Arcade Options.Restart Time` (float seconds) and `Price` (coins per credit).
const CONTEXT: usize = 0x11ADBC;
const RESTART_TIME: usize = 0x1B0;
const PRICE: usize = 0x1C0;
/// The restart time made a year: never.
const NO_RESTART: f32 = 31_536_000.0;

/// The boot's `push "BALLISTICS ARCADE COINDROP"` (`0x414659`): the build these addresses are
/// of.
const COIN_DROP_PUSH: usize = 0x1465A;
const COIN_DROP_STRING: u32 = 0x511E48;

fn exe() -> usize {
    unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as usize }
}

/// The settings over `Options.txt`'s, once the game has loaded them (each pass: the load may
/// come later): no restart; free play, a price of 0 (the game's own free play: a button press
/// without credits gives one).
fn settings(free_play: bool) -> bool {
    let context = unsafe { std::ptr::read_volatile((exe() + CONTEXT) as *const usize) };
    if context == 0 {
        return false;
    }
    let restart = (context + RESTART_TIME) as *mut f32;
    let price = (context + PRICE) as *mut u32;
    let mut changed = false;
    unsafe {
        if std::ptr::read_volatile(restart) != NO_RESTART && std::ptr::read_volatile(restart) > 0.0 {
            std::ptr::write_volatile(restart, NO_RESTART);
            changed = true;
        }
        if free_play && std::ptr::read_volatile(price) != 0 {
            std::ptr::write_volatile(price, 0);
            changed = true;
        }
    }
    changed
}

/// The coins posted, the virtual sticks' coin presses; free play's credit.
fn coin_thread() {
    let known = unsafe { std::ptr::read_unaligned((exe() + COIN_DROP_PUSH) as *const u32) } == COIN_DROP_STRING;
    let free_play = known && wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY").unwrap_or(false);
    if !known {
        log!("ballistics: unknown Ballistics.exe build, no settings");
    }
    log!("ballistics: free play {}", if free_play { "on" } else { "off" });
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowA, PostMessageA, RegisterWindowMessageA};
    let message = unsafe { RegisterWindowMessageA(COIN_DROP.as_ptr()) };
    let mut coins = 0isize;
    let mut held = false;
    loop {
        std::thread::sleep(Duration::from_millis(10));
        if known && settings(free_play) {
            log!("ballistics: settings applied (no restart{})", if free_play { ", price 0" } else { "" });
        }
        let pressed = (0..2).any(|p| input(p).pressed(button::COIN));
        if pressed && !held {
            let window = unsafe { FindWindowA(WINDOW_CLASS.as_ptr(), std::ptr::null()) };
            if !window.is_null() {
                coins += 1;
                unsafe { PostMessageA(window, message, 0, coins) };
                log!("ballistics: coin {coins}");
            }
        }
        held = pressed;
    }
}

/// The DirectInput keyboard's keys: none (the game binds W and F5, debug keys).
fn keyboard(_keys: &mut [u8; 256]) {}

type P = *mut std::ffi::c_void;
type DlgProc = unsafe extern "system" fn(P, u32, usize, isize) -> isize;
type DialogBoxParam = unsafe extern "system" fn(P, *const u8, P, Option<DlgProc>, isize) -> isize;

static DIALOG_BOX_PARAM: AtomicUsize = AtomicUsize::new(0);
static GAME_DLG_PROC: AtomicUsize = AtomicUsize::new(0);

const WM_INITDIALOG: u32 = 0x110;
const WM_COMMAND: u32 = 0x111;
const IDOK: usize = 1;

/// The display settings dialog's procedure: the game's, then OK at once.
unsafe extern "system" fn dialog_proc(window: P, message: u32, wparam: usize, lparam: isize) -> isize {
    let game: DlgProc = unsafe { std::mem::transmute(GAME_DLG_PROC.load(Ordering::Relaxed)) };
    let r = unsafe { game(window, message, wparam, lparam) };
    if message == WM_INITDIALOG {
        log!("ballistics: display settings dialog accepted");
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::PostMessageA(window as _, WM_COMMAND, IDOK, 0) };
    }
    r
}

/// `DialogBoxParamA`: the display settings dialog (shown when `display_settings.txt` does not
/// match the adapter) accepted with its defaults.
unsafe extern "system" fn dialog_box_param(instance: P, template: *const u8, parent: P, proc_: Option<DlgProc>, param: isize) -> isize {
    let orig: DialogBoxParam = unsafe { std::mem::transmute(DIALOG_BOX_PARAM.load(Ordering::Relaxed)) };
    match proc_ {
        Some(p) => {
            GAME_DLG_PROC.store(p as usize, Ordering::Relaxed);
            unsafe { orig(instance, template, parent, Some(dialog_proc), param) }
        }
        None => unsafe { orig(instance, template, parent, None, param) },
    }
}

static LOAD_LIBRARY: AtomicUsize = AtomicUsize::new(0);

/// `LoadLibraryA`: no `dbox.dll` (the D-Box motion seat's library; its COM server is not
/// there and its `programInit` crashed): the game runs without it, as on a cabinet without
/// the seat.
unsafe extern "system" fn load_library(name: *const u8) -> P {
    if !name.is_null() {
        let n = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes();
        if n.eq_ignore_ascii_case(b"dbox.dll") {
            log!("ballistics: dbox.dll not loaded (no motion seat)");
            return std::ptr::null_mut();
        }
    }
    let orig: unsafe extern "system" fn(*const u8) -> P = unsafe { std::mem::transmute(LOAD_LIBRARY.load(Ordering::Relaxed)) };
    unsafe { orig(name) }
}

pub fn init() {
    if let Some(o) = unsafe { iat::hook("kernel32.dll", "LoadLibraryA", load_library as *const () as usize) } {
        LOAD_LIBRARY.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { iat::hook("user32.dll", "DialogBoxParamA", dialog_box_param as *const () as usize) } {
        DIALOG_BOX_PARAM.store(o, Ordering::Relaxed);
    }
    dinput::set_keyboard(keyboard);
    std::thread::spawn(coin_thread);
    log!("ballistics: DirectInput keyboard and GameFrame");
}
