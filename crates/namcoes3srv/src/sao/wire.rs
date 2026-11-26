//! Messages as bytes, by the game's schema (`schema.rs`): big endian integers, strings as an int
//! byte length then UTF-16LE (GssSite.dll's `GameConnectUtil`), lists as an int count then the
//! elements. A field missing from the value is 0, an empty string or an empty list (Python's
//! `None` in ARTEMiS' encoders).

use std::collections::HashMap;
use std::sync::OnceLock;

use super::schema::MESSAGES;
use super::value::V;

#[derive(Clone, Copy, Debug)]
enum Ty {
    Byte,
    Short,
    Int,
    Str,
    /// A structure (its name).
    Struct(&'static str),
}

#[derive(Clone, Copy, Debug)]
struct Field {
    ty: Ty,
    list: bool,
    name: &'static str,
}

struct Schema {
    fields: HashMap<&'static str, Vec<Field>>,
    /// Command id -> message name.
    by_cmd: HashMap<u16, &'static str>,
}

fn ty(t: &'static str) -> Ty {
    match t {
        "b" => Ty::Byte,
        "h" => Ty::Short,
        "i" => Ty::Int,
        "s" => Ty::Str,
        s => Ty::Struct(s),
    }
}

fn schema() -> &'static Schema {
    static SCHEMA: OnceLock<Schema> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let mut fields = HashMap::new();
        let mut by_cmd = HashMap::new();
        for &(name, cmd, _url, spec) in MESSAGES {
            let list: Vec<Field> = spec
                .split(", ")
                .filter(|f| !f.is_empty())
                .map(|f| {
                    let (t, n) = f.split_once(' ').unwrap_or((f, ""));
                    match t.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
                        Some(inner) => Field { ty: ty(inner), list: true, name: n },
                        None => Field { ty: ty(t), list: false, name: n },
                    }
                })
                .collect();
            fields.insert(name, list);
            if cmd != 0 {
                by_cmd.insert(cmd, name);
            }
        }
        Schema { fields, by_cmd }
    })
}

/// The message name of a command id (`login_Q`, `login_R`).
pub(crate) fn message(cmd: u16) -> Option<&'static str> {
    schema().by_cmd.get(&cmd).copied()
}

/// The fields of a message or structure, in wire order (names and their type letters).
pub(crate) fn fields(name: &str) -> Vec<(&'static str, char)> {
    schema()
        .fields
        .get(name)
        .map(|f| {
            f.iter()
                .map(|f| {
                    let c = match f.ty {
                        Ty::Byte => 'b',
                        Ty::Short => 'h',
                        Ty::Int => 'i',
                        Ty::Str => 's',
                        Ty::Struct(_) => 'S',
                    };
                    (f.name, if f.list { c.to_ascii_uppercase() } else { c })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let bytes: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
    out.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
    out.extend_from_slice(&bytes);
}

fn put_one(out: &mut Vec<u8>, ty: Ty, v: &V) {
    match ty {
        // the game's fields are unsigned bytes and signed shorts / ints; ARTEMiS' struct.pack
        // refuses values out of the signed range (0 sent), the game's own encoder truncates
        Ty::Byte => out.push(v.int() as u8),
        Ty::Short => out.extend_from_slice(&(v.int() as i16).to_be_bytes()),
        Ty::Int => out.extend_from_slice(&(v.int() as i32).to_be_bytes()),
        Ty::Str => put_str(out, &v.text()),
        Ty::Struct(name) => encode_into(name, v, out),
    }
}

fn encode_into(name: &str, v: &V, out: &mut Vec<u8>) {
    let Some(fields) = schema().fields.get(name) else {
        return crate::log(&format!("sao: no schema for {name}"));
    };
    for f in fields {
        let value = v.get(f.name);
        if f.list {
            let items = value.list();
            out.extend_from_slice(&(items.len() as i32).to_be_bytes());
            for item in items {
                put_one(out, f.ty, item);
            }
        } else {
            put_one(out, f.ty, value);
        }
    }
}

/// `v` as the message (or structure) `name`'s bytes.
pub(crate) fn encode(name: &str, v: &V) -> Vec<u8> {
    let mut out = Vec::new();
    encode_into(name, v, &mut out);
    out
}

struct Reader<'a> {
    d: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> &[u8] {
        let start = self.i.min(self.d.len());
        let end = (self.i + n).min(self.d.len());
        self.i += n;
        &self.d[start..end]
    }

    fn num(&mut self, n: usize) -> i64 {
        let b = self.take(n);
        if b.len() < n {
            return 0;
        }
        match n {
            1 => b[0] as i64,
            2 => i16::from_be_bytes([b[0], b[1]]) as i64,
            _ => i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as i64,
        }
    }

    fn one(&mut self, ty: Ty) -> V {
        match ty {
            Ty::Byte => V::Int(self.num(1)),
            Ty::Short => V::Int(self.num(2)),
            Ty::Int => V::Int(self.num(4)),
            Ty::Str => {
                let len = self.num(4).max(0) as usize;
                let b = self.take(len);
                let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                V::Str(String::from_utf16_lossy(&units))
            }
            Ty::Struct(name) => self.message(name),
        }
    }

    fn message(&mut self, name: &str) -> V {
        let mut v = V::obj();
        let Some(fields) = schema().fields.get(name) else {
            return v;
        };
        for f in fields {
            let value = if f.list {
                let n = self.num(4).clamp(0, 100_000) as usize;
                let mut items = Vec::with_capacity(n.min(1024));
                for _ in 0..n {
                    if self.i >= self.d.len() {
                        break;
                    }
                    items.push(self.one(f.ty));
                }
                V::List(items)
            } else {
                self.one(f.ty)
            };
            v.set(f.name, value);
        }
        v
    }
}

/// The message `name` read from `data` (missing bytes read as zeros).
pub(crate) fn decode(name: &str, data: &[u8]) -> V {
    Reader { d: data, i: 0 }.message(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let v = V::obj()
            .with("result", 1)
            .with("player_rank_data_list", V::List(vec![V::obj().with("player_rank_id", 2).with("total_exp", 100).with("storage", 1505).with("team_preset", 2)]));
        let bytes = encode("get_m_player_ranks_R", &v);
        assert_eq!(bytes, [1, 0, 0, 0, 1, 0, 2, 0, 0, 0, 100, 5, 0xe1, 0, 2]);
        let back = decode("get_m_player_ranks_R", &bytes);
        assert_eq!(back.get("player_rank_data_list").list()[0].i("storage"), 1505);
        assert_eq!(message(0xc001), Some("ticket_R"));
        let s = encode("ticket_R", &V::obj().with("result", "1").with("ticket_id", "9"));
        assert_eq!(s, [0, 0, 0, 2, b'1', 0, 0, 0, 0, 2, b'9', 0]);
    }
}
