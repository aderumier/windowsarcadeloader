//! JVS-style packet framing used by arcade I/O boards (Taito RFID reader, Type X JVS...).
//!
//! Packet: `E0 <node> <size> <data...> <sum>`, where `size` counts the data and checksum
//! bytes, `sum` is the 8-bit sum of node..data, and `E0`/`D0` inside the packet are escaped
//! as `D0 DF` / `D0 CF`.
//!
//! Ported from WindowsLoader's RfidEmu (itself based on ttx_monitor), keeping its exact output.

pub const SYNC: u8 = 0xE0;
pub const MARK: u8 = 0xD0;
pub const ADDR_MASTER: u8 = 0x00;
pub const STATUS_OK: u8 = 0x01;
pub const REPORT_OK: u8 = 0x01;

/// Builds one reply packet.
pub struct Encoder {
    buf: Vec<u8>,
    size_at: usize,
    status_at: usize,
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Encoder {
    /// Starts a reply to the master with an OK status.
    pub fn new() -> Self {
        let mut e = Encoder { buf: vec![SYNC, ADDR_MASTER], size_at: 0, status_at: 0 };
        e.size_at = e.buf.len();
        e.push(0);
        e.status_at = e.buf.len();
        e.push(STATUS_OK);
        e
    }

    /// Appends a byte, escaped.
    pub fn push(&mut self, v: u8) {
        match v {
            MARK => self.buf.extend_from_slice(&[MARK, 0xCF]),
            SYNC => self.buf.extend_from_slice(&[MARK, 0xDF]),
            _ => self.buf.push(v),
        }
    }

    pub fn extend(&mut self, data: &[u8]) {
        for b in data {
            self.push(*b);
        }
    }

    pub fn set_status(&mut self, status: u8) {
        self.buf[self.status_at] = status;
    }

    /// Finishes the packet: size and checksum. Empty when nothing but the status was added.
    pub fn finish(mut self) -> Vec<u8> {
        if self.buf.len() == self.status_at + 1 {
            return Vec::new();
        }
        // unescaped bytes from the size field to the end (= data + checksum count)
        let size = self.buf[self.size_at..].iter().filter(|b| **b != MARK).count();
        self.buf[self.size_at] = size as u8;
        let mut sum = 0u32;
        let mut escaped = false;
        for &b in &self.buf[1..] {
            if b == MARK {
                escaped = true;
            } else {
                sum += (b as u32 + escaped as u32) & 0xFF;
                escaped = false;
            }
        }
        self.push((sum & 0xFF) as u8);
        self.buf
    }
}

/// A received request: node and the command bytes (without size and checksum).
pub struct Request<'a> {
    pub node: u8,
    pub commands: &'a [u8],
}

/// Splits a raw request. Requests are not unescaped (boards never receive E0/D0 in practice).
pub fn parse(packet: &[u8]) -> Option<Request<'_>> {
    if packet.len() < 4 {
        return None;
    }
    let size = packet[2] as usize;
    let end = (3 + size.saturating_sub(1)).min(packet.len());
    Some(Request { node: packet[1], commands: &packet[3..end] })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_reply() {
        assert!(Encoder::new().finish().is_empty());
    }

    #[test]
    fn reply_with_report() {
        let mut e = Encoder::new();
        e.push(REPORT_OK);
        e.push(0x13);
        // E0 00 size=4 (status, report, 0x13, sum) status report data sum
        let sum = (0x00 + 0x04 + 0x01 + 0x01 + 0x13) as u8;
        assert_eq!(e.finish(), vec![0xE0, 0x00, 0x04, 0x01, 0x01, 0x13, sum]);
    }

    #[test]
    fn escaping() {
        let mut e = Encoder::new();
        e.push(REPORT_OK);
        e.push(0xE0);
        let out = e.finish();
        // E0 escaped as D0 DF, counted once in the size, summed as 0xDF + 1
        assert_eq!(&out[..7], &[0xE0, 0x00, 0x04, 0x01, 0x01, 0xD0, 0xDF]);
        let sum = (0x00u32 + 0x04 + 0x01 + 0x01 + 0xE0) & 0xFF;
        assert_eq!(out[7] as u32, sum);
    }

    #[test]
    fn parse_request() {
        let r = parse(&[0xE0, 0xFF, 0x03, 0xF0, 0xD9, 0xCB]).unwrap();
        assert_eq!(r.node, 0xFF);
        assert_eq!(r.commands, &[0xF0, 0xD9]);
    }
}
