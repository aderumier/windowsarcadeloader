//! Rapture3D (Blue Ripple Sound's OpenAL driver, `rapture3d_oal.dll` next to the game, the
//! 2.7.4 one of its installer): its settings, written on the cabinet by the game's
//! `SetupAudioRegistry.bat`, in `HKCU\Software\BlueRippleSound`. Without them the driver
//! crashes the game when it opens the sound device.
//!
//! `WAL_SEGASHELL_RAPTURE3D=<format library>,<speakers .reg>` (files of the game directory,
//! Showdown: `showdown.pfl,Stereo.reg`): `Formats\UFL` the format library's path, and
//! `Speakers\Configuration` the speaker layout of the `.reg` file (stereo).

use wal_payload_common::log;
use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExA, RegSetValueExA};

fn set(key: &str, name: &str, value: &str) {
    let key = format!("Software\\BlueRippleSound\\{key}\0");
    let name = format!("{name}\0");
    let mut data = value.as_bytes().to_vec();
    data.push(0);
    unsafe {
        let mut h: HKEY = std::ptr::null_mut();
        let r = RegCreateKeyExA(HKEY_CURRENT_USER, key.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, std::ptr::null(), &mut h, std::ptr::null_mut());
        if r != 0 {
            log!("rapture3d: cannot create {key}: {r}");
            return;
        }
        RegSetValueExA(h, name.as_ptr(), 0, REG_SZ, data.as_ptr(), data.len() as u32);
        RegCloseKey(h);
    }
}

/// The `"Configuration"="..."` value of a `.reg` file (`\"` and `\\` unescaped).
fn reg_value(text: &str, name: &str) -> Option<String> {
    let line = text.lines().find(|l| l.trim_start().starts_with(&format!("\"{name}\"=\"")))?;
    let quoted = line.trim().split_once("=\"")?.1.strip_suffix('"')?;
    Some(quoted.replace("\\\"", "\"").replace("\\\\", "\\"))
}

pub(crate) fn init() {
    let Ok(spec) = std::env::var("WAL_SEGASHELL_RAPTURE3D") else { return };
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) else { return };
    let mut files = spec.split(',').map(str::trim);
    if let Some(pfl) = files.next().filter(|f| !f.is_empty()) {
        let path = dir.join(pfl).to_string_lossy().into_owned();
        set("Formats", "UFL", &path);
        log!("rapture3d: format library {path}");
    }
    if let Some(reg) = files.next().filter(|f| !f.is_empty()) {
        // .reg files: UTF-16 or ANSI text
        let text = std::fs::read(dir.join(reg)).ok().map(|b| {
            if b.starts_with(&[0xFF, 0xFE]) {
                String::from_utf16_lossy(&b[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
            } else {
                String::from_utf8_lossy(&b).into_owned()
            }
        });
        match text.as_deref().and_then(|t| reg_value(t, "Configuration")) {
            Some(config) => {
                set("Speakers", "Configuration", &config);
                log!("rapture3d: speakers from {reg}");
            }
            None => log!("rapture3d: no speaker configuration in {reg}"),
        }
    }
}
