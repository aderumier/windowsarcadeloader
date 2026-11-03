//! In-process replacement of the cabinet's AMCUS auth daemon (`AMCUS/AMAuthd.exe`).
//!
//! The game reaches the daemon through the out-of-process COM interface `IAuth`
//! (`CoCreateInstance` of CLSID {4603BB03-058D-43D9-B96F-639BE908C1ED}, local server). Without
//! it the game raises an error at boot and treats the network as down: the Banapass card is
//! refused ("you can't use the Banapassport card at this time"). This object reports a
//! standalone cabinet that passed ALL.Net and MUCHA authentication.
//!
//! The structure layouts follow bananatools' `amcus/iauth.c` (public domain). Three IAuth
//! versions, the game's identity set by its module ([`set_identity`]; Wangan Midnight Maximum
//! Tune 5's version table otherwise):
//! - [`OBJECT`]: Wangan Midnight Maximum Tune 5 / 5DX / 5DX+ / 6 and Time Crisis 5, the older
//!   one: bananatools' slots minus 5, a shorter state (`GetInfo`), cabinet config and MUCHA
//!   answer.
//! - [`OBJECT_TEKKEN`]: Tekken 7, between them: bananatools' slots minus 2 from the state on.
//! - [`OBJECT_NEW`]: 6R / 6RR, bananatools' own.
//!
//! The ALL.Net answer's `uri` is the game server (`https://` + `WAL_NAMCOES3_GAME_SERVER`,
//! `host:port`, default `127.0.0.1:9002`).

use std::ffi::c_void;

type P = *mut c_void;

pub(crate) const CLSID: [u8; 16] = [
    0x03, 0xbb, 0x03, 0x46, 0x8d, 0x05, 0xd9, 0x43, 0xb9, 0x6f, 0x63, 0x9b, 0xe9, 0x08, 0xc1, 0xed,
];

const S_OK: i32 = 0;

const GAME_ID: &str = "SBWJ";

/// The game the daemon answers for (Wangan Midnight Maximum Tune 5's version table otherwise).
#[derive(Clone, Copy)]
pub(crate) struct Identity {
    pub(crate) game_id: &'static str,
    pub(crate) game_cd: &'static str,
    pub(crate) game_ver: (i16, i16),
    pub(crate) serial: &'static str,
    /// Put before the server in the ALL.Net answer's uri (Time Crisis 5's launcher adds
    /// `https://` itself).
    pub(crate) scheme: &'static str,
    /// Put after the server in the uri (Pokkén Tournament parses `https://host:port/path`).
    pub(crate) path: &'static str,
}

static IDENTITY: std::sync::OnceLock<Identity> = std::sync::OnceLock::new();

pub(crate) fn set_identity(identity: Identity) {
    let _ = IDENTITY.set(identity);
}

fn identity() -> Identity {
    IDENTITY.get().copied().unwrap_or_else(|| {
        let v = crate::wmmt5::current();
        Identity { game_id: GAME_ID, game_cd: v.game_cd, game_ver: v.game_ver, serial: v.serial, scheme: "https://", path: "" }
    })
}


#[repr(C)]
pub(crate) struct Object {
    vtable: *const *const (),
}
unsafe impl Sync for Object {}
struct Vtable([*const (); 34]);
unsafe impl Sync for Vtable {}

/// Writes `s` (NUL-terminated, truncated to `len - 1`) at `buf + off`.
unsafe fn put_str(buf: *mut u8, off: usize, len: usize, s: &str) {
    let bytes = s.as_bytes();
    let n = bytes.len().min(len - 1);
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.add(off), n);
        *buf.add(off + n) = 0;
    }
}

unsafe fn put<T>(buf: *mut u8, off: usize, v: T) {
    unsafe { (buf.add(off) as *mut T).write_unaligned(v) };
}

unsafe extern "system" fn query(this: P, _iid: P, out: *mut P) -> i32 {
    if out.is_null() {
        return 0x80004003u32 as i32;
    }
    unsafe { *out = this };
    S_OK
}
unsafe extern "system" fn refs(_this: P) -> u32 {
    1
}
/// Methods the game does not call.
unsafe extern "system" fn unused(_this: P) -> i32 {
    S_OK
}
unsafe extern "system" fn finalize(_this: P) -> i32 {
    S_OK
}

/// `GetInfo(state*)` (slot 4), at most 0x90 bytes (the game's stack cookie follows): ALL.Net
/// and MUCHA idle (14) and authenticated (2, one authentication), the cacfg and application
/// versions, no offline flag (+0x8c).
unsafe extern "system" fn get_info(_this: P, out: *mut u8) -> i32 {
    if out.is_null() {
        return S_OK;
    }
    unsafe {
        std::ptr::write_bytes(out, 0, 0x90);
        // allnet: state, error, auth state, auth count
        put(out, 0x00, 14u32);
        put(out, 0x08, 2i32);
        put(out, 0x0c, 1i32);
        // mucha
        put(out, 0x18, 14u32);
        put(out, 0x20, 2i32);
        put(out, 0x24, 1i32);
        let (major, minor) = identity().game_ver;
        put(out, 0x3c, major);
        put(out, 0x3e, minor);
        put(out, 0x40, major);
        put(out, 0x42, minor);
    }
    S_OK
}

/// `GetCabinetConfig(config*)` (slot 5), 88 bytes: mode, PCB id, the dongle serial (+0x20, the
/// game compares it with its own), the shop router and auth server, the hop count (u16 +0x50),
/// the line type (+0x54, 1: B FLET'S).
unsafe extern "system" fn get_cabinet_config(_this: P, out: *mut u8) -> i32 {
    if out.is_null() {
        return S_OK;
    }
    unsafe {
        std::ptr::write_bytes(out, 0, 88);
        put_str(out, 0x00, 16, "STANDALONE");
        put_str(out, 0x10, 16, "ABGN6789012");
        put_str(out, 0x20, 16, identity().serial);
        put_str(out, 0x30, 16, "127.0.0.1");
        put_str(out, 0x40, 16, "127.0.0.1");
        put(out, 0x50, 1u16);
        put(out, 0x54, 1u32);
    }
    S_OK
}

/// `GetVersionInfo(amcus_version_info*)`, 316 bytes.
unsafe extern "system" fn get_version_info(_this: P, out: *mut u8) -> i32 {
    if out.is_null() {
        return S_OK;
    }
    unsafe {
        std::ptr::write_bytes(out, 0, 316);
        put_str(out, 0, 4, "1");
        put_str(out, 4, 16, "ALL.NET");
        let version = identity();
        put_str(out, 20, 8, version.game_id);
        let ver = format!("{}.{:02}", version.game_ver.0, version.game_ver.1);
        put_str(out, 28, 8, &ver);
        put_str(out, 36, 8, version.game_cd);
        put_str(out, 44, 8, &ver);
        put_str(out, 52, 4, "0");
        put_str(out, 56, 4, "PCB");
        put_str(out, 60, 256, "localhost");
    }
    S_OK
}

/// The game server, `host:port`.
pub(crate) fn game_server() -> String {
    std::env::var("WAL_NAMCOES3_GAME_SERVER").unwrap_or_else(|_| "127.0.0.1:9002".into())
}

/// `GetAuthServerResp(amcus_auth_server_resp*)`, 2210 bytes: the ALL.Net power-on answer.
unsafe extern "system" fn get_auth_server_resp(_this: P, out: *mut u8) -> i32 {
    if out.is_null() {
        return S_OK;
    }
    unsafe {
        std::ptr::write_bytes(out, 0, 2210);
        let server = game_server();
        put_str(out, 0, 257, &format!("{}{server}{}", identity().scheme, identity().path));
        put_str(out, 257, 257, &server);
        put_str(out, 514, 256, "Arcade");
        put_str(out, 770, 256, "Arcade");
        put_str(out, 1026, 16, "1");
        for (i, name) in ["W", "X", "Y", "Z"].iter().enumerate() {
            put_str(out, 1042 + 256 * i, 256, name);
        }
        put_str(out, 2066, 16, "123");
        put_str(out, 2082, 16, "1");
        put_str(out, 2098, 16, "JPN");
        put_str(out, 2114, 32, "+0900");
        put_str(out, 2146, 64, "PowerOnResponseVer2");
    }
    S_OK
}

/// `GetMuchaAuthResponse(mucha_boardauth_resp*)` (slot 13), 3800 bytes (this version has no
/// force boot field): the shop the board is placed in.
unsafe extern "system" fn get_mucha_auth_response(_this: P, out: *mut u8) -> i32 {
    if out.is_null() {
        return S_OK;
    }
    unsafe {
        std::ptr::write_bytes(out, 0, 3800);
        for (i, url) in ["charge", "file", "url1", "url2", "url3"].iter().enumerate() {
            put_str(out, 256 * i, 256, &format!("https://localhost/{url}/"));
        }
        put_str(out, 1280, 16, "JPN123"); // place id
        put_str(out, 1296, 16, "JPN"); // country
        put_str(out, 1312, 256, "Arcade"); // shop name
        put_str(out, 1568, 128, "Arcade");
        for (i, area) in ["008", "009", "010", "011"].iter().enumerate() {
            put_str(out, 1696 + 64 * i, 64, area);
            put_str(out, 3072 + 32 * i, 32, area);
        }
        put_str(out, 2976, 64, "Arcade");
        put_str(out, 3040, 32, "Arcade");
        put_str(out, 3712, 16, "1"); // prefecture
        put_str(out, 3728, 16, "null"); // expiration date
        put_str(out, 3744, 16, "0"); // use token
        put_str(out, 3760, 32, "0"); // consume token
        put_str(out, 3792, 8, "1"); // dongle flag
    }
    S_OK
}

/// `mucha_updater_state*`, 56 bytes: state 1, or the game reports the board unauthenticated.
unsafe extern "system" fn get_mucha_updater_state(_this: P, out: *mut u8) -> i32 {
    if !out.is_null() {
        unsafe {
            std::ptr::write_bytes(out, 0, 56);
            put(out, 0, 1u64);
        }
    }
    1
}

/// RequestAuthMucha, StopDownload (no argument).
unsafe extern "system" fn convert(_this: P) -> i32 {
    S_OK
}
/// InitAllnet, InitMucha, StartDownload (one argument).
unsafe extern "system" fn set_mode(_this: P, _mode: u32) -> i32 {
    S_OK
}

/// This game's IAuth: bananatools' slots minus 5.
static VTABLE: Vtable = Vtable([
    query as *const (),
    refs as *const (),
    refs as *const (),
    unused as *const (),
    get_info as *const (), // 4
    get_cabinet_config as *const (), // 5
    get_version_info as *const (), // 6
    set_mode as *const (), // 7: InitAllnet(0x1e)
    finalize as *const (), // 8: FinAllnet
    get_auth_server_resp as *const (), // 9
    unused as *const (),
    set_mode as *const (), // 11: InitMucha(0)
    finalize as *const (), // 12: FinMucha
    get_mucha_auth_response as *const (), // 13
    get_mucha_updater_state as *const (), // 14
    convert as *const (), // 15: RequestAuthMucha
    unused as *const (),
    unused as *const (),
    set_mode as *const (), // 18: StartDownload(0)
    convert as *const (), // 19: StopDownload
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
]);

pub(crate) static OBJECT: Object = Object { vtable: VTABLE.0.as_ptr() };

/// The newer IAuth's daemon state (bananatools' `amauthd_state`): `DL` (9), `INIT` (5) once
/// initialized, then `AUTH_START` (6), `IDLE` (15), `DL` at the next state reads.
static DAEMON_STATE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(9);

/// `Initialize(mode)`, `Finalize()` (newer IAuth): the daemon started again (`INIT`, 5), stopped
/// (0: Pokkén Tournament's re-initialization waits for it before `Initialize`).
unsafe extern "system" fn initialize_new(_this: P, _mode: u64) -> i32 {
    DAEMON_STATE.store(5, std::sync::atomic::Ordering::Relaxed);
    S_OK
}

unsafe extern "system" fn finalize_new(_this: P) -> i32 {
    DAEMON_STATE.store(0, std::sync::atomic::Ordering::Relaxed);
    S_OK
}

/// `GetUpdaterState(amcus_state*)` (newer IAuth, slot 9), 200 bytes: ALL.Net idle (15) and
/// authenticated, MUCHA downloading (9) with nothing to download (download step idle, LAN off,
/// WAN complete, check complete), the clock set, ALL.Net authentication, standalone cabinet.
unsafe extern "system" fn get_updater_state_new(_this: P, out: *mut u8) -> i32 {
    if out.is_null() {
        return S_OK;
    }
    use std::sync::atomic::Ordering::Relaxed;
    let state = DAEMON_STATE.load(Relaxed);
    let next = match state {
        5 => 6,
        6 => 15,
        15 => 9,
        s => s,
    };
    DAEMON_STATE.store(next, Relaxed);
    unsafe {
        std::ptr::write_bytes(out, 0, 200);
        // allnet: state, error, auth state, auth count
        put(out, 0, 15u32);
        put(out, 8, 2i32);
        put(out, 12, 1i32);
        // mucha (+24): state, auth state, count, download exec / step / LAN / WAN, I/O
        let m = 24;
        put(out, m, 9u32);
        put(out, m + 8, 2i32);
        put(out, m + 12, 1i32);
        put(out, m + 16, 27i32);
        put(out, m + 20, 32i32);
        put(out, m + 24, 43i32);
        put(out, m + 28, 52i32);
        put(out, m + 32, 59i32);
        let (major, minor) = identity().game_ver;
        put(out, m + 36, major);
        put(out, m + 38, minor);
        put(out, m + 40, major);
        put(out, m + 42, minor);
        put(out, m + 96, 1i32);
        // clock status, auth type ALL.Net, standalone, daemon state
        put(out, 168, 1i64);
        put(out, 184, 1u32);
        put(out, 188, 3u32);
        put(out, 192, state);
    }
    S_OK
}

/// `GetCabinetConfig(amcus_network_state*)` (newer IAuth, slot 10), 168 bytes: the older
/// config's fields, the cabinet's address, mask, gateway and DNS, the hop count (+144) and line
/// type (+148, 1: FTTH).
unsafe extern "system" fn get_cabinet_config_new(_this: P, out: *mut u8) -> i32 {
    if out.is_null() {
        return S_OK;
    }
    unsafe {
        std::ptr::write_bytes(out, 0, 168);
        put_str(out, 0x00, 16, "STANDALONE");
        put_str(out, 0x10, 16, "ABGN6789012");
        put_str(out, 0x20, 16, identity().serial);
        for (i, ip) in ["127.0.0.1", "127.0.0.1", "127.0.0.1", "255.0.0.0", "127.0.0.1", "127.0.0.1"].iter().enumerate() {
            put_str(out, 0x30 + 16 * i, 16, ip);
        }
        put(out, 144, 1i32);
        put(out, 148, 1u32);
    }
    S_OK
}

/// `GetMuchaAuthResponse(mucha_boardauth_resp*)` (newer IAuth, slot 18), 3808 bytes: the older
/// answer and no force boot (+3800).
unsafe extern "system" fn get_mucha_auth_response_new(this: P, out: *mut u8) -> i32 {
    if out.is_null() {
        return S_OK;
    }
    unsafe {
        get_mucha_auth_response(this, out);
        put_str(out, 3800, 8, "0");
    }
    S_OK
}

/// The newer IAuth (6R): bananatools' slots.
static VTABLE_NEW: Vtable = Vtable([
    query as *const (),
    refs as *const (),
    refs as *const (),
    initialize_new as *const (), // 3
    finalize_new as *const (), // 4
    set_mode as *const (), // 5
    unused as *const (),
    unused as *const (),
    unused as *const (),
    get_updater_state_new as *const (), // 9
    get_cabinet_config_new as *const (), // 10
    get_version_info as *const (), // 11
    unused as *const (),
    unused as *const (),
    get_auth_server_resp as *const (), // 14
    unused as *const (),
    unused as *const (),
    unused as *const (),
    get_mucha_auth_response_new as *const (), // 18
    get_mucha_updater_state as *const (), // 19
    convert as *const (), // 20: RequestAuthMucha
    convert as *const (), // 21: consume charge
    unused as *const (),
    set_mode as *const (), // 23: StartDownload(0)
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
]);

pub(crate) static OBJECT_NEW: Object = Object { vtable: VTABLE_NEW.0.as_ptr() };

/// `GetState(state*)` (Tekken 7's IAuth, slot 7), 184 bytes: ALL.Net and MUCHA idle (14) and
/// authenticated (2, one authentication), the versions, the clock set (+168), the daemon idle
/// (+176: the game waits for 14 after `Initialize`) without error (+180).
unsafe extern "system" fn get_state_tekken(_this: P, out: *mut u8) -> i32 {
    if out.is_null() {
        return S_OK;
    }
    unsafe {
        std::ptr::write_bytes(out, 0, 184);
        put(out, 0, 14u32);
        put(out, 8, 2i32);
        put(out, 12, 1i32);
        let m = 24;
        put(out, m, 14u32);
        put(out, m + 8, 2i32);
        put(out, m + 12, 1i32);
        put(out, m + 16, 27i32);
        put(out, m + 20, 32i32);
        put(out, m + 24, 43i32);
        put(out, m + 28, 52i32);
        put(out, m + 32, 59i32);
        let (major, minor) = identity().game_ver;
        put(out, m + 36, major);
        put(out, m + 38, minor);
        put(out, m + 40, major);
        put(out, m + 42, minor);
        put(out, m + 96, 1i32);
        put(out, 168, 1i64);
        put(out, 176, 14u32);
    }
    S_OK
}

/// `GetCabinetConfig(config*)` (Tekken 7's IAuth, slot 8), 152 bytes: the newer config without
/// its link status fields.
unsafe extern "system" fn get_cabinet_config_tekken(this: P, out: *mut u8) -> i32 {
    if out.is_null() {
        return S_OK;
    }
    let mut full = [0u8; 168];
    unsafe {
        get_cabinet_config_new(this, full.as_mut_ptr());
        std::ptr::copy_nonoverlapping(full.as_ptr(), out, 152);
    }
    S_OK
}

/// Tekken 7's IAuth (2015): the newer one's slots minus 2 from the state on, a shorter state
/// and cabinet config, the older MUCHA answer.
static VTABLE_TEKKEN: Vtable = Vtable([
    query as *const (),
    refs as *const (),
    refs as *const (),
    set_mode as *const (), // 3: Initialize(5)
    unused as *const (),
    unused as *const (),
    unused as *const (),
    get_state_tekken as *const (), // 7
    get_cabinet_config_tekken as *const (), // 8
    get_version_info as *const (), // 9
    unused as *const (),
    unused as *const (),
    get_auth_server_resp as *const (), // 12
    unused as *const (),
    unused as *const (),
    unused as *const (),
    get_mucha_auth_response as *const (), // 16
    get_mucha_updater_state as *const (), // 17
    convert as *const (), // 18
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
    unused as *const (),
]);

pub(crate) static OBJECT_TEKKEN: Object = Object { vtable: VTABLE_TEKKEN.0.as_ptr() };

/// The object a traced daemon forwards to (`WAL_NAMCOES3_AMCUS_TRACE=1`).
static TRACED: std::sync::atomic::AtomicPtr<Object> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

/// Slot `N` of the traced daemon: logs the call (once per slot), then the real object's slot.
unsafe extern "system" fn traced<const N: usize>(_this: P, a: usize, b: usize, c: usize, d: usize) -> i32 {
    static SEEN: std::sync::Mutex<u64> = std::sync::Mutex::new(0);
    {
        let mut seen = SEEN.lock().unwrap();
        if *seen & (1 << N) == 0 {
            *seen |= 1 << N;
            wal_payload_common::log!("amcus: slot {N} ({a:#x}, {b:#x})");
        }
    }
    let object = TRACED.load(std::sync::atomic::Ordering::Relaxed);
    let f: unsafe extern "system" fn(P, usize, usize, usize, usize) -> i32 = unsafe { std::mem::transmute(*(*object).vtable.add(N)) };
    unsafe { f(object.cast(), a, b, c, d) }
}

macro_rules! traced_vtable {
    ($($n:literal)*) => { Vtable([$(traced::<$n> as *const (),)*]) };
}

static VTABLE_TRACED: Vtable = traced_vtable!(0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33);
static OBJECT_TRACED: Object = Object { vtable: VTABLE_TRACED.0.as_ptr() };

/// `object`, or with `WAL_NAMCOES3_AMCUS_TRACE=1` a wrapper logging each slot's first call.
pub(crate) fn daemon(object: &'static Object) -> &'static Object {
    if std::env::var("WAL_NAMCOES3_AMCUS_TRACE").is_ok_and(|v| v == "1") {
        TRACED.store((object as *const Object).cast_mut(), std::sync::atomic::Ordering::Relaxed);
        return &OBJECT_TRACED;
    }
    object
}
