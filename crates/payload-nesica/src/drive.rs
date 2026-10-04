//! NESiCA D: data folder: generic redirection (`wal_payload_common::drive`, folder
//! `WAL_NESICA_DDRIVE`, default `WindowsLoader`) plus the NESYS news picture WindowsLoader provides.

use wal_payload_common::drive;

pub(crate) fn init() {
    drive::init("WAL_NESICA_DDRIVE", "WindowsLoader");
    let news = format!("{}\\news.png", data_dir());
    if !std::path::Path::new(&news).exists() {
        let _ = std::fs::write(&news, include_bytes!("news.png"));
    }
}

/// Windows path of the folder holding the game's D: data.
pub(crate) fn data_dir() -> String {
    drive::data_dir()
}
