//! In-process replacement of the cabinet's AMCUS auth daemon (`AMCUS/AMAuthd.exe`).
//!
//! The game reaches the daemon through the out-of-process COM interface `IAuth`
//! (`CoCreateInstance` of CLSID {4603BB03-058D-43D9-B96F-639BE908C1ED}, local server). Without
//! it the game raises an error at boot and treats the network as down: the Banapass card is
//! refused ("you can't use the Banapassport card at this time"). This object reports a
//! standalone cabinet that passed ALL.Net and MUCHA authentication.
//!
//! The method order, structure layouts and state values follow bananatools' `amcus/iauth.c`
//! (public domain), with MKDX's identity (game SBZB / MK31). Each method's stack arguments
//! match the game's 32-bit calls: one pointer (or one dword) after `this`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, Ordering};

use wal_payload_common::log;

type P = *mut c_void;

pub(crate) const CLSID: [u8; 16] = [
    0x03, 0xbb, 0x03, 0x46, 0x8d, 0x05, 0xd9, 0x43, 0xb9, 0x6f, 0x63, 0x9b, 0xe9, 0x08, 0xc1, 0xed,
];

const S_OK: i32 = 0;

const GAME_ID: &str = "SBZB";
const GAME_CD: &str = "MK31";
const GAME_VER: &str = "1.10";
const CACFG_VER: (i16, i16) = (18, 16);
const CACFG_VER_STR: &str = "18.16";

// daemon_state
const DAEMON_INIT: i32 = 5;
const DAEMON_AUTH_START: i32 = 6;
const DAEMON_DL: i32 = 9;
const DAEMON_IDLE: i32 = 15;

/// The daemon state reported by `GetUpdaterState`, advanced on each call like the real daemon.
static STATE: AtomicI32 = AtomicI32::new(DAEMON_DL);

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
/// `Initialize(mode)`: already initialized.
unsafe extern "system" fn initialize(_this: P, mode: u32) -> i32 {
    log!("amcus: IAuth::Initialize({mode})");
    1
}
unsafe extern "system" fn finalize(_this: P) -> i32 {
    S_OK
}

/// `GetUpdaterState(amcus_state*)`, 200 bytes: ALL.Net idle and authenticated, MUCHA
/// authenticated with nothing to download, standalone cabinet.
unsafe extern "system" fn get_updater_state(_this: P, out: *mut u8) -> i32 {
    if out.is_null() {
        return S_OK;
    }
    let state = STATE.load(Ordering::Relaxed);
    let next = match state {
        DAEMON_INIT => DAEMON_AUTH_START,
        DAEMON_AUTH_START => DAEMON_IDLE,
        DAEMON_IDLE => DAEMON_DL,
        s => s,
    };
    STATE.store(next, Ordering::Relaxed);
    unsafe {
        std::ptr::write_bytes(out, 0, 200);
        // allnet_state
        put(out, 0, DAEMON_IDLE);
        put(out, 8, 2i32); // AUTH_OK
        put(out, 12, 1i32); // auth count
        // mucha_state (offset 24)
        let m = 24;
        put(out, m, DAEMON_DL);
        put(out, m + 8, 2i32); // AUTH_OK
        put(out, m + 12, 1i32);
        put(out, m + 16, 27i32); // DLEXEC_PROC
        put(out, m + 20, 32i32); // DLSTEP_IDLE
        put(out, m + 24, 43i32); // DLLAN_DISABLE
        put(out, m + 28, 52i32); // DLWAN_COMPLETE
        put(out, m + 32, 59i32); // DAEMON_IO_NONE
        put(out, m + 36, CACFG_VER.0); // cacfg version
        put(out, m + 38, CACFG_VER.1);
        put(out, m + 40, CACFG_VER.0); // application version
        put(out, m + 42, CACFG_VER.1);
        put(out, m + 96, 1i32); // download check complete
        put(out, 168, 1i64); // clock status
        put(out, 184, 1u32); // AUTH_TYPE_ALLNET
        put(out, 188, 3u32); // DAEMON_MODE_STANDALONE
        put(out, 192, state);
    }
    S_OK
}

/// `GetCabinetConfig(amcus_network_state*)`, 168 bytes.
unsafe extern "system" fn get_cabinet_config(_this: P, out: *mut u8) -> i32 {
    if out.is_null() {
        return S_OK;
    }
    unsafe {
        std::ptr::write_bytes(out, 0, 168);
        put_str(out, 0, 16, "STANDALONE");
        put_str(out, 16, 16, "ABGN6789012"); // PCB id
        put_str(out, 32, 16, "271013020001"); // dongle serial
        for (i, ip) in ["192.168.112.1", "192.168.112.1", "192.168.112.11", "255.255.255.0", "192.168.112.1", "192.168.112.1"]
            .iter()
            .enumerate()
        {
            put_str(out, 48 + 16 * i, 16, ip); // router, auth server, local, mask, gateway, DNS
        }
        put(out, 144, 1i32); // hop count
        put(out, 148, 1u32); // TYPE_FFT
        // line, content router, shop (venue) router and hop checks: STATE_OK
        for off in [152, 156, 160, 164] {
            put(out, off, 1u32);
        }
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
        put_str(out, 20, 8, GAME_ID);
        put_str(out, 28, 8, GAME_VER);
        put_str(out, 36, 8, GAME_CD);
        put_str(out, 44, 8, CACFG_VER_STR);
        put_str(out, 52, 4, "0");
        put_str(out, 56, 4, "PCB");
        put_str(out, 60, 256, "localhost");
    }
    S_OK
}

/// `GetAuthServerResp(amcus_auth_server_resp*)`, 2210 bytes: the ALL.Net power-on answer.
unsafe extern "system" fn get_auth_server_resp(_this: P, out: *mut u8) -> i32 {
    if out.is_null() {
        return S_OK;
    }
    unsafe {
        std::ptr::write_bytes(out, 0, 2210);
        put_str(out, 0, 257, "localhost");
        put_str(out, 257, 257, "localhost");
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

/// `GetMuchaAuthResponse(mucha_boardauth_resp*)`, 3808 bytes: the shop the board is placed in.
unsafe extern "system" fn get_mucha_auth_response(_this: P, out: *mut u8) -> i32 {
    if out.is_null() {
        return S_OK;
    }
    unsafe {
        std::ptr::write_bytes(out, 0, 3808);
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
        put_str(out, 3800, 8, "0"); // force boot
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

/// Converts the ALL.Net answer to MUCHA information (no argument).
unsafe extern "system" fn convert(_this: P) -> i32 {
    S_OK
}
/// Called with a dword after a download-mode change.
unsafe extern "system" fn set_mode(_this: P, _mode: u32) -> i32 {
    S_OK
}

static VTABLE: Vtable = Vtable([
    query as *const (),
    refs as *const (),
    refs as *const (),
    initialize as *const (),
    finalize as *const (),
    unused as *const (), // 5
    unused as *const (),
    unused as *const (),
    unused as *const (),
    get_updater_state as *const (), // 9
    get_cabinet_config as *const (), // 10
    get_version_info as *const (), // 11
    unused as *const (),
    unused as *const (),
    get_auth_server_resp as *const (), // 14
    unused as *const (),
    unused as *const (),
    unused as *const (),
    get_mucha_auth_response as *const (), // 18
    get_mucha_updater_state as *const (), // 19
    convert as *const (), // 20
    unused as *const (),
    unused as *const (),
    set_mode as *const (), // 23
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
