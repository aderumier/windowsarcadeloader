//! The UNIS "game232" driving cabinet (Crazy Speed Arcade, `Client_r.exe`, Ogre, 32-bit):
//! `WAL_UNIS_BOARD=game232`.
//!
//! The game (`configure/ApplicationCfg.ini`: `UseGame232 = 1`) reads its I/O board on COM1
//! (19200 bauds, 8N1, overlapped): the board streams frames on its own, `AA AA`, 10 data bytes,
//! their CRC-16 (Modbus: polynomial 0xA001, initial 0xFFFF, low byte first). The PC's frames
//! (`AA AA`, a length, the lights and the wheel's force feedback) need no answer.
//!
//! - **Data.** Bytes 0..5: three axes (16-bit, little endian, 0..1024; the game takes
//!   `1 - raw / 1024`): the wheel, the gas, the brake; bytes 6..8: 20 buttons (bit n of the
//!   24 bits), byte 8 bit 7 a coin (the game counts one per frame it is set: one frame per
//!   press).
//! - **Mapping.** The game routes the axes and buttons to its actions through
//!   `configure/js_game232_map.dat` (axis entries `F0 + axis`, button entries the button
//!   number).
//! - **Offline.** The game logs in to an internet server (182.16.68.115:21114, every 30 s)
//!   through its network engine's connection thread, which crashes on the result (its queue's
//!   global overwritten at the engine's start; the crash dialog, "CrazyForSpeed", took the
//!   screen): that thread ends at once (the game shows OFFLINE, as without a server), and the
//!   game's `connect` calls (`ws2_32` #4) to non-local addresses are refused, nothing leaves
//!   the machine. The engine's shutdown skips the queue's release (its global overwritten: the
//!   release crashed at the game's exit).
//! - **Dongle.** The game's actions (64-byte blocks, `CLogicModuleRoot::SendAction`, then its
//!   dispatch) are RSA-encrypted with a public key in the game (OpenSSL) and decrypted by its
//!   dongle (`dic32u.dll`, the private key): without the dongle the dispatched actions came
//!   out garbled ("no module process this action", the start button's credit spent, the game
//!   stuck on its title). Both functions return at once: the actions in clear.
//! - **Machine id.** The game checks a licence key bound to the cabinet ("UNIDENTIFIED SYSTEM
//!   ID"): the check answers valid.
//! - **WMI.** At start the game reads the memory size (`Win32_LogicalMemoryConfiguration`, a
//!   class Wine lacks) and the video card (`Win32_VideoController`): an empty enumeration
//!   (`IEnumWbemClassObject::Next` returning `S_FALSE`) passes its error check and the game
//!   reads a null object. Its checks after `Next` are made `jne` (no object: skipped, as an
//!   error).
//! - **Buttons.** Button 5 is START/NOS (the menus' confirm, the race's nitro), 4 VIEW (the
//!   camera; on the title the game card's renewal), 9 the operator's (it quits the game: not
//!   mapped). The wheel selects in the menus.
//!
//! Natives: `button0`..`button21`, `coin`; the wheel `lx`, the gas `accel`, the brake `brake`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log, serial};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use wal_protocol::Axis;

const PORT: &str = "COM1";
/// The board's frames, at most this often.
const PERIOD: Duration = Duration::from_millis(8);
/// The axes' full range.
const AXIS_MAX: i32 = 1024;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Button(u8),
    Coin,
}

const NATIVES: &[(&str, Native)] = &[
    ("button0", Native::Button(0)),
    ("button1", Native::Button(1)),
    ("button2", Native::Button(2)),
    ("button3", Native::Button(3)),
    ("button4", Native::Button(4)),
    ("button5", Native::Button(5)),
    ("button6", Native::Button(6)),
    ("button7", Native::Button(7)),
    ("button8", Native::Button(8)),
    ("button9", Native::Button(9)),
    ("button10", Native::Button(10)),
    ("button11", Native::Button(11)),
    ("button12", Native::Button(12)),
    ("button13", Native::Button(13)),
    ("button14", Native::Button(14)),
    ("button15", Native::Button(15)),
    ("button16", Native::Button(16)),
    ("button17", Native::Button(17)),
    ("button18", Native::Button(18)),
    ("button19", Native::Button(19)),
    ("button20", Native::Button(20)),
    ("button21", Native::Button(21)),
    ("coin", Native::Coin),
];

const DEFAULTS: &[(&str, &str)] = &[
    // the START/NOS button: the menus' confirm, the race's nitro
    ("start", "button5"),
    ("b1", "button5"),
    // the VIEW button: the camera, the game card's renewal on the title
    ("b2", "button4"),
    ("coin", "coin"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// CRC-16/Modbus.
fn crc16(data: &[u8]) -> u16 {
    data.iter().fold(0xFFFF, |mut crc, b| {
        crc ^= *b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
        }
        crc
    })
}

/// A board frame: `AA AA`, the data, its CRC (low byte first).
fn frame(data: &[u8; 10]) -> Vec<u8> {
    let mut f = vec![0xAA, 0xAA];
    f.extend_from_slice(data);
    f.extend_from_slice(&crc16(data).to_le_bytes());
    f
}

struct State {
    last: Instant,
    coin_held: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static FRAMES: AtomicU64 = AtomicU64::new(0);

/// An axis as the board reports it: the game sees `1 - raw / 1024`, `value` 0..1.
fn axis(value: f32) -> u16 {
    (AXIS_MAX - (value.clamp(0.0, 1.0) * AXIS_MAX as f32).round() as i32) as u16
}

/// The board's frame when the game reads the port.
fn idle() -> Vec<u8> {
    let mut guard = STATE.lock().unwrap();
    let state = guard.get_or_insert_with(|| State { last: Instant::now() - PERIOD, coin_held: false });
    if state.last.elapsed() < PERIOD {
        return Vec::new();
    }
    state.last = Instant::now();
    let stick = input(0);
    let mut data = [0u8; 10];
    let wheel = (stick.axis(Axis::LeftX) as f32 / 32767.0 + 1.0) / 2.0;
    let pedal = |a: Axis| stick.axis(a).max(0) as f32 / 32767.0;
    data[0..2].copy_from_slice(&axis(wheel).to_le_bytes());
    data[2..4].copy_from_slice(&axis(pedal(Axis::Accel)).to_le_bytes());
    data[4..6].copy_from_slice(&axis(pedal(Axis::Brake)).to_le_bytes());
    let mut buttons: u32 = 0;
    let mut coin = false;
    map().for_each_pressed(&stick, |n| match n {
        Native::Button(bit) => buttons |= 1 << bit,
        Native::Coin => coin = true,
    });
    data[6..9].copy_from_slice(&buttons.to_le_bytes()[..3]);
    // one frame per press
    if coin && !state.coin_held {
        data[8] |= 0x80;
    }
    state.coin_held = coin;
    if FRAMES.fetch_add(1, Ordering::Relaxed) == 0 {
        log!("game232: first frame to the game");
    }
    frame(&data)
}

/// The PC's frames (lights, force feedback): nothing to answer.
fn handler(_packet: &[u8]) -> Vec<u8> {
    Vec::new()
}

/// The checks after the WMI enumerations' `Next` (RVA, the code there, its `jl`'s offset).
const WMI_CHECKS: [(usize, &[u8]); 2] =
    [(0x10_2C06, &[0x85, 0xC0, 0x7C, 0x4D]), (0x10_2CCC, &[0x85, 0xC0, 0x0F, 0x8C, 0x94, 0x00, 0x00, 0x00])];

/// `test eax, eax; jl` made `jne` after the WMI enumerations: an empty one skipped.
fn patch_wmi() {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    for (rva, expected) in WMI_CHECKS {
        let code = unsafe { std::slice::from_raw_parts_mut((base + rva) as *mut u8, expected.len()) };
        if code != expected {
            log!("game232: WMI check at {rva:#x} not the expected code, left alone");
            continue;
        }
        let mut old = 0;
        unsafe { VirtualProtect(code.as_ptr().cast(), code.len(), PAGE_EXECUTE_READWRITE, &mut old) };
        // short jl 7C -> jne 75; near jl 0F 8C -> jne 0F 85
        if code[2] == 0x7C {
            code[2] = 0x75;
        } else {
            code[3] = 0x85;
        }
        unsafe { VirtualProtect(code.as_ptr().cast(), code.len(), old, &mut old) };
    }
}

/// The network engine's connection thread (RVA, its first bytes).
const CONNECT_THREAD: usize = 0x34_7DC0;
const CONNECT_THREAD_CODE: [u8; 5] = [0x53, 0x8B, 0x1D, 0x08, 0x8A];

/// The network engine's shutdown (RVA, its `mov ecx, [queue]`): skips the connection queue's
/// release, the queue's global overwritten (the release crashed: the crash dialog instead of
/// the game's exit).
const NET_SHUTDOWN_QUEUE: usize = 0x34_73C5;
const NET_SHUTDOWN_QUEUE_CODE: [u8; 6] = [0x8B, 0x0D, 0x6C, 0x9D, 0x8C, 0x00];

/// The shutdown jumps over the queue's release (`jmp` to its `ret`, 0x2E further).
fn patch_net_shutdown() {
    let at = unsafe { GetModuleHandleW(std::ptr::null()) } as usize + NET_SHUTDOWN_QUEUE;
    let code = unsafe { std::slice::from_raw_parts_mut(at as *mut u8, NET_SHUTDOWN_QUEUE_CODE.len()) };
    if code != NET_SHUTDOWN_QUEUE_CODE {
        log!("game232: not Crazy Speed Arcade's network shutdown, left alone");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(code.as_ptr().cast(), 2, PAGE_EXECUTE_READWRITE, &mut old);
        code[..2].copy_from_slice(&[0xEB, 0x2E]);
        VirtualProtect(code.as_ptr().cast(), 2, old, &mut old);
    }
}

/// The connection thread ends at once (`xor eax, eax; ret 4`: a thread procedure).
fn patch_connect_thread() {
    let at = unsafe { GetModuleHandleW(std::ptr::null()) } as usize + CONNECT_THREAD;
    let code = unsafe { std::slice::from_raw_parts_mut(at as *mut u8, CONNECT_THREAD_CODE.len()) };
    if code != CONNECT_THREAD_CODE {
        log!("game232: not Crazy Speed Arcade's connection thread, left alone");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(code.as_ptr().cast(), code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        code.copy_from_slice(&[0x31, 0xC0, 0xC2, 0x04, 0x00]);
        VirtualProtect(code.as_ptr().cast(), code.len(), old, &mut old);
    }
}

/// The actions' encryption (the game's RSA public key) and decryption (the dongle's) (RVA,
/// their first bytes: `sub esp, 0x104` / `sub esp, 0x608`).
const ACTION_CIPHERS: [(usize, [u8; 6]); 2] =
    [(0x7_F1D0, [0x81, 0xEC, 0x04, 0x01, 0x00, 0x00]), (0x7_F2E0, [0x81, 0xEC, 0x08, 0x06, 0x00, 0x00])];

/// The action ciphers return at once (`ret`: `cdecl`, their callers ignore the result).
fn patch_action_cipher() {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    for (rva, expected) in ACTION_CIPHERS {
        let code = unsafe { std::slice::from_raw_parts_mut((base + rva) as *mut u8, expected.len()) };
        if code != expected {
            log!("game232: not Crazy Speed Arcade's action cipher at {rva:#x}, left alone");
            continue;
        }
        let mut old = 0;
        unsafe {
            VirtualProtect(code.as_ptr().cast(), 1, PAGE_EXECUTE_READWRITE, &mut old);
            code[0] = 0xC3;
            VirtualProtect(code.as_ptr().cast(), 1, old, &mut old);
        }
    }
}

/// The machine id check (RVA, its first bytes): its licence key, bound to the cabinet.
const MACHINE_ID_CHECK: usize = 0xF_7640;
const MACHINE_ID_CHECK_CODE: [u8; 4] = [0x6A, 0xFF, 0x68, 0x84];

/// The machine id check answers valid (`mov al, 1; ret`: a `thiscall` without arguments).
fn patch_machine_id() {
    let at = unsafe { GetModuleHandleW(std::ptr::null()) } as usize + MACHINE_ID_CHECK;
    let code = unsafe { std::slice::from_raw_parts_mut(at as *mut u8, 3) };
    if unsafe { std::slice::from_raw_parts(at as *const u8, 4) } != MACHINE_ID_CHECK_CODE {
        log!("game232: not Crazy Speed Arcade's machine id check, left alone");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(code.as_ptr().cast(), 3, PAGE_EXECUTE_READWRITE, &mut old);
        code.copy_from_slice(&[0xB0, 0x01, 0xC3]);
        VirtualProtect(code.as_ptr().cast(), 3, old, &mut old);
    }
}

static REAL_CONNECT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Whether an IPv4 address (network order bytes) is this machine's or its LAN's.
fn local(ip: [u8; 4]) -> bool {
    matches!(ip, [127, ..] | [10, ..] | [192, 168, ..] | [169, 254, ..]) || (ip[0] == 172 && (16..32).contains(&ip[1]))
}

/// `connect`: the internet refused (`WSAECONNREFUSED`), the rest the real one's.
unsafe extern "system" fn connect(socket: usize, addr: *const u8, len: i32) -> i32 {
    const AF_INET: u16 = 2;
    if !addr.is_null() && len >= 8 && unsafe { (addr as *const u16).read_unaligned() } == AF_INET {
        let ip: [u8; 4] = unsafe { std::ptr::read_unaligned(addr.add(4) as *const [u8; 4]) };
        if !local(ip) {
            static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !LOGGED.swap(true, Ordering::Relaxed) {
                log!("game232: connection to {}.{}.{}.{} refused (offline)", ip[0], ip[1], ip[2], ip[3]);
            }
            unsafe { windows_sys::Win32::Networking::WinSock::WSASetLastError(10061) };
            return -1;
        }
    }
    let real: unsafe extern "system" fn(usize, *const u8, i32) -> i32 =
        unsafe { std::mem::transmute(REAL_CONNECT.load(Ordering::Relaxed)) };
    unsafe { real(socket, addr, len) }
}

pub fn init() {
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    if let Some(real) = unsafe { wal_payload_common::iat::hook_ordinal(exe, "ws2_32.dll", 4, connect as *const () as usize) } {
        REAL_CONNECT.store(real, Ordering::Relaxed);
    }
    patch_wmi();
    patch_machine_id();
    patch_connect_thread();
    patch_net_shutdown();
    patch_action_cipher();
    serial::install_streaming(PORT, handler, idle);
    log!("game232: board on {PORT}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc() {
        // CRC-16/Modbus check value
        assert_eq!(crc16(b"123456789"), 0x4B37);
    }
}
