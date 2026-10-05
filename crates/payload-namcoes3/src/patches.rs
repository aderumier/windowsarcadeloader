//! Code patches of Star Wars Battle Pod (TFA), applied only when the original bytes match.
//!
//! `RSLauncher.exe` (the cabinet launcher, starts the game):
//! - the projector monitor thread opens COM1 and stops everything with error 0x31 "23-08
//!   PROJECTOR OTHER ERROR" when no projector answers: the thread returns at once
//! - `WAL_NAMCOES3_FLATSCREEN` (default 1): the game is started with `-FLATSCREEN` (no dome
//!   projection warp) whatever the cabinet type setting
//! - `WAL_NAMCOES3_LANGUAGE`: `ENG` (or `INT`) `JPN CHN ITA SPA RUS POR IND THA` forces the
//!   game language (default: the operator menu setting)
//!
//! `SWArcGame-Win64-Shipping.exe`: the USB key check enumerates USB devices (SetupDi) for one
//! key with the cabinet serial; the enumeration answers that key.

use std::ffi::c_void;

use wal_payload_common::log;
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

fn write(at: usize, bytes: &[u8]) {
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, bytes.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
        VirtualProtect(at as *const c_void, bytes.len(), old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), at as *const c_void, bytes.len());
    }
}

/// Writes `new` at `base + rva` when the current bytes are `old`.
fn patch(what: &str, base: usize, rva: usize, old: &[u8], new: &[u8]) -> bool {
    let cur = unsafe { std::slice::from_raw_parts((base + rva) as *const u8, old.len()) };
    if cur != old {
        log!("patch {what} at {rva:#x}: other bytes, not applied");
        return false;
    }
    write(base + rva, new);
    log!("patch {what} at {rva:#x}");
    true
}

pub(crate) fn launcher(base: usize) {
    patch("projector thread", base, 0x3e4d0, b"\x40\x53\x55\x56", b"\x31\xC0\xC3\x90");
    // cabinet type / 6: 1 -> "-FLATSCREEN", 2 -> "-PREMIUM", else dome
    if std::env::var("WAL_NAMCOES3_FLATSCREEN").map_or(true, |v| v != "0") {
        patch("flat screen", base, 0x32475, b"\x75\x09", b"\x90\x90");
    }
    // [0x33cb60] = operator language 1..8 -> "-Language=JPN..THA", else English: replaced by
    // `mov eax, <language>`
    const LANGUAGES: [&str; 9] = ["ENG", "JPN", "CHN", "ITA", "SPA", "RUS", "POR", "IND", "THA"];
    if let Ok(lang) = std::env::var("WAL_NAMCOES3_LANGUAGE") {
        let wanted = if lang.trim().eq_ignore_ascii_case("INT") { "ENG" } else { lang.trim() };
        match LANGUAGES.iter().position(|l| l.eq_ignore_ascii_case(wanted)) {
            Some(i) => {
                if patch("language", base, 0x324ef, b"\x8b\x05\x6b\xa6\x30\x00", &[0xB8, i as u8, 0, 0, 0, 0x90]) {
                    log!("game language {}", LANGUAGES[i]);
                }
            }
            None => log!("WAL_NAMCOES3_LANGUAGE={lang}: unknown language, operator setting kept"),
        }
    }
}

/// A serial template of the game, decrypted at run time ("27431022****" for the file's
/// "******22****"): the serial with '*' as '0'.
pub(crate) fn serial(template_rva: usize) -> [u8; 12] {
    let template = crate::game_base() + template_rva;
    let mut serial = [0u8; 12];
    for (i, c) in serial.iter_mut().enumerate() {
        let b = unsafe { *((template + i) as *const u8) };
        *c = if b == b'*' || b == 0 { b'0' } else { b };
    }
    serial
}

/// USB key enumeration (`0x20b0`): one device, id at +0x22, 12-wchar serial at +0x428.
/// Japanese (`[0x150d810]` set): the "USB DONGLE", id 0x0c10, template 0x12b4c98. Other
/// languages (`[0x2022254] != 1`): the game checks the HASP, then the "TFA KEY", id 0x0c20,
/// template 0x1573d48 (else error 0x39 "20-01"). Otherwise id 0x0c10, template 0x1573d28.
unsafe extern "C" fn usb_key_enum(_a: i32, _b: i32, _c: u16, _d: u16, out: *mut u8) -> i32 {
    let base = crate::game_base();
    let read = |rva: usize| unsafe { *((base + rva) as *const u32) };
    let tfa = read(0x2022254) != 1 && read(0x150d810) == 0;
    let (id, template) = match (tfa, read(0x150d810) != 0) {
        (true, _) => (0x0c20u16, 0x1573d48),
        (false, true) => (0x0c10, 0x12b4c98),
        (false, false) => (0x0c10, 0x1573d28),
    };
    let serial = serial(template);
    unsafe {
        std::ptr::write_bytes(out, 0, 0x628);
        *(out.add(0x22) as *mut u16) = id;
        let wide = out.add(0x428) as *mut u16;
        for (i, b) in serial.iter().enumerate() {
            *wide.add(i) = *b as u16;
        }
    }
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| {
        let kind = if tfa { " (TFA key)" } else { "" };
        log!("usb key: 1 device{kind}, serial {}", String::from_utf8_lossy(&serial))
    });
    1
}

pub(crate) fn game(base: usize) {
    // mov rax, usb_key_enum; jmp rax
    let mut jump = [0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xE0];
    jump[2..10].copy_from_slice(&(usb_key_enum as *const () as usize as u64).to_le_bytes());
    patch("usb key enumeration", base, 0x20b0, b"\x40\x55\x57\x41\x55\x41\x56\x41\x57\x48\x8d\xac", &jump);
}
