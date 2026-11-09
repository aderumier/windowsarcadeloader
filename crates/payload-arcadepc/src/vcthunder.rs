//! VCThunder 0.5 (<https://github.com/mourix/VCThunder>, MIT: `VCThunder.exe hydro|offroad`): a
//! Windows runtime of Midway's Hydro Thunder (`hydro\HYDRO.EXE`, the Diego I/O board) and
//! Offroad Thunder (`offroad\offroad3.exe`). Its stub executable reserves the games' address
//! span at 0x100000 and loads `VCThunder.dll`, which maps the original executable there, unpacks
//! it, emulates its hardware and draws its Glide through `vcglide.dll` (SDL3 GPU on Direct3D 12:
//! the runner's vkd3d-proton; else its CPU rasteriser). Settings: the profile's `vcthunder.ini`
//! in the run directory (the runtime's own keys, its `docs/configuration.md`).
//!
//! * Inputs: the common fake DirectInput wheel (`WAL_DINPUT_WHEEL`: X steering, Y gas, Z brake,
//!   the virtual sticks' buttons as its buttons, bound by the ini's `[gamepad]`), hooked into
//!   `VCThunder.dll` once it is loaded (it resolves `DirectInput8Create` at run time); the
//!   game's constant force goes to the launcher (`WAL_DINPUT_WHEEL_FFB_OUT`). The real keyboard
//!   is filtered, XInput disabled by the profile. The ini binds two kinds of keys, which
//!   [`key_held`] reports: the second coin slot (`6`: player 2's coin), and Offroad's gears
//!   (`E F G`): the runtime reads the gear whose key is held, neutral without one; its own
//!   shift up / down latch starts in neutral (the truck stays on the grid until a shift, even
//!   with the automatic transmission), the payload's starts in first gear.
//! * Hydro Thunder's network stack cleans Winsock up (`WSACleanup`, through the runtime) when
//!   it enters its operator menus: the launcher connection was closed and could not be opened
//!   again (no input in the menus); the payload holds one more Winsock reference.
//! * The ini's `[graphics]` from the profile's options at each start: `WAL_VCTHUNDER_WIDESCREEN=1`
//!   the runtime's 16:9 hack (`aspect = 16:9`, `widescreen = true`), else the cabinet's 4:3;
//!   `WAL_VCTHUNDER_60FPS` (`frame_rate = 60`: an interpolated picture between the game's own,
//!   else 30).
//! * Free play (`WAL_ARCADEPC_FREEPLAY`, else `WAL_FREEPLAY`): Hydro Thunder's credits to start
//!   and to continue (`_coins_CreditsToStart` 0x134508, `_coins_CreditsToContinue` 0x134518:
//!   the pricing table's) answer 0; Offroad Thunder's free play flag (`_money_GetFreePlay`
//!   0x1c4f1c: `[0x410e74] == 0xfeed12f8`, set by `_money_ModuleInit` from a magic value) is
//!   held set. The game's code is in place once `VCThunder.dll` copied it: the patch waits for
//!   the original bytes, then 1 s (the runtime measures the unmodified code's checksum and
//!   patches its hooks within milliseconds of the copy).

use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use wal_payload_common::{input, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect, VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT};

/// The second coin slot's key (the ini's `[digital] coin2`).
const VK_COIN2: i32 = b'6' as i32;
/// Offroad Thunder's gear keys (the ini's `[digital] shift1`..`shift3`): the runtime reads the
/// gear whose key is held, neutral without one.
const VK_GEARS: [u8; 3] = [b'E', b'F', b'G'];
/// The latched gear (1 to 3): first gear at start.
static GEAR: AtomicU8 = AtomicU8::new(1);
/// The shift buttons held at the last look (bit 0 up, bit 1 down).
static SHIFTS: AtomicU8 = AtomicU8::new(0);

/// The runtime's `GetAsyncKeyState`: the second coin slot (player 2's coin) and Offroad's
/// latched gear (b5 / b6 shift up / down on their press, b7 / b8 first / third gear); player 1's
/// other inputs are the wheel's.
pub fn key_held(vk: i32) -> bool {
    use wal_protocol::button::{B5, B6, B7, B8, COIN};
    if vk == VK_COIN2 {
        return input(1).pressed(COIN);
    }
    let Some(i) = VK_GEARS.iter().position(|&k| k as i32 == vk) else { return false };
    let p1 = input(0);
    if p1.pressed(B7) {
        GEAR.store(1, Ordering::Relaxed);
    } else if p1.pressed(B8) {
        GEAR.store(3, Ordering::Relaxed);
    }
    let shifts = p1.pressed(B5) as u8 | (p1.pressed(B6) as u8) << 1;
    let pressed = shifts & !SHIFTS.swap(shifts, Ordering::Relaxed);
    if pressed & 1 != 0 {
        GEAR.store((GEAR.load(Ordering::Relaxed) + 1).min(3), Ordering::Relaxed);
    }
    if pressed & 2 != 0 {
        GEAR.store(GEAR.load(Ordering::Relaxed).saturating_sub(1).max(1), Ordering::Relaxed);
    }
    GEAR.load(Ordering::Relaxed) as usize == i + 1
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Title {
    Hydro,
    Offroad,
}

/// The title on the command line (`VCThunder.exe hydro|offroad`, or `--title ID`).
fn title() -> Option<Title> {
    std::env::args().skip(1).find_map(|a| match a.to_ascii_lowercase().as_str() {
        "hydro" => Some(Title::Hydro),
        "offroad" => Some(Title::Offroad),
        _ => None,
    })
}

/// `vcthunder.ini`'s `key` of `[section]` set to `value` (added at the section's end when
/// missing).
fn set_key(text: &str, section: &str, key: &str, value: &str) -> String {
    let mut out = Vec::new();
    let mut in_section = false;
    let mut done = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if in_section && !done {
                out.push(format!("{key} = {value}"));
                done = true;
            }
            in_section = t.eq_ignore_ascii_case(&format!("[{section}]"));
        } else if in_section && !done && t.split('=').next().is_some_and(|k| k.trim().eq_ignore_ascii_case(key)) {
            out.push(format!("{key} = {value}"));
            done = true;
            continue;
        }
        out.push(line.to_string());
    }
    if !done {
        if !in_section {
            out.push(format!("[{section}]"));
        }
        out.push(format!("{key} = {value}"));
    }
    out.join("\r\n") + "\r\n"
}

/// The profile's options into the run directory's `vcthunder.ini` (the profile's copy, a file
/// of the run directory: never the dump's).
fn write_ini() {
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) else { return };
    let path = dir.join("vcthunder.ini");
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        log!("vcthunder: vcthunder.ini is the dump's (not the profile's copy): left alone");
        return;
    }
    let Ok(text) = std::fs::read_to_string(&path) else {
        log!("vcthunder: no vcthunder.ini next to the executable");
        return;
    };
    let wide = std::env::var("WAL_VCTHUNDER_WIDESCREEN").is_ok_and(|v| v.trim() == "1");
    let mut text = set_key(&text, "graphics", "aspect", if wide { "16:9" } else { "4:3" });
    text = set_key(&text, "graphics", "widescreen", if wide { "true" } else { "false" });
    let fps60 = std::env::var("WAL_VCTHUNDER_60FPS").map_or(true, |v| v.trim() != "0");
    text = set_key(&text, "graphics", "frame_rate", if fps60 { "60" } else { "30" });
    if let Ok(dips) = std::env::var("WAL_VCTHUNDER_DIPS") {
        if !dips.trim().is_empty() {
            text = set_key(&text, "controls", "dip_switches", dips.trim());
        }
    }
    match std::fs::write(&path, text) {
        Ok(()) => log!("vcthunder: vcthunder.ini: {} picture, {} fps", if wide { "16:9 widescreen" } else { "4:3" }, if fps60 { 60 } else { 30 }),
        Err(e) => log!("vcthunder: vcthunder.ini not written: {e}"),
    }
}

/// Whether `len` bytes at `addr` are committed memory.
fn committed(addr: usize, len: usize) -> bool {
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let n = unsafe { VirtualQuery(addr as _, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) };
    n != 0 && info.State == MEM_COMMIT && addr + len <= info.BaseAddress as usize + info.RegionSize
}

/// Writes `bytes` at `addr` in the game's span.
fn write(addr: usize, bytes: &[u8]) {
    let mut old = 0;
    unsafe {
        VirtualProtect(addr as _, bytes.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), addr as *mut u8, bytes.len());
        if old != PAGE_EXECUTE_READWRITE && old != 0 {
            VirtualProtect(addr as _, bytes.len(), old, &mut old);
        }
    }
}

fn bytes_at(addr: usize, len: usize) -> Option<Vec<u8>> {
    committed(addr, len).then(|| unsafe { std::slice::from_raw_parts(addr as *const u8, len) }.to_vec())
}

/// Hydro Thunder: `_coins_CreditsToStart` / `_coins_CreditsToContinue` (`mov eax, [x]; ret`)
/// answer 0 (`xor eax, eax; nop; nop; nop; ret`).
const HYDRO_PATCHES: &[(usize, &[u8], &[u8])] = &[
    (0x134508, &[0xA1, 0xD0, 0x59, 0x56, 0x00, 0xC3], &[0x31, 0xC0, 0x90, 0x90, 0x90, 0xC3]),
    (0x134518, &[0xA1, 0xD4, 0x59, 0x56, 0x00, 0xC3], &[0x31, 0xC0, 0x90, 0x90, 0x90, 0xC3]),
];

/// Offroad Thunder: `_money_GetFreePlay`'s code (`mov ecx, [0x410e74]`), its flag and value.
const OFFROAD_GET_FREE_PLAY: (usize, &[u8]) = (0x1c4f1c, &[0x8B, 0x0D, 0x74, 0x0E, 0x41, 0x00]);
const OFFROAD_FREE_PLAY_FLAG: usize = 0x410e74;
const OFFROAD_FREE_PLAY_ON: u32 = 0xfeed12f8;

fn free_play(title: Title) {
    std::thread::spawn(move || {
        let start = std::time::Instant::now();
        match title {
            Title::Hydro => {
                // the code copied (the first function's bytes), then both patched at once
                loop {
                    let (addr, orig, _) = HYDRO_PATCHES[0];
                    if bytes_at(addr, orig.len()).is_some_and(|b| b == orig) {
                        break;
                    }
                    if start.elapsed() > Duration::from_secs(120) {
                        log!("vcthunder: Hydro Thunder's code never appeared: no free play");
                        return;
                    }
                    std::thread::sleep(Duration::from_micros(500));
                }
                // the runtime's own patches and its checksum of the unmodified code (its shim
                // answers that value) are done within milliseconds of the copy
                std::thread::sleep(Duration::from_secs(1));
                for &(addr, orig, new) in HYDRO_PATCHES {
                    if bytes_at(addr, orig.len()).is_some_and(|b| b == orig) {
                        write(addr, new);
                    } else {
                        log!("vcthunder: unexpected code at {addr:#x}: not patched");
                    }
                }
                log!("vcthunder: free play (credits to start and to continue: 0)");
            }
            Title::Offroad => {
                let (addr, orig) = OFFROAD_GET_FREE_PLAY;
                loop {
                    if bytes_at(addr, orig.len()).is_some_and(|b| b == orig) {
                        break;
                    }
                    if start.elapsed() > Duration::from_secs(120) {
                        log!("vcthunder: Offroad Thunder's code never appeared: no free play");
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                log!("vcthunder: free play (the game's free play flag held set)");
                // _money_ModuleInit sets the flag at the game's start: kept set after it
                loop {
                    let flag = OFFROAD_FREE_PLAY_FLAG as *mut u32;
                    if committed(OFFROAD_FREE_PLAY_FLAG, 4) && unsafe { flag.read_volatile() } != OFFROAD_FREE_PLAY_ON {
                        unsafe { flag.write_volatile(OFFROAD_FREE_PLAY_ON) };
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    });
}

/// The fake DirectInput wheel in `VCThunder.dll`, as soon as the stub loaded it.
fn hook_runtime() {
    std::thread::spawn(|| {
        let start = std::time::Instant::now();
        loop {
            let m = unsafe { GetModuleHandleW(windows_sys::w!("VCThunder.dll")) } as usize;
            if m != 0 {
                wal_payload_common::dinput::hook_module(m);
                log!("vcthunder: VCThunder.dll's DirectInput is the fake wheel");
                return;
            }
            if start.elapsed() > Duration::from_secs(60) {
                log!("vcthunder: VCThunder.dll never loaded");
                return;
            }
            std::thread::sleep(Duration::from_micros(200));
        }
    });
}

/// One more Winsock reference: Hydro Thunder's network stack (`WSAStartup` / `WSACleanup`,
/// through the runtime) cleans Winsock up when it enters its operator menus, which closed the
/// payload's launcher connection and failed its reconnections (no input in the menus).
fn hold_winsock() {
    type Startup = unsafe extern "system" fn(u16, *mut u8) -> i32;
    unsafe {
        let ws2 = windows_sys::Win32::System::LibraryLoader::LoadLibraryW(windows_sys::w!("ws2_32.dll"));
        if let Some(f) = windows_sys::Win32::System::LibraryLoader::GetProcAddress(ws2, c"WSAStartup".as_ptr().cast()) {
            let f: Startup = std::mem::transmute(f);
            let mut data = [0u8; 512];
            let r = f(0x0202, data.as_mut_ptr());
            log!("vcthunder: Winsock held (WSAStartup {r})");
        }
    }
}

pub fn init() {
    hold_winsock();
    let title = title();
    log!("vcthunder: title {title:?}");
    write_ini();
    hook_runtime();
    if let Some(title) = title {
        match wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") {
            Some(true) => free_play(title),
            _ => log!("vcthunder: free play off: the operator settings' pricing"),
        }
    }
}
