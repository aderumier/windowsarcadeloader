//! Magician's Dead (Taito / Viking, NESiCAxLive 2, 64-bit Unreal Engine 4.12): the cabinet's
//! inputs are three devices, all answered by this module and `leap.rs`:
//! * the I/O board, `MAGIDEIO.dll` (loaded by the game with `LoadLibraryW`, functions found with
//!   `GetProcAddress`): replaced by this DLL (profile `payload_install`, started by wal-loader);
//! * the grip controller (left hand): an XInput pad, the game's `XINPUT1_3.dll` imports
//!   (ordinals 2 `XInputGetState`, 3 `XInputSetState`) hooked;
//! * the Leap Motion over which the right hand casts the spells: `Leap.dll` (delay loaded, the
//!   Leap SDK 3.2 C++ API) is this module too (`leap.rs`).
//!
//! `MAGIDEIO_Update(buffer)`: the game reads +0x00 the board status (0x200 OK, anything else an
//! I/O error), +0x10 and +0x12 the switches (the game's 16 USBIO keys, from bit 0: lever up,
//! down, left, right, switches 1-6, START, DUMMY1, TEST, SERVICE, COIN2, COIN1; +0x12 holds
//! bits 0-7, +0x10 bits 8-15), +0x18 / +0x1A two coin counters (14 bits, the top two an error
//! state), +0x34 the NESiCA reader (0x11: a card, its 16-character id at +0x3C). It writes its
//! lamps from +0x58 and a reader command at +0x74 (bit 0: reading). `MAGIDEIO_GetStatus` 0x200.
//!
//! The grip: the stick (`lx`/`ly`), its lower button (XInput A: jump), its upper button (LB:
//! lock-on, the menus' decide) and the stick's click (L3: centre).
//!
//! The dump's executable is another loader's build with the free play option forced on (its
//! test menu setting, the settings byte +0xEA read at +0x762ACE and set at +0xF029D7): the
//! common free play option writes the value there (`WAL_FREEPLAY`).
//!
//! Natives for `WAL_MAP`: `lup ldown lleft lright sw1-sw6 start test service coin`, the grip's
//! `jump lockon center`, and the hand's gestures (`leap.rs`) `thumb fist lshape point scissors
//! push both`.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, input_filter, log};
use wal_protocol::Axis;
use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GetModuleHandleExW, GetModuleHandleW};

use crate::leap::Gesture;

type P = *mut c_void;

/// The names this DLL is loaded under.
const MODULES: [&str; 2] = ["MAGIDEIO.dll", "Leap.dll"];

pub(crate) fn detected(module: HMODULE) -> bool {
    let mut buf = [0u16; 260];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    path.rsplit(['\\', '/']).next().is_some_and(|name| name.eq_ignore_ascii_case(MODULES[0]))
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Native {
    /// A USBIO switch: its bit in the game's 16-bit word.
    Switch(u8),
    Coin,
    /// The grip's lower button (jump), upper button (lock-on), the stick's click.
    Jump,
    LockOn,
    Center,
    Hand(Gesture),
}

const NATIVES: &[(&str, Native)] = &[
    ("lup", Native::Switch(0)),
    ("ldown", Native::Switch(1)),
    ("lleft", Native::Switch(2)),
    ("lright", Native::Switch(3)),
    ("sw1", Native::Switch(4)),
    ("sw2", Native::Switch(5)),
    ("sw3", Native::Switch(6)),
    ("sw4", Native::Switch(7)),
    ("sw5", Native::Switch(8)),
    ("sw6", Native::Switch(9)),
    ("start", Native::Switch(10)),
    ("test", Native::Switch(12)),
    ("service", Native::Switch(13)),
    ("coin", Native::Coin),
    ("jump", Native::Jump),
    ("lockon", Native::LockOn),
    ("center", Native::Center),
    ("thumb", Native::Hand(Gesture::Thumb)),
    ("fist", Native::Hand(Gesture::Fist)),
    ("lshape", Native::Hand(Gesture::LShape)),
    ("point", Native::Hand(Gesture::Point)),
    ("scissors", Native::Hand(Gesture::Scissors)),
    ("push", Native::Hand(Gesture::Push)),
    ("both", Native::Hand(Gesture::Both)),
];

/// Pad layout (b1-b4 = X Y R1 L1, b5-b8 = A B R2 L2): the grip's lower button (jump) on A, also
/// switch 2 (the first-play guide's "button for the next page"), its upper button (lock-on, the
/// menus' decide) on L1; the hand on the right stick, its gestures R2
/// (thumb bent: the shots), R1 (fist, opened on release), L2 (L shape), X (index finger), Y
/// (scissors), B (thrust).
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("up", "lup"),
    ("down", "ldown"),
    ("left", "lleft"),
    ("right", "lright"),
    ("b5", "jump+sw2"),
    ("b4", "lockon+sw1"),
    ("b7", "thumb"),
    ("b3", "fist"),
    ("b8", "lshape"),
    ("b1", "point"),
    ("b2", "scissors"),
    ("b6", "push"),
    ("start", "start"),
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
];

static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();

/// Calls `f` for every native input player 1 presses.
pub(crate) fn for_each_pressed(mut f: impl FnMut(Native)) {
    if let Some(map) = MAP.get() {
        map.for_each_pressed(&wal_payload_common::input(0), &mut f);
    }
}

pub(crate) fn init() {
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP));
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    for (name, hook, slot) in [
        ("LoadLibraryW", load_library_w as *const () as usize, &LOAD_LIBRARY_W),
        ("LoadLibraryExW", load_library_ex_w as *const () as usize, &LOAD_LIBRARY_EX_W),
        ("LoadLibraryExA", load_library_ex_a as *const () as usize, &LOAD_LIBRARY_EX_A),
        ("LoadLibraryA", load_library_a as *const () as usize, &LOAD_LIBRARY_A),
    ] {
        if let Some(o) = unsafe { iat::hook_module(exe, "kernel32.dll", name, hook) } {
            slot.store(o, Ordering::Relaxed);
        }
    }
    match unsafe { iat::hook_ordinal(exe, "xinput1_3.dll", 2, xinput_get_state as *const () as usize) } {
        Some(_) => {}
        None => log!("magiciansdead: XInputGetState not imported: no grip controller"),
    }
    unsafe { iat::hook_ordinal(exe, "xinput1_3.dll", 3, xinput_set_state as *const () as usize) };
    // the real keyboard and mouse: only the launcher's input
    input_filter::init(exe, |_| false);
    free_play(exe);
    nesica(exe);
    crate::leap::init();
    log!("magiciansdead: I/O board, grip controller and Leap Motion emulated");
}

/// The settings' free play byte as the common option says (the dump's executable forces it on).
fn free_play(exe: usize) {
    let Some(on) = wal_payload_common::free_play("WAL_NESICA_FREEPLAY") else { return };
    // (rva, the dump's bytes, the original's bytes, ours)
    let sites: [(usize, [u8; 7], [u8; 7], [u8; 7]); 2] = [
        // movzx eax, byte [rax+0xEA] -> mov eax, <on>
        (0x762ace, [0xb8, 1, 0, 0, 0, 0x90, 0x90], [0x0f, 0xb6, 0x80, 0xea, 0, 0, 0], [0xb8, on as u8, 0, 0, 0, 0x90, 0x90]),
        // cmp byte [rax+0xEA], 0 -> mov byte [rax+0xEA], <on>
        (0xf029d7, [0xc6, 0x80, 0xea, 0, 0, 0, 1], [0x80, 0xb8, 0xea, 0, 0, 0, 0], [0xc6, 0x80, 0xea, 0, 0, 0, on as u8]),
    ];
    for (rva, dump, original, _) in sites {
        let at = (exe + rva) as *mut [u8; 7];
        let now = unsafe { at.read_unaligned() };
        if now != dump && now != original {
            log!("magiciansdead: unknown executable at +{rva:#x}: free play not set");
            return;
        }
    }
    let spec: Vec<String> = sites.iter().map(|(rva, _, _, ours)| format!("{rva:x}:{}", ours.iter().map(|b| format!("{b:02X}")).collect::<String>())).collect();
    wal_payload_common::patches::apply_list(&spec.join(","));
    log!("magiciansdead: free play {}", if on { "on" } else { "off" });
}

/// NESiCA play (`WAL_MAGICIANS_NESICA`, default 1): the dump's executable turns the card off
/// (its NESiCA acceptance hours check, +0x75F448, made to answer "closed": the game ignores the
/// card): the original check is put back, and the game server the card's data names started
/// (`magiciansdead_server.rs`). 0: guest play only.
fn nesica(exe: usize) {
    if std::env::var("WAL_MAGICIANS_NESICA").is_ok_and(|v| v == "0") {
        log!("magiciansdead: NESiCA off (guest play)");
        return;
    }
    // jmp -> the original jge (inside the hours: open)
    let at = (exe + 0x75f448) as *const [u8; 2];
    match unsafe { at.read_unaligned() } {
        [0xeb, 0x1c] => wal_payload_common::patches::apply_list("75f448:7D1C"),
        [0x7d, 0x1c] => {}
        other => {
            log!("magiciansdead: unknown executable at +0x75f448 ({other:02x?}): NESiCA left as it is");
            return;
        }
    }
    crate::magiciansdead_server::start();
}

// ---- one module for MAGIDEIO.dll and Leap.dll ----

static LOAD_LIBRARY_W: AtomicUsize = AtomicUsize::new(0);
static LOAD_LIBRARY_EX_W: AtomicUsize = AtomicUsize::new(0);
static LOAD_LIBRARY_EX_A: AtomicUsize = AtomicUsize::new(0);
static LOAD_LIBRARY_A: AtomicUsize = AtomicUsize::new(0);

/// This module when the file `path` names is one of [`MODULES`] (a new reference, as a load
/// would take).
fn this_module(path: &str) -> Option<HMODULE> {
    let file = path.rsplit(['\\', '/']).next().unwrap_or_default();
    if !MODULES.iter().any(|m| m.eq_ignore_ascii_case(file) || m.trim_end_matches(".dll").eq_ignore_ascii_case(file)) {
        return None;
    }
    let mut module: HMODULE = std::ptr::null_mut();
    unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, this_module as *const () as *const u16, &mut module) };
    log!("magiciansdead: {path} -> this module");
    Some(module)
}

fn wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

fn ansi(p: *const u8) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_string_lossy().into_owned()
}

unsafe extern "system" fn load_library_w(name: *const u16) -> HMODULE {
    if let Some(m) = this_module(&wide(name)) {
        return m;
    }
    let orig: unsafe extern "system" fn(*const u16) -> HMODULE = unsafe { std::mem::transmute(LOAD_LIBRARY_W.load(Ordering::Relaxed)) };
    unsafe { orig(name) }
}

unsafe extern "system" fn load_library_ex_w(name: *const u16, file: P, flags: u32) -> HMODULE {
    if let Some(m) = this_module(&wide(name)) {
        return m;
    }
    let orig: unsafe extern "system" fn(*const u16, P, u32) -> HMODULE = unsafe { std::mem::transmute(LOAD_LIBRARY_EX_W.load(Ordering::Relaxed)) };
    unsafe { orig(name, file, flags) }
}

unsafe extern "system" fn load_library_a(name: *const u8) -> HMODULE {
    if let Some(m) = this_module(&ansi(name)) {
        return m;
    }
    let orig: unsafe extern "system" fn(*const u8) -> HMODULE = unsafe { std::mem::transmute(LOAD_LIBRARY_A.load(Ordering::Relaxed)) };
    unsafe { orig(name) }
}

unsafe extern "system" fn load_library_ex_a(name: *const u8, file: P, flags: u32) -> HMODULE {
    if let Some(m) = this_module(&ansi(name)) {
        return m;
    }
    let orig: unsafe extern "system" fn(*const u8, P, u32) -> HMODULE = unsafe { std::mem::transmute(LOAD_LIBRARY_EX_A.load(Ordering::Relaxed)) };
    unsafe { orig(name, file, flags) }
}

// ---- MAGIDEIO.dll ----

/// The board's "OK" status.
const STATUS_OK: u32 = 0x200;

static COIN_HELD: AtomicBool = AtomicBool::new(false);
static COINS: AtomicU32 = AtomicU32::new(0);
static LAST_SWITCHES: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub extern "C" fn MAGIDEIO_Open() -> u32 {
    log!("magiciansdead: MAGIDEIO_Open");
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn MAGIDEIO_Close() -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn MAGIDEIO_GetStatus() -> u32 {
    STATUS_OK
}

#[unsafe(no_mangle)]
pub extern "C" fn MAGIDEIO_GetDeviceVersion() -> u32 {
    0x0100
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn MAGIDEIO_Update(buffer: *mut u8) -> u32 {
    if buffer.is_null() {
        return 0;
    }
    let mut switches = 0u32;
    let mut coin = false;
    for_each_pressed(|n| match n {
        Native::Switch(bit) => switches |= 1 << bit,
        Native::Coin => coin = true,
        _ => {}
    });
    // a coin per press, on coin slot 1's counter
    if coin && !COIN_HELD.swap(true, Ordering::Relaxed) {
        COINS.fetch_add(1, Ordering::Relaxed);
    } else if !coin {
        COIN_HELD.store(false, Ordering::Relaxed);
    }
    if LAST_SWITCHES.swap(switches, Ordering::Relaxed) != switches {
        log!("magiciansdead: switches {switches:#06x}");
    }
    let coins = (COINS.load(Ordering::Relaxed) & 0x3fff) as u16;
    let card = crate::rfid::card_inserted();
    unsafe {
        let w32 = |off: usize, v: u32| (buffer.add(off) as *mut u32).write_unaligned(v);
        w32(0, STATUS_OK);
        w32(4, 0x40);
        // a command: acknowledged
        if (buffer.add(8) as *const u32).read_unaligned() != 0 {
            w32(8, 0);
            w32(0x4c, 0x180);
        }
        *buffer.add(0x10) = (switches >> 8) as u8;
        *buffer.add(0x12) = switches as u8;
        (buffer.add(0x18) as *mut u16).write_unaligned(coins);
        (buffer.add(0x1a) as *mut u16).write_unaligned(0);
        static COMMAND: AtomicU32 = AtomicU32::new(0xffff_ffff);
        let command = u32::from(*buffer.add(0x74));
        if COMMAND.swap(command, Ordering::Relaxed) != command {
            log!("magiciansdead: reader command {command:#04x}");
        }
        // the reader reading: the card's moment (WAL_NESICA_CARD_AUTO)
        if command & 0x40 != 0 {
            crate::rfid::card_requested();
        }
        *buffer.add(0x34) = if card { 0x11 } else { 0 };
        let id = crate::rfid::card_id();
        let mut field = [0u8; 16];
        for (d, c) in field.iter_mut().zip(id.bytes()) {
            *d = c;
        }
        std::ptr::copy_nonoverlapping(field.as_ptr(), buffer.add(0x3c), 16);
    }
    static CARD: AtomicBool = AtomicBool::new(false);
    if CARD.swap(card, Ordering::Relaxed) != card {
        log!("magiciansdead: NESiCA {}", if card { "on the reader" } else { "removed" });
    }
    0
}

// ---- the grip controller (XInput pad 0) ----

const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

/// XINPUT_STATE: packet number, buttons, triggers, sticks.
#[repr(C)]
struct XState {
    packet: u32,
    buttons: u16,
    left_trigger: u8,
    right_trigger: u8,
    lx: i16,
    ly: i16,
    rx: i16,
    ry: i16,
}

const XINPUT_LEFT_THUMB: u16 = 0x40;
const XINPUT_LEFT_SHOULDER: u16 = 0x100;
const XINPUT_A: u16 = 0x1000;

unsafe extern "system" fn xinput_get_state(index: u32, state: *mut XState) -> u32 {
    if index != 0 {
        return ERROR_DEVICE_NOT_CONNECTED;
    }
    let stick = wal_payload_common::input(0);
    let mut buttons = 0u16;
    for_each_pressed(|n| match n {
        Native::Jump => buttons |= XINPUT_A,
        Native::LockOn => buttons |= XINPUT_LEFT_SHOULDER,
        Native::Center => buttons |= XINPUT_LEFT_THUMB,
        _ => {}
    });
    let (mut lx, mut ly) = (stick.axis(Axis::LeftX), stick.axis(Axis::LeftY));
    // the directions (keyboard arrows, d-pad) push the stick all the way when it is centred
    if lx == 0 && ly == 0 {
        use wal_protocol::button::{DOWN, LEFT, RIGHT, UP};
        let axis = |minus: u32, plus: u32| match (stick.pressed(minus), stick.pressed(plus)) {
            (true, false) => -32767,
            (false, true) => 32767,
            _ => 0,
        };
        (lx, ly) = (axis(LEFT, RIGHT), axis(UP, DOWN));
    }
    static PACKET: AtomicU32 = AtomicU32::new(0);
    static LAST: AtomicU32 = AtomicU32::new(0);
    if LAST.swap(u32::from(buttons), Ordering::Relaxed) != u32::from(buttons) {
        log!("magiciansdead: grip buttons {buttons:#06x}");
    }
    if !state.is_null() {
        let packet = PACKET.fetch_add(1, Ordering::Relaxed);
        // XInput's y goes up
        let ly = (-i32::from(ly)).clamp(-32768, 32767) as i16;
        unsafe { state.write(XState { packet, buttons, left_trigger: 0, right_trigger: 0, lx, ly, rx: 0, ry: 0 }) };
    }
    0
}

unsafe extern "system" fn xinput_set_state(index: u32, _vibration: P) -> u32 {
    if index != 0 { ERROR_DEVICE_NOT_CONNECTED } else { 0 }
}
