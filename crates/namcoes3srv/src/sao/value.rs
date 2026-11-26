//! A message's values by field name (encoded by the game's schema, `wire.rs`), also the saved
//! players' rows (written as JSON).

use std::fmt::Write;

#[derive(Clone, Debug, PartialEq, Default)]
pub(crate) enum V {
    /// Python's `None`: 0, or an empty string.
    #[default]
    Null,
    Int(i64),
    Str(String),
    List(Vec<V>),
    Obj(Vec<(String, V)>),
}

static NULL: V = V::Null;

impl V {
    pub(crate) fn obj() -> V {
        V::Obj(Vec::new())
    }

    /// Sets `key` (an object's field).
    pub(crate) fn set(&mut self, key: &str, value: impl Into<V>) {
        let value = value.into();
        if !matches!(self, V::Obj(_)) {
            *self = V::obj();
        }
        if let V::Obj(fields) = self {
            match fields.iter_mut().find(|(k, _)| k == key) {
                Some(f) => f.1 = value,
                None => fields.push((key.to_string(), value)),
            }
        }
    }

    /// `self` with `key` set.
    pub(crate) fn with(mut self, key: &str, value: impl Into<V>) -> V {
        self.set(key, value);
        self
    }

    /// A field (`Null` when missing).
    pub(crate) fn get(&self, key: &str) -> &V {
        match self {
            V::Obj(fields) => fields.iter().find(|(k, _)| k == key).map_or(&NULL, |(_, v)| v),
            _ => &NULL,
        }
    }

    pub(crate) fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        match self {
            V::Obj(fields) => fields.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// The value as an integer, as Python's `int()` (a string's number; 0 otherwise).
    pub(crate) fn int(&self) -> i64 {
        match self {
            V::Int(n) => *n,
            V::Str(s) => s.trim().parse().unwrap_or(0),
            _ => 0,
        }
    }

    /// A field as an integer.
    pub(crate) fn i(&self, key: &str) -> i64 {
        self.get(key).int()
    }

    /// A field as a string.
    pub(crate) fn s(&self, key: &str) -> String {
        self.get(key).text()
    }

    /// The value as text, as Python's `str()` (`None`: empty).
    pub(crate) fn text(&self) -> String {
        match self {
            V::Int(n) => n.to_string(),
            V::Str(s) => s.clone(),
            _ => String::new(),
        }
    }

    pub(crate) fn is_null(&self) -> bool {
        matches!(self, V::Null)
    }

    /// Python's truth: not `None`, 0 or empty.
    pub(crate) fn truthy(&self) -> bool {
        match self {
            V::Null => false,
            V::Int(n) => *n != 0,
            V::Str(s) => !s.is_empty(),
            V::List(l) => !l.is_empty(),
            V::Obj(_) => true,
        }
    }

    pub(crate) fn list(&self) -> &[V] {
        match self {
            V::List(l) => l,
            _ => &[],
        }
    }

    /// Appends to the list field `key` (made when missing).
    pub(crate) fn push(&mut self, key: &str, value: V) {
        if !matches!(self.get(key), V::List(_)) {
            self.set(key, V::List(Vec::new()));
        }
        if let Some(V::List(l)) = self.get_mut(key) {
            l.push(value);
        }
    }

    /// JSON text.
    pub(crate) fn to_json(&self, out: &mut String) {
        match self {
            V::Null => out.push_str("null"),
            V::Int(n) => {
                let _ = write!(out, "{n}");
            }
            V::Str(s) => {
                out.push('"');
                for c in s.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
                        c if (c as u32) < 0x20 => {
                            let _ = write!(out, "\\u{:04x}", c as u32);
                        }
                        c => out.push(c),
                    }
                }
                out.push('"');
            }
            V::List(l) => {
                out.push('[');
                for (i, v) in l.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.to_json(out);
                }
                out.push(']');
            }
            V::Obj(fields) => {
                out.push('{');
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    V::Str(k.clone()).to_json(out);
                    out.push(':');
                    v.to_json(out);
                }
                out.push('}');
            }
        }
    }

    /// Parses JSON text (`to_json`'s: objects, arrays, integers, strings, null; booleans as 1 / 0).
    pub(crate) fn from_json(text: &str) -> Option<V> {
        let mut p = Parser { s: text.as_bytes(), i: 0 };
        let v = p.value()?;
        p.ws();
        (p.i == p.s.len()).then_some(v)
    }
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, word: &str) -> bool {
        if self.s[self.i..].starts_with(word.as_bytes()) {
            self.i += word.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Option<V> {
        self.ws();
        match *self.s.get(self.i)? {
            b'n' => self.eat("null").then_some(V::Null),
            b't' => self.eat("true").then_some(V::Int(1)),
            b'f' => self.eat("false").then_some(V::Int(0)),
            b'"' => self.string().map(V::Str),
            b'[' => {
                self.i += 1;
                let mut l = Vec::new();
                self.ws();
                if self.eat("]") {
                    return Some(V::List(l));
                }
                loop {
                    l.push(self.value()?);
                    self.ws();
                    if self.eat("]") {
                        return Some(V::List(l));
                    }
                    if !self.eat(",") {
                        return None;
                    }
                }
            }
            b'{' => {
                self.i += 1;
                let mut fields = Vec::new();
                self.ws();
                if self.eat("}") {
                    return Some(V::Obj(fields));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    if !self.eat(":") {
                        return None;
                    }
                    fields.push((k, self.value()?));
                    self.ws();
                    if self.eat("}") {
                        return Some(V::Obj(fields));
                    }
                    if !self.eat(",") {
                        return None;
                    }
                }
            }
            _ => {
                let start = self.i;
                while self.i < self.s.len() && (self.s[self.i] == b'-' || self.s[self.i].is_ascii_digit()) {
                    self.i += 1;
                }
                std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok().map(V::Int)
            }
        }
    }

    fn string(&mut self) -> Option<String> {
        if !self.eat("\"") {
            return None;
        }
        let mut out = Vec::new();
        loop {
            let c = *self.s.get(self.i)?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).ok(),
                b'\\' => {
                    let e = *self.s.get(self.i)?;
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let hex = std::str::from_utf8(self.s.get(self.i..self.i + 4)?).ok()?;
                            self.i += 4;
                            let c = char::from_u32(u32::from_str_radix(hex, 16).ok()?)?;
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                        }
                        e => out.push(e),
                    }
                }
                c => out.push(c),
            }
        }
    }
}

impl From<i64> for V {
    fn from(n: i64) -> V {
        V::Int(n)
    }
}

impl From<i32> for V {
    fn from(n: i32) -> V {
        V::Int(n as i64)
    }
}

impl From<u32> for V {
    fn from(n: u32) -> V {
        V::Int(n as i64)
    }
}

impl From<usize> for V {
    fn from(n: usize) -> V {
        V::Int(n as i64)
    }
}

impl From<bool> for V {
    fn from(b: bool) -> V {
        V::Int(b as i64)
    }
}

impl From<&str> for V {
    fn from(s: &str) -> V {
        V::Str(s.to_string())
    }
}

impl From<String> for V {
    fn from(s: String) -> V {
        V::Str(s)
    }
}

impl From<&String> for V {
    fn from(s: &String) -> V {
        V::Str(s.clone())
    }
}

impl From<Vec<V>> for V {
    fn from(l: Vec<V>) -> V {
        V::List(l)
    }
}

impl From<&V> for V {
    fn from(v: &V) -> V {
        v.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::V;

    #[test]
    fn json_round_trip() {
        let v = V::obj().with("a", 1).with("b", "x\"y").with("c", V::List(vec![V::Null, V::Int(-3)]));
        let mut s = String::new();
        v.to_json(&mut s);
        assert_eq!(V::from_json(&s), Some(v));
    }
}
