//! Mach Storm (Bandai Namco, 2013, `ACE7_WIN_10.exe`, Direct3D 9, 32-bit): a dome cabinet
//! flight game. Nothing loads `bngrw.dll`: the payload is loaded at the executable's entry
//! point (wal-loader), and the executable's build selects this code.
//!
//! * I/O board: NA-JV on COM3 (`jvs.rs`, flight layout): control lever x / y on analog
//!   channels 2 / 3, throttle lever on channel 1 (0x80 idle), machine gun byte 2 0x02,
//!   missile 0x01, a third lever button 0x04, the operator switches in byte 0.
//! * Errors: the game queues every device error it finds (cabinet camera, USB dongle, extra
//!   devices, network) in one list (0x47071e: the list's size increment); without the
//!   increment no error is ever shown.
//! * Extra devices: two boot checks of a serial device (`RSTALL`, `vER000`, `vST` commands,
//!   0x76430 and 0x76870) wait for minutes when it is absent: both return 1.
//! * Saves: the test mode data, bookkeeping, rankings... (`%c:\%s\%s.dat`, drives F and G)
//!   are `.\F<name>.dat` / `.\G<name>.dat` next to the executable, as dumps carry them.
//! * Dome: the picture is drawn through a 1920x1080 lookup texture made for the dome
//!   projector. `WAL_NAMCOES3_FLATSCREEN=1` (default) gives the texture creation at 0x3d2fee
//!   a flat screen map instead (a 9-byte stub in the padding at 0x3d3637 swaps its data
//!   argument).
//! * Input: no XInput pads (the game reads them besides the I/O board), the real keyboard
//!   and mouse filtered.

use wal_payload_common::log;
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::{game_base, safe_read};

/// PE timestamp and image size of `ACE7_WIN_10.exe`.
const BUILD: (u32, u32) = (0x52cf_bf33, 0xf3_a000);

/// The dome lookup texture's size.
const MAP_W: usize = 1920;
const MAP_H: usize = 1080;

pub(crate) fn is_machstorm() -> bool {
    let base = game_base();
    let Some(pe) = safe_read::<u32>(base + 0x3c).map(|p| p as usize) else { return false };
    if pe > 0x1000 || safe_read::<u32>(base + pe) != Some(0x4550) {
        return false;
    }
    (safe_read::<u32>(base + pe + 8), safe_read::<u32>(base + pe + 24 + 56)) == (Some(BUILD.0), Some(BUILD.1))
}

pub(crate) fn init() {
    wal_payload_common::start("namcoes3-machstorm");
    log!("machstorm: Mach Storm (ACE7_WIN_10)");
    let flag = |name: &str, default: bool| std::env::var(name).map_or(default, |v| v.trim() != "0");
    let patches: &[(usize, &[u8], &[u8], &str)] = &[
        // add dword [esi+4], 0x10: the error list's push
        (0x7071e, &[0x83, 0x46, 0x04, 0x10], &[0x90; 4], "device errors"),
        // xor eax,eax; inc eax; ret
        (0x76430, &[0x56, 0x68, 0xf0, 0x00], &[0x33, 0xc0, 0x40, 0xc3], "extra device check 1"),
        (0x76870, &[0x55, 0x8b, 0xec, 0x51], &[0x33, 0xc0, 0x40, 0xc3], "extra device check 2"),
        (0x544bb0, b"%c:\\%s\\%s.dat\0", b".\\%c%s.dat\0\0\0\0", "save file path"),
    ];
    for &(rva, expected, replacement, name) in patches {
        unsafe { patch(rva, expected, replacement, name) };
    }
    if flag("WAL_NAMCOES3_FLATSCREEN", true) {
        flatten_dome();
    }
    hide_xinput();
    wal_payload_common::input_filter::init(game_base(), |_| false);
    crate::jvs::init_flight(&[game_base()]);
}

/// Writes `replacement` at `rva` when the code there is `expected`.
unsafe fn patch(rva: usize, expected: &[u8], replacement: &[u8], name: &str) {
    let address = (game_base() + rva) as *mut u8;
    if unsafe { std::slice::from_raw_parts(address, expected.len()) } != expected {
        log!("machstorm: {name}: unexpected code at +{rva:#x}, not patched");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(address.cast(), replacement.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(replacement.as_ptr(), address, replacement.len());
        VirtualProtect(address.cast(), replacement.len(), old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), address.cast(), replacement.len());
    }
    log!("machstorm: {name} patched");
}

/// The flat screen lookup texture (two 16-bit texture coordinates per pixel): each half of
/// the screen bends toward the center as the dome image does, the rows are scaled with it.
/// The v values below 0 wrap (the game's sampler repeats), as the cabinet code's float to
/// 16-bit conversions do.
fn flat_map() -> Vec<u16> {
    let mut map = vec![0u16; MAP_W * MAP_H * 2];
    for y in 0..MAP_H {
        for x in 0..MAP_W {
            let mut xf = 2.0 * (x as f32 / MAP_W as f32) - 1.0;
            let mut yf = 2.0 * (y as f32 / MAP_H as f32) - 1.0;
            let k = 0.95 * xf.abs() / (xf.abs() + 0.5);
            xf = k.copysign(xf);
            yf *= 1.0 - k;
            let u = 0.5 * xf + 0.5;
            let v = -0.5 * yf - 0.5;
            let i = (y * MAP_W + x) * 2;
            map[i] = (65535.0 * u) as i32 as u16;
            map[i + 1] = (65535.0 * v) as i32 as u16;
        }
    }
    map
}

fn flatten_dome() {
    let map: &'static [u16] = Vec::leak(flat_map());
    let data = (map.as_ptr() as u32).to_le_bytes();
    // mov eax, map; mov [esp+0xc], eax (the third argument), then the creation function
    let stub = [0xb8, data[0], data[1], data[2], data[3], 0x89, 0x44, 0x24, 0x0c];
    unsafe {
        patch(0x3d3637, &[0xcc; 9], &stub, "flat screen map");
        // call 0x7d3640 -> call the stub (0x7d3637)
        patch(0x3d2fee, &[0x4e, 0x06, 0x00, 0x00], &[0x45, 0x06, 0x00, 0x00], "flat screen");
    }
}

const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

unsafe extern "system" fn xinput_none(_index: u32, _data: *mut core::ffi::c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

unsafe extern "system" fn xinput_caps_none(_index: u32, _flags: u32, _caps: *mut core::ffi::c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

/// `XInputGetState` (#2), `XInputSetState` (#3), `XInputGetCapabilities` (#4): no pads.
fn hide_xinput() {
    let base = game_base();
    unsafe {
        for ordinal in [2, 3] {
            wal_payload_common::iat::hook_ordinal(base, "XINPUT1_3.dll", ordinal, xinput_none as *const () as usize);
        }
        wal_payload_common::iat::hook_ordinal(base, "XINPUT1_3.dll", 4, xinput_caps_none as *const () as usize);
    }
}
