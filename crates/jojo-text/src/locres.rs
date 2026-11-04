//! A UE4 localization resource (`.locres`), version 1 ("compact"): the magic GUID, the version,
//! the offset of the string array, then per namespace its keys, each with the CRC of the
//! source it translates (a translation of another source is ignored) and an index in the
//! string array.

use std::collections::BTreeMap;

const MAGIC: [u32; 4] = [0x7574140E, 0xFC034A67, 0x9D90154A, 0x1B7F37C3];

/// `FCrc::StrCrc32`: the CRC-32 of each UTF-16 unit taken as 4 little-endian bytes.
pub fn str_crc32(s: &str) -> u32 {
    let mut table = [0u32; 256];
    for (i, t) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB88320 ^ (c >> 1) } else { c >> 1 };
        }
        *t = c;
    }
    let mut crc = !0u32;
    for unit in s.encode_utf16() {
        let mut c = unit as u32;
        for _ in 0..4 {
            crc = (crc >> 8) ^ table[((crc ^ c) & 0xFF) as usize];
            c >>= 8;
        }
    }
    !crc
}

fn fstring(out: &mut Vec<u8>, s: &str) {
    let w: Vec<u16> = s.encode_utf16().collect();
    if w.is_empty() {
        out.extend(0i32.to_le_bytes());
        return;
    }
    out.extend((-(w.len() as i32 + 1)).to_le_bytes());
    for c in w {
        out.extend(c.to_le_bytes());
    }
    out.extend([0, 0]);
}

/// Namespace -> key -> (source, translation).
#[derive(Default)]
pub struct Resource(pub BTreeMap<String, BTreeMap<String, (String, String)>>);

impl Resource {
    pub fn write(&self) -> Vec<u8> {
        let mut strings: Vec<&str> = Vec::new();
        let mut index = BTreeMap::new();
        let mut out = Vec::new();
        for m in MAGIC {
            out.extend(m.to_le_bytes());
        }
        out.push(1);
        let offset_at = out.len();
        out.extend(0i64.to_le_bytes());
        out.extend((self.0.len() as u32).to_le_bytes());
        for (namespace, keys) in &self.0 {
            fstring(&mut out, namespace);
            out.extend((keys.len() as u32).to_le_bytes());
            for (key, (source, translation)) in keys {
                let i = *index.entry(translation.as_str()).or_insert_with(|| {
                    strings.push(translation);
                    strings.len() - 1
                });
                fstring(&mut out, key);
                out.extend(str_crc32(source).to_le_bytes());
                out.extend((i as i32).to_le_bytes());
            }
        }
        let offset = out.len() as i64;
        out[offset_at..offset_at + 8].copy_from_slice(&offset.to_le_bytes());
        out.extend((strings.len() as i32).to_le_bytes());
        for s in strings {
            fstring(&mut out, s);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_is_crc32_of_4_byte_units() {
        // the CRC-32 of 61 00 00 00, of "Fail" as 4-byte units
        assert_eq!(str_crc32("a"), 0xA2DE4F7A);
        assert_eq!(str_crc32("Fail"), 0x9CF70707);
    }
}
