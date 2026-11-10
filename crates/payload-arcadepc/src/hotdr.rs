//! The House of the Dead: Remake (`WAL_ARCADEPC_BOARD=hotdr`, Unity 2020.3, Mono, 64-bit) with
//! the community arcade plugin (BepInEx, `HotdRemake_ArcadePlugin_2022xx.dll`, Argonlefou's):
//! arcade title and attract, coins and credits, free play, continues, its settings from
//! `ArcadeMod_Config.ini`.
//!
//! The plugin's MULTIPLAYER input (`InputType=1`) reads both guns from DemulShooter's shared
//! memory (`DemulShooter_MMF_Hotdra`, its mutex `DemulShooter_Mutex_Hotdra`), each frame: per
//! player the aim `x`, `y` (`f32` at 0 / 4, player 2 at 8 / 12: Unity's screen pixels, `y` from
//! the bottom), then the triggers (16, 17) and the reloads (18, 19), a byte each; its outputs
//! (lamps, life, ammo, credits) from byte 20 on. The payload writes the inputs, from the
//! virtual sticks as `demul.rs` maps them (a shot off the window, or with the gun off the
//! screen, reloads), every few milliseconds: the plugin writes the whole block back each frame
//! (its inputs as it read them).
//!
//! Start (`1`, `2`) and coin (`5`) are Unity's keys: natives `key_<name>` posted to the game
//! window (`demul.rs`), player 2's start its own key (`WAL_DEMUL_KEYS_P2`).
//!
//! The settings (the profile installs the plugin's ini: the dump's stays untouched) are set at
//! start: the input type, the resolution (the screen's, so the guns' pixels are Unity's), free
//! play (`WAL_FREEPLAY`), the crosshairs (`WAL_CROSSHAIR`) and `WAL_HOTDR_*`.

use std::time::Duration;

use wal_payload_common::log;
use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
use windows_sys::Win32::System::Memory::{CreateFileMappingW, FILE_MAP_WRITE, MapViewOfFile, PAGE_READWRITE};
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

use crate::demul::{client_area, find_window, player_state, screen_size, update_keys};

const INI: &str = "ArcadeMod_Config.ini";
/// The plugin's mapping size (2048: it maps the whole of it).
const MAPPING: usize = 2048;
/// The input bytes of the block (the outputs after them are the plugin's).
const INPUTS: usize = 20;

/// The plugin's ini keys from `WAL_HOTDR_*`: (variable, key, its named values).
const SETTINGS: &[(&str, &str, &[&str])] = &[
    ("WAL_HOTDR_DIFFICULTY", "GameDifficulty", &["easy", "normal", "hard", "arcade"]),
    ("WAL_HOTDR_LIFE", "InitialLife", &[]),
    ("WAL_HOTDR_MAX_LIFE", "MaxLife", &[]),
    ("WAL_HOTDR_CREDITS_TO_START", "CreditsToStart", &[]),
    ("WAL_HOTDR_CREDITS_TO_CONTINUE", "CreditsToContinue", &[]),
    (
        "WAL_HOTDR_LANGUAGE",
        "Language",
        &["", "english", "polish", "japanese", "german", "spanish", "italian", "french", "russian", "chinese"],
    ),
    ("WAL_HOTDR_BLOOD", "BloodColor", &["red", "green"]),
];

/// The settings, then the guns' writer.
pub fn init() {
    settings();
    std::thread::spawn(guns);
}

/// `ArcadeMod_Config.ini` with the payload's values, before the plugin reads it (its `Awake`).
fn settings() {
    let mut values: Vec<(&str, String)> = vec![("InputType", "1".into()), ("HideCursor", "1".into())];
    // `WAL_HOTDR_RESOLUTION=<w>x<h>`: a smaller picture than the screen's, scaled up by Unity
    // (the guns then aim in its pixels, `demul.rs`'s `WAL_DEMUL_RESOLUTION`)
    let chosen = std::env::var("WAL_HOTDR_RESOLUTION").ok().and_then(|v| {
        let (w, h) = v.trim().split_once('x')?;
        Some((w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?))
    });
    if let Some((w, h)) = chosen {
        unsafe { std::env::set_var("WAL_DEMUL_RESOLUTION", format!("{w}x{h}")) };
    }
    let (w, h) = chosen.unwrap_or_else(screen_size);
    if w > 0.0 && h > 0.0 {
        values.push(("ResolutionWidth", (w as u32).to_string()));
        values.push(("ResolutionHeight", (h as u32).to_string()));
    }
    if let Some(free) = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") {
        values.push(("Freeplay", (free as u8).to_string()));
    }
    if std::env::var("WAL_CROSSHAIR").is_ok() {
        values.push(("HideCrosshairs", (!wal_payload_common::crosshairs(true) as u8).to_string()));
    }
    for &(var, key, names) in SETTINGS {
        let Ok(v) = std::env::var(var) else { continue };
        let v = v.trim().to_ascii_lowercase();
        match names.iter().position(|n| !n.is_empty() && *n == v) {
            Some(i) => values.push((key, i.to_string())),
            None if v.parse::<u32>().is_ok() => values.push((key, v)),
            None => log!("hotdr: {var}={v} unknown"),
        }
    }
    let text = match std::fs::read_to_string(INI) {
        Ok(t) => t,
        Err(e) => {
            log!("hotdr: {INI} not read ({e}): the plugin's defaults");
            return;
        }
    };
    let out = set(&text, &values);
    for (key, value) in &values {
        log!("hotdr: {key}={value}");
    }
    if out != text {
        if let Err(e) = std::fs::write(INI, out) {
            log!("hotdr: {INI} not written: {e}");
        }
    }
}

/// `text` (an ini without sections) with `values`' keys set, the missing ones appended.
fn set(text: &str, values: &[(&str, String)]) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut seen = vec![false; values.len()];
    let mut lines: Vec<String> = text
        .lines()
        .map(|line| {
            let key = line.split_once('=').map(|(k, _)| k.trim()).filter(|_| !line.starts_with(';'));
            match key.and_then(|k| values.iter().position(|(name, _)| name.eq_ignore_ascii_case(k))) {
                Some(i) => {
                    seen[i] = true;
                    format!("{}={}", values[i].0, values[i].1)
                }
                None => line.to_string(),
            }
        })
        .collect();
    for (i, (key, value)) in values.iter().enumerate() {
        if !seen[i] {
            lines.push(format!("{key}={value}"));
        }
    }
    let mut out = lines.join(newline);
    out.push_str(newline);
    out
}

/// The plugin's shared memory, created first (the plugin opens the same), written from the
/// sticks; the keys posted.
fn guns() {
    let mapping = unsafe {
        CreateFileMappingW(INVALID_HANDLE_VALUE, std::ptr::null(), PAGE_READWRITE, 0, MAPPING as u32, windows_sys::w!("DemulShooter_MMF_Hotdra"))
    };
    let view = if mapping.is_null() { std::ptr::null_mut() } else { unsafe { MapViewOfFile(mapping, FILE_MAP_WRITE, 0, 0, MAPPING).Value.cast::<u8>() } };
    let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, windows_sys::w!("DemulShooter_Mutex_Hotdra")) };
    if view.is_null() || mutex.is_null() {
        log!("hotdr: shared memory not created: no guns");
        return;
    }
    log!("hotdr: guns into the plugin's shared memory");
    let mut keys = [0u64; 4];
    let mut last_area = None;
    loop {
        std::thread::sleep(Duration::from_millis(4));
        let hwnd = find_window();
        update_keys(hwnd, &mut keys);
        let area = client_area(hwnd);
        if area != last_area {
            log!("hotdr: game window area {area:?}");
            last_area = area;
        }
        if area.is_none() {
            continue;
        }
        let mut block = [0u8; INPUTS];
        for player in 0..2 {
            let s = player_state(player, area);
            block[8 * player..8 * player + 4].copy_from_slice(&s.x.to_le_bytes());
            block[8 * player + 4..8 * player + 8].copy_from_slice(&s.y.to_le_bytes());
            // a shot off the screen only reloads
            block[16 + player] = (s.trigger && !s.reload) as u8;
            block[18 + player] = s.reload as u8;
        }
        unsafe {
            WaitForSingleObject(mutex, 100);
            std::ptr::copy_nonoverlapping(block.as_ptr(), view, INPUTS);
            ReleaseMutex(mutex);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini() {
        let text = ";Gameplay\r\nFreeplay=0\r\nInputType = 0\r\n;HideCursor=0\r\n";
        let out = set(text, &[("InputType", "1".into()), ("Freeplay", "1".into()), ("MaxLife", "5".into())]);
        assert_eq!(out, ";Gameplay\r\nFreeplay=1\r\nInputType=1\r\n;HideCursor=0\r\nMaxLife=5\r\n");
    }
}
