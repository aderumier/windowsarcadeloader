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
