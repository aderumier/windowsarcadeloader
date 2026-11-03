//! Wangan Midnight Maximum Tune 5 (2014), 5DX (2015) and 5DX+ (2016) (64-bit `wmn5r.exe`,
//! Direct3D 11): the payload is the game's HASP library, `hasp_windows_x64_109906.dll` (5),
//! `hasp_windows_x64_98199.dll` (5DX) or `hasp_windows_x64_106482.dll` (5DX+), its import: it
//! tells the [`Version`].
//!
//! * Dongle: the HASP memory with the version's cabinet serial (`hasp.rs`).
//! * I/O board: the game's own JVS code on COM3 (`jvs.rs`): wheel, pedals, shift lever,
//!   buttons.
//! * Boot (the cabinet's network): the dongle / network checks of the boot sequence pass
//!   ([`patches`]); the store's card terminal is emulated (`wmmt5_terminal.rs`).
//! * Force feedback: the game keeps the forces of its drive board (off in its settings, it is
//!   never sent) in memory, [`ffb`].
//!
//! The saves (`F:`, `G:`: settings, bookkeeping) are the profile's drives. Their first
//! settings ([`initial_settings`]) leave the absent cabinet devices off.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use wal_payload_common::{log, send_output};
use wal_protocol::output;

use crate::patches::patch;
use crate::{GAME_BASE, module};

/// A version of the game: its HASP library, cabinet identity and the addresses in its exe.
pub(crate) struct Version {
    pub(crate) name: &'static str,
    /// The game executable, its build (PE time stamp: 6 and 6R share the exe and HASP names).
    exe: &'static str,
    timestamp: Option<u32>,
    hasp: &'static str,
    /// The cabinet serial (HASP memory 0xd00) and the store terminal's.
    pub(crate) serial: &'static str,
    pub(crate) terminal_serial: &'static str,
    /// ALL.Net game code and version (`AMCUS\AMConfig.ini`).
    pub(crate) game_cd: &'static str,
    pub(crate) game_ver: (i16, i16),
    /// The cabinet's software revision (a global, sent by the store terminal), and the
    /// revision sent while it is not known yet (6R computes it after its update check, which
    /// waits for the terminal; 0: wait).
    pub(crate) software_revision: usize,
    pub(crate) default_revision: u32,
    /// The drive board's forces ([`ffb`]).
    forces: usize,
    /// The network traces' addresses (`WAL_NAMCOES3_NET_LOG`, `wmmt5_net.rs`).
    pub(crate) traces: bool,
    /// The built-in game server's schema.
    server: wal_wm5srv::Game,
    /// The Banapass reader: the game's `bngrw.dll` (6), else the serial reader on COM4.
    bngrw: bool,
    /// The AMCUS auth daemon's newer IAuth (6R, `amcus.rs`).
    amcus_new: bool,
    /// The cabinet's USB memory key (6R, `nbamUsbFinder.dll`): its serial, else none.
    usb_serial: Option<&'static str>,
    patches: &'static [Patch],
}

/// name, address, original bytes, new bytes.
type Patch = (&'static str, usize, &'static [u8], &'static [u8]);

/// Wangan Midnight Maximum Tune 5.
const WMMT5: Version = Version {
    name: "wmmt5",
    exe: "wmn5r.exe",
    timestamp: None,
    hasp: "hasp_windows_x64_109906.dll",
    serial: "272213990002",
    terminal_serial: "272211990002",
    game_cd: "W5R1",
    game_ver: (3, 0),
    software_revision: 0x1948dbc,
    default_revision: 0,
    forces: 0x196f188,
    traces: true,
    server: wal_wm5srv::Game::Wm5,
    bngrw: false,
    amcus_new: false,
    usb_serial: None,
    patches: &[
        // the serial check against the network's ("Dongle Mismatch"): match
        ("dongle match", 0x58c6cc, b"\x01", b"\x00"),
        // the network state 3..4 test of the boot sequence: always on the next step
        ("network state", 0x58cac0, b"\x0f\x87\xb4\x00\x00\x00", &[0x90; 6]),
        // the network's error 0x965 (no network) not raised
        ("network error", 0x73782d, b"\xe8\x6e\x41\xab\xff", &[0x90; 5]),
        // the race clock's network time checks: the local time
        ("network time", 0x8a1d72, b"\x74", b"\xeb"),
        ("network time 2", 0x8a1d91, b"\x72\x25", &[0x90; 2]),
        ("network time 3", 0x8a1da2, b"\x77\x14", &[0x90; 2]),
        // Media Foundation start failure: no DebugBreak (the movies are skipped)
        ("movie start failure", 0x804872, b"\x79", b"\xeb"),
        // the game server's certificate is not verified (SSL_VERIFY_PEER -> none): the built-in
        // server's own certificate
        ("server certificate", 0x817ccd, b"\xba\x01\x00\x00\x00", b"\xba\x00\x00\x00\x00"),
        // E0554: the game stops when 36 hours passed since its last game server exchange (its
        // time at 0x6d8, loaded from the backup): never
        ("network offline period", 0x58157e, b"\x7c\x3e", b"\xeb\x3e"),
        // the store link counts a peer as linked from a private network address only (else state
        // 3, never linked): the terminal and the cabinet's own packets come from the loopback
        ("link private address", 0x8a56e3, b"\x74\x31", b"\xeb\x31"),
        // the terminal's software revision against the cabinet's (terminal link error 8, E2401
        // "revision mismatch"): the emulated terminal has the cabinet's, the game compares a stored
        // copy that differs
        ("terminal revision", 0x5809fe, b"\xb9\x08\x00\x00\x00", b"\xb9\x02\x00\x00\x00"),
        ("terminal revision at boot", 0x58e66a, b"\x75\x74", &[0x90; 2]),
        // the store link keeps one node per address (a new node drops the address's older one):
        // the cabinet and its emulated terminal share the loopback address
        ("link one node per address", 0x87bd6e, b"\x0f\x84\x88\x00\x00\x00", &[0x90; 6]),
        ("link one node per address 2", 0x87bd77, b"\x74\x60", &[0x90; 2]),
        ("link one node per address 3", 0x87bd7d, b"\x75\x4f", &[0x90; 2]),
    ],
};

/// Wangan Midnight Maximum Tune 5DX: the same code, other addresses (the boot's network state
/// test is inverted: its branch to the next step made unconditional).
const WMMT5DX: Version = Version {
    name: "wmmt5dx",
    exe: "wmn5r.exe",
    timestamp: None,
    hasp: "hasp_windows_x64_98199.dll",
    serial: "276413292430",
    terminal_serial: "276411292430",
    game_cd: "W5X1",
    game_ver: (5, 0),
    software_revision: 0x1bd8a84,
    default_revision: 0,
    forces: 0x1c02750,
    traces: false,
    server: wal_wm5srv::Game::Wm5Dx,
    bngrw: false,
    amcus_new: false,
    usb_serial: None,
    patches: &[
        ("dongle match", 0x61773e, b"\x01", b"\x00"),
        ("network state", 0x6185e1, b"\x76\x12", b"\xeb\x12"),
        ("network error", 0x79a3cd, b"\xe8\xee\xf3\xa8\xff", &[0x90; 5]),
        ("network time", 0x8f1702, b"\x74", b"\xeb"),
        ("network time 2", 0x8f1721, b"\x72\x25", &[0x90; 2]),
        ("network time 3", 0x8f1732, b"\x77\x14", &[0x90; 2]),
        ("movie start failure", 0x85de32, b"\x79", b"\xeb"),
        ("server certificate", 0x86e60b, b"\xba\x01\x00\x00\x00", b"\xba\x00\x00\x00\x00"),
        ("network offline period", 0x60ad2e, b"\x7c\x3e", b"\xeb\x3e"),
        ("link private address", 0x8f5173, b"\x74\x31", b"\xeb\x31"),
        ("terminal revision", 0x60a20e, b"\xb9\x08\x00\x00\x00", b"\xb9\x02\x00\x00\x00"),
        ("terminal revision at boot", 0x619925, b"\x75\x55", &[0x90; 2]),
        ("link one node per address", 0x8ccee3, b"\x0f\x84\x8b\x00\x00\x00", &[0x90; 6]),
        ("link one node per address 2", 0x8ccef3, b"\x74\x5c", &[0x90; 2]),
        ("link one node per address 3", 0x8ccef9, b"\x75\x4b", &[0x90; 2]),
    ],
};

/// Wangan Midnight Maximum Tune 5DX+: 5DX's code, other addresses.
const WMMT5DXP: Version = Version {
    name: "wmmt5dxp",
    exe: "wmn5r.exe",
    timestamp: None,
    hasp: "hasp_windows_x64_106482.dll",
    serial: "278313042069",
    terminal_serial: "278311042069",
    game_cd: "W5P1",
    game_ver: (7, 0),
    software_revision: 0x1f7d28c,
    default_revision: 0,
    forces: 0x1fa6f40,
    traces: false,
    server: wal_wm5srv::Game::Wm5DxPlus,
    bngrw: false,
    amcus_new: false,
    usb_serial: None,
    patches: &[
        ("dongle match", 0x706356, b"\x01", b"\x00"),
        ("network state", 0x7071d6, b"\x76\x12", b"\xeb\x12"),
        ("network error", 0x8b8d00, b"\xe8\x6b\x82\x97\xff", &[0x90; 5]),
        ("network time", 0xa16842, b"\x74", b"\xeb"),
        ("network time 2", 0xa16861, b"\x72\x25", &[0x90; 2]),
        ("network time 3", 0xa16872, b"\x77\x14", &[0x90; 2]),
        ("movie start failure", 0x984522, b"\x79", b"\xeb"),
        ("server certificate", 0x9958bb, b"\xba\x01\x00\x00\x00", b"\xba\x00\x00\x00\x00"),
        ("network offline period", 0x6f7d6e, b"\x7c\x3e", b"\xeb\x3e"),
        ("link private address", 0xa1a2b3, b"\x74\x31", b"\xeb\x31"),
        ("terminal revision", 0x6f724e, b"\xb9\x08\x00\x00\x00", b"\xb9\x02\x00\x00\x00"),
        ("terminal revision at boot", 0x708605, b"\x75\x55", &[0x90; 2]),
        ("link one node per address", 0x9f2ba3, b"\x0f\x84\x8b\x00\x00\x00", &[0x90; 6]),
        ("link one node per address 2", 0x9f2bb3, b"\x74\x5c", &[0x90; 2]),
        ("link one node per address 3", 0x9f2bb9, b"\x75\x4b", &[0x90; 2]),
    ],
};

/// Wangan Midnight Maximum Tune 6 (2018, `wmn6r.exe`): 5's code reworked, the same cabinet.
/// Its boot's network state test and two of the race clock's checks are gone; the terminal
/// revision mismatch is error 7.
const WMMT6: Version = Version {
    name: "wmmt6",
    exe: "wmn6r.exe",
    timestamp: Some(0x5bac_b621),
    hasp: "hasp_windows_x64_28756.dll",
    serial: "280813990002",
    terminal_serial: "280811990002",
    game_cd: "WM61",
    game_ver: (9, 0),
    software_revision: 0x1e18114,
    default_revision: 0,
    forces: 0x1e3c610,
    traces: false,
    server: wal_wm5srv::Game::Wm6,
    bngrw: true,
    amcus_new: false,
    usb_serial: None,
    patches: &[
        ("dongle match", 0x69e92a, b"\x01", b"\x00"),
        ("network error", 0x8d6cb0, b"\xe8\x5b\xd3\x98\xff", &[0x90; 5]),
        ("network time", 0xa36caa, b"\x74", b"\xeb"),
        ("movie start failure", 0x9a8062, b"\x79", b"\xeb"),
        ("server certificate", 0x9b848b, b"\xba\x01\x00\x00\x00", b"\xba\x00\x00\x00\x00"),
        ("network offline period", 0x6916bd, b"\x7c\x41", b"\xeb\x41"),
        ("link private address", 0xa3a863, b"\x74\x31", b"\xeb\x31"),
        ("terminal revision", 0x690ad8, b"\xb9\x07\x00\x00\x00", b"\xb9\x02\x00\x00\x00"),
        ("terminal revision at boot", 0x6a0bb0, b"\x75\x4d", &[0x90; 2]),
        ("link one node per address", 0xa131a9, b"\x0f\x84\x85\x00\x00\x00", &[0x90; 6]),
        ("link one node per address 2", 0xa131b3, b"\x74\x5c", &[0x90; 2]),
        ("link one node per address 3", 0xa131b9, b"\x75\x4b", &[0x90; 2]),
    ],
};

/// Wangan Midnight Maximum Tune 6R (2019): 6's code; its offline period checks are two
/// predicates (the warning hour, the end), both false.
const WMMT6R: Version = Version {
    name: "wmmt6r",
    exe: "wmn6r.exe",
    timestamp: Some(0x5dee_006c),
    hasp: "hasp_windows_x64_28756.dll",
    serial: "280813401138",
    terminal_serial: "280811401138",
    game_cd: "W6R1",
    game_ver: (11, 0),
    software_revision: 0x20228ac,
    // the build's revision (0x6a4960)
    default_revision: 0x2ae4,
    forces: 0x2047128,
    traces: false,
    server: wal_wm5srv::Game::Wm6R,
    bngrw: true,
    amcus_new: true,
    // the USB memory key: "2842", the region (0..2), "1", "48" (or "99"), the game's checks
    usb_serial: Some("284201481138"),
    patches: &[
        ("dongle match", 0x716bc6, b"\x01", b"\x00"),
        ("network error", 0x9aa160, b"\xe8\xeb\xb7\x8b\xff", &[0x90; 5]),
        ("network time", 0xb0eb4a, b"\x74", b"\xeb"),
        ("movie start failure", 0xa7da92, b"\x79", b"\xeb"),
        ("server certificate", 0xa8cd2b, b"\xba\x01\x00\x00\x00", b"\xba\x00\x00\x00\x00"),
        ("network offline period warning", 0x706867, b"\x0f\x9d\xc0", b"\x30\xc0\x90"),
        ("network offline period", 0x7068d1, b"\x0f\x9d\xc0", b"\x30\xc0\x90"),
        ("link private address", 0xb12853, b"\x74\x31", b"\xeb\x31"),
        ("terminal revision", 0x708708, b"\xb9\x07\x00\x00\x00", b"\xb9\x02\x00\x00\x00"),
        ("terminal revision at boot", 0x718ec9, b"\x75\x4e", &[0x90; 2]),
        ("link one node per address", 0xae92c9, b"\x0f\x84\x85\x00\x00\x00", &[0x90; 6]),
        ("link one node per address 2", 0xae92d3, b"\x74\x5c", &[0x90; 2]),
        ("link one node per address 3", 0xae92d9, b"\x75\x4b", &[0x90; 2]),
    ],
};

/// Wangan Midnight Maximum Tune 6RR (2022): 6R's code; no USB memory key check, the dongle
/// check left as is (the auth daemon reports the dongle's serial).
const WMMT6RR: Version = Version {
    name: "wmmt6rr",
    exe: "wmn6r.exe",
    timestamp: None,
    hasp: "hasp_windows_x64_30382.dll",
    // the game's serial checks: "2850" (else E1921), the region or 0, the cabinet type (1, 3,
    // 4, 5), "50" or "99" (else E1923)
    serial: "285003501138",
    terminal_serial: "285001501138",
    game_cd: "W6W1",
    game_ver: (13, 0),
    software_revision: 0x1f009b4,
    // the build's revision (0x6c0650)
    default_revision: 0x38c7,
    forces: 0x1f25260,
    traces: false,
    server: wal_wm5srv::Game::Wm6RR,
    bngrw: true,
    amcus_new: true,
    usb_serial: None,
    patches: &[
        ("network error", 0x9de680, b"\xe8\xdb\x3c\xd5\xff", &[0x90; 5]),
        ("network time", 0xb41daa, b"\x74", b"\xeb"),
        ("movie start failure", 0xab0be2, b"\x79", b"\xeb"),
        ("server certificate", 0xac034b, b"\xba\x01\x00\x00\x00", b"\xba\x00\x00\x00\x00"),
        ("network offline period warning", 0x7312c7, b"\x0f\x9d\xc0", b"\x30\xc0\x90"),
        ("network offline period", 0x731331, b"\x0f\x9d\xc0", b"\x30\xc0\x90"),
        ("link private address", 0xb45973, b"\x74\x31", b"\xeb\x31"),
        ("terminal revision", 0x733198, b"\xb9\x07\x00\x00\x00", b"\xb9\x02\x00\x00\x00"),
        ("terminal revision at boot", 0x74447b, b"\x75\x4d", &[0x90; 2]),
        ("link one node per address", 0xb1c739, b"\x0f\x84\x85\x00\x00\x00", &[0x90; 6]),
        ("link one node per address 2", 0xb1c743, b"\x74\x5c", &[0x90; 2]),
        ("link one node per address 3", 0xb1c749, b"\x75\x4b", &[0x90; 2]),
    ],
};

/// The version whose HASP library is loaded (and exe build, when one is given).
pub(crate) fn version() -> Option<&'static Version> {
    let stamp = exe_timestamp();
    [&WMMT5, &WMMT5DX, &WMMT5DXP, &WMMT6, &WMMT6R, &WMMT6RR].into_iter().find(|v| module(v.hasp) != 0 && v.timestamp.is_none_or(|t| Some(t) == stamp))
}

/// The PE time stamp of the process's executable.
fn exe_timestamp() -> Option<u32> {
    let base = crate::module_base();
    (base != 0).then(|| crate::exe_timestamp(base))
}

/// The loaded version (the payload runs in this game only).
pub(crate) fn current() -> &'static Version {
    version().unwrap_or(&WMMT5)
}

pub(crate) fn is_wmmt5() -> bool {
    version().is_some()
}

/// The dongle's check bytes 0xd3e / 0xd3f: the serial's byte sum, its complement.
pub(crate) fn serial_check(serial: &str) -> [u8; 2] {
    let sum = serial.bytes().fold(0u8, |a, b| a.wrapping_add(b));
    [sum, !sum]
}

static FFB_LOG: AtomicBool = AtomicBool::new(false);

pub(crate) fn attach(exe: &str, base: usize, version: &'static Version) {
    if !exe.eq_ignore_ascii_case(version.exe) {
        log!("{}: loaded in {exe}, nothing to do", version.name);
        return;
    }
    GAME_BASE.store(base, Ordering::Relaxed);
    initial_settings();
    crate::wmmt5_net::init(base);
    crate::wmmt5_terminal::start(base);
    patches(base, version);
    for (dll, name, f) in [
        ("msvcr100.dll", "_wassert", wassert as *const () as usize),
        ("kernel32.dll", "DebugBreak", debug_break as *const () as usize),
    ] {
        if unsafe { wal_payload_common::iat::hook_module(base, dll, name, f) }.is_none() {
            log!("wmmt5: {name} not hooked");
        }
    }
    if std::env::var("WAL_NAMCOES3_GAME_LOG").is_ok_and(|v| v == "1") {
        for (name, f) in [("OutputDebugStringA", debug_string_a as *const () as usize), ("OutputDebugStringW", debug_string_w as *const () as usize)] {
            if unsafe { wal_payload_common::iat::hook_module(base, "kernel32.dll", name, f) }.is_none() {
                log!("wmmt5: {name} not hooked");
            }
        }
    }
    match unsafe { wal_payload_common::iat::hook_module(base, "ole32.dll", "CoCreateInstance", co_create_instance as *const () as usize) } {
        Some(o) => CO_CREATE_INSTANCE.store(o, Ordering::Relaxed),
        None => log!("wmmt5: CoCreateInstance not hooked"),
    }
    // the other cabinet devices (IC card reader, touch panel, drive board) are off in the
    // settings: their ports stay absent
    crate::jvs::init(&[base]);
    card_reader(base, version);
    if let Some(serial) = version.usb_serial {
        usb_finder(base, serial);
    }
    // the game server (the ALL.Net answer's uri, amcus.rs)
    // and the store terminal's HTTPS proxy, where a drive cabinet sends its requests while the
    // terminal links (its address, port 50765: wmmt5_terminal.rs)
    wal_wm5srv::start(
        version.server,
        &[crate::amcus::game_server(), crate::wmmt5_terminal::PROXY.to_string()],
        "F:\\wm5srv",
        |line| log!("{line}"),
        std::env::var("WAL_NAMCOES3_SERVER_LOG").is_ok_and(|v| v == "1"),
    );
    FFB_LOG.store(std::env::var("WAL_NAMCOES3_FFB_LOG").is_ok_and(|v| v == "1"), Ordering::Relaxed);
    std::thread::spawn(move || ffb(base + version.forces));
}

/// The operator settings a new cabinet starts with (`setting.lua`), but the card dispenser,
/// touch panel and drive board off (the game stops on their absent ports otherwise), the IC
/// card reader on (the emulated Banapass reader), free play, and the pedals' rest at 0.
const SETTINGS: &[(&str, i32)] = &[
    ("mCalibHandleCenter", 32768),
    ("mCalibAccelCenter", 0),
    ("mCalibBrakeCenter", 0),
    ("mCalibMotorCenter", 511),
    ("mCalibTouchLeft", 2048),
    ("mCalibTouchRight", 14336),
    ("mCalibTouchTop", 14336),
    ("mCalibTouchBottom", 2048),
    ("mCoinChute", 1),
    ("mBuyCardCost", 3),
    ("mGameCost", 2),
    ("mContinueCost", 2),
    ("mFullCourseCost", 4),
    ("mFreePlay", 1),
    ("mPcbId", 0),
    ("mIcCardRw", 1),
    ("mIcCardVender", 0),
    ("mMgCardRw", 0),
    ("mForceFeedback", 0),
    ("mWinsAndRemains", 1),
    ("mEventMode", 0),
    ("mEventModeDist", 0),
    ("mEventModeCount", 4),
    ("mCloseType", 2),
    ("mCloseSun", 96),
    ("mCloseMon", 96),
    ("mCloseTue", 96),
    ("mCloseWed", 96),
    ("mCloseThu", 96),
    ("mCloseFri", 96),
    ("mCloseSat", 96),
    ("mCloseDay", 96),
    ("mTouchPanel", 0),
    ("mGameVol", 10),
    ("mAttractVol", 10),
    ("mOfflineMode", 0),
    ("mClockOffset", 0),
    ("mTimeZoneType", 36),
    ("mTimeDayLight", 0),
    ("mTimeZoneHourOffset", 8),
    ("mTimeZoneMinOffset", 0),
    ("mSummerTimeHourOffset", 0),
    ("mSummerTimeMinOffset", 0),
];

/// Writes the first settings on both drives (`F:\setting.lua.gz`, `G:\setting.lua.gz`, the
/// same file) when there are none.
fn initial_settings() {
    const RULE: &str = "--------------------------------\n";
    let mut text = format!("{RULE}---- setting.lua\n{RULE}\n");
    for (key, value) in SETTINGS {
        text += &format!("{key} = {value}\n");
    }
    text += &format!("{RULE}---- EOF\n{RULE}");
    for drive in ["F", "G"] {
        let path = format!("{drive}:\\setting.lua.gz");
        if std::path::Path::new(&path).exists() {
            continue;
        }
        match std::fs::write(&path, gzip_stored(text.as_bytes())) {
            Ok(()) => log!("wmmt5: {path} written (cabinet devices off, free play)"),
            Err(e) => log!("wmmt5: {path}: {e}"),
        }
    }
}

/// A gzip file holding `data` uncompressed (one stored deflate block).
fn gzip_stored(data: &[u8]) -> Vec<u8> {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    let len = data.len() as u16;
    let mut out = vec![0x1F, 0x8B, 8, 0, 0, 0, 0, 0, 0, 0xFF];
    out.push(1); // final stored block
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&(!len).to_le_bytes());
    out.extend_from_slice(data);
    out.extend_from_slice(&(!crc).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

/// The Banapass reader (COM4, `bpreader.rs`; 6: `bngrw.rs`) and the player's card: `WAL_NAMCOES3_ACCESS_CODE`
/// (20 digits), else a card made once and kept in `F:\banapass.txt`. The `card` button puts it
/// on the reader (while held, and 3 seconds after).
fn card_reader(base: usize, version: &Version) {
    if version.bngrw {
        crate::bngrw::init(base);
    } else {
        crate::bpreader::init("COM4", &[base]);
    }
    let bngrw = version.bngrw;
    let card = crate::bpreader::player_card();
    std::thread::spawn(move || loop {
        if wal_payload_common::input(0).pressed(wal_protocol::button::CARD) {
            if bngrw {
                crate::bngrw::insert(card, Duration::from_secs(3));
            } else {
                crate::bpreader::insert(card, Duration::from_secs(3));
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    });
}

/// The boot sequence's checks (`game::InfoSystem`) and the movie player's (see [`WMMT5`]).
fn patches(base: usize, version: &Version) {
    for &(name, rva, old, new) in version.patches {
        patch(name, base, rva, old, new);
    }
}

/// The USB memory key's serial, NUL-terminated (`nbamUsbFinderGetSerialNumber`).
static USB_SERIAL: std::sync::OnceLock<[u8; 13]> = std::sync::OnceLock::new();

unsafe extern "system" fn usb_finder_ok() -> u16 {
    0
}

/// `nbamUsbFinderGetSerialNumber(index, out)`: the device's serial (12 characters and a NUL);
/// 0: found (the game's cabinet mode check: 2, absent, and a serial of another form are error
/// E2501).
unsafe extern "system" fn usb_finder_serial(_index: u32, out: *mut u8) -> u16 {
    if let Some(serial) = USB_SERIAL.get().filter(|_| !out.is_null()) {
        unsafe { std::ptr::copy_nonoverlapping(serial.as_ptr(), out, serial.len()) };
    }
    0
}

/// The cabinet's USB memory key (6R): the game's `nbamUsbFinder.dll` imports, `serial` (12
/// characters).
fn usb_finder(base: usize, serial: &str) {
    let mut out = [0u8; 13];
    out[..12].copy_from_slice(&serial.as_bytes()[..12]);
    let _ = USB_SERIAL.set(out);
    for (name, f) in [
        ("nbamUsbFinderInitialize", usb_finder_ok as *const () as usize),
        ("nbamUsbFinderGetSerialNumber", usb_finder_serial as *const () as usize),
        ("nbamUsbFinderRelease", usb_finder_ok as *const () as usize),
    ] {
        if unsafe { wal_payload_common::iat::hook_module(base, "nbamUsbFinder.dll", name, f) }.is_none() {
            log!("wmmt5: {name} not hooked");
        }
    }
}

static CO_CREATE_INSTANCE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The cabinet's AMCUS auth daemon (`IAuth`), in-process (`amcus.rs`).
unsafe extern "system" fn co_create_instance(clsid: *const u8, outer: *mut std::ffi::c_void, context: u32, iid: *const u8, out: *mut *mut std::ffi::c_void) -> i32 {
    if !clsid.is_null() && unsafe { std::slice::from_raw_parts(clsid, 16) } == crate::amcus::CLSID {
        if !out.is_null() {
            let object = if current().amcus_new { &crate::amcus::OBJECT_NEW } else { &crate::amcus::OBJECT };
            unsafe { *out = (object as *const crate::amcus::Object).cast_mut().cast() };
        }
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| log!("wmmt5: in-process AMCUS auth (IAuth)"));
        return 0;
    }
    type F = unsafe extern "system" fn(*const u8, *mut std::ffi::c_void, u32, *const u8, *mut *mut std::ffi::c_void) -> i32;
    let real: F = unsafe { std::mem::transmute(CO_CREATE_INSTANCE.load(Ordering::Relaxed)) };
    unsafe { real(clsid, outer, context, iid, out) }
}

/// The game's debug messages (`WAL_NAMCOES3_GAME_LOG=1`): logged.
unsafe extern "system" fn debug_string_a(text: *const u8) {
    if !text.is_null() {
        let len = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
        let line = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(text, len) });
        log!("game: {}", line.trim_end());
    }
}

unsafe extern "system" fn debug_string_w(text: *const u16) {
    if !text.is_null() {
        let len = (0..).take_while(|&i| unsafe { *text.add(i) } != 0).count();
        let line = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(text, len) });
        log!("game: {}", line.trim_end());
    }
}

/// The game's debug asserts (`_wassert`): logged, the game goes on.
unsafe extern "C" fn wassert(message: *const u16, file: *const u16, line: u32) {
    let text = |p: *const u16| {
        if p.is_null() {
            return String::new();
        }
        let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
    };
    log!("wmmt5: assert {} ({}:{line})", text(message), text(file));
}

/// The game's failure traps (`DebugBreak`, 2500 calls: a failed Media Foundation call...),
/// which end the process without a debugger: logged (where from), the game goes on.
unsafe extern "system" fn debug_break() {
    static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    if COUNT.fetch_add(1, Ordering::Relaxed) < 50 {
        let mut frames = [std::ptr::null_mut(); 1];
        unsafe extern "system" {
            fn RtlCaptureStackBackTrace(skip: u32, count: u32, frames: *mut *mut std::ffi::c_void, hash: *mut u32) -> u16;
        }
        unsafe { RtlCaptureStackBackTrace(1, 1, frames.as_mut_ptr(), std::ptr::null_mut()) };
        let at = (frames[0] as usize).wrapping_sub(crate::game_base());
        log!("wmmt5: DebugBreak from {at:#x} ignored");
    }
}


/// A collision force in this range is the road's joints and stripes (a vibration).
const JOINTS: std::ops::RangeInclusive<f32> = 0.209..=0.311;

/// Plays the game's forces (`forces`: the drive board's, written by the game each frame,
/// floats: tire slip, centering spring, friction, collisions, the side positive from the
/// right; in 5 the speed, km/h integer, at 0x196febc): the spring as the centering spring (a light one until the game
/// sets any), a collision as a constant push away from its side, the road joints, stripes and
/// tire slip as a vibration.
fn ffb(forces: usize) {
    let mut started = false;
    let mut sent = [None; 3];
    loop {
        std::thread::sleep(Duration::from_millis(10));
        let [slip, spring, _friction, collision] = unsafe { *(forces as *const [f32; 4]) };
        let finite = |v: f32| if v.is_finite() { v.clamp(-1.0, 1.0) } else { 0.0 };
        let (slip, spring, collision) = (finite(slip), finite(spring), finite(collision));
        started |= spring >= 0.001;
        let spring = if started { spring } else { 0.3 };
        let (constant, joints) = if JOINTS.contains(&collision.abs()) { (0.0, collision.abs()) } else { (-collision, 0.0) };
        let vibration = joints.max(slip.abs());
        let scale = |v: f32| (v * output::FFB_MAX as f32) as i32;
        let values = [(output::FFB_SPRING, scale(spring.max(0.0))), (output::FFB_CONSTANT, scale(constant)), (output::FFB_VIBRATION, scale(vibration))];
        let mut changed = false;
        for (i, (id, value)) in values.into_iter().enumerate() {
            if sent[i] != Some(value) {
                sent[i] = Some(value);
                send_output(0, id, value);
                changed = true;
            }
        }
        if changed && FFB_LOG.load(Ordering::Relaxed) {
            log!("wmmt5 ffb: slip {slip:.3} spring {spring:.3} collision {collision:.3}");
        }
    }
}
