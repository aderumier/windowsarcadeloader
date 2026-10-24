//! Winning Eleven 2008 (WORLD SOCCER Winning Eleven ARCADE CHAMPIONSHIP 2008): the payload is
//! HCK/bin/game.exe's `libacio.dll` (ACIO bus, card reader, PJEI board `pjei.rs`, PJEC pad port
//! `pjec.rs`); its `libtouchpanel.dll` imports are pointed at this DLL's functions of the same
//! names (`touchpanel.rs`), as for Otomedius: present, never touched (no pointer: the pad plays).
//!
//! Of the four security plugs the game tries, the `C` cabinet's (`GCHCKJBA`: card reader and pad)
//! is the one present (`pjei.rs`).
//!
//! The game runs from the cabinet's D: drive (`D:/HCK/data`, `D:/HCK/save`, `D:\romcheck.bin`):
//! D: is the dump root, two levels above the executable (`WAL_KONAMI_DDRIVE` to change it), for
//! the executable and sound.dll.

use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{drive, iat, log};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect};

/// The game: an executable importing libtouchpanel.dll's `ac_touchpanel_new2` (loaded with it).
pub fn detected() -> bool {
    let name: Vec<u16> = "libtouchpanel.dll\0".encode_utf16().collect();
    let panel = unsafe { GetModuleHandleW(name.as_ptr()) };
    !panel.is_null() && unsafe { GetProcAddress(panel, c"ac_touchpanel_new2".as_ptr().cast()) }.is_some()
}

pub fn init(module: *mut c_void) {
    crate::pjei::set_software_plug(b"GCHCKJBA");
    // every virtual button is the pad's: the screen is not touched
    crate::touchpanel::set_touch_button(0);
    drive::init("WAL_KONAMI_DDRIVE", "..\\..");
    // sound.dll opens its sound banks (`D:/HCK/data/sddata/...`) itself
    drive::redirect_module("sound.dll");
    let mut hooked = 0;
    for (name, address) in crate::otomedius::exports(module) {
        if name.starts_with("ac_touchpanel_") && unsafe { iat::hook("libtouchpanel.dll", &name, address) }.is_some() {
            hooked += 1;
        }
    }
    crate::net::adapters_fit();
    // libacweeq sets the cabinet's speaker equalizer through the Realtek driver's effects
    // (RtEffect.dll): without it the call fails, SOUND INIT ERROR 5-1511-0101
    if unsafe { iat::hook("libacweeq.dll", "fck_set_eq", fck_set_eq as *const () as usize) }.is_none() {
        log!("winning eleven: fck_set_eq import not found");
    }
    rom_check_init();
    settings_init();
    notice_init();
    controller_select_init();
    sound_effects_init();
    game_log_init();
    log!("winning eleven: {hooked} libtouchpanel imports answered by the payload");
}

// The game's log function `(fmt, ...)`: emptied in the release build (`xor eax, eax; ret`).
core::arch::global_asm!(
    ".globl {name}\n{name}:",
    "lea eax, [esp + 8]",
    "push eax",
    "push dword ptr [esp + 8]",
    "call {log}",
    "add esp, 8",
    "xor eax, eax",
    "ret",
    name = sym game_log,
    log = sym log_va,
);
unsafe extern "C" {
    fn game_log();
}

extern "C" fn log_va(fmt: *const c_char, args: *const u32) {
    // the function is the build's shared `return 0`: other empty functions called with other
    // arguments land there too; only a text first argument is a message
    let Some(text) = (unsafe { c_string(fmt as usize) }) else { return };
    if text.is_empty() || text.iter().any(|&b| b < 0x20 && !b"\t\r\n".contains(&b)) {
        return;
    }
    let msg = unsafe { format(fmt, args) };
    log!("game: {}", msg.trim_end());
}

/// A printf format with its 32-bit stack arguments; `%s` of an unreadable pointer is shown as
/// `(bad)` (some messages pass other values there: the CRT's own printf faulted).
unsafe fn format(fmt: *const c_char, mut args: *const u32) -> String {
    let fmt = unsafe { std::ffi::CStr::from_ptr(fmt) }.to_bytes();
    let mut out = Vec::new();
    let mut next = || {
        let v = unsafe { *args };
        args = unsafe { args.add(1) };
        v
    };
    let mut i = 0;
    while i < fmt.len() {
        if fmt[i] != b'%' {
            out.push(fmt[i]);
            i += 1;
            continue;
        }
        i += 1;
        // flags, width, precision, length: kept for the conversion
        let spec_start = i;
        while i < fmt.len() && b"-+ #0123456789.hlI64".contains(&fmt[i]) {
            i += 1;
        }
        let Some(&conv) = fmt.get(i) else { break };
        i += 1;
        let spec = &fmt[spec_start..i - 1];
        let wide = spec.windows(2).any(|w| w == b"ll" || w == b"64");
        let width: usize = std::str::from_utf8(spec)
            .ok()
            .and_then(|s| s.trim_start_matches(['-', '0']).split('.').next()?.parse().ok())
            .unwrap_or(0);
        let text = match conv {
            b'%' => "%".to_string(),
            b'd' | b'i' if wide => (next() as u64 | (next() as u64) << 32).to_string(),
            b'd' | b'i' => (next() as i32).to_string(),
            b'u' if wide => (next() as u64 | (next() as u64) << 32).to_string(),
            b'u' => next().to_string(),
            b'x' | b'X' | b'p' => {
                let v = if wide { next() as u64 | (next() as u64) << 32 } else { next() as u64 };
                if conv == b'x' { format!("{v:x}") } else { format!("{v:X}") }
            }
            b'c' => ((next() & 0xff) as u8 as char).to_string(),
            b'f' | b'g' | b'e' => f64::from_bits(next() as u64 | (next() as u64) << 32).to_string(),
            b's' => {
                let p = next() as usize;
                match unsafe { c_string(p) } {
                    // Shift-JIS messages, shown as Japanese
                    Some(bytes) => shift_jis(&bytes),
                    None => format!("(bad {p:#x})"),
                }
            }
            _ => String::new(),
        };
        let pad = if spec.first() == Some(&b'0') { '0' } else { ' ' };
        let text = if text.chars().count() < width && spec.first() != Some(&b'-') {
            pad.to_string().repeat(width - text.chars().count()) + &text
        } else {
            text
        };
        out.extend_from_slice(text.as_bytes());
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `WAL_KONAMI_GAME_LOG=1`: the game's messages (its boot modules, errors) in the payload log.
/// Its log function (rva 0x30510, `xor eax, eax; ret` and padding, shared with the build's other
/// empty functions) jumps to ours.
fn game_log_init() {
    if !std::env::var("WAL_KONAMI_GAME_LOG").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    let at = unsafe { GetModuleHandleW(std::ptr::null()) } as usize + 0x30510;
    if unsafe { std::slice::from_raw_parts(at as *const u8, 5) } != [0x33, 0xc0, 0xc3, 0xcc, 0xcc] {
        log!("winning eleven: game log function differs, no game log");
        return;
    }
    let rel = (game_log as usize).wrapping_sub(at + 5) as i32;
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, 5, PAGE_EXECUTE_READWRITE, &mut old);
        *(at as *mut u8) = 0xe9;
        std::ptr::write_unaligned((at + 1) as *mut i32, rel);
        VirtualProtect(at as *const c_void, 5, old, &mut old);
    }
}

/// Shift-JIS text (the game's messages are Japanese).
fn shift_jis(bytes: &[u8]) -> String {
    use windows_sys::Win32::Globalization::MultiByteToWideChar;
    let mut wide = vec![0u16; bytes.len() + 1];
    let n = unsafe { MultiByteToWideChar(932, 0, bytes.as_ptr(), bytes.len() as i32, wide.as_mut_ptr(), wide.len() as i32) };
    String::from_utf16_lossy(&wide[..n.max(0) as usize])
}

/// The NUL-terminated string at `p` (at most 1024 bytes), when its pages are readable.
unsafe fn c_string(p: usize) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    for at in p..p + 1024 {
        if (at == p || at & 0xfff == 0) && !wal_payload_common::crash::readable(at, 1) {
            return None;
        }
        match unsafe { *(at as *const u8) } {
            0 => return Some(bytes),
            b => bytes.push(b),
        }
    }
    Some(bytes)
}

/// Where the game's file check continues (its first instructions, then a jump back).
static ROM_CHECK: AtomicUsize = AtomicUsize::new(0);

type RomCheck = unsafe extern "C" fn(*const c_char, *const c_char, *const c_char) -> i32;

/// One file of the boot's ROM check (`kind` CRC32/MD5/..., the expected sum, the path): 1 when
/// it matches. libacio.dll is the payload: passed (5-1515-0000 "file broken" otherwise).
unsafe extern "C" fn rom_check(kind: *const c_char, expected: *const c_char, path: *const c_char) -> i32 {
    if !path.is_null() {
        let p = unsafe { std::ffi::CStr::from_ptr(path) }.to_bytes().to_ascii_lowercase();
        if p.ends_with(b"\\libacio.dll") || p.ends_with(b"/libacio.dll") {
            log!("winning eleven: ROM check of libacio.dll (the payload) passed");
            return 1;
        }
    }
    let original: RomCheck = unsafe { std::mem::transmute(ROM_CHECK.load(Ordering::Relaxed)) };
    unsafe { original(kind, expected, path) }
}

/// The boot's ROM check (romcheck.bin: every file of the game and its sum) runs each file
/// through rva 0x17b40: it is entered through `rom_check`.
fn rom_check_init() {
    // sub esp, 0x48; mov eax, [0xf6dad0]
    let head = [0x83, 0xec, 0x48, 0xa1, 0xd0, 0xda, 0xf6, 0x00];
    match unsafe { detour(0x17b40, &head, rom_check as *const () as usize) } {
        Some(t) => ROM_CHECK.store(t, Ordering::Relaxed),
        None => log!("winning eleven: ROM check code differs, libacio.dll not passed"),
    }
}

/// Where the test-mode data load continues.
static SETTINGS_LOAD: AtomicUsize = AtomicUsize::new(0);

/// The network settings (test menu NETWORK OPTIONS) in the test-mode data: e-AMUSEMENT, then
/// LIVE MONITOR CONNECT.
const NETWORK_SETTINGS: usize = 0x883088;

/// Loads the test-mode data (D:/HCK/save/weags_config_data.bin, else the factory settings),
/// then turns e-AMUSEMENT and LIVE MONITOR CONNECT off unless the profile keeps them
/// (`WAL_KONAMI_EAMUSEMENT=1`, `WAL_KONAMI_LIVE_MONITOR=1`). On: the boot stops on CANNOT
/// COMMUNICATE WITH THE CENTER SERVER 5-2002-0000 (SERVICE continues), then on LIVE MONITOR NOT
/// FOUND 5-2505-0000 (the spectator screen PC of a shop's cabinets).
unsafe extern "C" fn settings_load() -> i32 {
    let original: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(SETTINGS_LOAD.load(Ordering::Relaxed)) };
    let ret = unsafe { original() };
    let network = (unsafe { GetModuleHandleW(std::ptr::null()) } as usize + NETWORK_SETTINGS) as *mut u32;
    for (i, (var, name)) in [("WAL_KONAMI_EAMUSEMENT", "e-AMUSEMENT"), ("WAL_KONAMI_LIVE_MONITOR", "LIVE MONITOR CONNECT")].iter().enumerate() {
        if !std::env::var(var).is_ok_and(|v| v.trim() == "1") {
            unsafe { *network.add(i) = 0 };
            log!("winning eleven: {name} off");
        }
    }
    ret
}

fn settings_init() {
    // sub esp, 0x8c; mov eax, [0xf6dad0]
    let head = [0x81, 0xec, 0x8c, 0x00, 0x00, 0x00, 0xa1, 0xd0, 0xda, 0xf6, 0x00];
    match unsafe { detour(0x13400, &head, settings_load as *const () as usize) } {
        Some(t) => SETTINGS_LOAD.store(t, Ordering::Relaxed),
        None => log!("winning eleven: test-mode data load code differs, network settings kept"),
    }
}

/// Where the network notice setter continues.
static NOTICE_SET: AtomicUsize = AtomicUsize::new(0);

/// The network notice shown over the attract (`id`: a row of the table at 0xc898c0, title and
/// message; 0 none, drawn by rva 0x28880 while it is not 0). Id 1, "operating in local mode,
/// e-AMUSEMENT PASS and e-AMUSEMENT service unavailable", is always the case here: none.
unsafe extern "C" fn notice_set(id: i32) -> i32 {
    let original: unsafe extern "C" fn(i32) -> i32 = unsafe { std::mem::transmute(NOTICE_SET.load(Ordering::Relaxed)) };
    unsafe { original(if id == 1 { 0 } else { id }) }
}

fn notice_init() {
    // sub esp, 0xc; push ebx; push ebp; push esi
    let head = [0x83, 0xec, 0x0c, 0x53, 0x55, 0x56];
    match unsafe { detour(0x28770, &head, notice_set as *const () as usize) } {
        Some(t) => NOTICE_SET.store(t, Ordering::Relaxed),
        None => log!("winning eleven: network notice code differs, the local mode notice stays"),
    }
}

/// The controller choice before a match (ControllerSelect scene, rva 0x467d0 each frame): touch
/// buttons, the arcade panel (0) and the home controller (1); a button touched twice is taken.
/// The touch hit test (rva 0x46670: the button under the finger, 2 none) answers the arcade
/// panel: taken in two frames, as if touched.
fn controller_select_init() {
    let at = unsafe { GetModuleHandleW(std::ptr::null()) } as usize + 0x467f6;
    // call 0x446670 -> xor eax, eax
    if unsafe { std::slice::from_raw_parts(at as *const u8, 5) } != [0xe8, 0x75, 0xfe, 0xff, 0xff] {
        log!("winning eleven: controller choice code differs, chosen by touch or timer");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, 5, PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping([0x33, 0xc0, 0x90, 0x90, 0x90].as_ptr(), at as *mut u8, 5);
        VirtualProtect(at as *const c_void, 5, old, &mut old);
    }
}

/// Where the sound effects setup continues.
static SOUND_EFFECTS_SETUP: AtomicUsize = AtomicUsize::new(0);

/// The end of the sound effects setup (rva 0x57df30: the performance's port, default audio path,
/// reverb). Wine's `InitAudio` creates the default audio path inactive, which deactivates the
/// synthesizer's port: every note is dropped ("Failed to update performance latency"). DirectX's
/// is active: activated here.
unsafe extern "C" fn sound_effects_started() -> u32 {
    let original: unsafe extern "C" fn() -> u32 = unsafe { std::mem::transmute(SOUND_EFFECTS_SETUP.load(Ordering::Relaxed)) };
    let ret = unsafe { original() };
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let on = unsafe { *((base + 0x44cbd40) as *const u32) };
    // IDirectMusicAudioPath*
    let path = unsafe { *((base + 0x44cbd38) as *const *mut *const usize) };
    if on != 0 && !path.is_null() {
        // IDirectMusicAudioPath::Activate(TRUE)
        let activate: unsafe extern "system" fn(*mut *const usize, i32) -> i32 = unsafe { std::mem::transmute(*(*path).add(4)) };
        let hr = unsafe { activate(path, 1) };
        log!("winning eleven: sound effects on, default audio path activated ({hr:#x})");
    } else {
        log!("winning eleven: sound effects off");
    }
    ret
}

/// The match's sound effects (players, ball, referee) are DirectMusic (the `text/sound/internal`
/// banks): after the performance starts, rva 0x57d1a0 sets its audio path's reverb (a DMO effect
/// of the path's buffer) and turns the sound effects off when that fails, as with Wine's
/// DirectMusic (no DMO effects). Without the reverb they stay on.
fn sound_effects_init() {
    // mov eax, [0x48cbd40]
    match unsafe { detour(0x57df30, &[0xa1, 0x40, 0xbd, 0x8c, 0x04], sound_effects_started as *const () as usize) } {
        Some(t) => SOUND_EFFECTS_SETUP.store(t, Ordering::Relaxed),
        None => log!("winning eleven: sound effects setup code differs, their audio path kept"),
    }
    let at = unsafe { GetModuleHandleW(std::ptr::null()) } as usize + 0x57d20f;
    // mov dword ptr [0x48cbd40], 0 -> jmp over it
    let off = [0xc7, 0x05, 0x40, 0xbd, 0x8c, 0x04, 0x00, 0x00, 0x00, 0x00];
    if unsafe { std::slice::from_raw_parts(at as *const u8, off.len()) } != off {
        log!("winning eleven: reverb setup code differs, sound effects off without it");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, off.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping([0xeb, 0x08].as_ptr(), at as *mut u8, 2);
        VirtualProtect(at as *const c_void, off.len(), old, &mut old);
    }
}

/// Enters the executable's function at `rva` through `target` when it starts with `head`
/// (whole instructions, none relative): returns the trampoline that runs them and continues.
unsafe fn detour(rva: usize, head: &[u8], target: usize) -> Option<usize> {
    let at = unsafe { GetModuleHandleW(std::ptr::null()) } as usize + rva;
    if unsafe { std::slice::from_raw_parts(at as *const u8, head.len()) } != head {
        return None;
    }
    let tramp = unsafe { VirtualAlloc(std::ptr::null(), 32, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) } as *mut u8;
    if tramp.is_null() {
        return None;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(head.as_ptr(), tramp, head.len());
        *tramp.add(head.len()) = 0xe9;
        let back = (at + head.len()).wrapping_sub(tramp as usize + head.len() + 5) as i32;
        std::ptr::write_unaligned(tramp.add(head.len() + 1) as *mut i32, back);
    }
    let rel = target.wrapping_sub(at + 5) as i32;
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, 5, PAGE_EXECUTE_READWRITE, &mut old);
        *(at as *mut u8) = 0xe9;
        std::ptr::write_unaligned((at + 1) as *mut i32, rel);
        VirtualProtect(at as *const c_void, 5, old, &mut old);
    }
    Some(tramp as usize)
}

/// The speaker equalizer (mode 0 or 1, from the cabinet type): set.
extern "C" fn fck_set_eq(_mode: i32) -> i32 {
    0
}

