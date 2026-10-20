//! Sega Europa-R payload (Ford Racing Full Blown, SEGA Rally 3).
//!
//! Ford Racing imports `inpout32.dll` (parallel port access, the force feedback board)
//! statically: this DLL takes its place in the run directory, so it loads before the game code
//! without any injector. SEGA Rally 3 loads `inpout32.dll` at run time (`LoadLibraryA`, then
//! `Out32` by name) and imports `xinput1_3.dll`: the payload is installed as that one (its
//! `XInput*` exports: no pad), and its `LoadLibraryA` of `inpout32.dll` gets this DLL.
//!
//! * `happ.rs`: the Happ UGCI USB board (HID: wheel, pedals, buttons), found through SetupAPI
//!   and read with `ReadFile`, emulated from the virtual sticks.
//! * `shell.rs`: the cabinet's GameShell (coins, credits): its window, the game's
//!   `WM_COPYDATA` messages answered.
//! * Force feedback: the bytes the game writes to the parallel port (`Out32`, payload-common's
//!   `port_ffb`).
//! * `settings.rs`: operator settings (resolution, free play) from profile options.
//! * `testmenu.rs`: TEST runs the GameShell's test menu (the payload is loaded there too).
//! * Profile code patches (`WAL_PATCHES`).

#![allow(non_snake_case)]

mod happ;
mod settings;
mod shell;
mod testmenu;

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log, patches};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

/// This DLL.
static MODULE: AtomicUsize = AtomicUsize::new(0);
static ORIG_LOAD_LIBRARY: AtomicUsize = AtomicUsize::new(0);
/// `LoadLibraryA("./inpout32.dll")` (SEGA Rally 3): this DLL, whatever its file name.
unsafe extern "system" fn load_library(name: *const u8) -> usize {
    if !name.is_null() {
        let n = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy().to_ascii_lowercase();
        if n.rsplit(['/', '\\']).next() == Some("inpout32.dll") {
            log!("europar: LoadLibraryA({n:?}): the payload");
            return MODULE.load(Ordering::Relaxed);
        }
    }
    let f: unsafe extern "system" fn(*const u8) -> usize = unsafe { std::mem::transmute(ORIG_LOAD_LIBRARY.load(Ordering::Relaxed)) };
    unsafe { f(name) }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        MODULE.store(module as usize, Ordering::Relaxed);
        wal_payload_common::start("europar");
        if let Some(o) = unsafe { iat::hook("kernel32.dll", "LoadLibraryA", load_library as *const () as usize) } {
            ORIG_LOAD_LIBRARY.store(o, Ordering::Relaxed);
        }
        // XInput pads hidden from the game (its input comes from the launcher only): its
        // xinput1_3.dll imports by ordinal (2 GetState, 3 SetState, 4 GetCapabilities, 5 Enable;
        // GRID imports Wine's, SEGA Rally 3 this DLL's, whose ordinal 2 is Out32)
        let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
        let xinput: [(u16, usize); 4] = [
            (2, xinput_get_state as *const () as usize),
            (3, xinput_set_state as *const () as usize),
            (4, xinput_get_capabilities as *const () as usize),
            (5, xinput_enable as *const () as usize),
        ];
        for (ordinal, f) in xinput {
            unsafe { iat::hook_ordinal(exe, "xinput1_3.dll", ordinal, f) };
        }
        if testmenu::is_test_menu() {
            // the GameShell's test menu (loaded by wal-loader)
            happ::init(true);
            testmenu::init_shell();
            log!("europar: initialized in the test menu");
        } else {
            patches::apply();
            settings::init();
            happ::init(false);
            // SEGA Rally 3 counts its credits itself (no GameShell window)
            match credits_rva() {
                Some(rva) => count_coins(rva),
                None => shell::init(),
            }
            testmenu::watch_game();
            log!("europar: initialized");
        }
    }
    TRUE
}

/// `short Inp32(short port)`: nothing is read from the parallel port.
#[unsafe(no_mangle)]
pub extern "system" fn inp32(port: i16) -> i16 {
    log!("europar: Inp32({port:#x})");
    0
}

/// `void Out32(short port, short data)`: the force feedback board on the parallel port.
#[unsafe(no_mangle)]
pub extern "system" fn out32(port: i16, data: i16) {
    wal_payload_common::port_ffb::out("WAL_EUROPAR", port as u16, data as u16);
}

/// `WAL_EUROPAR_CREDITS=<rva>`: the game's credit count (u32, SEGA Rally 3: `0x741748`, the
/// object at `0x741738`; a game starts when it is above 0 unless in free play): each coin of
/// player 1 adds one.
fn credits_rva() -> Option<usize> {
    let v = std::env::var("WAL_EUROPAR_CREDITS").ok()?;
    usize::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok()
}

fn count_coins(rva: usize) {
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    std::thread::spawn(move || {
        let mut held = false;
        loop {
            let coin = wal_payload_common::input(0).pressed(wal_protocol::button::COIN);
            if coin && !held {
                let credits = unsafe { &*((base + rva) as *const std::sync::atomic::AtomicU32) };
                log!("europar: coin, {} credits", credits.fetch_add(1, Ordering::Relaxed) + 1);
            }
            held = coin;
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    });
}

const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

/// `XInputGetState(user, state)`: no pad (the cabinet's controls are its board's).
#[unsafe(no_mangle)]
pub extern "system" fn xinput_get_state(_user: u32, _state: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

#[unsafe(no_mangle)]
pub extern "system" fn xinput_set_state(_user: u32, _vibration: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

#[unsafe(no_mangle)]
pub extern "system" fn xinput_get_capabilities(_user: u32, _flags: u32, _caps: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

#[unsafe(no_mangle)]
pub extern "system" fn xinput_enable(_enable: i32) {}
