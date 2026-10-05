//! Global VR PC based cabinets (Far Cry Paradise Lost).
//!
//! The game imports `USBIOExtreme.dll` (USBIO board driver: guns, panel buttons, coins)
//! statically: our payload replaces it, so it is loaded before the game code without any
//! injector.
//!
//! Payload options (profile `env`):
//! - `WAL_GLOBALVR_WDRIVE`: folder for the game's `W:\` paths (the cabinet runs the game from
//!   `subst W: .`), relative to the game directory (default `.`, the game directory) or absolute
//! - `WAL_PATCHES`: game code patches `<rva>:<hex bytes>,...`
//!
//! Native inputs for `native_map`: `trigger grenade start action coin panel1..panel4`.

use super::{Payload, System};

pub struct GlobalVr;

impl System for GlobalVr {
    fn name(&self) -> &'static str {
        "globalvr"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_globalvr.dll", install_as: "USBIOExtreme.dll" }]
    }
}
