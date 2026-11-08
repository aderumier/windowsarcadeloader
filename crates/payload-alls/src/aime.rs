//! The Aime card reader (837-15396) amdaemon talks to on a serial port (config.json's
//! `aime.unit[].port`, `WAL_ALLS_AIME_PORT`): `wal_payload_common::sgreader`. Player 1's `card`
//! input touches the reader with the player's card (`WAL_ALLS_ACCESS_CODE`, 20 digits, default
//! `50000000000000000001`) for 3 seconds, until amdaemon reads it.

use std::time::Duration;

use wal_payload_common::{log, sgreader};
use wal_protocol::button;

fn access_code() -> String {
    let code: String = std::env::var("WAL_ALLS_ACCESS_CODE").unwrap_or_default().chars().filter(char::is_ascii_digit).collect();
    if code.len() == 20 { code } else { "50000000000000000001".into() }
}

/// The reader on its port, for amdaemon's serial calls (before its other device hooks: they
/// pass the calls they do not handle on to these).
pub(crate) fn init() {
    let Some(port) = std::env::var("WAL_ALLS_AIME_PORT").ok().map(|p| p.trim().to_ascii_uppercase()).filter(|p| p.starts_with("COM")) else { return };
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    sgreader::init(&port, &[exe]);
    let code = access_code();
    log!("aime: card {code}");
    std::thread::spawn(move || {
        let mut held = false;
        loop {
            let card = wal_payload_common::input(0).pressed(button::CARD);
            if card && !held {
                sgreader::insert(&code, Duration::from_secs(3));
            }
            held = card;
            std::thread::sleep(Duration::from_millis(50));
        }
    });
}
