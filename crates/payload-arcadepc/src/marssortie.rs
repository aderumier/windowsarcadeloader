//! Mars Sortie (`WAL_ARCADEPC_BOARD=marssortie`, Unity 5.4, 64-bit): the cabinet's hardware and
//! settings made by the payload (the dump's DemulShooter BepInEx plugin is not loaded).
//!
//! The guns are its laser camera's: `MRCQ.Hardware.GetInputAxis` ("x01".."y04", each station's
//! laser spot as a fraction of the camera's picture, y from the bottom, which `LaserLocator`
//! turns into the screen through `LaserCamera.dat`) is the payload's: each gun's place on
//! Unity's picture (`demul.rs`'s mapping, `WAL_DEMUL_RESOLUTION`). Its buttons are the GMB board's
//! signals, each with the game's own keyboard key (`SignalKey`'s simulator): a station's fire
//! `a s d f` (its shot also starts it), its weapon switch `q w e r` (its start signal's key too,
//! which does not start it), its coin `1 2 3 4`, posted to the
//! window (`demul.rs`'s `update_keys`, the profile's keys). The board itself is absent: its error
//! (`SystemError.SetIOBoardError`) and the laser camera's (`SetLaserCamError`) return at once,
//! the SuperDog dongle's check (`SuperDogManager.DoCheckKey`) is true (their IL). The operator's
//! keys are the game's too (space, the arrows). The game is in English (its language getter).
//!
//! Getters of the game's settings return constants: their IL patched before their first call
//! (`nop`s, `ldc.i4.<n>`, `ret`; callers inlining them compile the patched body), the operator's
//! values kept in the records:
//! - free play (`WAL_FREEPLAY`): `MRCQ.GameDataBase`'s coins per game (`GetCoinsPerGame`, a
//!   continue's `GetCoinsPerGame2`, the instance `_GetCoinsPerGame` / `_GetCoinsPerGame2` /
//!   `get_CoinsPerGame`) 0: a shot starts without credits;
//! - the crosshairs (`WAL_CROSSHAIR`): `GameData.DisplayHairCross` (each gun's cross, from the
//!   laser locator: the guns' place);
//! - the players' photos are the game's: its webcam when present (not a Sinden gun's camera,
//!   [`webcam::skip_sinden`](wal_payload_common::webcam::skip_sinden)), blank photos without one.
//!
//! The operator menus are Unity UI driven by the keypad: `MRCQ.UIInputHelper` turns the board's
//! up / down / left / right / OK into `keybd_event` calls (keypad 8 5 4 6 +), whose messages the
//! payload's keyboard filter drops. Its `DllImport` is the payload's: the same keys posted to the
//! game window as the payload's own (`input_filter::post_key`), a key's repeats (flag 1) dropped.

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::mono::{self, P, mono};
use wal_payload_common::{input_filter, log, mono_imports};

use crate::demul::{client_area, find_window, player_state, resolution, update_keys};

const STATIONS: usize = 4;

/// Each station's aim (`x`, `y` as `f32` bits).
static AIM: [AtomicU32; 2 * STATIONS] = [const { AtomicU32::new(0) }; 2 * STATIONS];

/// The guns' places, from the virtual sticks; the posted keys.
fn input_thread() {
    let mut keys = [0u64; 4];
    loop {
        std::thread::sleep(Duration::from_millis(4));
        let hwnd = find_window();
        update_keys(hwnd, &mut keys);
        let area = client_area(hwnd);
        let (rw, rh) = resolution().or_else(|| area.map(|(_, _, w, h)| (w, h))).unwrap_or((1.0, 1.0));
        for p in 0..STATIONS {
            let s = player_state(p, area);
            AIM[2 * p].store((s.x / rw).clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
            AIM[2 * p + 1].store((s.y / rh).clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }
}

/// `Hardware.GetInputAxis(string axisName)`: the guns' places ("x01".."y04").
extern "C" fn get_input_axis(name: *const u8) -> f32 {
    if name.is_null() {
        return 0.0;
    }
    // a managed string: its length (i32) after the object header, then its UTF-16 characters
    let len = unsafe { mono::read::<i32>(name, 2 * size_of::<usize>()) }.clamp(0, 16) as usize;
    let chars: Vec<u16> = (0..len).map(|i| unsafe { mono::read::<u16>(name, 2 * size_of::<usize>() + 4 + 2 * i) }).collect();
    let axis = String::from_utf16_lossy(&chars);
    let station = axis.get(1..).and_then(|n| n.parse::<usize>().ok()).filter(|n| (1..=STATIONS).contains(n));
    match (axis.as_bytes().first(), station) {
        (Some(b'x'), Some(n)) => f32::from_bits(AIM[2 * (n - 1)].load(Ordering::Relaxed)),
        (Some(b'y'), Some(n)) => f32::from_bits(AIM[2 * (n - 1) + 1].load(Ordering::Relaxed)),
        _ => 0.0,
    }
}

/// The game in English: `MRCQ.Localization.get_Language` returns "English" (its IL: the
/// literal of `Load`, the language's default when the configuration names none), whatever the
/// configuration or the operator menu chose (the cabinet's default was Chinese).
fn english(m: &mono::Mono, image: P) {
    let localization = m.class(image, "MRCQ", "Localization");
    // ldstr <token>
    let token = m.il_code(m.method(localization, "Load", 0)).and_then(|il| {
        (0..il.len().saturating_sub(4))
            .filter(|&i| il[i] == 0x72 && il[i + 4] == 0x70)
            .map(|i| u32::from_le_bytes([il[i + 1], il[i + 2], il[i + 3], il[i + 4]]))
            .find(|&t| m.user_string(image, t).as_deref() == Some("English"))
    });
    match (token, m.il_code(m.method(localization, "get_Language", 0))) {
        (Some(token), Some(il)) if il.len() >= 6 => {
            il.fill(0x00);
            let n = il.len();
            il[n - 6] = 0x72;
            il[n - 5..n - 1].copy_from_slice(&token.to_le_bytes());
            il[n - 1] = 0x2A;
            log!("marssortie: the game in English");
        }
        _ => log!("marssortie: Localization.get_Language not patched (its language unchanged)"),
    }
}

/// `Hardware`'s input replaced (from a thread of Mono's: compiling the replacement).
fn hardware(m: &'static mono::Mono, image: P) {
    let hardware = m.class(image, "MRCQ", "Hardware");
    let replaced = m.replace(m.method(hardware, "GetInputAxis", 1), get_input_axis as *const () as usize);
    log!("marssortie: Hardware.GetInputAxis {}", if replaced { "replaced: the guns" } else { "not replaced" });
}

/// The game's `DllImport`s answered by the payload.
fn resolve(name: &str, module: &str) -> Option<usize> {
    (name == "keybd_event" && module.to_ascii_lowercase().starts_with("user32")).then_some(keybd_event as *const () as usize)
}

/// `user32!keybd_event` (`UIInputHelper`'s keypad): a key posted to the game window.
unsafe extern "system" fn keybd_event(vk: u8, _scan: u8, flags: u32, _extra: usize) {
    // 1: held (each frame), 2: released
    if flags & 1 == 0 {
        input_filter::post_key(crate::demul::find_window(), vk as u16, flags & 2 == 0);
    }
}

/// The settings, once Mono has the game's assembly (a thread of the payload's: not from
/// `DllMain`).
pub fn init() {
    wal_payload_common::webcam::skip_sinden();
    let mut patches: Vec<(&str, &str, &str, u8)> = Vec::new();
    if wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") == Some(true) {
        for name in ["GetCoinsPerGame", "GetCoinsPerGame2", "_GetCoinsPerGame", "_GetCoinsPerGame2", "get_CoinsPerGame"] {
            patches.push(("MRCQ", "GameDataBase", name, 0));
        }
    }
    if std::env::var("WAL_CROSSHAIR").is_ok() {
        patches.push(("", "GameData", "get_DisplayHairCross", wal_payload_common::crosshairs(false) as u8));
    }
    mono_imports::hook(resolve);
    std::thread::spawn(move || {
        let start = Instant::now();
        let (m, image) = loop {
            if let Some(m) = mono() {
                let image = m.image("Assembly-CSharp");
                if !image.is_null() {
                    break (m, image);
                }
            }
            if start.elapsed() > Duration::from_secs(60) {
                log!("marssortie: Assembly-CSharp not loaded: settings unchanged");
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        // a thread of Mono's: the lookups below load classes (the dongle check's nested type)
        while m.domain().is_null() || !m.attach() {
            std::thread::sleep(Duration::from_millis(1));
        }
        // the absent board, laser camera and dongle: (namespace, class, method, its arguments,
        // the body's end)
        let hardware_patches: &[(&str, &str, &str, i32, &[u8])] = &[
            ("MRCQ", "SystemError", "SetIOBoardError", 1, &[0x2A]),
            ("MRCQ", "SystemError", "SetLaserCamError", 1, &[0x2A]),
            ("MRCQ", "SuperDogManager", "DoCheckKey", 2, &[0x17, 0x2A]),
        ];
        for &(namespace, class, name, args, body) in hardware_patches {
            match m.il_code(m.method(m.class(image, namespace, class), name, args)) {
                Some(il) if il.len() >= body.len() => {
                    il.fill(0x00);
                    let n = il.len();
                    il[n - body.len()..].copy_from_slice(body);
                    log!("marssortie: {class}.{name} patched");
                }
                _ => log!("marssortie: {class}.{name} not found"),
            }
        }
        for (namespace, class, name, value) in patches {
            match m.il_code(m.method(m.class(image, namespace, class), name, 0)) {
                // a getter without exception clauses: any body (Mono decodes it whole, no bytes
                // after the ret)
                Some(il) if il.len() >= 2 => {
                    il.fill(0x00);
                    let n = il.len();
                    il[n - 2..].copy_from_slice(&[0x16 + value, 0x2A]);
                    log!("marssortie: {class}.{name} = {value}");
                }
                _ => log!("marssortie: {class}.{name} not found"),
            }
        }
        if wal_payload_common::crosshairs(false) {
            playing_crosshairs(m, image);
        }
        english(m, image);
        hardware(m, image);
    });
    std::thread::spawn(input_thread);
}

/// `Now.HairCrossManager.Update` as the game ships it: each station's cross placed at its gun.
const HAIRCROSS_UPDATE: [u8; 62] = [
    0x28, 0x2d, 0x02, 0x00, 0x06, // call GameData.get_DisplayHairCross
    0x39, 0x33, 0x00, 0x00, 0x00, // brfalse ret
    0x16, 0x0a, 0x38, 0x21, 0x00, 0x00, 0x00, // i = 0, br test
    0x02, 0x7b, 0x54, 0x12, 0x00, 0x04, 0x06, 0x9a, // hairCrosses[i]
    0x6f, 0x89, 0x00, 0x00, 0x0a, 0x75, 0x7e, 0x00, 0x00, 0x01, // .transform as RectTransform
    0x06, 0x28, 0x7a, 0x05, 0x00, 0x06, // LaserLocator.GetPosition(i)
    0x6f, 0x00, 0x02, 0x00, 0x0a, // .anchoredPosition =
    0x06, 0x17, 0x58, 0x0a, 0x06, 0x7e, 0x1b, 0x03, 0x00, 0x04, 0x3f, 0xd4, 0xff, 0xff, 0xff, // i++, i < GameData.stations
    0x2a,
];

/// Its replacement: a station's cross shown only once its player plays, from the player's first
/// shot (`hairCrosses[i].SetActive(ScoreManager.IsPlaying(i) &&
/// PropWeaponSpawner.IsDefaultWeaponMounted(i))`, every frame; none in attract), then placed as
/// before (its `isinst` dropped: the transform is the cross's `RectTransform`). With free play
/// (price 0) every station plays from a stage's start: the default weapon, given at a playing
/// station's first shot, tells the players in. `DisplayHairCross` is the payload's (on), not
/// tested again.
const HAIRCROSS_PLAYING: [u8; 58] = [
    0x16, 0x0a, // i = 0 (a station at least)
    0x02, 0x7b, 0x54, 0x12, 0x00, 0x04, 0x06, 0x9a, // loop: hairCrosses[i]
    0x25, 0x06, 0x28, 0xd3, 0x10, 0x00, 0x06, // dup, ScoreManager.IsPlaying(i)
    0x06, 0x28, 0xb1, 0x10, 0x00, 0x06, 0x5f, // & PropWeaponSpawner.IsDefaultWeaponMounted(i)
    0x6f, 0xb0, 0x00, 0x00, 0x0a, // .SetActive()
    0x6f, 0x89, 0x00, 0x00, 0x0a, // .transform
    0x06, 0x28, 0x7a, 0x05, 0x00, 0x06, // LaserLocator.GetPosition(i)
    0x6f, 0x00, 0x02, 0x00, 0x0a, // .anchoredPosition =
    0x06, 0x17, 0x58, 0x0a, 0x06, 0x7e, 0x1b, 0x03, 0x00, 0x04, 0x32, 0xc9, // i++, i < stations: loop
    0x2a,
];

/// Idle stations' crosses hidden: the game shows every station's (`GameData.stations`), played
/// or not. Its build checked by the original body (the tokens are this build's).
fn playing_crosshairs(m: &wal_payload_common::mono::Mono, image: wal_payload_common::mono::P) {
    match m.il_code(m.method(m.class(image, "Now", "HairCrossManager"), "Update", 0)) {
        Some(il) if *il == HAIRCROSS_UPDATE => {
            // nops first: the body keeps its size
            let pad = il.len() - HAIRCROSS_PLAYING.len();
            il[..pad].fill(0x00);
            il[pad..].copy_from_slice(&HAIRCROSS_PLAYING);
            log!("marssortie: crosshairs of the playing stations only");
        }
        _ => log!("marssortie: HairCrossManager.Update not this build's: every station's crosshair"),
    }
}
