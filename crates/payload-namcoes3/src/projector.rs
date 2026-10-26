//! NEC projector on a serial port (Lost Land Adventure's launcher, COM1): without one the
//! launcher shows 09-08 "SERIAL COMMUNICATION WARNING". It polls every 3 seconds:
//! - `00 88 00 00 00 88` error status: `20 88 id id 0C` + 12 data bytes + checksum, all zero
//!   (no error);
//! - `00 BF 00 00 01 02 C2` running status: `20 BF id id 10` + 16 data bytes + checksum; the
//!   launcher (0x41790) wants data byte 2 = 0x04, and raises 09-10 when byte 3 is 1.
//!
//! The checksum is the low byte of the sum of the bytes before it.

use wal_payload_common::{log, serial};

pub(crate) fn init(port: &str, modules: &[usize]) {
    serial::install_in(port, process, modules);
    log!("projector: NEC projector on {port}");
}

fn reply(command: u8, data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x20, command, 0x01, 0x00, data.len() as u8];
    out.extend_from_slice(data);
    out.push(out.iter().fold(0u8, |s, b| s.wrapping_add(*b)));
    out
}

fn process(data: &[u8]) -> Vec<u8> {
    match data {
        [0x00, 0x88, ..] => reply(0x88, &[0; 12]),
        [0x00, 0xBF, ..] => {
            let mut status = [0u8; 16];
            status[0] = 0x02;
            status[1] = 0x04;
            reply(0xBF, &status)
        }
        _ => {
            log!("projector: unknown command {data:02X?}");
            Vec::new()
        }
    }
}
