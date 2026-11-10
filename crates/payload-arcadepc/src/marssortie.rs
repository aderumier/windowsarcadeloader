//! Mars Sortie (`WAL_ARCADEPC_BOARD=marssortie`, Unity 5.4, 64-bit): its settings made by the
//! payload. The guns, coins and keys are the DemulShooter plugin's (`demul.rs`, the profile's
//! `WAL_DEMUL_PORT`).
//!
//! Getters of the game's settings return constants: their IL patched before their first call
//! (`nop`s, `ldc.i4.<n>`, `ret`; callers inlining them compile the patched body), the operator's
//! values kept in the records:
//! - free play (`WAL_FREEPLAY`): `MRCQ.GameDataBase`'s coins per game (`GetCoinsPerGame`, a
//!   continue's `GetCoinsPerGame2`, the instance `_GetCoinsPerGame` / `_GetCoinsPerGame2` /
//!   `get_CoinsPerGame`) 0: a shot starts without credits;
//! - the crosshairs (`WAL_CROSSHAIR`): `GameData.DisplayHairCross` (each gun's cross, from the
//!   laser locator: the guns' place);
//! - no player photos at a game's start (`GameData.TakePhoto` false): no camera needed.
//!
//! The operator menus are Unity UI driven by the keypad: `MRCQ.UIInputHelper` turns the board's
//! up / down / left / right / OK into `keybd_event` calls (keypad 8 5 4 6 +), whose messages the
//! payload's keyboard filter drops. Its `DllImport` is the payload's: the same keys posted to the
//! game window as the payload's own (`input_filter::post_key`), a key's repeats (flag 1) dropped.

use std::time::{Duration, Instant};

use wal_payload_common::mono::mono;
use wal_payload_common::{input_filter, log, mono_imports};

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
    let mut patches: Vec<(&str, &str, &str, u8)> = vec![("", "GameData", "get_TakePhoto", 0)];
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
    });
}
