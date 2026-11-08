//! The cabinet's system values amdaemon reads under `HKLM\SYSTEM\SEGA\SystemProperty` (the
//! ALLS system image writes them): the keychip's ids, the mounted drives, the video setup.
//! Written in the 64-bit view at each start, from the payload options. Values per segatools'
//! `platform/` (nusec, misc, amvideo, vfs; public domain).

use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_WOW64_64KEY, KEY_WRITE, REG_BINARY, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegSetValueExW,
};

use wal_payload_common::log;

pub(crate) enum Value<'a> {
    Dword(u32),
    Str(&'a str),
    Bin(&'a [u8]),
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub(crate) fn set(key: &str, values: &[(&str, Value)]) {
    let path = wide(&format!("SYSTEM\\SEGA\\SystemProperty{}{key}", if key.is_empty() { "" } else { "\\" }));
    let mut h: HKEY = std::ptr::null_mut();
    let rc = unsafe {
        RegCreateKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE | KEY_WOW64_64KEY, std::ptr::null(), &mut h, std::ptr::null_mut())
    };
    if rc != 0 {
        log!("alls: registry key {key}: error {rc}");
        return;
    }
    for (name, value) in values {
        let name = wide(name);
        let (kind, data): (u32, Vec<u8>) = match value {
            Value::Dword(v) => (REG_DWORD, v.to_le_bytes().to_vec()),
            Value::Str(s) => (REG_SZ, wide(s).iter().flat_map(|c| c.to_le_bytes()).collect()),
            Value::Bin(b) => (REG_BINARY, b.to_vec()),
        };
        unsafe { RegSetValueExW(h, name.as_ptr(), 0, kind, data.as_ptr(), data.len() as u32) };
    }
    unsafe { RegCloseKey(h) };
}

/// The keychip and cabinet values.
pub(crate) struct Cabinet {
    pub game_id: String,
    pub platform_id: String,
    pub keychip: String,
    pub region: u32,
    pub subnet: [u8; 4],
    /// The platform's name the games read (`WAL_ALLS_PLATFORM_NAME`, default `ALLS HX2`)
    pub platform_name: String,
}

impl Cabinet {
    pub(crate) fn from_env() -> Cabinet {
        let var = |k: &str, d: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).unwrap_or_else(|| d.to_string());
        let mut subnet = [192, 168, 149, 0];
        for (s, part) in subnet.iter_mut().zip(var("WAL_ALLS_SUBNET", "192.168.149.0").split('.')) {
            *s = part.parse().unwrap_or(0);
        }
        subnet[3] = 0;
        Cabinet {
            game_id: var("WAL_ALLS_GAME_ID", "SDFV"),
            platform_id: var("WAL_ALLS_PLATFORM_ID", "ACA1"),
            keychip: var("WAL_ALLS_KEYCHIP", "A69E-01A88888888"),
            region: var("WAL_ALLS_REGION", "1").parse().unwrap_or(1),
            subnet,
            platform_name: var("WAL_ALLS_PLATFORM_NAME", "ALLS HX2"),
        }
    }
}

pub(crate) fn write(cab: &Cabinet, video_dll: &str) {
    let mut game_id = [0u8; 4];
    game_id.iter_mut().zip(cab.game_id.bytes()).for_each(|(d, s)| *d = s);
    let mut keychip = [0u8; 16];
    keychip.iter_mut().zip(cab.keychip.bytes()).for_each(|(d, s)| *d = s);
    let platform = cab.platform_id.as_bytes();
    let model = platform.get(3).filter(|c| c.is_ascii_digit()).map_or(0, |c| (c - b'0') as u32);
    set(
        "keychip",
        &[
            ("gameId", Value::Bin(&game_id)),
            ("keychipId", Value::Bin(&keychip)),
            ("modelType", Value::Dword(model)),
            ("platformId", Value::Bin(&platform[..3.min(platform.len())])),
            ("region", Value::Dword(cab.region)),
            ("serverIpIpv4", Value::Bin(&cab.subnet)),
            ("serverIpIpv6", Value::Bin(&[0; 16])),
            ("systemFlag", Value::Dword(0x64)),
        ],
    );
    set("", &[("OSVersion", Value::Str("0_0_0"))]);
    set(
        "static",
        &[
            ("CpuTempError", Value::Dword(100)),
            ("CpuTempWarning", Value::Dword(95)),
            ("PlatformId", Value::Str(&cab.platform_id)),
            ("PlatformName", Value::Str(&cab.platform_name)),
        ],
    );
    set("Master", &[("AppLoaderCount", Value::Dword(1)), ("NextProcess", Value::Str("")), ("SystemError", Value::Str(""))]);
    set("wirelessNetwork", &[("main_nic", Value::Str("")), ("extend_nic", Value::Str(""))]);
    set("downloadui", &[("IsDone", Value::Dword(1))]);
    // the drives the profile maps: E: the AMFS (ICF files, the system's backup), Y: the
    // game's application data
    set("mount", &[("AMFS", Value::Str("E:\\")), ("APPDATA", Value::Str("Y:\\"))]);
    set("amVideo", &[("name", Value::Str(video_dll)), ("name_x86", Value::Str(video_dll))]);
    let mut display: Vec<(&str, Value)> = vec![
        ("monitor_setting_1", Value::Str("0")),
        ("monitor_setting_2", Value::Str("0")),
        ("resolution_1", Value::Str("1920x1080")),
        ("resolution_2", Value::Str("1920x1080")),
        ("use_segatiming", Value::Dword(0)),
    ];
    for name in ["port_1", "port_2", "port_3", "port_4", "port_5", "port_6", "port_7", "port_8"] {
        display.push((name, Value::Dword(1)));
    }
    set("sgsetdisplaysetting\\CurrentSetting", &display);
    log!("alls: system registry written (game {}, platform {}, region {})", cab.game_id, cab.platform_id, cab.region);
}
