//! NTXL string tables, little-endian: a header (size at 0x10, entry count at 0x14, entry
//! table offset at 0x18), entries of (label offset, text offset, 0), and at the end of the
//! file the texts, each a chain of records: type and length (u16), data padded to 4 bytes.
//! Type 2 is UTF-16 text (null included), 1 a line break, 3 a number slot, 4 / 5 / 6 / 11
//! styles, 0 ends the text. Labels are a u16 hash, a u16 length and the ASCII name.

use anyhow::{Result, ensure};

const NO_OFFSET: u32 = 0xffff_ffff;
pub const TEXT: u16 = 2;

fn le16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}

fn le32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(d[o..o + 4].try_into().unwrap())
}

pub struct Record {
    pub kind: u16,
    pub data: Vec<u8>,
}

impl Record {
    pub fn text(&self) -> Option<String> {
        if self.kind != TEXT {
            return None;
        }
        let units: Vec<u16> = self.data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
        Some(String::from_utf16_lossy(&units[..end]))
    }

    pub fn set_text(&mut self, s: &str) {
        self.data = s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
    }
}

pub struct Entry {
    pub label: String,
    /// None: no text.
    pub records: Option<Vec<Record>>,
}

pub struct Table {
    /// Everything before the texts, offsets patched on write.
    head: Vec<u8>,
    table: usize,
    tail: Vec<u8>,
    pub entries: Vec<Entry>,
}

impl Table {
    pub fn is_table(d: &[u8]) -> bool {
        d.starts_with(b"NTXL")
    }

    pub fn read(d: &[u8]) -> Result<Table> {
        ensure!(Table::is_table(d) && d.len() >= 0x40, "not an NTXL table");
        ensure!(le32(d, 0x10) as usize == d.len(), "NTXL size mismatch");
        let count = le16(d, 0x14) as usize;
        let table = le32(d, 0x18) as usize;
        ensure!(table + 12 * count <= d.len(), "NTXL entry table past the end");
        let mut entries = Vec::with_capacity(count);
        let mut first = d.len() - 4;
        for i in 0..count {
            let o = table + 12 * i;
            let (label_at, text_at) = (le32(d, o), le32(d, o + 4));
            let label = if label_at == NO_OFFSET {
                String::new()
            } else {
                let a = label_at as usize + 4;
                let end = d[a..].iter().position(|&b| b == 0).map_or(d.len(), |n| a + n);
                String::from_utf8_lossy(&d[a..end]).into_owned()
            };
            let records = if text_at == NO_OFFSET {
                None
            } else {
                let mut p = text_at as usize;
                first = first.min(p);
                let mut records = Vec::new();
                loop {
                    ensure!(p + 4 <= d.len(), "{label}: text past the end");
                    let (kind, len) = (le16(d, p), le16(d, p + 2) as usize);
                    ensure!(p + 4 + len <= d.len(), "{label}: record past the end");
                    records.push(Record { kind, data: d[p + 4..p + 4 + len].to_vec() });
                    p += 4 + len.next_multiple_of(4);
                    if kind == 0 {
                        break;
                    }
                }
                Some(records)
            };
            entries.push(Entry { label, records });
        }
        // Texts are the end of the file: nothing before them points into them.
        if entries.iter().any(|e| e.records.is_some()) {
            for o in (0x18..0x40).step_by(4) {
                let v = le32(d, o) as usize;
                ensure!(v < first, "NTXL header field {o:#x} points into the texts");
            }
        }
        Ok(Table { head: d[..first].to_vec(), table, tail: d[d.len() - 4..].to_vec(), entries })
    }

    pub fn write(&self) -> Vec<u8> {
        let mut d = self.head.clone();
        for (i, e) in self.entries.iter().enumerate() {
            let at = match &e.records {
                None => NO_OFFSET,
                Some(records) => {
                    let at = d.len() as u32;
                    for r in records {
                        d.extend_from_slice(&r.kind.to_le_bytes());
                        d.extend_from_slice(&(r.data.len() as u16).to_le_bytes());
                        d.extend_from_slice(&r.data);
                        d.resize(d.len().next_multiple_of(4), 0);
                    }
                    at
                }
            };
            let o = self.table + 12 * i + 4;
            d[o..o + 4].copy_from_slice(&at.to_le_bytes());
        }
        d.extend_from_slice(&self.tail);
        let len = d.len() as u32;
        d[0x10..0x14].copy_from_slice(&len.to_le_bytes());
        d
    }
}
