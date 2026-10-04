//! Settings the NESiCAxLive launcher normally provides in the registry.
//!
//! Instead of hooking the registry API (WindowsLoader's RegHooks), the values are written to
//! the real Wine registry before the game runs. Error values the game reported during the
//! previous run (GameResult, IOError*) are reset every launch.
//!
//! Override with `WAL_NESICA_REG`, e.g. `Resolution=0,CoinCredit=1`.

use wal_payload_common::log;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, RegCloseKey,
    RegCreateKeyExA, RegSetValueExA,
};

const VALUES: &[(&str, u32)] = &[
    ("CoinCredit", 0), // 0 = free play
    ("Resolution", 1), // 0 = SD, 1 = HD
    ("ScreenVertical", 0),
    ("EventModeEnable", 0),
    ("UserSelectEnable", 0),
    ("GameResult", 0),
    ("IOErrorCoin", 0),
    ("IOErrorCredit", 0),
    ("SystemType", 0),
    ("ConditionTime", 300),
    ("EventNextTime", 900),
    ("GameKind", 1234),
    ("LogLevel", 0),
    ("TrafficCount", 2),
    ("UpdateStep", 0),
    ("Country", 1),
    ("AppVer", 1),
];

const KEYS: &[&str] = &["SOFTWARE\\TAITO\\NESiCAxLive\0", "SOFTWARE\\TAITO\\TYPEX\0"];

pub(crate) fn init() {
    let overrides = std::env::var("WAL_NESICA_REG").unwrap_or_default();
    let value_of = |name: &str, default: u32| {
        overrides
            .split(',')
            .filter_map(|p| p.split_once('='))
            .find(|(k, _)| k.trim().eq_ignore_ascii_case(name))
            .and_then(|(_, v)| v.trim().parse().ok())
            .unwrap_or(default)
    };

    for key in KEYS {
        let mut hkey: HKEY = std::ptr::null_mut();
        let status = unsafe {
            RegCreateKeyExA(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                std::ptr::null(),
                &mut hkey,
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            log!("registry: cannot create {}: error {status}", key.trim_end_matches('\0'));
            continue;
        }
        for (name, default) in VALUES {
            let value = value_of(name, *default);
            let cname = format!("{name}\0");
            unsafe {
                RegSetValueExA(hkey, cname.as_ptr(), 0, REG_DWORD, (&value as *const u32).cast(), 4);
            }
        }
        unsafe { RegCloseKey(hkey) };
    }
    log!("registry: NESiCAxLive settings written");
}
