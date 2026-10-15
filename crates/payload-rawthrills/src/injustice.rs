//! Injustice Arcade (Raw Thrills / NetherRealm, UDK build 1.74, 2022),
//! `WAL_RAWTHRILLS_GAME=injustice`.
//!
//! A two-player fighter played with four buttons per player (no joystick). The cabinet I/O
//! the game opens itself:
//! * the RIO USB board through `libusb0` (bulk transfers): coins, service, test, volume and
//!   the players' buttons, including the card dispensers' "vend" buttons;
//! * the Sentinel HASP dongle, its API linked into the game;
//! * the NTI smart card reader through WinSCard (absent: free play, handled by the game);
//! * a webcam per player through OpenCV, its frames decoded by ZBar: the character cards are
//!   barcodes shown to the camera.

use std::ffi::c_void;
use std::sync::Mutex;

use wal_payload_common::{iat, log};

use crate::common::{Site, matches, redirect};

/// RIO switch ids (`URawGameViewportClient::Create`'s button table).
const NATIVES: &[(&str, u8)] = &[
    ("coin1", 0x00),
    ("coin2", 0x01),
    ("bill", 0x02),
    ("test", 0x03),
    ("service", 0x04),
    ("volup", 0x05),
    ("voldown", 0x06),
    ("start1", 0x07),
    ("start2", 0x08),
    ("block2", 0x09),
    ("heavy2", 0x0a),
    ("special2", 0x0b),
    ("normal2", 0x0c),
    ("normal1", 0x0d),
    ("block1", 0x0e),
    ("heavy1", 0x0f),
    ("special1", 0x10),
    ("vend1", 0x11),
    ("vend2", 0x12),
];

const P1_MAP: &[(&str, &str)] = &[
    ("coin", "coin1"),
    ("start", "start1"),
    ("service", "service"),
    ("test", "test"),
    ("b1", "normal1"),
    ("b2", "heavy1"),
    ("b3", "block1"),
    ("b4", "special1"),
    ("b5", "vend1"),
];

const P2_MAP: &[(&str, &str)] = &[
    ("coin", "coin2"),
    ("start", "start2"),
    ("b1", "normal2"),
    ("b2", "heavy2"),
    ("b3", "block2"),
    ("b4", "special2"),
    ("b5", "vend2"),
];

pub fn init(game: &str) -> bool {
    if game != "injustice" {
        return false;
    }
    crate::rio::enable_status_reports();
    crate::rio::init("injustice", NATIVES, P1_MAP, P2_MAP, true, || 0.0);
    hasp_init();
    dispensers_init();
    crate::injustice_cards::init();
    for (name, f) in [
        ("SCardEstablishContext", scard_establish_context as *const () as usize),
        ("SCardListReadersW", scard_list_readers_w as *const () as usize),
    ] {
        unsafe { iat::hook("WinSCard.dll", name, f) };
    }
    true
}

// ── Card dispensers ──────────────────────────────────────────────────────────────────────────
//
// Each player's card dispenser (a CD-1300 on the RIO) reports ready / empty / low / jam
// sensor lines; without them the dispensers end in an "unknown error" state. The sensor
// reads (UCardDispenser::Is*SignalSet) answer a ready, full dispenser.

const DISPENSER_SIGNALS: [(usize, bool); 4] = [
    (0x1a30840, true),  // ready to vend
    (0x1a30880, false), // empty
    (0x1a30900, false), // low
    (0x1a308c0, false), // jam
];

fn dispensers_init() {
    const ENTRY: &[u8] = &[0x8b, 0x41, 0x7c, 0x85, 0xc0, 0x74];
    if !DISPENSER_SIGNALS.iter().all(|(at, _)| matches((*at, ENTRY))) {
        log!("injustice: unexpected card dispenser code, left alone");
        return;
    }
    for (at, set) in DISPENSER_SIGNALS {
        // mov eax, 1 / xor eax, eax; ret
        crate::common::write(at, if set { &[0xb8, 0x01, 0x00, 0x00, 0x00, 0xc3] } else { &[0x31, 0xc0, 0xc3] });
    }
}

// ── WinSCard: the NTI reader ─────────────────────────────────────────────────────────────────
//
// The prefix has no smart card service: a context is established and no reader is found,
// which the game handles ("SmartCard init failed", free play). WinSCard is WINAPI (stdcall).

const SCARD_E_NO_READERS_AVAILABLE: i32 = 0x8010002eu32 as i32;

unsafe extern "system" fn scard_establish_context(_scope: u32, _r1: *mut c_void, _r2: *mut c_void, context: *mut u32) -> i32 {
    if !context.is_null() {
        unsafe { context.write(1) };
    }
    0
}

unsafe extern "system" fn scard_list_readers_w(_context: u32, _groups: *mut c_void, _readers: *mut c_void, length: *mut u32) -> i32 {
    if !length.is_null() {
        unsafe { length.write(0) };
    }
    SCARD_E_NO_READERS_AVAILABLE
}

// ── Sentinel HASP ────────────────────────────────────────────────────────────────────────────
//
// The frontend logs in periodically (0x183b930 -> 0x183b530) and, after 6 failed logins, spins
// forever on purpose (`jmp $` at 0x183b990: the window stops responding). Login, logout, read,
// write, encrypt and decrypt (stdcall) answer from a dongle memory kept in
// `WindowsLoader\dongle.bin`; the crypt functions leave the data as is.
//
// The dongle holds the number of cards the operator may still dispense (dword at 0x30, one
// less per card): without any, no game starts ("authorize more cards"). It is refilled at
// each start.

/// login(feature, vendor code, &handle), logout(handle), read/write(handle, file, offset,
/// length, buffer), encrypt/decrypt(handle, buffer, length).
const HASP: [Site; 6] = [
    (0x2178e21, &[0x81, 0xec, 0xb4, 0x01, 0x00, 0x00, 0x74]),
    (0x216ab73, &[0x51, 0x56, 0xc7, 0x44, 0x24, 0x04, 0x00]),
    (0x20a9788, &[0x51, 0x57, 0x8b, 0x7c, 0x24, 0x1c, 0x85]),
    (0x21847b0, &[0x51, 0x57, 0x8b, 0x7c, 0x24, 0x1c, 0x85]),
    (0x213422e, &[0x51, 0x53, 0x8b, 0x5c, 0x24, 0x10, 0x85]),
    (0x213dc5b, &[0x51, 0x53, 0x8b, 0x5c, 0x24, 0x10, 0x85]),
];

const DONGLE_SIZE: usize = 4096;
const CARDS_AUTHORIZED: usize = 0x30;
const CARDS_REFILL: u32 = 10_000;
static DONGLE: Mutex<Vec<u8>> = Mutex::new(Vec::new());

fn dongle_path() -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent().map(|d| d.join("WindowsLoader").join("dongle.bin")).unwrap_or_default()
}

fn hasp_init() {
    if !HASP.iter().all(|s| matches(*s)) {
        log!("injustice: unexpected dongle API code, left alone");
        return;
    }
    let mut memory = std::fs::read(dongle_path()).unwrap_or_default();
    memory.resize(DONGLE_SIZE, 0);
    let left = u32::from_le_bytes(memory[CARDS_AUTHORIZED..CARDS_AUTHORIZED + 4].try_into().unwrap());
    if left < 100 || left > CARDS_REFILL {
        memory[CARDS_AUTHORIZED..CARDS_AUTHORIZED + 4].copy_from_slice(&CARDS_REFILL.to_le_bytes());
    }
    *DONGLE.lock().unwrap() = memory;
    let fakes = [
        hasp_login as *const () as usize,
        hasp_logout as *const () as usize,
        hasp_read as *const () as usize,
        hasp_write as *const () as usize,
        hasp_crypt as *const () as usize,
        hasp_crypt as *const () as usize,
    ];
    for ((at, _), fake) in HASP.iter().zip(fakes) {
        redirect(*at, fake);
    }
    log!("injustice: dongle emulated ({})", dongle_path().display());
}

unsafe extern "system" fn hasp_login(_feature: u32, _vendor: usize, handle: *mut u32) -> u32 {
    if !handle.is_null() {
        unsafe { handle.write(1) };
    }
    0
}

unsafe extern "system" fn hasp_logout(_handle: u32) -> u32 {
    0
}

unsafe extern "system" fn hasp_read(_handle: u32, _file: u32, offset: u32, length: u32, buffer: *mut u8) -> u32 {
    let memory = DONGLE.lock().unwrap();
    for i in 0..length as usize {
        let v = memory.get(offset as usize + i).copied().unwrap_or(0);
        unsafe { buffer.add(i).write(v) };
    }
    0
}

unsafe extern "system" fn hasp_write(_handle: u32, _file: u32, offset: u32, length: u32, buffer: *const u8) -> u32 {
    let mut memory = DONGLE.lock().unwrap();
    for i in 0..length as usize {
        if let Some(m) = memory.get_mut(offset as usize + i) {
            *m = unsafe { buffer.add(i).read() };
        }
    }
    let _ = std::fs::write(dongle_path(), &*memory);
    0
}

unsafe extern "system" fn hasp_crypt(_handle: u32, _buffer: *mut u8, _length: u32) -> u32 {
    0
}
