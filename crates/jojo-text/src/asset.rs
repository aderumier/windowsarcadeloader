//! The localizable texts of UE 4.20 cooked exports (`.uexp`), found by their serialized form
//! rather than by parsing every property:
//!
//! - an `FText` with a base history (a property, a struct member): `int32 flags`, `int8 0`,
//!   then the namespace, key and source `FString`s;
//! - a Blueprint text literal (`EX_TextConst` 0x29, `LocalizedText` 1): the source, key and
//!   namespace as string expressions (`EX_StringConst` 0x1F, ANSI, or `EX_UnicodeStringConst`
//!   0x34, UTF-16, both zero-terminated);
//! - a string table (`SDT_*`): after the export's property terminator and its no-GUID flag,
//!   the table's namespace and its key / source `FString` pairs.
//!
//! An `FString` is an `int32` length with the terminator, positive for ANSI, negative for
//! UTF-16. Texts are keyed by namespace and key at run time, so a `Game.locres` replaces them
//! without touching the assets.

use anyhow::{Result, ensure};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Text {
    pub namespace: String,
    pub key: String,
    pub source: String,
}

/// Japanese (kana, CJK ideographs, full-width forms): the texts left to translate.
pub fn is_japanese(s: &str) -> bool {
    s.encode_utf16().any(|c| matches!(c, 0x3040..=0x30FF | 0x4E00..=0x9FFF | 0xFF01..=0xFF9F))
}

/// The `FString` at `i` and the offset after it. Only plausible strings: no control
/// characters but line breaks and tabs, ANSI ones printable ASCII.
pub fn fstring(b: &[u8], i: usize) -> Option<(String, usize)> {
    let n = i32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?);
    let start = i + 4;
    match n {
        0 => Some((String::new(), start)),
        1..4096 => {
            let s = b.get(start..start + n as usize)?;
            let (&0, text) = s.split_last()? else { return None };
            text.iter()
                .all(|&c| (0x20..0x7f).contains(&c) || matches!(c, 9 | 10 | 13))
                .then(|| (String::from_utf8_lossy(text).into_owned(), start + s.len()))
        }
        -4095..0 => {
            let len = -n as usize;
            let s = b.get(start..start + 2 * len)?;
            let w: Vec<u16> = s.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let (&0, text) = w.split_last()? else { return None };
            text.iter()
                .all(|&c| c >= 0x20 || matches!(c, 9 | 10 | 13))
                .then(|| (String::from_utf16_lossy(text), start + s.len()))
        }
        _ => None,
    }
}

/// A Kismet string expression at `i` and the offset after it.
fn script_string(b: &[u8], i: usize) -> Option<(String, usize)> {
    match *b.get(i)? {
        0x1F => {
            let z = b.get(i + 1..)?.iter().position(|&c| c == 0)?;
            Some((String::from_utf8_lossy(&b[i + 1..i + 1 + z]).into_owned(), i + 2 + z))
        }
        0x34 => {
            let mut w = Vec::new();
            let mut k = i + 1;
            loop {
                let c = u16::from_le_bytes([*b.get(k)?, *b.get(k + 1)?]);
                k += 2;
                if c == 0 {
                    return Some((String::from_utf16_lossy(&w), k));
                }
                w.push(c);
            }
        }
        _ => None,
    }
}

/// An `FText` with a base history at `i`.
fn base_text(b: &[u8], i: usize) -> Option<(Text, usize)> {
    let flags = u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?);
    if flags >= 0x100 || *b.get(i + 4)? != 0 {
        return None;
    }
    let (namespace, j) = fstring(b, i + 5)?;
    let (key, k) = fstring(b, j)?;
    if key.is_empty() || !key.is_ascii() {
        return None;
    }
    let (source, end) = fstring(b, k)?;
    Some((Text { namespace, key, source }, end))
}

/// A Blueprint localized text literal at `i`.
fn script_text(b: &[u8], i: usize) -> Option<(Text, usize)> {
    if b.get(i..i + 2)? != [0x29, 1] {
        return None;
    }
    let (source, j) = script_string(b, i + 2)?;
    let (key, k) = script_string(b, j)?;
    let (namespace, end) = script_string(b, k)?;
    (!key.is_empty()).then_some((Text { namespace, key, source }, end))
}

/// The keyed texts of an export whose source `keep` accepts.
pub fn texts(b: &[u8], keep: impl Fn(&str) -> bool) -> Vec<Text> {
    let mut v = Vec::new();
    let mut i = 0;
    while i + 9 < b.len() {
        if let Some((t, end)) = base_text(b, i).or_else(|| script_text(b, i)) {
            if keep(&t.source) {
                v.push(t);
                i = end;
                continue;
            }
        }
        i += 1;
    }
    v
}

/// A string table export (`SDT_*`): every entry, namespace the table's.
pub fn string_table(b: &[u8]) -> Result<Vec<Text>> {
    // the property terminator (the name `None`, 8 bytes) and no object GUID (4 bytes)
    let (namespace, mut i) = fstring(b, 12).ok_or_else(|| anyhow::anyhow!("no table namespace"))?;
    let n = i32::from_le_bytes(b.get(i..i + 4).ok_or_else(|| anyhow::anyhow!("truncated"))?.try_into()?);
    ensure!((0..100_000).contains(&n), "{n} entries");
    i += 4;
    let mut v = Vec::new();
    for _ in 0..n {
        let (key, j) = fstring(b, i).ok_or_else(|| anyhow::anyhow!("bad key at {i:#x}"))?;
        let (source, k) = fstring(b, j).ok_or_else(|| anyhow::anyhow!("bad source at {j:#x}"))?;
        v.push(Text { namespace: namespace.clone(), key, source });
        i = k;
    }
    Ok(v)
}
