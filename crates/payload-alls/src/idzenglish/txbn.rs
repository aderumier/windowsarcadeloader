//! The game's text files (`TxtMessage\data_xx\txbn\X.txbn`), little endian:
//! - header: `TXBN`, the entries' count, the lines' count (all entries)
//! - the entries, 23 `u32`s (0x5C bytes) each: [0..=9] its display settings (26, 9, 0, 0, 1, 1,
//!   0, color 0xffffff, 0, -1: kept as they are), [10] its lines' count (its text's `\n`s + 1),
//!   [11] its first line's number (the previous entries' lines), [12] its text's size in bytes
//!   (UTF-16, its null included), [13] its text's offset in the strings, [14] [15] [19] 0,
//!   [16..=18] [20..=22] its text's end (offset + size)
//! - each line's size in bytes (UTF-16, without the `\n` and the null), the lines' count of them
//! - the strings: each entry's text in UTF-16LE, null terminated, in the entries' order, up to
//!   the file's end
//!
//! A text replaced gets its entry's lines, size and offsets, the following entries' line
//! numbers and offsets, and the lines' sizes recomputed.

/// One entry: its 23 fields as read (the computed ones rewritten on save) and its text.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Entry {
    pub fields: [u32; 23],
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Txbn {
    pub entries: Vec<Entry>,
}

const MAGIC: &[u8; 4] = b"TXBN";
const FIELDS: usize = 23;
const LINES: usize = 10;
const FIRST_LINE: usize = 11;
const SIZE: usize = 12;
const OFFSET: usize = 13;
const ZERO: [usize; 3] = [14, 15, 19];
const END: [usize; 6] = [16, 17, 18, 20, 21, 22];

fn u32_at(data: &[u8], at: usize) -> Result<u32, String> {
    data.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).ok_or_else(|| format!("truncated at {at:#x}"))
}

impl Txbn {
    /// Parses `data`, checking every computed field (a file that would not save back to the
    /// same bytes is refused).
    pub(crate) fn parse(data: &[u8]) -> Result<Txbn, String> {
        if data.get(..4) != Some(MAGIC) {
            return Err("no TXBN magic".into());
        }
        let count = u32_at(data, 4)? as usize;
        let lines = u32_at(data, 8)? as usize;
        let table = 12usize.checked_add(count.checked_mul(FIELDS * 4).ok_or("bad count")?).ok_or("bad count")?;
        let pool = table.checked_add(lines.checked_mul(4).ok_or("bad lines")?).ok_or("bad lines")?;
        if pool > data.len() {
            return Err(format!("{count} entries / {lines} lines past the file's end"));
        }
        let mut entries = Vec::with_capacity(count);
        let (mut line, mut offset) = (0usize, 0usize);
        for i in 0..count {
            let mut fields = [0u32; FIELDS];
            for (f, v) in fields.iter_mut().enumerate() {
                *v = u32_at(data, 12 + i * FIELDS * 4 + f * 4)?;
            }
            let size = fields[SIZE] as usize;
            if fields[FIRST_LINE] as usize != line || fields[OFFSET] as usize != offset {
                return Err(format!("entry {i}: line {} offset {} (expected {line} {offset})", fields[FIRST_LINE], fields[OFFSET]));
            }
            if size < 2 || size % 2 != 0 || END.iter().any(|&f| fields[f] as usize != offset + size) || ZERO.iter().any(|&f| fields[f] != 0) {
                return Err(format!("entry {i}: unexpected size/end fields {:?}", &fields[LINES..]));
            }
            let bytes = data.get(pool + offset..pool + offset + size).ok_or_else(|| format!("entry {i}: text past the file's end"))?;
            let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let (last, body) = units.split_last().unwrap();
            if *last != 0 || body.contains(&0) {
                return Err(format!("entry {i}: text not a single null terminated string"));
            }
            let text = String::from_utf16(body).map_err(|_| format!("entry {i}: invalid UTF-16"))?;
            let n = fields[LINES] as usize;
            let mut sizes = Vec::with_capacity(n);
            for l in line..line + n {
                if l >= lines {
                    return Err(format!("entry {i}: line {l} past the lines' count {lines}"));
                }
                sizes.push(u32_at(data, table + l * 4)?);
            }
            if sizes != line_sizes(&text) {
                return Err(format!("entry {i}: lines' sizes {sizes:?} do not match its text"));
            }
            line += n;
            offset += size;
            entries.push(Entry { fields, text });
        }
        if line != lines {
            return Err(format!("{line} lines in the entries, {lines} in the header"));
        }
        if pool + offset != data.len() {
            return Err(format!("{} bytes after the strings", data.len() as isize - (pool + offset) as isize));
        }
        Ok(Txbn { entries })
    }

    /// The file's bytes, every computed field from the texts.
    pub(crate) fn serialize(&self) -> Vec<u8> {
        let units: Vec<Vec<u16>> = self.entries.iter().map(|e| e.text.encode_utf16().chain([0]).collect()).collect();
        let sizes: Vec<Vec<u32>> = self.entries.iter().map(|e| line_sizes(&e.text)).collect();
        let lines: usize = sizes.iter().map(Vec::len).sum();
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        out.extend_from_slice(&(lines as u32).to_le_bytes());
        let (mut line, mut offset) = (0u32, 0u32);
        for ((e, u), s) in self.entries.iter().zip(&units).zip(&sizes) {
            let mut f = e.fields;
            let size = u.len() as u32 * 2;
            f[LINES] = s.len() as u32;
            f[FIRST_LINE] = line;
            f[SIZE] = size;
            f[OFFSET] = offset;
            for z in ZERO {
                f[z] = 0;
            }
            for end in END {
                f[end] = offset + size;
            }
            for v in f {
                out.extend_from_slice(&v.to_le_bytes());
            }
            line += s.len() as u32;
            offset += size;
        }
        for v in sizes.iter().flatten() {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in units.iter().flatten() {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }
}

/// Each line's size in bytes (UTF-16, without the `\n`).
fn line_sizes(text: &str) -> Vec<u32> {
    text.split('\n').map(|l| l.encode_utf16().count() as u32 * 2).collect()
}

/// Whether `text` has Japanese (kana, kanji, half-width katakana).
pub(crate) fn has_japanese(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(c as u32,
            0x3005 | 0x3040..=0x30FF | 0x31F0..=0x31FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF66..=0xFF9F)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file as the game's tools write it, from its texts.
    fn build(texts: &[&str]) -> Vec<u8> {
        let mut out = b"TXBN".to_vec();
        let lines: usize = texts.iter().map(|t| t.split('\n').count()).sum();
        out.extend_from_slice(&(texts.len() as u32).to_le_bytes());
        out.extend_from_slice(&(lines as u32).to_le_bytes());
        let (mut line, mut offset) = (0u32, 0u32);
        for (i, t) in texts.iter().enumerate() {
            let size = (t.encode_utf16().count() as u32 + 1) * 2;
            let n = t.split('\n').count() as u32;
            let end = offset + size;
            let color = if i == 1 { 0 } else { 0xffffff };
            for v in [26, 9, 0, 0, 1, 1, 0, color, 0, u32::MAX, n, line, size, offset, 0, 0, end, end, end, 0, end, end, end] {
                out.extend_from_slice(&v.to_le_bytes());
            }
            line += n;
            offset = end;
        }
        for t in texts {
            for l in t.split('\n') {
                out.extend_from_slice(&(l.encode_utf16().count() as u32 * 2).to_le_bytes());
            }
        }
        for t in texts {
            for u in t.encode_utf16().chain([0]) {
                out.extend_from_slice(&u.to_le_bytes());
            }
        }
        out
    }

    #[test]
    fn round_trip() {
        let texts = ["\u{3000}", "プレイヤー", "Free Continue Ticket", "きついコーナーは\n手前でブレーキ！", "", "a\n\nb"];
        let data = build(&texts);
        let t = Txbn::parse(&data).unwrap();
        assert_eq!(t.entries.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), texts);
        assert_eq!(t.serialize(), data);
    }

    #[test]
    fn replaced_text() {
        let data = build(&["プレイヤー", "きついコーナーは\n手前でブレーキ！", "テスト"]);
        let mut t = Txbn::parse(&data).unwrap();
        t.entries[1].text = "Brake before\ntight corners\nto make them!".into();
        t.entries[0].text = "P".into();
        let out = t.serialize();
        assert_eq!(out, build(&["P", "Brake before\ntight corners\nto make them!", "テスト"]));
        let back = Txbn::parse(&out).unwrap();
        assert_eq!(back.entries.iter().map(|e| &e.text).collect::<Vec<_>>(), t.entries.iter().map(|e| &e.text).collect::<Vec<_>>());
        assert_eq!(back.entries[1].fields[10..14], [3, 1, 82, 4]);
        assert_eq!(back.entries[2].fields[11..14], [4, 8, 86]);
        assert_eq!(back.entries[1].fields[7], 0);
        assert_eq!(back.serialize(), out);
    }

    #[test]
    fn refused() {
        let mut data = build(&["プレイヤー", "テスト"]);
        assert!(Txbn::parse(&data[..data.len() - 1]).is_err());
        data.extend_from_slice(&[0, 0]);
        assert!(Txbn::parse(&data).is_err());
        assert!(Txbn::parse(b"TXBN\xff\xff\xff\xff\0\0\0\0").is_err());
    }

    #[test]
    fn japanese() {
        assert!(has_japanese("Ｅｒｒｏｒ１００１\nゲーム"));
        assert!(has_japanese("峠"));
        assert!(has_japanese("ｱ"));
        assert!(!has_japanese("ＥＲＲＯＲ\u{3000}０００８ ［START］ １８０°"));
    }

    /// Every file of the dump (both languages) and of the patch saves back to its bytes, and
    /// with every text lengthened parses back (`cargo test -- --ignored`, from the repository).
    #[test]
    #[ignore]
    fn game_files() {
        let game = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../games/seganu/Initial D - Arcade Stage Zero");
        let dirs = ["app/package/data/TxtMessage/data_jp/txbn", "app/package/data/TxtMessage/data_ex/txbn", "english/data/TxtMessage/data_ex/txbn"];
        let mut n = 0;
        for dir in dirs {
            for f in std::fs::read_dir(game.join(dir)).unwrap() {
                let path = f.unwrap().path();
                let data = std::fs::read(&path).unwrap();
                let mut t = Txbn::parse(&data).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                assert_eq!(t.serialize(), data, "{}", path.display());
                for e in &mut t.entries {
                    e.text = format!("{} longer\nline", e.text);
                }
                let out = t.serialize();
                let back = Txbn::parse(&out).unwrap_or_else(|e| panic!("{} lengthened: {e}", path.display()));
                assert!(back.entries.iter().zip(&t.entries).all(|(a, b)| a.text == b.text), "{}", path.display());
                assert_eq!(back.serialize(), out);
                n += 1;
            }
        }
        assert_eq!(n, 105);
    }
}
