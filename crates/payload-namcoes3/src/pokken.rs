//! Pokkén Tournament (2015, 64-bit, Direct3D 11): update 24.05 (`ferrum_app.exe`, PE time stamp
//! 0x58DDBEF3, online) and 26.02 (0x5C18DFCF, the last one, offline). The game imports no cabinet
//! library of ours: the payload is installed as the `xinput1_3.dll` beside it, whose ordinals
//! 2 / 3 / 4 the game imports.
//!
//! - controller: the cabinet's Pokkén controller is an XInput pad (USB 0f0d:0073); the game
//!   lists the USB controllers (`driverUtil.dll`) and stops on any other one: it gets that one,
//!   and the pad from player 1's virtual stick ([`gamepad`]).
//! - I/O board: the Namco JVS board on COM3 through `wajvio.dll` (`jvs.rs`): test, service,
//!   coin, start.
//! - boot checks: the dongle serial, the cabinet serial step, the gateway ping and the store
//!   subnet (192.168.123.x) pass.
//! - the game's language and region follow the system locale (`GetUserDefaultLCID`,
//!   `GetSystemDefaultUILanguage`, `GetUserGeoID`); it only carries the Japanese texts (an
//!   English locale gets the texts' other columns, Spanish): the game sees Japan.
//! - `BlockInput` (the cabinet's keyboard and mouse off) does nothing.
//! - `WAL_NAMCOES3_BANAPASS=1`: the Banapassport card on the reader (COM4, `bpreader.rs`), the
//!   AMCUS auth daemon in-process (`amcus.rs`, the newer IAuth; ALL.Net SDAK, PKF1), the game
//!   server built into the payload (`wal_namcoes3srv::graphene` with the game's `jackal.proto`,
//!   answers in `jackal.rs`), the players in the dump's `F\jackal`.

use std::sync::OnceLock;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, input, log};
use wal_protocol::Axis;

use crate::patches;

pub(crate) const GAME: &str = "ferrum_app.exe";

/// An update: its name, PE time stamp, its USB dongle enumeration, the gateway ping, the store
/// subnet check, the network manager's log `(manager, std::string*)`, the network libraries'
/// `https://` checks of their URL at start (`call`s), the guest player's name (UTF-8).
struct Build {
    name: &'static str,
    timestamp: u32,
    dongle: usize,
    ping: usize,
    subnet: usize,
    netlog: usize,
    https_checks: &'static [usize],
    guest_name: usize,
}

const GUEST_JA: &[u8] = "プレイヤー(No Card)".as_bytes();
const GUEST_EN: &[u8] = b"Player (No Card)";

const BUILDS: &[Build] = &[
    // the last update, offline (made when the network service ended)
    Build { name: "26.02", timestamp: 0x5C18DFCF, dongle: 0x30b1d0, ping: 0x5f0d30, subnet: 0x5f1980, netlog: 0, https_checks: &[], guest_name: 0x958798 },
    // online
    Build { name: "24.05", timestamp: 0x58DDBEF3, dongle: 0x318700, ping: 0x60a700, subnet: 0x60b890, netlog: 0x60ef70, https_checks: &[0x1db120, 0x3122dd], guest_name: 0x97e298 },
];

static NETLOG_NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The network manager's log line (an MSVC `std::string`: inline below 16 bytes) to the payload
/// log, then the game's own log.
unsafe extern "system" fn netlog_line(manager: usize, line: *const u8) -> usize {
    if !line.is_null() {
        unsafe {
            let len = *(line.add(0x10) as *const usize);
            let cap = *(line.add(0x18) as *const usize);
            let text = if cap >= 16 { *(line as *const *const u8) } else { line };
            let bytes = std::slice::from_raw_parts(text, len.min(4096));
            log!("game: {}", String::from_utf8_lossy(bytes).trim_end());
        }
    }
    type F = unsafe extern "system" fn(usize, *const u8) -> usize;
    let next: F = unsafe { std::mem::transmute(NETLOG_NEXT.load(std::sync::atomic::Ordering::Relaxed)) };
    unsafe { next(manager, line) }
}

/// `WAL_NAMCOES3_GAMELOG=1`: the network manager's log (`[network][Manager]: ...`) to the payload
/// log; its 13-byte prologue (`mov [rsp+0x10], rdx; push rbp; push rsi; push rdi; lea rbp,
/// [rsp-0x10]`) moved to a trampoline.
fn netlog(base: usize, rva: usize) {
    const PROLOGUE: &[u8] = b"\x48\x89\x54\x24\x10\x55\x56\x57\x48\x8d\x6c\x24\xf0";
    let at = base + rva;
    if unsafe { std::slice::from_raw_parts(at as *const u8, PROLOGUE.len()) } != PROLOGUE {
        log!("pokken: network log at {rva:#x}: other bytes");
        return;
    }
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};
    let tramp = unsafe { VirtualAlloc(std::ptr::null(), 64, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) } as *mut u8;
    if tramp.is_null() {
        return;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(PROLOGUE.as_ptr(), tramp, PROLOGUE.len());
        std::ptr::copy_nonoverlapping(jump(at + PROLOGUE.len()).as_ptr(), tramp.add(PROLOGUE.len()), 12);
    }
    NETLOG_NEXT.store(tramp as usize, std::sync::atomic::Ordering::Relaxed);
    patches::write(at, &jump(netlog_line as *const () as usize));
    log!("pokken: network log at {rva:#x}");
}

/// The cabinet's dongle serial (2747, region 1, role 3, cabinet 19: the ALL.Net servers' rule).
const SERIAL: &str = "274713190001";
/// The ALL.Net game id and MUCHA game code.
const GAME_ID: &str = "SDAK";
const GAME_CD: &str = "PKF1";
/// The game server's address: loopback without a 0 byte (see [`gethostbyname`]).
const SERVER_ADDR: &str = "127.1.1.1:9002";
/// The game server's players, in the dump (drive F).
const SERVER_DIR: &str = "F:\\jackal";
/// The cabinet controller's USB ids (the only ones the game accepts).
const CONTROLLER: [u16; 2] = [0x0f0d, 0x0073];

/// A jump to `to` (12 bytes).
fn jump(to: usize) -> [u8; 12] {
    let mut code = [0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xE0];
    code[2..10].copy_from_slice(&(to as u64).to_le_bytes());
    code
}

/// The dongle serial reader `(.., .., .., .., info)`: the serial (UTF-16) at `info + 0x428` of
/// its 0x628 bytes.
unsafe extern "system" fn dongle_serial(_a: usize, _b: usize, _c: usize, _d: usize, info: *mut u8) -> i32 {
    if !info.is_null() {
        let mut serial = [0u8; 12];
        serial.copy_from_slice(SERIAL.as_bytes());
        // the cabinet's "USB DONGLE" (0b9a:0c10)
        unsafe { crate::patches::usb_key(info, 0x0c10, &serial) };
    }
    1
}

/// `driverUtilGetControllerUsbIdPairs(pairs, count)`: the cabinet controller.
unsafe extern "system" fn controller_ids(pairs: *mut u16, count: *mut u32) -> i32 {
    unsafe {
        if count.is_null() || *count == 0 || pairs.is_null() {
            return -1;
        }
        *pairs = CONTROLLER[0];
        *pairs.add(1) = CONTROLLER[1];
        *count = 1;
    }
    0
}

unsafe extern "system" fn block_input(_block: i32) -> i32 {
    1
}

/// OSVERSIONINFOEX: the cabinet's Windows Embedded Standard 7 SP1 (6.1.7601, workstation, the
/// embedded suite): the boot's system check ("19-12 SYSTEM ERROR 12" otherwise). `wide`: the W
/// layout (RtlGetVersion), else the A one (GetVersionExA).
unsafe fn windows7(info: *mut u8, wide: bool) {
    let size = unsafe { *(info as *const u32) } as usize;
    // the service pack fields after szCSDVersion (128 characters)
    let tail = 20 + if wide { 256 } else { 128 };
    unsafe {
        std::ptr::write_bytes(info.add(4), 0, size.saturating_sub(4));
        *(info.add(4) as *mut u32) = 6;
        *(info.add(8) as *mut u32) = 1;
        *(info.add(12) as *mut u32) = 7601;
        *(info.add(16) as *mut u32) = 2;
        for (i, c) in "Service Pack 1".bytes().enumerate() {
            if wide {
                *(info.add(20) as *mut u16).add(i) = c as u16;
            } else {
                *info.add(20 + i) = c;
            }
        }
        if size >= tail + 8 {
            *(info.add(tail) as *mut u16) = 1;
            // VER_SUITE_EMBEDDEDNT | VER_SUITE_SINGLEUSERTS
            *(info.add(tail + 4) as *mut u16) = 0x40 | 0x100;
            // VER_NT_WORKSTATION
            *info.add(tail + 6) = 1;
        }
    }
}

unsafe extern "system" fn rtl_get_version(info: *mut u8) -> i32 {
    if !info.is_null() {
        unsafe { windows7(info, true) };
    }
    0
}

unsafe extern "system" fn get_version_ex_a(info: *mut u8) -> i32 {
    if info.is_null() {
        return 0;
    }
    unsafe { windows7(info, false) };
    1
}

/// `GetUserNameA`: the cabinet's Windows account, `arcade` (the same boot check).
unsafe extern "system" fn get_user_name_a(buffer: *mut u8, size: *mut u32) -> i32 {
    const NAME: &[u8] = b"arcade\0";
    unsafe {
        if size.is_null() {
            return 0;
        }
        if buffer.is_null() || (*size as usize) < NAME.len() {
            *size = NAME.len() as u32;
            windows_sys::Win32::Foundation::SetLastError(122);
            return 0;
        }
        std::ptr::copy_nonoverlapping(NAME.as_ptr(), buffer, NAME.len());
        *size = NAME.len() as u32;
    }
    1
}

static GET_PROC_ADDRESS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `GetProcAddress`: the game looks up ntdll's `RtlGetVersion` for its system check.
unsafe extern "system" fn get_proc_address(module: usize, name: *const u8) -> usize {
    if name as usize > 0xffff && unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes() == b"RtlGetVersion" {
        return rtl_get_version as *const () as usize;
    }
    type F = unsafe extern "system" fn(usize, *const u8) -> usize;
    let real: F = unsafe { std::mem::transmute(GET_PROC_ADDRESS.load(std::sync::atomic::Ordering::Relaxed)) };
    unsafe { real(module, name) }
}

/// Japanese (ja-JP), the game's own language.
const LANG_JA_JP: u32 = 0x0411;
/// Japan's GEOID.
const GEO_JAPAN: i32 = 122;

unsafe extern "system" fn user_default_lcid() -> u32 {
    LANG_JA_JP
}

unsafe extern "system" fn system_default_ui_language() -> u16 {
    LANG_JA_JP as u16
}

unsafe extern "system" fn user_geo_id(_class: u32) -> i32 {
    GEO_JAPAN
}

#[derive(Clone, Copy, PartialEq)]
enum Native {
    A,
    B,
    X,
    Y,
    L,
    R,
    ZL,
    ZR,
    Start,
    Back,
    Up,
    Down,
    Left,
    Right,
}

const NATIVES: &[(&str, Native)] = &[
    ("a", Native::A),
    ("b", Native::B),
    ("x", Native::X),
    ("y", Native::Y),
    ("l", Native::L),
    ("r", Native::R),
    ("zl", Native::ZL),
    ("zr", Native::ZR),
    ("start", Native::Start),
    ("back", Native::Back),
    ("up", Native::Up),
    ("down", Native::Down),
    ("left", Native::Left),
    ("right", Native::Right),
];

/// Virtual stick -> pad: buttons 1-4 X A B Y (the cabinet's order), 5 / 6 L / R.
const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "x"),
    ("b2", "a"),
    ("b3", "b"),
    ("b4", "y"),
    ("b5", "l"),
    ("b6", "r"),
    ("b7", "zl"),
    ("b8", "zr"),
    ("start", "start"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
];

/// XINPUT_GAMEPAD button bits.
const BUTTONS: &[(Native, u16)] = &[
    (Native::Up, 0x0001),
    (Native::Down, 0x0002),
    (Native::Left, 0x0004),
    (Native::Right, 0x0008),
    (Native::Start, 0x0010),
    (Native::Back, 0x0020),
    (Native::L, 0x0100),
    (Native::R, 0x0200),
    (Native::A, 0x1000),
    (Native::B, 0x2000),
    (Native::X, 0x4000),
    (Native::Y, 0x8000),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// XINPUT_STATE: packet number, buttons, left and right triggers, lx ly rx ry.
#[repr(C)]
struct State {
    packet: u32,
    buttons: u16,
    left_trigger: u8,
    right_trigger: u8,
    sticks: [i16; 4],
}

/// Player 1's pad.
fn gamepad(state: &mut State) {
    let stick = input(0);
    let (mut buttons, mut zl, mut zr) = (0u16, false, false);
    map().for_each_pressed(&stick, |n| match n {
        Native::ZL => zl = true,
        Native::ZR => zr = true,
        n => buttons |= BUTTONS.iter().find(|(b, _)| *b == n).map_or(0, |(_, bit)| *bit),
    });
    state.buttons = buttons;
    state.left_trigger = if zl { 255 } else { 0 };
    state.right_trigger = if zr { 255 } else { 0 };
    // XInput's y goes up
    state.sticks = [
        stick.axis(Axis::LeftX),
        stick.axis(Axis::LeftY).saturating_neg(),
        stick.axis(Axis::RightX),
        stick.axis(Axis::RightY).saturating_neg(),
    ];
}

const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

unsafe extern "system" fn xinput_get_state(index: u32, state: *mut State) -> u32 {
    if index != 0 {
        return ERROR_DEVICE_NOT_CONNECTED;
    }
    if let Some(state) = unsafe { state.as_mut() } {
        let before = (state.buttons, state.left_trigger, state.right_trigger, state.sticks);
        gamepad(state);
        if before != (state.buttons, state.left_trigger, state.right_trigger, state.sticks) {
            state.packet = state.packet.wrapping_add(1);
        }
    }
    0
}

unsafe extern "system" fn xinput_set_state(index: u32, _vibration: *mut u8) -> u32 {
    if index != 0 { ERROR_DEVICE_NOT_CONNECTED } else { 0 }
}

/// XINPUT_CAPABILITIES: a gamepad, every button and axis.
unsafe extern "system" fn xinput_get_capabilities(index: u32, _flags: u32, caps: *mut u8) -> u32 {
    if index != 0 {
        return ERROR_DEVICE_NOT_CONNECTED;
    }
    if !caps.is_null() {
        let mut c = [0u8; 20];
        c[0] = 1; // XINPUT_DEVTYPE_GAMEPAD
        c[1] = 1; // XINPUT_DEVSUBTYPE_GAMEPAD
        c[4..6].copy_from_slice(&0xF3FFu16.to_le_bytes());
        c[6] = 255;
        c[7] = 255;
        for k in 0..4 {
            c[8 + 2 * k..10 + 2 * k].copy_from_slice(&(-64i16).to_le_bytes());
        }
        c[16..20].copy_from_slice(&[0xFF; 4]);
        unsafe { std::ptr::copy_nonoverlapping(c.as_ptr(), caps, c.len()) };
    }
    0
}

/// The auth daemon's `F:\\WritableConfig.ini` (`[RuntimeConfig]`): the cabinet's serial and
/// network id. The boot's serial check compares the dongle's serial with it ("19-1 SYSTEM ERROR
/// 1" otherwise).
fn writable_config() {
    const PATH: &str = "F:\\WritableConfig.ini";
    let net_id = format!("ABGN{}", &SERIAL[5..]);
    let text = format!("[RuntimeConfig]\r\nserialID={SERIAL}\r\nnetID={net_id}\r\n");
    if std::fs::read_to_string(PATH).ok().as_deref() != Some(text.as_str()) {
        match std::fs::write(PATH, &text) {
            Ok(()) => log!("pokken: {PATH}: serial {SERIAL}, network id {net_id}"),
            Err(e) => log!("pokken: {PATH}: {e}"),
        }
    }
}

static LOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The payload runs in Pokkén Tournament.
pub(crate) fn is_loaded() -> bool {
    LOADED.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) fn attach(base: usize) {
    LOADED.store(true, std::sync::atomic::Ordering::Relaxed);
    writable_config();
    let stamp = crate::exe_timestamp(base);
    match BUILDS.iter().find(|b| b.timestamp == stamp) {
        None => log!("pokken: build {stamp:#x} unknown, no patches"),
        Some(build) => {
            log!("pokken: update {}", build.name);
            patches::patch("dongle serial", base, build.dongle, b"\x40\x55\x56\x41\x54\x41\x56\x41\x57\x48\x8d\xac", &jump(dongle_serial as *const () as usize));
            // mov eax, 1; ret
            patches::patch("gateway ping", base, build.ping, b"\x44\x89\x4c\x24\x20\x53", &[0xB8, 1, 0, 0, 0, 0xC3]);
            patches::patch("store subnet", base, build.subnet, b"\x48\x8b\xc4\x55\x48\x8d", &[0xB8, 1, 0, 0, 0, 0xC3]);
            // the game server library speaks TLS 1.1 (axTLS, RSA key exchange) to an `https://`
            // server, which the built-in server does not: plain HTTP, the URL check answering
            // "not https" (xor eax, eax; the URL's copy it frees stays)
            if std::env::var("WAL_NAMCOES3_BANAPASS").is_ok_and(|v| v == "1") {
                for &rva in build.https_checks {
                    let cur = unsafe { std::slice::from_raw_parts((base + rva) as *const u8, 5) };
                    if cur[0] == 0xE8 {
                        let old = cur.to_vec();
                        patches::patch("game server plain HTTP", base, rva, &old, &[0x31, 0xC0, 0x90, 0x90, 0x90]);
                    }
                }
            }
            // the name of a player without a card, in the HUD: hard-coded Japanese
            let mut name = [0u8; GUEST_JA.len()];
            name[..GUEST_EN.len()].copy_from_slice(GUEST_EN);
            patches::patch("guest name", base, build.guest_name, GUEST_JA, &name);
            if build.netlog != 0 && std::env::var("WAL_NAMCOES3_GAMELOG").is_ok_and(|v| v == "1") {
                netlog(base, build.netlog);
            }
        }
    }
    unsafe {
        if iat::hook_module(base, "driverUtil.dll", "driverUtilGetControllerUsbIdPairs", controller_ids as *const () as usize).is_none() {
            log!("pokken: driverUtilGetControllerUsbIdPairs not hooked");
        }
        iat::hook_module(base, "user32.dll", "BlockInput", block_input as *const () as usize);
        iat::hook_module(base, "kernel32.dll", "GetVersionExA", get_version_ex_a as *const () as usize);
        iat::hook_module(base, "advapi32.dll", "GetUserNameA", get_user_name_a as *const () as usize);
        match iat::hook_module(base, "iphlpapi.dll", "GetAdaptersInfo", get_adapters_info as *const () as usize) {
            Some(o) => GET_ADAPTERS_INFO.store(o, std::sync::atomic::Ordering::Relaxed),
            None => log!("pokken: GetAdaptersInfo not hooked"),
        }
        match iat::hook_module(base, "iphlpapi.dll", "GetIfTable2", get_if_table2 as *const () as usize) {
            Some(o) => GET_IF_TABLE2.store(o, std::sync::atomic::Ordering::Relaxed),
            None => log!("pokken: GetIfTable2 not hooked"),
        }
        match iat::hook_module(base, "kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) {
            Some(o) => GET_PROC_ADDRESS.store(o, std::sync::atomic::Ordering::Relaxed),
            None => log!("pokken: GetProcAddress not hooked"),
        }
        iat::hook_module(base, "kernel32.dll", "GetUserDefaultLCID", user_default_lcid as *const () as usize);
        iat::hook_module(base, "kernel32.dll", "GetSystemDefaultUILanguage", system_default_ui_language as *const () as usize);
        iat::hook_module(base, "kernel32.dll", "GetUserGeoID", user_geo_id as *const () as usize);
        for (ordinal, f) in [(2, xinput_get_state as *const () as usize), (3, xinput_set_state as *const () as usize), (4, xinput_get_capabilities as *const () as usize)] {
            if iat::hook_ordinal(base, "xinput1_3.dll", ordinal, f).is_none() {
                log!("pokken: XInput ordinal {ordinal} not hooked");
            }
        }
    }
    let wajvio = crate::module("wajvio.dll");
    if wajvio == 0 {
        log!("pokken: wajvio.dll not loaded, no I/O board");
    }
    crate::jvs::init(&[wajvio]);
    if std::env::var("WAL_NAMCOES3_BANAPASS").is_ok_and(|v| v == "1") {
        network(base);
    }
    wal_payload_common::input_filter::init(base, |_| false);
}

static GET_IF_TABLE2: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// MIB_IF_ROW2: its size, `Type`, `InterfaceAndOperStatusFlags` (bit 0: hardware),
/// `OperStatus` (1: up) and `MediaConnectState` (1: connected) offsets.
const IF_ROW2: usize = 0x548;
const IF_TYPE: usize = 0x468;
const IF_FLAGS: usize = 0x480;
const IF_OPER_STATUS: usize = 0x484;
const IF_MEDIA_STATE: usize = 0x48c;
const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;

/// `GetIfTable2(table**)`: the game's link check wants a hardware interface with its cable
/// connected (the cabinet's network, "4-1 LAN ERROR 1" otherwise); Wine marks none as
/// hardware: the up interfaces but the loopback are (connected), else the loopback.
unsafe extern "system" fn get_if_table2(table: *mut *mut u8) -> u32 {
    type F = unsafe extern "system" fn(*mut *mut u8) -> u32;
    let real: F = unsafe { std::mem::transmute(GET_IF_TABLE2.load(std::sync::atomic::Ordering::Relaxed)) };
    let status = unsafe { real(table) };
    if status != 0 || table.is_null() || unsafe { (*table).is_null() } {
        return status;
    }
    unsafe {
        let t = *table;
        let count = *(t as *const u32) as usize;
        let row = |i: usize| t.add(8 + i * IF_ROW2);
        let up = |r: *mut u8| *(r.add(IF_OPER_STATUS) as *const u32) == 1;
        let lo = |r: *mut u8| *(r.add(IF_TYPE) as *const u32) == IF_TYPE_SOFTWARE_LOOPBACK;
        let mut marked = false;
        for i in 0..count {
            if up(row(i)) && !lo(row(i)) {
                *row(i).add(IF_FLAGS) |= 1;
                *(row(i).add(IF_MEDIA_STATE) as *mut u32) = 1;
                marked = true;
            }
        }
        if !marked && let Some(i) = (0..count).find(|&i| up(row(i))) {
            *row(i).add(IF_FLAGS) |= 1;
            *(row(i).add(IF_MEDIA_STATE) as *mut u32) = 1;
        }
    }
    status
}

static GET_ADAPTERS_INFO: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// IP_ADAPTER_INFO (x64): its size, `IpAddressList.IpAddress` (the first address, text).
const ADAPTER_INFO: usize = 0x2c0;
const ADAPTER_IP: usize = 0x1c0;

/// `GetAdaptersInfo(adapters, size)`: the game takes the first adapter's address as the
/// cabinet's ("address acquisition NG" when it is 0.0.0.0, an interface without address listed
/// first): one adapter, the first with an address (the cabinet's single network card).
unsafe extern "system" fn get_adapters_info(adapters: *mut u8, size: *mut u32) -> u32 {
    type F = unsafe extern "system" fn(*mut u8, *mut u32) -> u32;
    let real: F = unsafe { std::mem::transmute(GET_ADAPTERS_INFO.load(std::sync::atomic::Ordering::Relaxed)) };
    let status = unsafe { real(adapters, size) };
    if status != 0 || adapters.is_null() {
        return status;
    }
    unsafe {
        let addressed = |a: *mut u8| {
            let ip = std::ffi::CStr::from_ptr(a.add(ADAPTER_IP).cast()).to_bytes();
            !ip.is_empty() && ip != b"0.0.0.0"
        };
        let mut node = adapters;
        while !node.is_null() && !addressed(node) {
            node = *(node as *const *mut u8);
        }
        if !node.is_null() {
            if node != adapters {
                std::ptr::copy_nonoverlapping(node, adapters, ADAPTER_INFO);
            }
            *(adapters as *mut *mut u8) = std::ptr::null_mut();
        }
    }
    status
}

static GETHOSTBYNAME: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `gethostbyname` (ws2_32 #52): the game server library copies the address as a C string
/// (`strlen` of `h_addr_list[0]`): an address with a 0 byte is cut there, the rest of its copy
/// left over memory (127.0.0.1 connected to 127.116.116.112). The game server's address has no
/// 0 byte ([`SERVER_ADDR`]); the answer is copied to memory that stays, each address followed by
/// a 0 byte.
unsafe extern "system" fn gethostbyname(name: *const u8) -> *mut u8 {
    type F = unsafe extern "system" fn(*const u8) -> *mut u8;
    let real: F = unsafe { std::mem::transmute(GETHOSTBYNAME.load(std::sync::atomic::Ordering::Relaxed)) };
    let host = unsafe { real(name) };
    if host.is_null() {
        return host;
    }
    static KEPT: std::sync::Mutex<Vec<(Vec<u8>, usize)>> = std::sync::Mutex::new(Vec::new());
    let key = if name.is_null() { Vec::new() } else { unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes().to_vec() };
    let mut kept = KEPT.lock().unwrap();
    if let Some((_, h)) = kept.iter().find(|(k, _)| *k == key) {
        return *h as *mut u8;
    }
    // hostent (x64): name, aliases, address type (i16), address length (i16), addresses
    unsafe {
        let len = *(host.add(18) as *const i16) as usize;
        let list = *(host.add(24) as *const *const *const u8);
        let mut addrs: Vec<*const u8> = Vec::new();
        let mut i = 0;
        while !list.is_null() && !(*list.add(i)).is_null() {
            let mut bytes = std::slice::from_raw_parts(*list.add(i), len).to_vec();
            bytes.push(0);
            let a: &'static mut [u8] = Box::leak(bytes.into_boxed_slice());
            addrs.push(a.as_ptr());
            i += 1;
        }
        addrs.push(std::ptr::null());
        let addrs: &'static mut [*const u8] = Box::leak(addrs.into_boxed_slice());
        let hname: &'static mut [u8] = Box::leak([key.as_slice(), &[0]].concat().into_boxed_slice());
        let aliases: &'static mut [*const u8; 1] = Box::leak(Box::new([std::ptr::null()]));
        let copy: &'static mut [u8; 32] = Box::leak(Box::new([0u8; 32]));
        std::ptr::copy_nonoverlapping(host, copy.as_mut_ptr(), 32);
        *(copy.as_mut_ptr() as *mut *const u8) = hname.as_ptr();
        *(copy.as_mut_ptr().add(8) as *mut *const *const u8) = aliases.as_ptr();
        *(copy.as_mut_ptr().add(24) as *mut *const *const u8) = addrs.as_ptr();
        kept.push((key, copy.as_mut_ptr() as usize));
        copy.as_mut_ptr()
    }
}

static SSL_CTX_CTRL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// `SSL_CTX_ctrl(ctx, cmd, larg, parg)` (ssleay32 #6): the game's options keep TLS 1.2 off
/// (`SSL_OP_NO_TLSv1_2` in `SSL_CTRL_OPTIONS`), the built-in server needs it.
unsafe extern "C" fn ssl_ctx_ctrl(ctx: usize, cmd: i32, larg: i64, parg: usize) -> i64 {
    const SSL_CTRL_OPTIONS: i32 = 32;
    const SSL_OP_NO_TLSV1_2: i64 = 0x0800_0000;
    type F = unsafe extern "C" fn(usize, i32, i64, usize) -> i64;
    let real: F = unsafe { std::mem::transmute(SSL_CTX_CTRL.load(std::sync::atomic::Ordering::Relaxed)) };
    let larg = if cmd == SSL_CTRL_OPTIONS { larg & !SSL_OP_NO_TLSV1_2 } else { larg };
    unsafe { real(ctx, cmd, larg, parg) }
}

static CO_CREATE_INSTANCE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The AMCUS auth daemon (`IAuth`), in-process (`amcus.rs`).
unsafe extern "system" fn co_create_instance(clsid: *const u8, outer: *mut std::ffi::c_void, context: u32, iid: *const u8, out: *mut *mut std::ffi::c_void) -> i32 {
    if !clsid.is_null() && unsafe { std::slice::from_raw_parts(clsid, 16) } == crate::amcus::CLSID {
        if !out.is_null() {
            let object = crate::amcus::daemon(&crate::amcus::OBJECT_NEW);
            unsafe { *out = (object as *const crate::amcus::Object).cast_mut().cast() };
        }
        log!("pokken: in-process AMCUS auth (IAuth)");
        return 0;
    }
    type F = unsafe extern "system" fn(*const u8, *mut std::ffi::c_void, u32, *const u8, *mut *mut std::ffi::c_void) -> i32;
    let real: F = unsafe { std::mem::transmute(CO_CREATE_INSTANCE.load(std::sync::atomic::Ordering::Relaxed)) };
    unsafe { real(clsid, outer, context, iid, out) }
}

/// The game's `jackal.proto` file descriptor (its protocol), from its start to the end of the
/// executable's image.
fn descriptor(base: usize) -> Option<&'static [u8]> {
    const START: &[u8] = b"\n\x0cjackal.proto\x12";
    let image = unsafe {
        let pe = base + *((base + 0x3c) as *const u32) as usize;
        let size = *((pe + 0x50) as *const u32) as usize;
        std::slice::from_raw_parts(base as *const u8, size)
    };
    let at = image.windows(START.len()).position(|w| w == START)?;
    Some(&image[at..])
}

/// Online: the card reader on COM4 (the player's card with the `card` button, or by itself
/// after start), the AMCUS daemon, the game server.
fn network(base: usize) {
    if std::env::var("WAL_NAMCOES3_GAME_SERVER").is_err() {
        // SAFETY: DllMain, before the game's threads
        unsafe { std::env::set_var("WAL_NAMCOES3_GAME_SERVER", SERVER_ADDR) };
    }
    let card = crate::bpreader::player_card();
    crate::bpreader::card_button(card);
    if std::env::var("WAL_NAMCOES3_AUTO_CARD").is_ok_and(|v| v.trim() == "1") {
        // the card goes on the reader when the game looks for it after start (its card question
        // answered yes); a card left on the reader is read again and again
        crate::bpreader::card_on_poll_after(card, wal_protocol::button::START);
    }
    crate::bpreader::init("COM4", &[base]);
    // the ALL.Net game server uri: the title's path
    crate::amcus::set_identity(crate::amcus::Identity { game_id: GAME_ID, game_cd: GAME_CD, game_ver: (1, 0), serial: SERIAL, scheme: "https://", path: "/pokken/" });
    match unsafe { iat::hook_module(base, "ole32.dll", "CoCreateInstance", co_create_instance as *const () as usize) } {
        Some(o) => CO_CREATE_INSTANCE.store(o, std::sync::atomic::Ordering::Relaxed),
        None => log!("pokken: CoCreateInstance not hooked"),
    }
    match unsafe { iat::hook_ordinal(base, "ws2_32.dll", 52, gethostbyname as *const () as usize) } {
        Some(o) => GETHOSTBYNAME.store(o, std::sync::atomic::Ordering::Relaxed),
        None => log!("pokken: gethostbyname not hooked"),
    }
    // the game server's TLS: the game asks OpenSSL 1.0.1 for TLS 1.0 only (`TLSv1_method`,
    // ssleay32 #170), the built-in server speaks TLS 1.2: the version negotiated instead
    // (`SSLv23_method`), TLS 1.2 not excluded (`ssl_ctx_ctrl`)
    let ssl = crate::module("ssleay32.dll");
    let negotiated = if ssl != 0 { unsafe { windows_sys::Win32::System::LibraryLoader::GetProcAddress(ssl as _, c"SSLv23_method".as_ptr().cast()) } } else { None };
    match negotiated {
        Some(f) if unsafe { iat::hook_ordinal(base, "ssleay32.dll", 170, f as usize) }.is_some() => {}
        _ => log!("pokken: TLSv1_method not replaced"),
    }
    match unsafe { iat::hook_ordinal(base, "ssleay32.dll", 6, ssl_ctx_ctrl as *const () as usize) } {
        Some(o) => SSL_CTX_CTRL.store(o, std::sync::atomic::Ordering::Relaxed),
        None => log!("pokken: SSL_CTX_ctrl not hooked"),
    }
    let verbose = std::env::var("WAL_NAMCOES3_SERVER_LOG").is_ok_and(|v| v == "1");
    let name = std::env::var("WAL_NAMCOES3_PROFILE_NAME").ok().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "PLAYER".into());
    match descriptor(base) {
        Some(d) => wal_namcoes3srv::graphene::start(&crate::amcus::game_server(), d, SERVER_DIR, &name, |line| log!("{line}"), verbose),
        None => log!("pokken: jackal.proto descriptor not found, no game server"),
    }
}


