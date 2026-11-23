//! Puzzle & Dragons Battle Tournament (GungHo / Taito, NESiCAxLive, Type X3, 2016: `game.exe`,
//! 32-bit), `WAL_NESICA_GAME=puzzledragons`.
//!
//! * The touch screen (the cabinet's only control besides the operator buttons): the game's
//!   touch device (+0x22460) asks `GetSystemMetrics(SM_DIGITIZER)` for a ready integrated
//!   multi-touch digitizer and registers its window (`RegisterTouchWindow`), else "touch monitor
//!   error" (and it asks again every frame, +0x22580); it reads `WM_TOUCH` (`GetTouchInputInfo`,
//!   +0x226F0). Answered by `wmtouch.rs`: the mouse (player 1's gun) touches while `b1` is held,
//!   the touchpad's or touchscreen's fingers (`WAL_TOUCH_SURFACE`). The real keyboard and mouse
//!   never reach the game (`input_filter`). A red dot where player 1's mouse or gun touches
//!   (`WAL_CROSSHAIR`, `pointer_d3d11.rs`).

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::{gun, iat, input_filter, log, pointer_d3d11};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

use crate::wmtouch;

pub(crate) fn detected() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAL_NESICA_GAME").is_ok_and(|v| v.trim().eq_ignore_ascii_case("puzzledragons")))
}

/// The settings manager's load of the play settings (`FUN_0041d1a0(name "cost2", file, dest,
/// 0x218, "COS2", 2, 0)`, a call at +0x3EEDF; the older "cost" record without the bytes is
/// migrated to it): its destination, the bytes free play (the test menu's play settings) and
/// free play tickets, then the continues, bonus continues and the credits of a game start and
/// of the continues.
const COST_LOAD_CALL: usize = 0x3EEDF;
const LOAD: usize = 0x41D1A0;
static COST: AtomicUsize = AtomicUsize::new(0);

type Load = unsafe extern "thiscall" fn(*const u8, *mut c_void, *mut u8, u32, *const u8, u32, u32) -> u32;

unsafe extern "thiscall" fn cost_load(name: *const u8, file: *mut c_void, dest: *mut u8, size: u32, magic: *const u8, version: u32, flags: u32) -> u32 {
    COST.store(dest as usize, Ordering::Relaxed);
    let load: Load = unsafe { std::mem::transmute(LOAD) };
    unsafe { load(name, file, dest, size, magic, version, flags) }
}

/// The common free play option: the play settings' free play kept on in memory (the test
/// menu's own setting otherwise).
fn free_play() {
    if wal_payload_common::free_play("WAL_NESICA_FREEPLAY") != Some(true) {
        return;
    }
    let base = iat::game_module();
    let at = base + COST_LOAD_CALL;
    let rel = (LOAD as isize - (at as isize + 5)) as i32;
    if unsafe { (at as *const [u8; 5]).read_unaligned() } != [0xE8, rel.to_le_bytes()[0], rel.to_le_bytes()[1], rel.to_le_bytes()[2], rel.to_le_bytes()[3]] {
        log!("puzzledragons: unknown executable at +{COST_LOAD_CALL:#x}: free play not set");
        return;
    }
    let ours = (cost_load as *const () as isize - (at as isize + 5)) as i32;
    let hex: String = ours.to_le_bytes().iter().map(|b| format!("{b:02X}")).collect();
    wal_payload_common::patches::apply_list(&format!("{:X}:{hex}", COST_LOAD_CALL + 1));
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_millis(100));
        let cost = COST.load(Ordering::Relaxed);
        if cost != 0 {
            let free = cost as *mut u8;
            if unsafe { free.read_volatile() } != 1 {
                unsafe { free.write_volatile(1) };
                log!("puzzledragons: free play");
            }
        }
    });
}

pub(crate) fn init() {
    free_play();
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let touch: [(&str, usize); 4] = [
        ("RegisterTouchWindow", wmtouch::register_touch_window as *const () as usize),
        ("UnregisterTouchWindow", wmtouch::unregister_touch_window as *const () as usize),
        ("GetTouchInputInfo", wmtouch::get_touch_input_info as *const () as usize),
        ("CloseTouchInputHandle", wmtouch::close_touch_input_handle as *const () as usize),
    ];
    for (name, ours) in touch {
        if unsafe { iat::hook_module(exe, "user32.dll", name, ours) }.is_none() {
            log!("puzzledragons: {name} not hooked");
        }
    }
    match unsafe { iat::hook_module(exe, "user32.dll", "GetSystemMetrics", wmtouch::get_system_metrics as *const () as usize) } {
        Some(o) => wmtouch::GET_SYSTEM_METRICS.store(o, std::sync::atomic::Ordering::Relaxed),
        None => log!("puzzledragons: GetSystemMetrics not hooked"),
    }
    input_filter::init(exe, |_| false);
    input_filter::own_mouse(exe);
    wmtouch::start("puzzledragons", "WAL_TOUCH_SURFACE");
    // the game shows no pointer: a dot where player 1's mouse or gun touches (its Direct3D 10 is
    // DXVK's, on Direct3D 11)
    if wal_payload_common::crosshairs(true) {
        let size = std::env::var("WAL_CROSSHAIR_SIZE").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.5);
        pointer_d3d11::init(pointer_d3d11::Dot { color: [1.0, 0.125, 0.125], size: (size / 100.0).clamp(0.005, 0.2), position: gun::gun_in_picture, crosshair: false });
    }
    log!("puzzledragons: touch screen emulated");
}
