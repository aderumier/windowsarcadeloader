//! A UE 4.20 cooked package (`.uasset` header, `.uexp` export data): the name map, the export
//! table and each export's tagged properties, enough to follow widget trees (a panel's
//! `Slots`, a slot's `Content`, a text block's `Text`).
//!
//! A tagged property: its name and type (`FName`s), the value size (`int32`), the array index
//! (`int32`), the type's extras (a struct's name and GUID, an enum or byte property's enum
//! name, an array's or set's inner type, a map's key and value types, a bool's value), a
//! property GUID flag (and GUID); then the value. A `None` name ends the list.

use anyhow::{Context, Result, bail, ensure};

use crate::asset::{self, Text};

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Result<&[u8]> {
        let s = self.b.get(self.i..self.i + n).with_context(|| format!("truncated at {:#x}", self.i))?;
        self.i += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.bytes(4)?.try_into()?))
    }
    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.bytes(8)?.try_into()?))
    }
    fn fstring(&mut self) -> Result<String> {
        let (s, end) = asset::fstring(self.b, self.i).with_context(|| format!("bad string at {:#x}", self.i))?;
        self.i = end;
        Ok(s)
    }
    fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }
}

/// A property value the widget trees need.
#[derive(Debug)]
pub enum Value {
    /// An object reference: an export (`> 0`, 1-based), an import (`< 0`) or none.
    Object(i32),
    Objects(Vec<i32>),
    Text(Text),
    Other,
}

pub struct Export {
    pub properties: Vec<(String, Value)>,
}

impl Export {
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.properties.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

pub struct Package {
    pub exports: Vec<Export>,
}

fn name(names: &[String], r: &mut Reader) -> Result<String> {
    let index = r.i32()?;
    let number = r.i32()?;
    let n = names.get(index as usize).with_context(|| format!("name {index}"))?;
    Ok(if number > 0 { format!("{n}_{}", number - 1) } else { n.clone() })
}

/// The tagged properties at the reader's offset, up to `None`.
fn properties(names: &[String], r: &mut Reader) -> Result<Vec<(String, Value)>> {
    let mut v = Vec::new();
    loop {
        let prop = name(names, r)?;
        if prop == "None" {
            return Ok(v);
        }
        let ty = name(names, r)?;
        let size = r.i32()? as usize;
        let _array_index = r.i32()?;
        let mut inner = String::new();
        match ty.as_str() {
            "StructProperty" => {
                name(names, r)?;
                r.skip(16)?;
            }
            "BoolProperty" => r.skip(1)?,
            "ByteProperty" | "EnumProperty" => {
                name(names, r)?;
            }
            "ArrayProperty" | "SetProperty" => inner = name(names, r)?,
            "MapProperty" => {
                name(names, r)?;
                name(names, r)?;
            }
            _ => {}
        }
        if r.u8()? != 0 {
            r.skip(16)?;
        }
        let start = r.i;
        let value = match ty.as_str() {
            "ObjectProperty" => Value::Object(r.i32()?),
            "ArrayProperty" if inner == "ObjectProperty" => {
                let n = r.i32()?;
                Value::Objects((0..n).map(|_| r.i32()).collect::<Result<_>>()?)
            }
            "TextProperty" => {
                let mut t = Reader { b: r.b, i: start };
                let _flags = t.i32()?;
                if t.u8()? == 0 {
                    let namespace = t.fstring()?;
                    let key = t.fstring()?;
                    let source = t.fstring()?;
                    Value::Text(Text { namespace, key, source })
                } else {
                    Value::Other
                }
            }
            _ => Value::Other,
        };
        r.i = start + size;
        v.push((prop, value));
    }
}

impl Package {
    /// Reads the package of `uasset` and `uexp`.
    pub fn read(uasset: &[u8], uexp: &[u8]) -> Result<Package> {
        let mut r = Reader { b: uasset, i: 0 };
        ensure!(r.i32()? as u32 == 0x9E2A83C1, "not a package");
        let legacy = r.i32()?;
        ensure!(legacy == -7, "legacy version {legacy}");
        r.skip(4 * 3)?; // UE3 version, UE4 version, licensee version
        let custom = r.i32()?;
        r.skip(20 * custom as usize)?;
        let header_size = r.i32()? as usize;
        r.fstring()?; // folder name
        r.skip(4)?; // package flags
        let name_count = r.i32()? as usize;
        let name_offset = r.i32()? as usize;
        r.skip(8)?; // gatherable text data
        let export_count = r.i32()? as usize;
        let export_offset = r.i32()? as usize;

        let mut names = Vec::with_capacity(name_count);
        let mut n = Reader { b: uasset, i: name_offset };
        for _ in 0..name_count {
            names.push(n.fstring()?);
            n.skip(4)?; // the name's hashes
        }

        let mut exports = Vec::with_capacity(export_count);
        let mut e = Reader { b: uasset, i: export_offset };
        for _ in 0..export_count {
            e.skip(4 * 4)?; // class, super, template, outer
            let object = name(&names, &mut e)?;
            e.skip(4)?; // flags
            let size = e.i64()? as usize;
            let offset = e.i64()? as usize;
            e.skip(4 * 3 + 16 + 4 + 4 * 2 + 4 * 5)?;
            let start = offset.checked_sub(header_size).context("export inside the header")?;
            let data = uexp.get(start..start + size).with_context(|| format!("export {object} outside the data"))?;
            // class defaults and blueprint classes carry more than properties: only the
            // property list is read, what follows is ignored
            let properties = properties(&names, &mut Reader { b: data, i: 0 }).unwrap_or_default();
            exports.push(Export { properties });
        }
        Ok(Package { exports })
    }

    pub fn export(&self, index: i32) -> Option<&Export> {
        (index > 0).then(|| self.exports.get(index as usize - 1)).flatten()
    }
}

/// The words a widget spells out one text block per character (or a few): in each panel, the
/// runs of slots holding text blocks with short texts, all single characters or (three blocks
/// or more) half of them, with some Japanese and no digit or letter (a count set at run time);
/// those texts in slot order (a horizontal box's order on screen).
pub fn spelled_words(p: &Package) -> Result<Vec<Vec<Text>>> {
    fn spelled(run: &[Text]) -> bool {
        let single = run.iter().filter(|t| t.source.chars().count() == 1).count();
        let counter = run.iter().any(|t| t.source.chars().any(|c| c.is_ascii_alphanumeric() || ('０'..='９').contains(&c)));
        (single == run.len() && run.len() >= 2 || run.len() >= 3 && 2 * single >= run.len())
            && !counter
            && run.iter().any(|t| asset::is_japanese(&t.source))
    }
    let mut words = Vec::new();
    for e in &p.exports {
        let Some(Value::Objects(slots)) = e.get("Slots") else { continue };
        let mut run = Vec::new();
        for &s in slots {
            let content = match p.export(s).and_then(|s| s.get("Content")) {
                Some(Value::Object(c)) => p.export(*c),
                _ => None,
            };
            match content.and_then(|c| c.get("Text")) {
                Some(Value::Text(t)) if (1..=5).contains(&t.source.chars().count()) => run.push(t.clone()),
                _ => {
                    if spelled(&run) {
                        words.push(std::mem::take(&mut run));
                    }
                    run.clear();
                }
            }
        }
        if spelled(&run) {
            words.push(run);
        }
    }
    if words.iter().any(|w| w.iter().any(|t| t.key.is_empty())) {
        bail!("a spelled word without keys");
    }
    Ok(words)
}
