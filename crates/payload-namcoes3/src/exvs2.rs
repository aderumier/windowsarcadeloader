//! Mobile Suit Gundam Extreme Vs. 2 (Bandai Namco, 2018, Direct3D 12, 64-bit),
//! `exvs2_exe_Release.exe` 4.00 (ALL.Net SBUZ, MUCHA GX21 29.31). The game imports nothing of
//! ours: the payload is loaded at the executable's entry point (`wal-loader64`, profile
//! `payload_loader`). It reproduces the cabinet devices of bananatools' `exvs2hook` and
//! EXVS2-POC's client hook (asesidaa/EXVS2-POC, reference only: no license):
//!
//! - two programs, as the shop's two PCs: the cabinet (the game played, client type 1, dongle
//!   serial `28111401nnnn`) and the live monitor (LM, the shop's server and display, client
//!   type 2, serial `28111101nnnn`, started beside it with `-wal-lm`: profile `background`). A
//!   cabinet without an LM on its network stops on error 21-01 LIVEMONITOR DISCONNECT; the LM's
//!   check of the shop's cabinets 1 and 2 is off (developer option 27, EXVS2-POC's LM mode). The
//!   PCB id is `ABLN` + the serial's last 7 digits. The LM gets no player input.
//! - the JVS I/O board (NA-JV) on COM3 through the game's `wajvio.dll` (`jvs.rs`, one player:
//!   the lever, A shoot, B melee, C jump, D switch target).
//! - the Banapassport reader (`bngrw.dll` imports, `bngrw.rs` in callback mode).
//! - the AMCUS auth daemon (`IAuth`) in-process (`amcus.rs`, bananatools' layout), its ALL.Net
//!   answer pointing at the game server (`wal_namcoes3srv::exvs2`).
//! - EXVS2-POC's patches of this build: the content router check (the shop network must be
//!   192.168.85.0, bananatools' `netenv`), the client type (the cabinet, not the live monitor),
//!   the network adapter check, the developer option "server connection after 1 am JST", the
//!   opening's reminder screen skipped.
//! - free play (the common option, `WAL_FREEPLAY`): the cabinet's test mode settings
//!   (`testmode_pcb.json` on F: and its mirror on G:, [`free_play`]), coin options free play on and
//!   remote setup off (else the LM's coin settings take their place).
//! - the game's own XInput pads (developer controls) neutral, its keyboard hidden: input from the
//!   launcher only.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::{iat, log};

use crate::patches;

pub(crate) const GAME: &str = "exvs2_exe_Release.exe";

/// The build's entry point (EXVS2-POC's version check: 4.00).
const ENTRY_400: u32 = 0x4dad54;

/// A program's role: the cabinet (the game played) or the live monitor (`-wal-lm`).
struct Role {
    name: &'static str,
    /// The client type (1 the cabinet, 2 the LM).
    client_type: u8,
    /// The dongle serial and network id.
    serial: &'static str,
    net_id: &'static str,
}

const CABINET: Role = Role { name: "cabinet", client_type: 1, serial: "281114010001", net_id: "ABLN4010001" };
const LIVE_MONITOR: Role = Role { name: "live monitor", client_type: 2, serial: "281111010001", net_id: "ABLN1010001" };

/// The program's role, from its command line.
fn role() -> &'static Role {
    static LM: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *LM.get_or_init(|| std::env::args().any(|a| a.eq_ignore_ascii_case("-wal-lm"))) { &LIVE_MONITOR } else { &CABINET }
}

/// The program is the live monitor.
pub(crate) fn is_live_monitor() -> bool {
    role().client_type == 2
}
const GAME_ID: &str = "SBUZ";
const GAME_CD: &str = "GX21";
const CACFG_VER: &str = "29.31";

/// The player's card, made once (F: the game's save drive, `WindowsLoader\F`).
const CARD_FILE: &str = "F:\\banapass.txt";
/// The game server's players.
const SERVER_DIR: &str = "F:\\exvs2srv";

static LOADED: AtomicBool = AtomicBool::new(false);

/// The payload runs in the game.
pub(crate) fn is_loaded() -> bool {
    LOADED.load(Ordering::Relaxed)
}

fn entry_point(base: usize) -> u32 {
    unsafe {
        let pe = base + *((base + 0x3c) as *const u32) as usize;
        *((pe + 0x28) as *const u32)
    }
}

// --- the USB dongle ---

unsafe extern "system" fn usb_finder_ok() -> u64 {
    0
}

/// `nbamUsbFinderGetSerialNumber(index, out)`: the dongle's serial (16 bytes at most).
unsafe extern "system" fn usb_finder_serial(_index: i32, out: *mut u8) -> u64 {
    if !out.is_null() {
        unsafe {
            let serial = role().serial;
            std::ptr::copy_nonoverlapping(serial.as_ptr(), out, serial.len());
            *out.add(serial.len()) = 0;
        }
    }
    0
}

// --- the game's pads ---

/// XINPUT_STATE: packet number, then the pad state: neutral.
unsafe extern "system" fn xinput_get_state(_index: u32, state: *mut u8) -> u32 {
    if !state.is_null() {
        unsafe { std::ptr::write_bytes(state, 0, 16) };
    }
    0
}

unsafe extern "system" fn xinput_set_state(_index: u32, _vibration: *mut c_void) -> u32 {
    0
}

/// No pad connected (ERROR_DEVICE_NOT_CONNECTED) for the capability query.
unsafe extern "system" fn xinput_get_capabilities(_index: u32, _flags: u32, _caps: *mut c_void) -> u32 {
    1167
}

// --- the AMCUS daemon ---

static CO_CREATE_INSTANCE: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn co_create_instance(clsid: *const u8, outer: *mut c_void, context: u32, iid: *const u8, out: *mut *mut c_void) -> i32 {
    if !clsid.is_null() && unsafe { std::slice::from_raw_parts(clsid, 16) } == crate::amcus::CLSID {
        if !out.is_null() {
            let object = crate::amcus::daemon(&crate::amcus::OBJECT_NEW);
            unsafe { *out = (object as *const crate::amcus::Object).cast_mut().cast() };
        }
        log!("exvs2: in-process AMCUS auth (IAuth)");
        return 0;
    }
    type F = unsafe extern "system" fn(*const u8, *mut c_void, u32, *const u8, *mut *mut c_void) -> i32;
    let real: F = unsafe { std::mem::transmute(CO_CREATE_INSTANCE.load(Ordering::Relaxed)) };
    unsafe { real(clsid, outer, context, iid, out) }
}

// --- the developer options ---

static DEV_OPTION: AtomicUsize = AtomicUsize::new(0);

/// The developer menu's option `index` ('1' on): 6 "server connection after 1 am JST" on
/// (EXVS2-POC; the game otherwise refuses the network at night, Japan time), 27 "no check of
/// PCB 1 and 2 in the LM" on (EXVS2-POC's live monitor mode).
unsafe extern "system" fn dev_option(menu: usize, index: i32) -> u8 {
    if index == 6 || index == 27 && is_live_monitor() {
        return b'1';
    }
    let real: unsafe extern "system" fn(usize, i32) -> u8 = unsafe { std::mem::transmute(DEV_OPTION.load(Ordering::Relaxed)) };
    unsafe { real(menu, index) }
}

/// EXVS2-POC's patches of 4.00 (`game/*.cpp`).
fn patches_400(base: usize) {
    // ContentRouterPatcher: `(ip & mask) == 192.168.85.0` -> true
    patches::patch("content router", base, 0x652DD0, b"\xc7\x44\x24\x10\xc0\xa8\x55\x00", &[0xB0, 0x01, 0xC3]);
    // ClientTypePatcher: the client type check skipped, the program's type
    patches::patch("client type jump", base, 0x66DF3C, b"\x0f\x85\xd1\x00\x00\x00", &[0x90; 6]);
    let mut client = [0x90u8; 18];
    client[14..18].copy_from_slice(&[0xB8, role().client_type, 0x00, 0x00]);
    patches::patch("client type", base, 0x66DF47, b"\x48\x8b\xc8\x48\x8d\x15\xa7\xce\xa5\x00\x41\x83\xc8\xff\xe8\xb6\x0e\x00", &client);
    // NetworkAdapterCheckPatcher
    patches::patch("network adapter check", base, 0x2E59F7, b"\x74\x0a", &[0xEB]);
    // OpeningScreenSkipper (SkipReminder, EXVS2-POC's default)
    patches::patch("opening reminder", base, 0x8AEC37 + 5, b"\x74\x67", &[0x74, 0x52]);
    // DevMenuSubOptionsEnabler
    match unsafe { wal_payload_common::patches::detour(base + 0x64C320, b"\x48\x89\x5c\x24\x08", dev_option as *const () as usize) } {
        Some(t) => DEV_OPTION.store(t, Ordering::Relaxed),
        None => log!("exvs2: developer options not hooked"),
    }
}

// --- the live monitor's window: off the screen ---

/// Where the LM's window goes (left of the screen): the picture shown is the cabinet's.
const OFFSCREEN_X: i32 = -20000;

static CREATE_WINDOW: AtomicUsize = AtomicUsize::new(0);
static SET_WINDOW_POS: AtomicUsize = AtomicUsize::new(0);
static MOVE_WINDOW: AtomicUsize = AtomicUsize::new(0);
static SHOW_WINDOW: AtomicUsize = AtomicUsize::new(0);

type P = *mut c_void;

/// `CreateWindowExW`: a top-level window off the screen, never activated, out of the taskbar
/// (`WS_EX_NOACTIVATE`, `WS_EX_TOOLWINDOW`).
unsafe extern "system" fn create_window(ex: u32, class: P, name: P, style: u32, x: i32, y: i32, w: i32, h: i32, parent: P, menu: P, inst: P, param: P) -> P {
    type F = unsafe extern "system" fn(u32, P, P, u32, i32, i32, i32, i32, P, P, P, P) -> P;
    let real: F = unsafe { std::mem::transmute(CREATE_WINDOW.load(Ordering::Relaxed)) };
    let (ex, x) = if parent.is_null() { (ex | 0x0800_0000 | 0x80, OFFSCREEN_X) } else { (ex, x) };
    unsafe { real(ex, class, name, style, x, y, w, h, parent, menu, inst, param) }
}

unsafe extern "system" fn set_window_pos(hwnd: P, after: P, _x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32 {
    type F = unsafe extern "system" fn(P, P, i32, i32, i32, i32, u32) -> i32;
    let real: F = unsafe { std::mem::transmute(SET_WINDOW_POS.load(Ordering::Relaxed)) };
    // SWP_NOACTIVATE
    unsafe { real(hwnd, after, OFFSCREEN_X, y, cx, cy, flags | 0x10) }
}

unsafe extern "system" fn move_window(hwnd: P, _x: i32, y: i32, w: i32, h: i32, repaint: i32) -> i32 {
    type F = unsafe extern "system" fn(P, i32, i32, i32, i32, i32) -> i32;
    let real: F = unsafe { std::mem::transmute(MOVE_WINDOW.load(Ordering::Relaxed)) };
    unsafe { real(hwnd, OFFSCREEN_X, y, w, h, repaint) }
}

/// `ShowWindow`: shown without being activated (`SW_SHOWNOACTIVATE`).
unsafe extern "system" fn show_window(hwnd: P, cmd: i32) -> i32 {
    type F = unsafe extern "system" fn(P, i32) -> i32;
    let real: F = unsafe { std::mem::transmute(SHOW_WINDOW.load(Ordering::Relaxed)) };
    let cmd = if matches!(cmd, 1 | 3 | 5 | 9 | 10) { 4 } else { cmd };
    unsafe { real(hwnd, cmd) }
}

/// user32's own `name` (0: not found).
fn user32_function(name: &str) -> usize {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    let module: Vec<u16> = "user32.dll".encode_utf16().chain(Some(0)).collect();
    let name = std::ffi::CString::new(name).unwrap_or_default();
    unsafe { GetProcAddress(GetModuleHandleW(module.as_ptr()), name.as_ptr().cast()) }.map_or(0, |f| f as usize)
}

/// The LM's window off the screen (the cabinet's is the one played). The hooks call user32's own
/// functions, not the earlier hooks of the import table: the common window options of the
/// profile (screen-sized popup at 0,0, `window.rs`) are the cabinet's.
fn hide_live_monitor(base: usize) {
    for (name, f, slot) in [
        ("CreateWindowExW", create_window as *const () as usize, &CREATE_WINDOW),
        ("SetWindowPos", set_window_pos as *const () as usize, &SET_WINDOW_POS),
        ("MoveWindow", move_window as *const () as usize, &MOVE_WINDOW),
        ("ShowWindow", show_window as *const () as usize, &SHOW_WINDOW),
    ] {
        let real = user32_function(name);
        match unsafe { iat::hook_module(base, "user32.dll", name, f) } {
            Some(o) => slot.store(if real != 0 { real } else { o }, Ordering::Relaxed),
            None => log!("exvs2: {name} not hooked"),
        }
    }
}

/// The shop's network of the two programs on this PC: the link of instances on one PC
/// (`link.rs`), each its loopback address: the LM the link's master at 127.0.0.2, the cabinet at
/// 127.0.0.3. The game's own LAN (its P2P terminals' broadcasts) reaches the other program.
fn shop_network(base: usize) {
    let lm = is_live_monitor();
    let vars = [
        ("WAL_LINK", "1"),
        ("WAL_CABINET", if lm { "1" } else { "2" }),
        ("WAL_LINK_ADDRESS", if lm { "127.0.0.2" } else { "127.0.0.3" }),
        ("WAL_LINK_MASTER", "127.0.0.2"),
    ];
    for (k, v) in vars {
        // SAFETY: set before the link's threads start, on the loader's thread
        unsafe { std::env::set_var(k, v) };
    }
    wal_payload_common::iat::set_game_module(base);
    let n = wal_payload_common::link::init();
    log!("exvs2: shop network, {n} programs");
}

/// A save file of the game's `nbamsavdat.dll`: `svd1`, version 1, the data's size, 0, the SHA-1
/// of these 16 bytes and the data, the data.
fn svd1(data: &[u8]) -> Vec<u8> {
    let mut out = b"svd1".to_vec();
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
    ctx.update(&out);
    ctx.update(data);
    out.extend_from_slice(ctx.finish().as_ref());
    out.extend_from_slice(data);
    out
}

/// The cabinet's test mode settings (`testmode_pcb.json`, F: and its mirror G:): free play as the
/// common option asks (`WAL_FREEPLAY`; unset: the operator's setting kept). On: coin options' free
/// play on, remote setup off (the LM's coin settings would take their place). A missing file is
/// made with these keys alone (the game keeps its defaults for the others).
fn free_play() {
    use wal_payload_common::json::Json;
    let Some(on) = wal_payload_common::free_play("WAL_NAMCOES3_FREEPLAY") else { return };
    let entry = |v: u32| Json::obj().with("type", Json::num(1)).with("value", Json::num(v));
    for path in ["F:\\testmode_pcb.json", "G:\\testmode_pcb.json"] {
        let old = std::fs::read(path).ok().filter(|d| d.len() > 0x24 && d.starts_with(b"svd1"));
        let mut doc = old.as_ref().and_then(|d| Json::parse(std::str::from_utf8(&d[0x24..]).ok()?)).unwrap_or_else(|| {
            Json::obj().with("storage", Json::obj().with(".TestModeParam.version", Json::obj().with("type", Json::num(2)).with("value", Json::num(0)))).with("format_version", Json::num(1))
        });
        let Some(storage) = doc.get_mut("storage") else { continue };
        storage.set(".TestModeParam.coin.freePlay", entry(on as u32));
        if on {
            storage.set(".TestModeParam.coin.remoteSetup", entry(0));
        }
        match std::fs::write(path, svd1(doc.to_string().as_bytes())) {
            Ok(()) => log!("exvs2: free play {} ({path})", if on { "on" } else { "off" }),
            Err(e) => log!("exvs2: {path}: {e}"),
        }
    }
}

/// The cabinet starts once the LM listens on its P2P port (127.0.0.2:30000 taken: our own bind
/// fails), at most 3 minutes: a cabinet that looks for the LM before it is up stops on error
/// 21-01 (the LM's first boot builds its shader cache, slower than the cabinet's).
fn wait_for_live_monitor() {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(180) {
        if std::net::UdpSocket::bind("127.0.0.2:30000").is_err() {
            log!("exvs2: the LM is up ({:.1} s)", start.elapsed().as_secs_f32());
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    log!("exvs2: no LM after 3 minutes, starting anyway");
}

pub(crate) fn is_exvs2(exe: &str) -> bool {
    exe.eq_ignore_ascii_case(GAME)
}

pub(crate) fn attach(base: usize) {
    LOADED.store(true, Ordering::Relaxed);
    crate::GAME_BASE.store(base, Ordering::Relaxed);
    if !is_live_monitor() {
        wait_for_live_monitor();
    }
    let entry = entry_point(base);
    if entry == ENTRY_400 {
        log!("exvs2: 4.00, {}", role().name);
        patches_400(base);
    } else {
        log!("exvs2: build with entry point {entry:#x} unknown, no patches");
    }
    for (name, f) in [
        ("nbamUsbFinderInitialize", usb_finder_ok as *const () as usize),
        ("nbamUsbFinderGetSerialNumber", usb_finder_serial as *const () as usize),
    ] {
        if unsafe { iat::hook_module(base, "nbamUsbFinder.dll", name, f) }.is_none() {
            log!("exvs2: {name} not hooked");
        }
    }
    for (name, f) in [
        ("XInputGetState", xinput_get_state as *const () as usize),
        ("XInputSetState", xinput_set_state as *const () as usize),
        ("XInputGetCapabilities", xinput_get_capabilities as *const () as usize),
    ] {
        unsafe { iat::hook_module(base, "XINPUT9_1_0.dll", name, f) };
    }
    wal_payload_common::input_filter::init(base, |_| false);
    shop_network(base);
    if is_live_monitor() {
        hide_live_monitor(base);
    } else {
        free_play();
    }

    // the I/O board, opened by wajvio.dll
    let wajvio = crate::module("wajvio.dll");
    if wajvio == 0 {
        log!("exvs2: wajvio.dll not loaded, no I/O board");
    }
    crate::jvs::init(&[wajvio, base]);

    // the Banapassport reader: the card button, or by itself after start
    crate::bngrw::init_callbacks(base);
    let card = crate::bpreader::player_card_in(CARD_FILE);
    let auto = std::env::var("WAL_NAMCOES3_AUTO_CARD").is_ok_and(|v| v.trim() == "1");
    std::thread::spawn(move || {
        if is_live_monitor() {
            return;
        }
        let mut start_held = false;
        loop {
            let input = wal_payload_common::input(0);
            if input.pressed(wal_protocol::button::CARD) {
                crate::bngrw::insert(card, Duration::from_secs(3));
            }
            let start = input.pressed(wal_protocol::button::START);
            if auto && start && !start_held {
                // the attract screen reads it, then the game data screen some seconds after
                // start asks for it again
                crate::bngrw::insert(card, Duration::from_secs(25));
                log!("exvs2: card inserted automatically");
            }
            start_held = start;
            std::thread::sleep(Duration::from_millis(50));
        }
    });

    // the cabinet authentication and the game server
    crate::amcus::set_identity(crate::amcus::Identity { game_id: GAME_ID, game_cd: GAME_CD, game_ver: (4, 0), serial: role().serial, scheme: "", path: "", net_id: role().net_id });
    crate::amcus::set_cacfg_version(CACFG_VER);
    crate::amcus::set_place_id("JPN1");
    crate::amcus::set_cabinet_network(["192.168.85.1", "127.0.0.1", "192.168.85.11", "255.255.255.0", "192.168.85.1", "192.168.85.1"]);
    match unsafe { iat::hook_module(base, "ole32.dll", "CoCreateInstance", co_create_instance as *const () as usize) } {
        Some(o) => CO_CREATE_INSTANCE.store(o, Ordering::Relaxed),
        None => log!("exvs2: CoCreateInstance not hooked"),
    }
    let verbose = std::env::var("WAL_NAMCOES3_SERVER_LOG").is_ok_and(|v| v == "1");
    let name = std::env::var("WAL_NAMCOES3_PROFILE_NAME").ok().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "PLAYER".into());
    match (descriptor(base, b"\n\x15game_server_nue.proto\x12"), descriptor(base, b"\n\x1cmatchmaking_server_nue.proto\x12")) {
        (Some(game), Some(mms)) => wal_namcoes3srv::exvs2::start(&crate::amcus::game_server(), game, mms, SERVER_DIR, &name, |line| log!("{line}"), verbose),
        _ => log!("exvs2: protocol descriptors not found, no game server"),
    }
}

/// A protocol file descriptor of the game (`start`: its first bytes, the file name), from its start
/// to the end of the executable's section (the parser stops at its end).
fn descriptor(base: usize, start: &[u8]) -> Option<&'static [u8]> {
    // the sections one by one (the gaps between them are not mapped)
    let pe = base + unsafe { *((base + 0x3c) as *const u32) } as usize;
    let sections = unsafe { *((pe + 6) as *const u16) } as usize;
    let optional = unsafe { *((pe + 20) as *const u16) } as usize;
    for i in 0..sections {
        let s = pe + 24 + optional + i * 40;
        let (vsize, va) = unsafe { (*((s + 8) as *const u32) as usize, *((s + 12) as *const u32) as usize) };
        let data = unsafe { std::slice::from_raw_parts((base + va) as *const u8, vsize) };
        if let Some(at) = data.windows(start.len()).position(|w| w == start) {
            return Some(&data[at..]);
        }
    }
    None
}
