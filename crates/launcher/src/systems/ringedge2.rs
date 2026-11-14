//! Sega RingEdge 2 (NRS engine): Border Break Scramble (`nrs.exe`).
//!
//! The same amLib stack as RingEdge (amDongle keychip, amJvs JVS board, amSram/amEeprom,
//! amBackup, amPlatform, ALL.Net): it reuses the `payload-ringedge` payload (`wal_ringedge.dll`),
//! with `WAL_RINGEDGE_BOARD=ringedge2` (platform id `AAL2`). This is its own system only so the
//! `games/ringedge2/` dumps get their own run directory and log; everything else is the RingEdge
//! payload. See `ringedge.rs` for the full list of `WAL_RINGEDGE_*` / `WAL_TYPEX_JVS_*` options.

use super::{Payload, System};

pub struct RingEdge2;

impl System for RingEdge2 {
    fn name(&self) -> &'static str {
        "ringedge2"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_ringedge.dll", install_as: "wal_ringedge.dll" }]
    }

    fn loader(&self) -> Option<&'static str> {
        Some("wal_ringedge.dll")
    }

    fn data_dirs(&self) -> &'static [&'static str] {
        &["sv"]
    }
}
