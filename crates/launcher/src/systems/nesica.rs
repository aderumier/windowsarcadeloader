//! Taito NESiCAxLive (Type X based).
//!
//! The game imports `iDmacDrv32.dll` (FastIO driver) statically: our payload replaces it,
//! so it is loaded before the game code without any injector.
//!
//! Payload options (profile `env`):
//! - `WAL_NESICA_REG`: registry values, e.g. `Resolution=0` (SD mode), `CoinCredit=1`
//! - `WAL_NESICA_DDRIVE`: folder for the game's `D:\` data, relative to the game
//!   directory (default `WindowsLoader`) or absolute
//! - `WAL_NESICA_NESYS=0`: disable the NESYS service emulation
//! - `WAL_NESICA_NESYS_HOST`: the certificate's server list (default `card_id=...`)
//! - `WAL_NESICA_KEY`: crypto service key, built-in name or key file (default `usf4`)
//! - `WAL_NESICA_CRYPT_REPLY=plaintext`: KOF XIII Climax key reply format
//! - `WAL_NESICA_CRYPT=0`, `WAL_NESICA_RFID=0`: disable the crypto service / card reader
//! - `WAL_NESICA_RFID_PORT` (default `COM2`), `WAL_NESICA_CARD_ID` (16 digits)
//! - `WAL_NESICA_IP`, `_MASK`, `_GATEWAY`, `_DNS`: reported network settings

use super::{Payload, System};

pub struct Nesica;

impl System for Nesica {
    fn name(&self) -> &'static str {
        "nesica"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_nesica.dll", install_as: "iDmacDrv32.dll" }]
    }

    fn data_dirs(&self) -> &'static [&'static str] {
        // the game's D: data (default WAL_NESICA_DDRIVE)
        &["WindowsLoader"]
    }
}

/// Taito NESiCAxLive 2: the game imports `NxL2Core.dll` (the NESiCAxLive 2 API, on top of the
/// FastIO driver): the same payload replaces it (`nxl2.rs`). Options as `nesica`, plus
/// `WAL_NXL2_SETTINGS`: game/system settings, `Name=value,...`. 64-bit games (The King of
/// Fighters XIV) get the payload's x86_64 build.
pub struct Nesica2;

impl System for Nesica2 {
    fn name(&self) -> &'static str {
        "nesica2"
    }

    fn payloads(&self) -> &'static [Payload] {
        &[Payload { file: "wal_nesica.dll", install_as: "NxL2Core.dll" }]
    }

    fn payloads64(&self) -> Option<&'static [Payload]> {
        Some(&[Payload { file: "wal_nesica64.dll", install_as: "NxL2Core.dll" }])
    }

    fn data_dirs(&self) -> &'static [&'static str] {
        &["WindowsLoader"]
    }
}
