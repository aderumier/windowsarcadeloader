//! The Futaba GP1232A02A VFD of Sega's middleware (`libamw.dll`'s "display": the e-money status
//! screen) on a serial port, as bananatools' and segatools' `board/vfd.c` (public domain): commands
//! start with `1B` or `1A`; the display only answers the version query `5B` (`01.20` and `01`),
//! everything else (text, images, brightness...) is drawn without an answer.

use wal_payload_common::log;

fn handle(bytes: &[u8]) -> Vec<u8> {
    let query = bytes.windows(2).any(|w| (w[0] == 0x1B || w[0] == 0x1A) && w[1] == 0x5B);
    if query { b"01.20\x01".to_vec() } else { Vec::new() }
}

/// The display on `port`, for the serial calls of `modules`.
pub(crate) fn init(port: &str, modules: &[usize]) {
    wal_payload_common::serial::install_in(port, handle, modules);
    log!("vfd: display on {port}");
}
