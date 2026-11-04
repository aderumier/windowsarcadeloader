//! Hot Wheels: Victory Lap (Unity 6, IL2CPP, 64-bit): Need for Speed Heat Takedown's steering
//! board (`steering.rs`) on the game's RS-232 library (`rs232.rs`), and its webcams.
//!
//! - **Webcams.** The cabinet's cameras take the player's picture before a race
//!   (`SBK.FaceDetection.FDManager`) when the operator's `TakePicture` setting is on (its
//!   default); the boxes have none. The setting (`OperatorUtil.get_TakePicture`) is the
//!   payload's `false` (IL2CPP's API, at the game's dongle's first call): no capture, the
//!   operator's setting ignored.

use std::sync::atomic::{AtomicBool, Ordering};

use wal_payload_common::il2cpp;
use wal_payload_common::log;

/// `OperatorUtil.get_TakePicture()`: no picture.
extern "C" fn take_picture(_method: usize) -> bool {
    false
}

/// At the game's first dongle call (IL2CPP running), once: nothing for another game.
pub fn init() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(i) = il2cpp::il2cpp() else { return };
    let game = i.image("Assembly-CSharp");
    if i.class(game, "SBK.FaceDetection", "FDManager").is_null() {
        return;
    }
    let get = i.method(i.class(game, "", "OperatorUtil"), "get_TakePicture", 0);
    log!("victorylap: webcam pictures off ({})", if i.replace(get, take_picture as *const () as usize) { "ok" } else { "OperatorUtil.get_TakePicture not found" });
}
