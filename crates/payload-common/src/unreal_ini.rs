//! Unreal Engine 4 console variables set through the game's user `Engine.ini`
//! (`Saved\Config\WindowsNoEditor\Engine.ini`): its `[SystemSettings]` section, read at the
//! engine's start, outranks the scalability levels and the project's defaults. Written by the
//! payload before the engine reads it (graphics options: resolution, effects).

use std::path::Path;

use crate::log;

const SECTION: &str = "[SystemSettings]";

/// Replaces the `[SystemSettings]` section of `ini` with `settings` (`cvar`, value), the rest of
/// the file kept (the engine writes its own sections there). No settings: the section removed.
pub fn system_settings(ini: &Path, settings: &[(&str, String)]) {
    let old = std::fs::read_to_string(ini).unwrap_or_default();
    let mut out = String::new();
    let mut skipping = false;
    for line in old.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            skipping = trimmed.eq_ignore_ascii_case(SECTION);
        }
        if !skipping {
            out.push_str(line);
            out.push_str("\r\n");
        }
    }
    if !settings.is_empty() {
        if !out.is_empty() && !out.ends_with("\r\n\r\n") {
            out.push_str("\r\n");
        }
        out.push_str(SECTION);
        out.push_str("\r\n");
        for (cvar, value) in settings {
            out.push_str(&format!("{cvar}={value}\r\n"));
        }
    }
    if out == old {
        return;
    }
    if let Some(dir) = ini.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(ini, out) {
        Ok(()) => log!("unreal: {}: {}", ini.display(), settings.iter().map(|(c, v)| format!("{c}={v}")).collect::<Vec<_>>().join(" ")),
        Err(e) => log!("unreal: {}: {e}", ini.display()),
    }
}
