//! Chase Chase Jokers' managed settings, set through Mono's embedding API
//! (`mono-2.0-bdwgc.dll`'s exports): a static field of a game class.
//!
//! * `BootItemIO.processed`: the boot's I/O check has looked at the monitors. The cabinet has a
//!   1920x1080 main monitor and a 1280x1024 sub monitor; anything else stops the boot on a
//!   monitor error (5-1525) until Service is pressed. Set, the check only reports the I/O
//!   board's status.
//!
//! Called from the game's managed threads (an AIO call), never from `DllMain`.

use wal_payload_common::log;

use crate::mono::mono;

/// Sets the static `bool` field `class.field` of the assembly `assembly` (global namespace).
fn set_static_bool(assembly: &str, class: &str, field: &str, value: bool) -> bool {
    let Some(m) = mono() else {
        log!("ccj: Mono not found");
        return false;
    };
    if !m.attach() {
        return false;
    }
    let klass = m.class(m.image(assembly), "", class);
    let f = m.field(klass, field);
    let mut v = value as u8;
    if f.is_null() || !m.static_set(klass, f, &mut v as *mut u8 as crate::mono::P) {
        log!("ccj: {assembly} {class}.{field} not found");
        return false;
    }
    true
}

/// The boot's monitor check passed (`BootItemIO.processed`).
pub fn skip_monitor_check() {
    if set_static_bool("ChaseGameScripts", "BootItemIO", "processed", true) {
        log!("ccj: boot monitor check skipped (one screen)");
    }
}
