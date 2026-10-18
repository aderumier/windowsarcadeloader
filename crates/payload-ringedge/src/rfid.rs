//! The RFID chip in an Initial D card (read and written through the CRP-1231BR's `D1`
//! command, sub-commands from `picodaemon.exe`, the cabinet's card daemon).
//!
//! The chip: 2 KB in 8-byte units, a 32-bit serial. Unit 0-15 a header (`BML ` at 0x10, four
//! big-endian counters at 0x30, the game's identifiers at 0x40), then 15 pages of 128 bytes of
//! player data. A factory blank card: counters 0xFFF0, `InitialD` + `AA 55` at 0x40, 0xFF
//! elsewhere (picodaemon then reports a new card).
//!
//! Sub-commands (the reply: a status byte, 0x0B success, then the data):
//! * `F7`, `48`, `28`, `38`, `2D <serial> <response>` (authentication), `AD <serial>`: the
//!   reader's carrier and the chip's selection, accepted.
//! * `4D 00..`: the chip's information, the serial in bytes 4-7.
//! * `0D <unit:2>`: 128 bytes from a unit.
//! * `ED`: the write pointer to 0; `8D <offset:2>`: the next write's unit relative to it;
//!   `CD <8 bytes>`: writes them there (picodaemon keeps track of the pointer).
//! * `6D 00 <mask>`: decrements a counter (mask C0, 30, 0C, 03: counter 0-3); picodaemon
//!   decrements counter 0 before writing the player data, counter 1 after (a write cut
//!   in the middle: they differ).

use std::path::Path;

use wal_payload_common::log;

pub(crate) const SIZE: usize = 2048;
const OK: u8 = 0x0B;

pub(crate) struct Chip {
    pub(crate) memory: Vec<u8>,
    /// The write pointer (`ED`) and the next write's offset from it (`8D`).
    pointer: usize,
    offset: usize,
    pub(crate) changed: bool,
}

impl Chip {
    /// A factory blank card with a new serial.
    pub(crate) fn blank() -> Chip {
        let mut memory = vec![0xFF; SIZE];
        let serial = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0x1234_5678, |d| d.as_nanos() as u32 ^ (d.as_secs() as u32).rotate_left(16));
        memory[0..4].copy_from_slice(&serial.to_be_bytes());
        memory[4..0x10].fill(0);
        memory[0x10..0x18].copy_from_slice(b"BML \0\0\0\0");
        memory[0x18..0x30].fill(0);
        for counter in memory[0x30..0x38].chunks_mut(2) {
            counter.copy_from_slice(&0xFFF0u16.to_be_bytes());
        }
        memory[0x40..0x48].copy_from_slice(b"InitialD");
        memory[0x48..0x4A].copy_from_slice(&[0xAA, 0x55]);
        Chip { memory, pointer: 0, offset: 0, changed: true }
    }

    /// The card's chip from its file (a blank one when missing or invalid).
    pub(crate) fn load(path: &Path) -> Chip {
        match std::fs::read(path) {
            Ok(memory) if memory.len() == SIZE => Chip { memory, pointer: 0, offset: 0, changed: false },
            Ok(memory) => {
                log!("card: {} has a wrong size ({}): blank chip", path.display(), memory.len());
                Chip::blank()
            }
            Err(_) => {
                log!("card: no {}: blank chip", path.display());
                Chip::blank()
            }
        }
    }

    pub(crate) fn save(&mut self, path: &Path) {
        if !self.changed {
            return;
        }
        match std::fs::write(path, &self.memory) {
            Ok(()) => self.changed = false,
            Err(e) => log!("card: {}: {e}", path.display()),
        }
    }

    fn serial(&self) -> [u8; 4] {
        self.memory[0..4].try_into().unwrap()
    }

    /// A `D1` sub-command (`params`): the reply data.
    pub(crate) fn command(&mut self, params: &[u8]) -> Vec<u8> {
        let word = |at: usize| u16::from_be_bytes([params.get(at).copied().unwrap_or(0), params.get(at + 1).copied().unwrap_or(0)]) as usize;
        let sub = params.first().copied().unwrap_or(0);
        match sub {
            0x4D => {
                let mut data = vec![OK, 0x00, 0x00, 0x00, 0x00];
                data.extend_from_slice(&self.serial());
                data
            }
            0x0D => {
                let at = word(1) * 8 % SIZE;
                let mut data = vec![OK];
                data.extend((0..128).map(|i| self.memory[(at + i) % SIZE]));
                data
            }
            0xED => {
                self.pointer = 0;
                self.offset = 0;
                vec![OK]
            }
            0x8D => {
                self.offset = word(1);
                vec![OK]
            }
            0xCD => {
                let at = (self.pointer + self.offset) * 8;
                self.offset = 0;
                if at + 8 <= SIZE && params.len() >= 9 {
                    self.memory[at..at + 8].copy_from_slice(&params[1..9]);
                    self.changed = true;
                } else {
                    log!("card: chip write out of range: unit {} {params:02X?}", at / 8);
                }
                vec![OK]
            }
            0x6D => {
                let mask = params.get(2).copied().unwrap_or(0);
                for (i, bits) in [0xC0u8, 0x30, 0x0C, 0x03].into_iter().enumerate() {
                    if mask & bits != 0 {
                        let at = 0x30 + 2 * i;
                        let value = u16::from_be_bytes([self.memory[at], self.memory[at + 1]]).wrapping_sub(1);
                        self.memory[at..at + 2].copy_from_slice(&value.to_be_bytes());
                        self.changed = true;
                    }
                }
                vec![OK]
            }
            _ => vec![OK, 0x00, 0x00],
        }
    }
}
