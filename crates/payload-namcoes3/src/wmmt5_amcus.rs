//! In-process replacement of the cabinet's AMCUS auth daemon (`AMCUS/AMAuthd.exe`).
//!
//! The game reaches the daemon through the out-of-process COM interface `IAuth`
//! (`CoCreateInstance` of CLSID {4603BB03-058D-43D9-B96F-639BE908C1ED}, local server). Without
//! it the game raises an error at boot and treats the network as down: the Banapass card is
//! refused ("you can't use the Banapassport card at this time"). This object reports a
//! standalone cabinet that passed ALL.Net and MUCHA authentication.
//!
//! The structure layouts follow bananatools' `amcus/iauth.c` (public domain), with Wangan
//! Midnight Maximum Tune 5's identity (game SBWJ 3.00, PCB ABGN, its dongle serial). This game
//! uses an older IAuth: bananatools' slots minus 5, a shorter state (`GetInfo`), cabinet config
//! and MUCHA answer. The ALL.Net answer's `uri` is the game server
//! (`https://` + `WAL_NAMCOES3_GAME_SERVER`, `host:port`, default `127.0.0.1:9002`).

use std::ffi::c_void;

type P = *mut c_void;

pub(crate) const CLSID: [u8; 16] = [
    0x03, 0xbb, 0x03, 0x46, 0x8d, 0x05, 0xd9, 0x43, 0xb9, 0x6f, 0x63, 0x9b, 0xe9, 0x08, 0xc1, 0xed,
];

const S_OK: i32 = 0;

const GAME_ID: &str = "SBWJ";
const GAME_CD: &str = "W5R1";
const GAME_VER: &str = "3.00";
const CACFG_VER: (i16, i16) = (3, 0);
const CACFG_VER_STR: &str = "3.00";


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
        put(out, 0x3c, CACFG_VER.0);
        put(out, 0x3e, CACFG_VER.1);
        put(out, 0x40, CACFG_VER.0);
        put(out, 0x42, CACFG_VER.1);
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
        put_str(out, 0x20, 16, crate::wmmt5::SERIAL_STR);
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
        put_str(out, 0, 257, &format!("https://{server}"));
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
