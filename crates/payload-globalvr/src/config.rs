//! The game's saved settings, `systemcfg.lua` next to the executable (the game reads it at
//! boot and rewrites it).
//!
//! `g_arcadeError` keeps the cabinet error flags across boots. Bits 4/8 ("One or more Gun
//! PCB(s) Missing") are only cleared by the game's DirectInput gun path, never with a USBIO
//! board: a value saved by another setup (one running the DirectInput path without gun
//! PCBs: 1038) blocks the game on CONTROL ERROR. The flags are reset before the game reads them;
//! it sets again the ones still true.

use wal_payload_common::log;

const FILE: &str = "systemcfg.lua";
const KEY: &str = "g_arcadeError";

/// `content` with `g_arcadeError = "<v>"` set to 0, or None when already 0 / missing.
fn cleared(content: &str) -> Option<String> {
    let mut changed = false;
    let lines: Vec<String> = content
        .split_inclusive('\n')
        .map(|line| {
            let is_key = line.trim_start().strip_prefix(KEY).is_some_and(|r| r.trim_start().starts_with('='));
            let value = line.split_once('=').map(|(_, v)| v.trim().trim_matches('"'));
            if is_key && value != Some("0") {
                changed = true;
                let eol = &line[line.trim_end().len()..];
                format!("{KEY} = \"0\"{eol}")
            } else {
                line.to_string()
            }
        })
        .collect();
    changed.then(|| lines.concat())
}

pub(crate) fn clear_errors() {
    let Ok(content) = std::fs::read_to_string(FILE) else { return };
    if let Some(new) = cleared(&content) {
        match std::fs::write(FILE, new) {
            Ok(()) => log!("config: {KEY} reset in {FILE}"),
            Err(e) => log!("config: cannot write {FILE}: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resets_error() {
        let cfg = "g_arcadeDemoRecording = \"0\"\r\ng_arcadeError = \"1038\"\r\ng_arcadeErrorX = \"5\"\r\n";
        assert_eq!(
            cleared(cfg).unwrap(),
            "g_arcadeDemoRecording = \"0\"\r\ng_arcadeError = \"0\"\r\ng_arcadeErrorX = \"5\"\r\n"
        );
        assert!(cleared("g_arcadeError = \"0\"\n").is_none());
        assert!(cleared("r_Width = \"1920\"\n").is_none());
    }
}
