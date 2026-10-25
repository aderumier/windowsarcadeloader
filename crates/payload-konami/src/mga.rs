//! Metal Gear Arcade (game code I36): the game is `launch.dll` (exports `dll_entry_init` /
//! `dll_entry_main`), run by `wal-avs-boot.exe`; it loads `system.dll` (the game: Direct3D 9,
//! window, sound, I/O), whose `libacio.dll` is the payload. spice2x (the open-source reference)
//! has the boards' layout.
//!
//! * The I36G board, two ACIO nodes, each an 88-byte status buffer (`get_control_status_buffer
//!   (node, out)`; `ac_io_i36s_*` are the same functions): node 21 the cabinet (bits 42 coin
//!   mech, 44 service, 45 test, 124 start), node 22 the gun controller (bits 104 side lever,
//!   105 side right, 106 front bottom, 107 side left, 108 front top, 109 trigger, 124 top, 125
//!   switch; bytes 42-43 the thumb stick's y, 44-45 its x, 16 bits centered on 0x7fff, forward
//!   0).
//! * The gun's aim: its board on COM1 (Team FrontLine gun I/O): a 1-byte command, then the
//!   answer and a checksum (0xff minus the bytes). 0x08 the position (x, y 10 bits, big endian,
//!   y 0 at the bottom: the dump's calibration L 0 R 1023 U 1023 D 0), 0x04 the version, 0x10
//!   the frequency DIP switches, 0x14 / 0x18 / 0x19 / 0x1a set the frequency, 0x1c the LED
//!   board check.
//! * The card reader's status takes `(device, out)` (`icca.rs` takes either order).
//!
//! Virtual stick: lx / ly the aim (gun or mouse), up / down / left / right or rx / ry the thumb
//! stick (moving).
//!
//! Native inputs for `native_map`: `test service coin start trigger top front_top front_bottom
//! side_left side_right side_lever switch up down left right`.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log};
use wal_protocol::Axis;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

/// launch.dll's base, when this process runs it.
pub fn launch_module() -> usize {
    let name: Vec<u16> = "launch.dll\0".encode_utf16().collect();
    unsafe { GetModuleHandleW(name.as_ptr()) as usize }
}

/// system.dll's base (the game; it imports this DLL, so it is loaded first).
pub fn game_module() -> usize {
    if launch_module() == 0 {
        return 0;
    }
    let name: Vec<u16> = "system.dll\0".encode_utf16().collect();
    unsafe { GetModuleHandleW(name.as_ptr()) as usize }
}

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    Start,
    Trigger,
    Top,
    FrontTop,
    FrontBottom,
    SideLeft,
    SideRight,
    SideLever,
    Switch,
    Up,
    Down,
    Left,
    Right,
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("start", Native::Start),
    ("trigger", Native::Trigger),
    ("top", Native::Top),
    ("front_top", Native::FrontTop),
    ("front_bottom", Native::FrontBottom),
    ("side_left", Native::SideLeft),
    ("side_right", Native::SideRight),
    ("side_lever", Native::SideLever),
    ("switch", Native::Switch),
    ("up", Native::Up),
    ("down", Native::Down),
    ("left", Native::Left),
    ("right", Native::Right),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("b1", "trigger"),
    ("b2", "front_top"),
    ("b3", "front_bottom"),
    ("b4", "side_left"),
    ("b5", "side_right"),
    ("b6", "side_lever"),
    ("b7", "top"),
    ("b8", "switch"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
];

const NODE_CABINET: u32 = 21;
const NODE_GUN: u32 = 22;
const STATUS_SIZE: usize = 88;

static MAP: Mutex<Option<ButtonMap<Native>>> = Mutex::new(None);

fn pressed() -> Vec<Native> {
    let mut guard = MAP.lock().unwrap();
    let map = guard.get_or_insert_with(|| ButtonMap::new(NATIVES, DEFAULT_MAP));
    let mut out = Vec::new();
    map.for_each_pressed(&wal_payload_common::input(0), |n| out.push(n));
    out
}

fn set_bit(buf: &mut [u8], bit: usize) {
    buf[bit / 8] |= 1 << (bit % 8);
}

/// A node's status buffer.
fn status(node: u32) -> [u8; STATUS_SIZE] {
    let mut buf = [0u8; STATUS_SIZE];
    let natives = pressed();
    match node {
        NODE_CABINET => {
            for n in natives {
                match n {
                    Native::Coin => set_bit(&mut buf, 42),
                    Native::Service => set_bit(&mut buf, 44),
                    Native::Test => set_bit(&mut buf, 45),
                    Native::Start => set_bit(&mut buf, 124),
                    _ => {}
                }
            }
        }
        NODE_GUN => {
            let stick = wal_payload_common::input(0);
            // the thumb stick: the right stick, or full tilt from the directions
            let mut x = (stick.axis(Axis::RightX) as i32 + 0x8000) as u16;
            let mut y = (stick.axis(Axis::RightY) as i32 + 0x8000) as u16;
            let (mut dx, mut dy) = (0i32, 0i32);
            for n in natives {
                match n {
                    Native::SideLever => set_bit(&mut buf, 104),
                    Native::SideRight => set_bit(&mut buf, 105),
                    Native::FrontBottom => set_bit(&mut buf, 106),
                    Native::SideLeft => set_bit(&mut buf, 107),
                    Native::FrontTop => set_bit(&mut buf, 108),
                    Native::Trigger => set_bit(&mut buf, 109),
                    Native::Top => set_bit(&mut buf, 124),
                    Native::Switch => set_bit(&mut buf, 125),
                    Native::Up => dy -= 1,
                    Native::Down => dy += 1,
                    Native::Left => dx -= 1,
                    Native::Right => dx += 1,
                    _ => {}
                }
            }
            if dx != 0 {
                x = (0x7fff + dx * 0x7fff).clamp(0, 0xffff) as u16;
            }
            if dy != 0 {
                y = (0x7fff + dy * 0x7fff).clamp(0, 0xffff) as u16;
            }
            buf[42..44].copy_from_slice(&y.to_le_bytes());
            buf[44..46].copy_from_slice(&x.to_le_bytes());
        }
        _ => {}
    }
    buf
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36g_update_control_status_buffer(_node: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36g_get_control_status_buffer(node: u32, out: *mut u8) {
    if out.is_null() {
        return;
    }
    let buf = status(node);
    unsafe { std::ptr::copy_nonoverlapping(buf.as_ptr(), out, STATUS_SIZE) };
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36s_update_control_status_buffer(node: u32) -> bool {
    ac_io_i36g_update_control_status_buffer(node)
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36s_get_control_status_buffer(node: u32, out: *mut u8) {
    ac_io_i36g_get_control_status_buffer(node, out)
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36g_set_cmdmode(_node: u32, _mode: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36s_set_cmdmode(_node: u32, _mode: u32) -> bool {
    true
}

// Lamps, the gun's vibration motor, coin blockers and counters: outputs without a device here.

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36g_control_lamp_bright(_node: u32, _lamps: u32, _bright: u8) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36g_control_motor_power(_node: u32, _power: u8) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36g_control_coin_blocker_open(_node: u32, _slot: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36g_control_coin_blocker_close(_node: u32, _slot: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36g_lock_coincounter(_node: u32, _slot: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36g_unlock_coincounter(_node: u32, _slot: u32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36g_req_volume_control(_node: u32, _a: u8, _b: u8, _c: u8, _d: u8) -> bool {
    true
}

/// The volume request done (3, spice2x's answer).
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_i36g_req_volume_control_isfinished(_node: u32, status: *mut u32) -> bool {
    if !status.is_null() {
        unsafe { *status = 3 };
    }
    true
}

/// The card reader's status request: no card event (11, spice2x's answer).
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_req_status(_device: u32, _a: u8) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_io_icca_req_status_isfinished(_device: u32, status: *mut u32) -> bool {
    if !status.is_null() {
        unsafe { *status = 11 };
    }
    true
}

// The gun board (COM1).

/// The frequency DIP switch chosen (1-4).
static FREQUENCY: AtomicU8 = AtomicU8::new(1);

/// The aim, 10 bits each: x left to right, y bottom to top.
fn aim() -> (u16, u16) {
    let stick = wal_payload_common::input(0);
    let x = (stick.axis(Axis::LeftX) as i32 + 32768) as u32 * 1023 / 65535;
    let y = (32767 - stick.axis(Axis::LeftY) as i32) as u32 * 1023 / 65535;
    (x as u16, y as u16)
}

fn gun(packet: &[u8]) -> Vec<u8> {
    let mut reply = Vec::new();
    for &command in packet {
        let mut data = match command {
            0x04 => vec![62],
            0x08 => {
                let (x, y) = aim();
                let [xh, xl] = x.to_be_bytes();
                let [yh, yl] = y.to_be_bytes();
                vec![0, xh, xl, yh, yl, 0, 0, 0, 0x75, 0x75, 0x75]
            }
            0x0c => vec![0],
            0x10 => vec![0x80 >> (FREQUENCY.load(Ordering::Relaxed) - 1)],
            0x14 | 0x18 | 0x19 | 0x1a => {
                let f = match command {
                    0x14 => 1,
                    0x18 => 2,
                    0x19 => 3,
                    _ => 4,
                };
                FREQUENCY.store(f, Ordering::Relaxed);
                vec![0]
            }
            0x1c => vec![0xff; 10],
            _ => {
                log!("mga: gun command {command:#04x} unknown");
                vec![1]
            }
        };
        let sum = data.iter().fold(0xffu8, |s, &b| s.wrapping_sub(b));
        data.push(sum);
        reply.extend(data);
    }
    reply
}

/// `setvolume` (setvolume_MGA_dll.dll): the cabinet's amplifier volume, not set (spice2x).
unsafe extern "C" fn setvolume(_volume: *const u8) -> i32 {
    0
}

type LoadLibraryA = unsafe extern "system" fn(*const u8) -> usize;
static ORIG_LOAD_LIBRARY: AtomicUsize = AtomicUsize::new(0);

/// `LoadLibraryA` (system.dll loads its modes: bootmode, testmode, gamemode): their boot check
/// library lookups answered too.
unsafe extern "system" fn load_library(name: *const u8) -> usize {
    let original: LoadLibraryA = unsafe { std::mem::transmute(ORIG_LOAD_LIBRARY.load(Ordering::Relaxed)) };
    let module = unsafe { original(name) };
    if module != 0 {
        crate::kbt::hook_module(module);
        let n = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy().to_lowercase();
        if n.contains("bootmode") {
            // with the fake e-Amusement server (WAL_KONAMI_EASRV=1) the check passes for real
            if !std::env::var("WAL_KONAMI_EASRV").is_ok_and(|v| v.trim() == "1") {
                skip_network_error(module);
            }
        }
    }
    module
}

/// Writes `code` at system.dll's `rva` when the bytes there are `head`.
fn patch(rva: usize, head: &[u8], code: &[u8]) -> bool {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    let at = game_module() + rva;
    if unsafe { std::slice::from_raw_parts(at as *const u8, head.len()) } != head {
        return false;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, code.len());
        VirtualProtect(at as *const c_void, code.len(), old, &mut old);
    }
    true
}

/// bootmode.dll's e-Amusement connection check (its checker's poll, rva 0x2a00): the
/// e-Amusement services' error (2000, no server: the cabinet's network is gone) is waited on
/// forever unless the error table marks it non-fatal. Taken as non-fatal: the check goes on to
/// the license setting (`GsNetwork::SetEaLicense`) and ends, e-Amusement (ess) started offline.
fn skip_network_error(module: usize) {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    // sub-state 2 (rva 0x2b20: mov edx, [GsNetwork::m_pInst]; mov eax, [edx+0x4c]): the error
    // shown and waited on
    let at = module + 0x2b20;
    let head = unsafe { std::slice::from_raw_parts(at as *const u8, 9) };
    if head[..2] != [0x8b, 0x15] || head[6..] != [0x8b, 0x42, 0x4c] {
        log!("mga: e-Amusement check code differs, kept");
        return;
    }
    // mov dword [ebx+0x11c], 7 (the non-fatal error's next step); mov dword [ebx+0xc], 1 (the
    // check's result, which its last step returns: 0 is "not done"); jmp rva 0x2ceb (return 0)
    let mut code = vec![0xc7, 0x83, 0x1c, 0x01, 0x00, 0x00, 0x07, 0x00, 0x00, 0x00, 0xc7, 0x43, 0x0c, 0x01, 0x00, 0x00, 0x00, 0xe9];
    code.extend(((module + 0x2ceb).wrapping_sub(at + code.len() + 4) as u32).to_le_bytes());
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, code.len());
        VirtualProtect(at as *const c_void, code.len(), old, &mut old);
    }
    log!("mga: boot e-Amusement error taken as non-fatal");
}

pub fn init(module: *mut c_void) {
    crate::icca::set_compat();
    crate::kbt::init(module);
    // the sound engine's failure (error 500, rva 0x571852) ignored: spice2x's patch
    if !patch(0x571850, &[0x84, 0xc0, 0x75, 0x0a, 0x68, 0xf4, 0x01, 0x00, 0x00, 0xe8], &[0x84, 0xc0, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90]) {
        log!("mga: sound engine check differs, kept");
    }
    match unsafe { iat::hook("kernel32.dll", "LoadLibraryA", load_library as *const () as usize) } {
        Some(o) => ORIG_LOAD_LIBRARY.store(o, Ordering::Relaxed),
        None => log!("mga: LoadLibraryA import not found"),
    }
    wal_payload_common::serial::install_in("COM1", gun, &[game_module()]);
    if unsafe { iat::hook("setvolume_MGA_dll.dll", "?setvolume@@YAHPAD@Z", setvolume as *const () as usize) }.is_none() {
        log!("mga: setvolume import not found");
    }
    crate::net::adapters_fit();
}
