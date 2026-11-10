//! Settings of a game's XML file set at start (`WAL_ARCADEPC_XML`): Wild West Shoot Out's
//! `GameConfig.xml` (`<Free>`, the common free play).
//!
//! `WAL_ARCADEPC_XML=<file>:<Tag>=<value>,<Tag>=<value>;<file>:...` (the file relative to the
//! game's directory): each element `<Tag>…</Tag>` of the file gets the value, written back only
//! when one changed (the profile installs the file: the dump's own stays untouched). The value
//! `{freeplay}` is the common free play option (`WAL_FREEPLAY`): `1` / `0`, unchanged when unset;
//! `{crosshair}` the common crosshair option (`WAL_CROSSHAIR`), likewise.

use wal_payload_common::log;

/// The settings of `WAL_ARCADEPC_XML`, applied.
pub fn init() {
    let Ok(list) = std::env::var("WAL_ARCADEPC_XML") else { return };
    let free = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY");
    let crosshair = std::env::var("WAL_CROSSHAIR").is_ok().then(|| wal_payload_common::crosshairs(false));
    for spec in list.split(';').filter(|s| !s.trim().is_empty()) {
        let Some((file, settings)) = spec.split_once(':') else { continue };
        let file = file.trim();
        let Ok(text) = std::fs::read_to_string(file) else {
            log!("xml: {file} not found");
            continue;
        };
        let mut out = text.clone();
        for (tag, value) in settings.split(',').filter_map(|s| s.split_once('=')) {
            let (tag, value) = (tag.trim(), value.trim());
            let value = match value {
                "{freeplay}" | "{crosshair}" => match if value == "{freeplay}" { free } else { crosshair } {
                    Some(f) => if f { "1" } else { "0" },
                    None => continue,
                },
                v => v,
            };
            out = set(&out, tag, value);
            log!("xml: {file} <{tag}> = {value}");
        }
        if out != text {
            if let Err(e) = std::fs::write(file, out) {
                log!("xml: {file} not written: {e}");
            }
        }
    }
}

/// `text` with every `<tag>…</tag>` holding `value`.
fn set(text: &str, tag: &str, value: &str) -> String {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(&open) {
        let start = i + open.len();
        let Some(len) = rest[start..].find(&close) else { break };
        out.push_str(&rest[..start]);
        out.push_str(value);
        rest = &rest[start + len..];
    }
    out.push_str(rest);
    out
}
