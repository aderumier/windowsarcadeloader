//! DRP archives: an encrypted big-endian container of zlib-compressed files.
//!
//! The format follows Sammi Husky's Pokken-Tools (MIT): a 0x60-byte header (file count at
//! 0x16, the key stream seed at 0x1c), then per file a 0x60-byte entry header (name, types,
//! size, sub-file count, sub-file end offsets) and each sub-file as its decompressed size
//! and zlib data, aligned to 0x10. The whole archive is XOR-ed, 32-bit big-endian words,
//! with a xorshift key stream seeded from the plain word at 0x1c.

use std::io::{Read, Write};

use anyhow::{Context, Result, bail, ensure};
use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;

const HEADER: usize = 0x60;
const ENTRY_HEADER: usize = 0x60;

/// The key stream: 4 words from a linear congruential generator, then xorshift.
struct KeyStream([i32; 4]);

impl KeyStream {
    fn new(seed: i32) -> KeyStream {
        let mut s = [0i32; 4];
        let mut v = seed;
        for w in &mut s {
            v = v.wrapping_mul(0x41C6_4E6D).wrapping_add(0x3039);
            *w = v;
        }
        KeyStream(s)
    }

    fn next(&mut self) -> u32 {
        let s = &mut self.0;
        let t = s[3] ^ (s[3] << 11);
        let x = s[0] ^ (s[0] >> 19) ^ t ^ (t >> 8);
        let tmp = s[1];
        s[1] = s[0];
        s[3] = s[2];
        s[2] = tmp;
        s[0] = x;
        (x & 0x7fff_ffff) as u32
    }
}

/// Decrypts or encrypts (the same operation) a whole archive, its seed word left plain.
fn crypt(data: &mut [u8]) {
    let seed = i32::from_be_bytes(data[0x1c..0x20].try_into().unwrap());
    let mut keys = KeyStream::new(seed);
    let mut chain = 0u32;
    for (i, word) in data.chunks_exact_mut(4).enumerate() {
        let key = keys.next();
        if i != 7 {
            let v = u32::from_be_bytes((&*word).try_into().unwrap()) ^ key ^ chain;
            word.copy_from_slice(&v.to_be_bytes());
        }
        chain = (key << 13) & 0x8000_0000;
    }
}

fn be16(d: &[u8], o: usize) -> u16 {
    u16::from_be_bytes([d[o], d[o + 1]])
}

fn be32(d: &[u8], o: usize) -> u32 {
    u32::from_be_bytes(d[o..o + 4].try_into().unwrap())
}

/// One file of an archive: its entry header, kept as read, and its sub-files decompressed.
pub struct Entry {
    pub name: String,
    header: Vec<u8>,
    pub files: Vec<Vec<u8>>,
}

pub struct Archive {
    header: Vec<u8>,
    pub entries: Vec<Entry>,
}

impl Archive {
    pub fn read(encrypted: &[u8]) -> Result<Archive> {
        ensure!(encrypted.len() >= HEADER && encrypted.len() % 4 == 0, "not a DRP archive");
        let mut d = encrypted.to_vec();
        crypt(&mut d);
        let count = be16(&d, 0x16) as usize;
        let mut entries = Vec::with_capacity(count);
        let mut p = HEADER;
        for _ in 0..count {
            ensure!(p + ENTRY_HEADER <= d.len(), "entry header past the end");
            let h = &d[p..p + ENTRY_HEADER];
            let name = String::from_utf8_lossy(&h[..h[..0x40].iter().position(|&b| b == 0).unwrap_or(0x40)]).into_owned();
            let size = be32(h, 0x44) as usize;
            let subfiles = be16(h, 0x4a) as usize;
            ensure!(subfiles <= 4 && p + size <= d.len(), "{name}: bad entry size");
            let mut files = Vec::new();
            let mut start = 0;
            for i in 0..subfiles {
                let end = be32(h, 0x50 + 4 * i) as usize;
                let f = &d[p + ENTRY_HEADER + start..p + ENTRY_HEADER + end];
                let len = be32(f, 0) as usize;
                let mut out = Vec::with_capacity(len);
                ZlibDecoder::new(&f[4..]).read_to_end(&mut out).with_context(|| format!("{name}: zlib"))?;
                ensure!(out.len() == len, "{name}: {} bytes, header says {len}", out.len());
                files.push(out);
                start = end;
            }
            entries.push(Entry { name, header: h.to_vec(), files });
            p += size;
        }
        Ok(Archive { header: d[..HEADER].to_vec(), entries })
    }

    pub fn write(&self) -> Result<Vec<u8>> {
        let mut d = self.header.clone();
        for e in &self.entries {
            let mut body = Vec::new();
            let mut ends = Vec::new();
            for f in &e.files {
                let mut z = ZlibEncoder::new(Vec::new(), Compression::best());
                z.write_all(f)?;
                body.extend_from_slice(&(f.len() as u32).to_be_bytes());
                body.extend_from_slice(&z.finish()?);
                body.resize(body.len().next_multiple_of(0x10), 0);
                ends.push(body.len() as u32);
            }
            if ends.is_empty() {
                bail!("{}: no sub-file", e.name);
            }
            let mut h = e.header.clone();
            h[0x44..0x48].copy_from_slice(&((ENTRY_HEADER + body.len()) as u32).to_be_bytes());
            for i in 0..4 {
                let end = ends.get(i).or(ends.last()).unwrap();
                h[0x50 + 4 * i..0x54 + 4 * i].copy_from_slice(&end.to_be_bytes());
            }
            d.extend_from_slice(&h);
            d.extend_from_slice(&body);
        }
        crypt(&mut d);
        Ok(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crypt_round_trip() {
        let mut d: Vec<u8> = (0..0x100u32).map(|i| (i * 7) as u8).collect();
        let plain = d.clone();
        crypt(&mut d);
        assert_ne!(d, plain);
        assert_eq!(d[0x1c..0x20], plain[0x1c..0x20]);
        crypt(&mut d);
        assert_eq!(d, plain);
    }
}
