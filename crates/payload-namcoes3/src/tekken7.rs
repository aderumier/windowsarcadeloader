//! Tekken 7 (2015, Unreal Engine 4, 64-bit): v1.12 (`TekkenGame-Win64-Shipping_612.exe`, PE
//! time stamp 0x566C3FBD), Tekken 7 Fated Retribution (2016,
//! `TekkenGame-Win64-Shipping.exe`, 0x573CBE0E) and Fated Retribution Round 2 (2019, 0x5CDB8E5A).
//! The game imports no cabinet library: the payload is installed as
//! the `xinput1_3.dll` beside it, whose ordinals 2 / 3 the game imports (neutral pads, see
//! [`crate::game_pads_hidden`]).
//!
//! - I/O board: the Namco JVS board on COM3 (`jvs.rs`, its Namco commands `70`), two players
//!   of 16 switches (the stick, punches and kicks on buttons 1-4).
//! - network: the cabinet authentication (AMCUS, ALL.Net) is not there; the game's online check
//!   answers offline and the error list it fills at boot (no network, no server) stays empty
//!   (the same code in every build, at other addresses: [`BUILDS`]).
//! - the game's keyboard and raw input (developer controls): from the launcher only.
//! - the engine's usage reports (`wininet`, `etsource.epicgames.com`): no connection.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use wal_payload_common::log;

use crate::patches;

const GAME_PREFIX: &str = "TekkenGame-Win64-Shipping";

/// A build: its name, PE time stamp, the online check, the two pushes to the boot error list,
/// its ALL.Net identity (game id, MUCHA game code, version).
struct Build {
    name: &'static str,
    timestamp: u32,
    online: usize,
    errors: [usize; 2],
    game_cd: &'static str,
    game_ver: (i16, i16),
    /// The game's own printf-like debug log `(format, ...)`, silent in this build
    /// (`WAL_NAMCOES3_GAMELOG=1`: to the payload log), 0: unknown.
    gamelog: usize,
    /// Its AMCUS auth daemon (`IAuth` version, `amcus.rs`).
    amcus: &'static crate::amcus::Object,
}

const BUILDS: &[Build] = &[
    Build { name: "v1.12", timestamp: 0x566C3FBD, online: 0x147380, errors: [0x16bc41, 0x179050], game_cd: "TE76", game_ver: (1, 0), gamelog: 0x223aa0, amcus: &crate::amcus::OBJECT_TEKKEN },
    Build { name: "Fated Retribution", timestamp: 0x573CBE0E, online: 0x204af0, errors: [0x25e791, 0x268e00], game_cd: "TER1", game_ver: (1, 50), gamelog: 0x3319f0, amcus: &crate::amcus::OBJECT_NEW },
    Build { name: "Fated Retribution Round 2", timestamp: 0x5CDB8E5A, online: 0x1ed080, errors: [0x244e31, 0x24d8c0], game_cd: "TR21", game_ver: (1, 0), gamelog: 0x32d470, amcus: &crate::amcus::OBJECT_NEW },
];

/// The ALL.Net game id of both builds.
const GAME_ID: &str = "SDBS";
/// The cabinet's dongle serial.
const SERIAL: &str = "282814450001";
/// The player's card, made once: in the dump's save folder (`c_folders`).
const CARD_FILE: &str = "C:\\perforce\\tk7\\master\\ue4\\TekkenGame\\Saved\\banapass.txt";

// (format, ...): the arguments after the format spilled to their home slots, a `va_list` of them.
core::arch::global_asm!(
    ".globl {name}",
    "{name}:",
    "mov [rsp + 16], rdx",
    "mov [rsp + 24], r8",
    "mov [rsp + 32], r9",
    "lea rdx, [rsp + 16]",
    "sub rsp, 40",
    "call {log}",
    "add rsp, 40",
    "ret",
    name = sym gamelog_stub,
    log = sym gamelog_va,
);

unsafe extern "C" {
    fn gamelog_stub();
    fn _vsnprintf(buf: *mut std::ffi::c_char, size: usize, fmt: *const std::ffi::c_char, args: *const c_void) -> i32;
}

extern "C" fn gamelog_va(fmt: *const std::ffi::c_char, args: *const c_void) {
    if fmt.is_null() {
        return;
    }
    let mut buf = [0 as std::ffi::c_char; 1024];
    unsafe { _vsnprintf(buf.as_mut_ptr(), buf.len() - 1, fmt, args) };
    let msg = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
    log!("game: {}", msg.trim_end());
}

/// The game's debug log to the payload log: a jump to [`gamelog_stub`] at `base + rva`.
fn gamelog(base: usize, rva: usize) {
    let at = base + rva;
    let mut code = [0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xE0];
    code[2..10].copy_from_slice(&(gamelog_stub as usize as u64).to_le_bytes());
    patches::write(at, &code);
    log!("tekken7: game log at {rva:#x}");
}

/// The game server's players: in the dump's save folder.
const SERVER_DIR: &str = "C:\\perforce\\tk7\\master\\ue4\\TekkenGame\\Saved\\graphene";

/// The game's `graphene.proto` file descriptor (its protocol), from its start to the end of the
/// executable's image.
fn descriptor(base: usize) -> Option<&'static [u8]> {
    const START: &[u8] = b"\n\x0egraphene.proto\x12";
    let image = unsafe {
        let pe = base + *((base + 0x3c) as *const u32) as usize;
        let size = *((pe + 0x50) as *const u32) as usize;
        std::slice::from_raw_parts(base as *const u8, size)
    };
    let at = image.windows(START.len()).position(|w| w == START)?;
    Some(&image[at..])
}

/// The build's AMCUS auth daemon.
static AMCUS: std::sync::OnceLock<&'static crate::amcus::Object> = std::sync::OnceLock::new();

static CO_CREATE_INSTANCE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The AMCUS auth daemon (`IAuth`), in-process (`amcus.rs`).
unsafe extern "system" fn co_create_instance(clsid: *const u8, outer: *mut c_void, context: u32, iid: *const u8, out: *mut *mut c_void) -> i32 {
    if !clsid.is_null() && unsafe { std::slice::from_raw_parts(clsid, 16) } == crate::amcus::CLSID {
        if !out.is_null() {
            let object = crate::amcus::daemon(AMCUS.get().copied().unwrap_or(&crate::amcus::OBJECT_TEKKEN));
            unsafe { *out = (object as *const crate::amcus::Object).cast_mut().cast() };
        }
        log!("tekken7: in-process AMCUS auth (IAuth)");
        return 0;
    }
    type F = unsafe extern "system" fn(*const u8, *mut c_void, u32, *const u8, *mut *mut c_void) -> i32;
    let real: F = unsafe { std::mem::transmute(CO_CREATE_INSTANCE.load(Ordering::Relaxed)) };
    unsafe { real(clsid, outer, context, iid, out) }
}

static CONNECT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// The game server's address (`host:port`), connections to its host on 443 sent to its port.
static SERVER: std::sync::OnceLock<std::net::SocketAddrV4> = std::sync::OnceLock::new();

/// `connect` (ws2_32 #4): Round 2's game client drops the ALL.Net uri's port (HTTPS on 443).
unsafe extern "system" fn connect(socket: usize, addr: *const u8, len: i32) -> i32 {
    type F = unsafe extern "system" fn(usize, *const u8, i32) -> i32;
    let real: F = unsafe { std::mem::transmute(CONNECT.load(Ordering::Relaxed)) };
    if let Some(server) = SERVER.get()
        && !addr.is_null()
        && len >= 16
    {
        let mut sin = [0u8; 16];
        unsafe { std::ptr::copy_nonoverlapping(addr, sin.as_mut_ptr(), 16) };
        // AF_INET, port 443, the server's host
        if sin[..4] == [2, 0, 0x01, 0xBB] && sin[4..8] == server.ip().octets() {
            sin[2..4].copy_from_slice(&server.port().to_be_bytes());
            return unsafe { real(socket, sin.as_ptr(), len) };
        }
    }
    unsafe { real(socket, addr, len) }
}

/// `WAL_NAMCOES3_BANAPASS=1`: online, the Banapass reader on COM4 (the player's card with the
/// `card` button), the AMCUS auth daemon in-process.
fn network(base: usize, build: &Build) {
    let _ = AMCUS.set(build.amcus);
    let card = crate::bpreader::player_card_in(CARD_FILE);
    crate::bpreader::card_button(card);
    if std::env::var("WAL_NAMCOES3_AUTO_CARD").is_ok_and(|v| v.trim() == "1") {
        crate::bpreader::card_on_start(card);
    }
    crate::bpreader::init("COM4", &[base]);
    crate::amcus::set_identity(crate::amcus::Identity { game_id: GAME_ID, game_cd: build.game_cd, game_ver: build.game_ver, serial: SERIAL, scheme: "https://", path: "" });
    match unsafe { wal_payload_common::iat::hook_module(base, "ole32.dll", "CoCreateInstance", co_create_instance as *const () as usize) } {
        Some(o) => CO_CREATE_INSTANCE.store(o, Ordering::Relaxed),
        None => log!("tekken7: CoCreateInstance not hooked"),
    }
    if let Ok(server) = crate::amcus::game_server().parse() {
        let _ = SERVER.set(server);
        match unsafe { wal_payload_common::iat::hook_ordinal(base, "ws2_32.dll", 4, connect as *const () as usize) } {
            Some(o) => CONNECT.store(o, Ordering::Relaxed),
            None => log!("tekken7: connect not hooked"),
        }
    }
    let verbose = std::env::var("WAL_NAMCOES3_SERVER_LOG").is_ok_and(|v| v == "1");
    let name = std::env::var("WAL_NAMCOES3_PROFILE_NAME").ok().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "PLAYER".into());
    match descriptor(base) {
        Some(d) => wal_namcoes3srv::graphene::start(&crate::amcus::game_server(), d, SERVER_DIR, &name, |line| log!("{line}"), verbose),
        None => log!("tekken7: graphene.proto descriptor not found, no game server"),
    }
}

/// `InternetConnectW`: none (Unreal Engine's usage reports to the Internet, the game's only
/// `wininet` use).
unsafe extern "system" fn internet_connect(_session: *mut c_void, _server: *const u16, _port: u16, _user: *const u16, _password: *const u16, _service: u32, _flags: u32, _context: usize) -> *mut c_void {
    unsafe { windows_sys::Win32::Foundation::SetLastError(12007) };
    std::ptr::null_mut()
}

static LOADED: AtomicBool = AtomicBool::new(false);

/// The payload runs in Tekken 7.
pub(crate) fn is_loaded() -> bool {
    LOADED.load(Ordering::Relaxed)
}

/// The game's executable (any build).
pub(crate) fn is_tekken7(exe: &str) -> bool {
    exe.len() >= GAME_PREFIX.len() && exe[..GAME_PREFIX.len()].eq_ignore_ascii_case(GAME_PREFIX)
}

pub(crate) fn attach(exe: &str, base: usize) {
    let stamp = crate::exe_timestamp(base);
    match BUILDS.iter().find(|b| b.timestamp == stamp) {
        None => log!("tekken7: {exe} build {stamp:#x} unknown, no patches"),
        Some(build) => {
            log!("tekken7: {}", build.name);
            if build.gamelog != 0 && std::env::var("WAL_NAMCOES3_GAMELOG").is_ok_and(|v| v == "1") {
                gamelog(base, build.gamelog);
            }
            // the online check (`authtype` == "OFFLINE", Fated Retribution's tekken.ini sets
            // it): offline, WAL_NAMCOES3_BANAPASS=1: online
            let online = std::env::var("WAL_NAMCOES3_BANAPASS").is_ok_and(|v| v == "1");
            let offline: u8 = if online { 0 } else { 1 };
            patches::patch("offline mode", base, build.online, b"\x4c\x8b\xdc\x57\x48\x83", &[0xB8, offline, 0, 0, 0, 0xC3]);
            if online {
                network(base, build);
            }
            // boot errors (network, server, ...): the push to the error list does not advance
            for (what, rva) in ["boot error list", "first check error"].into_iter().zip(build.errors) {
                patches::patch(what, base, rva, b"\x48\x83\x43\x08\x04", &[0x90; 5]);
            }
        }
    }
    unsafe { wal_payload_common::iat::hook_module(base, "wininet.dll", "InternetConnectW", internet_connect as *const () as usize) };
    LOADED.store(true, Ordering::Relaxed);
    crate::jvs::init(&[base]);
    crate::game_pads_hidden(base);
    wal_payload_common::input_filter::init(base, |_| false);
}
