//! Steel Chronicle Victroopers (game code KGG): the game is `arkkgg.dll` (exports
//! `dll_entry_init` / `dll_entry_main`), run by `wal-avs-boot.exe`; its I/O library
//! `lib_acio.dll` drives `libacio.dll`, the payload (the HBHI board, the card reader). The game's
//! code proper is `gamekgg.dll` (Direct3D 9, the window, DirectInput), which arkkgg.dll loads
//! later: the common hooks start then, on it. spice2x (the open-source reference) has the board's
//! layout.
//!
//! Two flight sticks, each with a trigger button (L / R) and two buttons (L1 L2 / R1 R2), and a
//! jog (left / right). The HBHI packet: byte 1 test 0x20, service 0x10; byte 8 L 0x01, L1 0x02,
//! L2 0x04, jog left 0x08, jog right 0x10, R 0x20, R1 0x40, R2 0x80; analog channels 0-3 the
//! left stick's x (inverted: a flight stick) and y, the right stick's x (inverted) and y. Coins
//! are the board's coin stock.
//!
//! Touch screen: libtouchpanel3m.dll reads `\\.\TwTouchDriver` (24-byte reports): player 1's
//! `start` touches at the pointer `accel` / `brake` (the profile's mouse or gun).
//!
//! arkkgg.dll also opens an LCD monitor on COM1 (Winning Eleven 2012's) and looks up kbt.dll /
//! kld.dll (`kbt.rs`).
//!
//! Native inputs for `native_map`: `test service coin l l1 l2 r r1 r2 jog_left jog_right`.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log};
use wal_protocol::Axis;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

static ACTIVE: AtomicBool = AtomicBool::new(false);

fn module(name: &str) -> usize {
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    unsafe { GetModuleHandleW(name.as_ptr()) as usize }
}

/// arkkgg.dll's base, when this process runs it (it imports lib_acio.dll, which imports this
/// DLL: it is loaded first).
pub fn ark_module() -> usize {
    module("arkkgg.dll")
}

pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    /// The packet's byte 8 bit.
    Button(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("l", Native::Button(0)),
    ("l1", Native::Button(1)),
    ("l2", Native::Button(2)),
    ("jog_left", Native::Button(3)),
    ("jog_right", Native::Button(4)),
    ("r", Native::Button(5)),
    ("r1", Native::Button(6)),
    ("r2", Native::Button(7)),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("b1", "l"),
    ("b2", "l1"),
    ("b3", "l2"),
    ("b4", "r"),
    ("b5", "r1"),
    ("b6", "r2"),
    ("b7", "jog_left"),
    ("b8", "jog_right"),
];

struct Board {
    map: ButtonMap<Native>,
    coin_held: bool,
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);

/// The HBHI status packet.
pub(crate) fn packet() -> [u8; 15] {
    let mut guard = BOARD.lock().unwrap();
    let board = guard.get_or_insert_with(|| Board { map: ButtonMap::new(NATIVES, DEFAULT_MAP), coin_held: false });
    let stick = wal_payload_common::input(0);
    let mut raw = [0u8; 15];
    let mut coin = false;
    board.map.for_each_pressed(&stick, |n| match n {
        Native::Test => raw[1] |= 0x20,
        Native::Service => raw[1] |= 0x10,
        Native::Coin => coin = true,
        Native::Button(bit) => raw[8] |= 1 << bit,
    });
    if coin && !board.coin_held {
        crate::hbhi::coin_inserted();
    }
    board.coin_held = coin;
    // analog channels 0-3 (10 bits: high byte, then the 2 low bits on top of the next byte)
    let full = |v: i16| (v as i32 + 0x8000) as u16;
    let channels = [
        0xffff - full(stick.axis(Axis::LeftX)),
        full(stick.axis(Axis::LeftY)),
        0xffff - full(stick.axis(Axis::RightX)),
        full(stick.axis(Axis::RightY)),
    ];
    for (ch, v) in channels.into_iter().enumerate() {
        raw[ch * 2] = (v >> 8) as u8;
        raw[ch * 2 + 1] |= (v & 0xc0) as u8;
    }
    raw
}

/// `setvolume` (setvolume.dll): the cabinet's amplifier volume, not set (spice2x: it crashes on
/// some PCs).
unsafe extern "C" fn setvolume(_file: *const u8) -> i32 {
    0
}

type LoadLibraryW = unsafe extern "system" fn(*const u16) -> usize;
type LoadLibraryA = unsafe extern "system" fn(*const u8) -> usize;
static ORIG_LOAD_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_LOAD_A: AtomicUsize = AtomicUsize::new(0);

/// gamekgg.dll loaded: the common hooks start on it.
fn loaded(name: &str, base: usize) {
    if base != 0 && name.rsplit(['\\', '/']).next().unwrap_or(name).eq_ignore_ascii_case("gamekgg.dll") && iat::game_module() != base {
        iat::set_game_module(base);
        wal_payload_common::start("konami");
        match unsafe { iat::hook("gdiplus.dll", "GdipPrivateAddMemoryFont", add_memory_font as *const () as usize) } {
            Some(o) => ORIG_ADD_FONT.store(o, Ordering::Relaxed),
            None => log!("sc: GdipPrivateAddMemoryFont import not found"),
        }
        // the touch pointer shown (WAL_KONAMI_CROSSHAIRS)
        crate::crosshair::init();
        let touch = module("libtouchpanel3m.dll");
        if touch != 0 {
            wal_payload_common::serial::install_streaming_in("TwTouchDriver", touch_write, touch_report, &[touch]);
        } else {
            log!("sc: libtouchpanel3m.dll not loaded, no touch screen");
        }
        log!("konami: HBHI board, touch screen, card reader and LCD emulated (Steel Chronicle Victroopers)");
    }
}

unsafe extern "system" fn load_library_w(name: *const u16) -> usize {
    let original: LoadLibraryW = unsafe { std::mem::transmute(ORIG_LOAD_W.load(Ordering::Relaxed)) };
    let base = unsafe { original(name) };
    if !name.is_null() {
        let len = (0..).take_while(|&i| unsafe { *name.add(i) } != 0).count();
        loaded(&String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) }), base);
    }
    base
}

unsafe extern "system" fn load_library_a(name: *const u8) -> usize {
    let original: LoadLibraryA = unsafe { std::mem::transmute(ORIG_LOAD_A.load(Ordering::Relaxed)) };
    let base = unsafe { original(name) };
    if !name.is_null() {
        loaded(&unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy(), base);
    }
    base
}

/// The touch driver takes no command.
fn touch_write(_packet: &[u8]) -> Vec<u8> {
    Vec::new()
}

static TOUCHING: AtomicBool = AtomicBool::new(false);
/// A report was just given: the next read is empty (the library reads until nothing is left,
/// at most 100 reports at a time, more is an error).
static GIVEN: AtomicBool = AtomicBool::new(false);

/// The touch driver's next report (24 bytes: type 5, status at 12, 1 touching / 3 released,
/// x and y at 16 and 18), spice2x's format and factory calibration (offsets, scale, both axes
/// flipped). The touch: player 1's `start` (the profile's mouse or gun trigger) at the pointer
/// `accel` (x) and `brake` (y), 0..32767 across the screen.
fn touch_report() -> Vec<u8> {
    if GIVEN.swap(false, Ordering::Relaxed) {
        return Vec::new();
    }
    let stick = wal_payload_common::input(0);
    let down = stick.pressed(wal_protocol::button::START);
    let was = TOUCHING.swap(down, Ordering::Relaxed);
    let status: u32 = match (down, was) {
        (true, _) => 1,
        (false, true) => 3,
        (false, false) => return Vec::new(),
    };
    let (mut x, mut y) = (0u16, 0u16);
    if down {
        let px = stick.axis(Axis::Accel).max(0) as f32 / 32767.0 * 1920.0;
        let py = stick.axis(Axis::Brake).max(0) as f32 / 32767.0 * 1080.0;
        x = 0xffff - (-1500.0 + px * (65535.0 + 2500.0) / 1920.0).clamp(0.0, 65535.0) as u16;
        y = 0xffff - (3100.0 + py * (65535.0 - 5800.0) / 1080.0).clamp(0.0, 65535.0) as u16;
    }
    let mut report = vec![0u8; 24];
    report[..4].copy_from_slice(&5u32.to_le_bytes());
    report[12..16].copy_from_slice(&status.to_le_bytes());
    report[16..18].copy_from_slice(&x.to_le_bytes());
    report[18..20].copy_from_slice(&y.to_le_bytes());
    GIVEN.store(true, Ordering::Relaxed);
    report
}

type AddMemoryFont = unsafe extern "system" fn(*mut c_void, *const u8, i32) -> i32;
static ORIG_ADD_FONT: AtomicUsize = AtomicUsize::new(0);

/// `GdipPrivateAddMemoryFont`: the game's fonts are TrueType collections (`.ttc`, three faces
/// each), which Wine's GDI+ does not parse (no family added: the game then reads an
/// uninitialized family count and crashes). Each face is added on its own: a copy of the
/// collection with that face's table directory moved to the start (its table offsets count from
/// the file's start, so the copy is a valid TrueType font of that face).
unsafe extern "system" fn add_memory_font(collection: *mut c_void, memory: *const u8, length: i32) -> i32 {
    let original: AddMemoryFont = unsafe { std::mem::transmute(ORIG_ADD_FONT.load(Ordering::Relaxed)) };
    if memory.is_null() || length < 16 {
        return unsafe { original(collection, memory, length) };
    }
    let data = unsafe { std::slice::from_raw_parts(memory, length as usize) };
    if &data[..4] != b"ttcf" {
        return unsafe { original(collection, memory, length) };
    }
    let be32 = |o: usize| u32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]) as usize;
    let faces = be32(8);
    let mut status = 0;
    for i in 0..faces {
        let Some(dir) = data.get(12 + i * 4..16 + i * 4).map(|_| be32(12 + i * 4)) else { break };
        if dir + 12 > data.len() {
            break;
        }
        let tables = u16::from_be_bytes([data[dir + 4], data[dir + 5]]) as usize;
        let size = 12 + 16 * tables;
        if dir + size > data.len() {
            break;
        }
        // kept for the collection's lifetime (GDI+ keeps a pointer to the font's memory)
        let mut font = data.to_vec();
        font.copy_within(dir..dir + size, 0);
        let font = font.leak();
        status = unsafe { original(collection, font.as_ptr(), font.len() as i32) };
    }
    log!("sc: font collection of {faces} faces added face by face (status {status})");
    status
}

/// CRC-32 (IEEE), the checksum of `testmode-v.crc`.
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// Sets `<name>...<current __type="s32">value</current>` in the test menu settings.
fn set_current(xml: &mut String, name: &str, value: i32) -> bool {
    let open = format!("<{name}>");
    let Some(start) = xml.find(&open) else { return false };
    let Some(end) = xml[start..].find(&format!("</{name}>")).map(|e| start + e) else { return false };
    let tag = "<current __type=\"s32\">";
    let Some(cur) = xml[start..end].find(tag).map(|c| start + c + tag.len()) else { return false };
    let Some(stop) = xml[cur..end].find('<').map(|e| cur + e) else { return false };
    xml.replace_range(cur..stop, &value.to_string());
    true
}

/// The test menu settings (`dev/nvram/testmode-v.xml` and its CRC-32): the touch panel and the
/// sticks marked calibrated (the factory calibration already spans the payload's full ranges;
/// unset, the boot stops on 5-2506-0000 "calibration not set"), the clock set, free play from
/// `WAL_KONAMI_FREEPLAY` (`1`: on, `0`: off, unset: the dump's).
fn settings() {
    const XML: &str = "dev/nvram/testmode-v.xml";
    const CRC: &str = "dev/nvram/testmode-v.crc";
    let Ok(text) = std::fs::read(XML) else {
        log!("sc: no {XML}, settings kept");
        return;
    };
    // Shift-JIS, but the tags are ASCII: edited as Latin-1
    let mut xml: String = text.iter().map(|&b| b as char).collect();
    for name in ["touchpanelCheck", "analogstickCheck"] {
        let Some(at) = xml.find(&format!("<{name}>")) else { continue };
        let mut rest = xml[at..].to_string();
        set_current(&mut rest, "calibrated", 1);
        xml.replace_range(at.., &rest);
    }
    // the operator clock set (the PC's clock, offset 0; unset: 5-2503-0000 "set the clock")
    xml = xml.replace("<is_set __type=\"s32\">0</is_set>", "<is_set __type=\"s32\">1</is_set>");
    match std::env::var("WAL_KONAMI_FREEPLAY").ok().as_deref().map(str::trim) {
        Some("1") => _ = set_current(&mut xml, "freeplay", 1),
        Some("0") => _ = set_current(&mut xml, "freeplay", 0),
        _ => {}
    }
    let bytes: Vec<u8> = xml.chars().map(|c| c as u8).collect();
    if bytes == text {
        return;
    }
    let crc = crc32(&bytes);
    if std::fs::write(XML, &bytes).is_ok() && std::fs::write(CRC, crc.to_le_bytes()).is_ok() {
        log!("sc: test menu settings set (calibrated, free play {:?})", std::env::var("WAL_KONAMI_FREEPLAY").ok());
    }
}

/// arkkgg.dll's hooks, from this DLL's `DllMain` (before the common payload starts).
pub fn init(payload: *mut c_void) {
    ACTIVE.store(true, Ordering::Relaxed);
    // AVS's /dev/raw (the game's log, the share cache): not in the dump
    if let Err(e) = std::fs::create_dir_all("dev/raw") {
        log!("sc: dev/raw: {e}");
    }
    settings();
    let ark = ark_module();
    // kbt.rs hooks the game module's imports: arkkgg.dll's, until gamekgg.dll is loaded
    iat::set_game_module(ark);
    crate::kbt::init(payload);
    for (name, f, orig) in [
        ("LoadLibraryW", load_library_w as *const () as usize, &ORIG_LOAD_W),
        ("LoadLibraryA", load_library_a as *const () as usize, &ORIG_LOAD_A),
    ] {
        if let Some(o) = unsafe { iat::hook_module(ark, "kernel32.dll", name, f) } {
            orig.store(o, Ordering::Relaxed);
        }
    }
    if unsafe { iat::hook_module(ark, "setvolume.dll", "?setvolume@@YAHPAD@Z", setvolume as *const () as usize) }.is_none() {
        log!("sc: setvolume import not found");
    }
    wal_payload_common::serial::install_in("COM1", crate::we2012::lcd, &[ark]);
}
