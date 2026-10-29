//! The Beach Head shells' state kept between launches. The launcher imports the cabinet's
//! registry at each launch (the profile's `cabinet.reg`, its defaults); the shell keeps its
//! operator settings, audits and high scores in `HKLM\Software\GVRShell\Operator\Games\<game>\
//! GameSpecific` (`<game>` its `Operator\CurrentGame`: BH2000, MULTIGAME for Beach Head 2002),
//! the coin counters in `HKLM\Software\GVRShell`. The frontend's payload restores them at start
//! from the dump's `C:\<game>_Shell\gvrshell-saved.reg` and writes that file again whenever they
//! change (checked every few seconds).
//!
//! The file is a registry file (`regedit` format, `dword` and string values only).

use std::time::Duration;

use wal_payload_common::log;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExA,
    RegEnumKeyExA, RegEnumValueA, RegOpenKeyExA, RegSetValueExA,
};

const ROOT: &str = "Software\\GVRShell";

/// The shell's game key and save file, for its current game.
fn paths() -> (String, String) {
    let game = values("Software\\GVRShell\\Operator")
        .into_iter()
        .find_map(|(name, value)| match value {
            Value::Str(s) if name.eq_ignore_ascii_case("CurrentGame") => Some(s),
            _ => None,
        })
        .unwrap_or_else(|| "BH2000".to_string());
    (format!("{ROOT}\\Operator\\Games\\{game}\\GameSpecific"), format!("C:\\{game}_Shell\\gvrshell-saved.reg"))
}

#[derive(Debug, PartialEq)]
enum Value {
    Dword(u32),
    Str(String),
}

fn open(path: &str, write: bool) -> Option<HKEY> {
    let c = std::ffi::CString::new(path).ok()?;
    let mut key: HKEY = std::ptr::null_mut();
    let status = unsafe {
        if write {
            RegCreateKeyExA(HKEY_LOCAL_MACHINE, c.as_ptr().cast(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, std::ptr::null(), &mut key, std::ptr::null_mut())
        } else {
            RegOpenKeyExA(HKEY_LOCAL_MACHINE, c.as_ptr().cast(), 0, KEY_READ, &mut key)
        }
    };
    (status == 0).then_some(key)
}

/// The `dword` and string values of `path`, by name.
fn values(path: &str) -> Vec<(String, Value)> {
    let Some(key) = open(path, false) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0.. {
        let mut name = [0u8; 256];
        let mut name_len = name.len() as u32;
        let mut data = [0u8; 1024];
        let mut data_len = data.len() as u32;
        let mut kind = 0u32;
        let status = unsafe {
            RegEnumValueA(key, i, name.as_mut_ptr(), &mut name_len, std::ptr::null(), &mut kind, data.as_mut_ptr(), &mut data_len)
        };
        if status != 0 {
            break;
        }
        let name = String::from_utf8_lossy(&name[..name_len as usize]).into_owned();
        match kind {
            REG_DWORD if data_len >= 4 => out.push((name, Value::Dword(u32::from_le_bytes([data[0], data[1], data[2], data[3]])))),
            REG_SZ => {
                let s = &data[..data_len as usize];
                let s = s.split(|&b| b == 0).next().unwrap_or(s);
                out.push((name, Value::Str(String::from_utf8_lossy(s).into_owned())));
            }
            _ => {}
        }
    }
    unsafe { RegCloseKey(key) };
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn subkeys(path: &str) -> Vec<String> {
    let Some(key) = open(path, false) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for i in 0.. {
        let mut name = [0u8; 256];
        let mut len = name.len() as u32;
        let status = unsafe {
            RegEnumKeyExA(key, i, name.as_mut_ptr(), &mut len, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
        };
        if status != 0 {
            break;
        }
        out.push(String::from_utf8_lossy(&name[..len as usize]).into_owned());
    }
    unsafe { RegCloseKey(key) };
    out.sort();
    out
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn section(out: &mut String, path: &str, values: &[(String, Value)]) {
    out.push_str(&format!("\r\n[HKEY_LOCAL_MACHINE\\{path}]\r\n"));
    for (name, value) in values {
        match value {
            Value::Dword(d) => out.push_str(&format!("\"{}\"=dword:{d:08x}\r\n", escape(name))),
            Value::Str(s) => out.push_str(&format!("\"{}\"=\"{}\"\r\n", escape(name), escape(s))),
        }
    }
}

fn export_tree(out: &mut String, path: &str) {
    section(out, path, &values(path));
    for sub in subkeys(path) {
        export_tree(out, &format!("{path}\\{sub}"));
    }
}

/// The saved state as a registry file: the root's values and the game's whole key.
fn export(game: &str) -> String {
    let mut out = String::from("Windows Registry Editor Version 5.00\r\n");
    section(&mut out, ROOT, &values(ROOT));
    export_tree(&mut out, game);
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(n) = chars.next() {
                out.push(n);
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A quoted string at the start of `s`: its text and what follows.
fn quoted(s: &str) -> Option<(String, &str)> {
    let s = s.strip_prefix('"')?;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        match c {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => return Some((unescape(&s[..i]), &s[i + 1..])),
            _ => escaped = false,
        }
    }
    None
}

/// The keys and values of a registry file (paths under HKEY_LOCAL_MACHINE).
fn parse(text: &str) -> Vec<(String, Vec<(String, Value)>)> {
    let mut out: Vec<(String, Vec<(String, Value)>)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(path) = line.strip_prefix("[HKEY_LOCAL_MACHINE\\").and_then(|l| l.strip_suffix(']')) {
            out.push((path.to_string(), Vec::new()));
        } else if let Some((name, rest)) = quoted(line)
            && let Some(rest) = rest.strip_prefix('=')
            && let Some((_, values)) = out.last_mut()
        {
            if let Some(hex) = rest.strip_prefix("dword:") {
                if let Ok(d) = u32::from_str_radix(hex.trim(), 16) {
                    values.push((name, Value::Dword(d)));
                }
            } else if let Some((s, _)) = quoted(rest) {
                values.push((name, Value::Str(s)));
            }
        }
    }
    out
}

fn import(text: &str) -> usize {
    let mut count = 0;
    for (path, values) in parse(text) {
        let Some(key) = open(&path, true) else {
            continue;
        };
        for (name, value) in values {
            let Ok(cname) = std::ffi::CString::new(name) else {
                continue;
            };
            let (kind, data) = match value {
                Value::Dword(d) => (REG_DWORD, d.to_le_bytes().to_vec()),
                Value::Str(s) => (REG_SZ, format!("{s}\0").into_bytes()),
            };
            unsafe { RegSetValueExA(key, cname.as_ptr().cast(), 0, kind, data.as_ptr(), data.len() as u32) };
            count += 1;
        }
        unsafe { RegCloseKey(key) };
    }
    count
}

/// The saved state restored, then kept saved.
pub(crate) fn init() {
    let (game, save) = paths();
    let mut last = match std::fs::read(&save) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            log!("beachhead: shell state restored from {save} ({} values)", import(&text));
            text
        }
        Err(_) => String::new(),
    };
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(3));
            let now = export(&game);
            if now != last {
                match std::fs::write(&save, &now) {
                    Ok(()) => last = now,
                    Err(e) => log!("beachhead: cannot save the shell state: {e}"),
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_file_parsed() {
        let mut text = String::from("Windows Registry Editor Version 5.00\r\n");
        let values = vec![
            ("Credits".to_string(), Value::Dword(12)),
            ("WinName0".to_string(), Value::Str("A\"B\\C".to_string())),
        ];
        let game = "Software\\GVRShell\\Operator\\Games\\BH2000\\GameSpecific";
        section(&mut text, game, &values);
        assert_eq!(parse(&text), vec![(game.to_string(), values)]);
    }
}
