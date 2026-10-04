//! Emulated arcade systems: what each one needs on the Linux side. The in-game part is the
//! matching `payload-<system>` crate; both only share the common protocol.

mod nesica;

use anyhow::{Result, bail};

/// A payload DLL installed in the game run directory.
pub struct Payload {
    /// File name in `payloads_dir`.
    pub file: &'static str,
    /// Name in the run directory: the game DLL it replaces.
    pub install_as: &'static str,
}

pub trait System {
    fn name(&self) -> &'static str;
    /// Payload DLLs replacing game files, loaded by the game itself.
    fn payloads(&self) -> &'static [Payload];
    /// Game files never exposed to the game.
    fn hidden(&self) -> &'static [&'static str] {
        &[]
    }
    /// Directories created in the game directory before launch (game data/saves), so the run
    /// directory links them and the data lands in the game directory.
    fn data_dirs(&self) -> &'static [&'static str] {
        &[]
    }
}

pub fn by_name(name: &str) -> Result<Box<dyn System>> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "nesica" | "nesicax" | "nesicaxlive" => Box::new(nesica::Nesica),
        _ => bail!("unknown system '{name}' (supported: nesica)"),
    })
}
