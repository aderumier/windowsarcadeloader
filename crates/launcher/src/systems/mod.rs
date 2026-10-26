//! Emulated arcade systems: what each one needs on the Linux side. The in-game part is the
//! matching `payload-<system>` crate; both only share the common protocol.

mod arcadepc;
mod europar;
mod globalvr;
mod konami;
mod namcoes3;
mod nesica;
mod rawthrills;
mod ringedge;
mod segashell;
mod tsunami;
mod typex;
mod unis;
mod wahlap;

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
    /// Payloads for a 64-bit (PE32+) game executable, when the system has both kinds.
    fn payloads64(&self) -> Option<&'static [Payload]> {
        None
    }
    /// Game files never exposed to the game.
    fn hidden(&self) -> &'static [&'static str] {
        &[]
    }
    /// Directories created in the game directory before launch (game data/saves), so the run
    /// directory links them and the data lands in the game directory.
    fn data_dirs(&self) -> &'static [&'static str] {
        &[]
    }
    /// Payload the game is started with through `wal-loader` (games without a driver DLL
    /// to replace). None: the payload replaces a DLL the game imports.
    fn loader(&self) -> Option<&'static str> {
        None
    }
    /// Program that runs a game whose code is a DLL (the dump's `.windowsloader` names the DLL):
    /// started as `<host> <game.dll> [args]`, installed beside it from `payloads_dir`.
    fn dll_host(&self) -> Option<&'static str> {
        None
    }
    /// Payload `wal-loader` starts another program of the game with, when the game relaunches
    /// into it (`exe=`: the cabinet's test menu program), for systems whose game loads the
    /// payload through its own imports. Installed under its file name beside the game.
    fn program_loader(&self) -> Option<&'static str> {
        None
    }
}

pub fn by_name(name: &str) -> Result<Box<dyn System>> {
    Ok(match name.to_ascii_lowercase().as_str() {
        "nesica" | "nesicax" | "nesicaxlive" => Box::new(nesica::Nesica),
        "nesica2" | "nesicax2" | "nesicaxlive2" => Box::new(nesica::Nesica2),
        "typex" | "typex2" => Box::new(typex::TypeX),
        "globalvr" => Box::new(globalvr::GlobalVr),
        "namcoes3" => Box::new(namcoes3::NamcoEs3),
        "tsunami" => Box::new(tsunami::Tsunami),
        "rawthrills" => Box::new(rawthrills::RawThrills),
        "ringedge" => Box::new(ringedge::RingEdge),
        "segashell" => Box::new(segashell::SegaShell),
        "wahlap" => Box::new(wahlap::Wahlap),
        "konami" => Box::new(konami::Konami),
        "arcadepc" => Box::new(arcadepc::ArcadePc),
        "unis" => Box::new(unis::Unis),
        "europar" | "europa-r" | "segaeuropar" | "segaeuropa-r" => Box::new(europar::EuropaR),
        _ => bail!("unknown system '{name}' (supported: nesica, nesica2, typex, globalvr, namcoes3, tsunami, rawthrills, ringedge, segashell, wahlap, konami, europar, arcadepc, unis)"),
    })
}
