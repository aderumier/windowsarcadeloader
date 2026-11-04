//! MessagePack (https://msgpack.org): the values JoJo's Bizarre Adventure: Last Survivor's
//! network messages use, read and written.

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bin(Vec<u8>),
    Array(Vec<Value>),
    Map(Vec<(Value, Value)>),
}

impl Value {
    pub fn str(s: &str) -> Value {
        Value::Str(s.to_string())
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> &[Value] {
        match self {
            Value::Array(a) => a,
            _ => &[],
        }
    }

    /// A map's value at the string key `key`.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(m) => m.iter().find(|(k, _)| k.as_str() == Some(key)).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.write(&mut out);
        out
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Value::Nil => out.push(0xc0),
            Value::Bool(b) => out.push(if *b { 0xc3 } else { 0xc2 }),
            Value::Int(i) => write_int(out, *i),
            Value::Float(f) => {
                out.push(0xcb);
                out.extend(f.to_be_bytes());
            }
            Value::Str(s) => {
                let n = s.len();
                match n {
                    0..32 => out.push(0xa0 | n as u8),
                    32..256 => out.extend([0xd9, n as u8]),
                    256..65536 => {
                        out.push(0xda);
                        out.extend((n as u16).to_be_bytes());
                    }
                    _ => {
                        out.push(0xdb);
                        out.extend((n as u32).to_be_bytes());
                    }
                }
                out.extend(s.as_bytes());
            }
            Value::Bin(b) => {
                let n = b.len();
                match n {
                    0..256 => out.extend([0xc4, n as u8]),
                    256..65536 => {
                        out.push(0xc5);
                        out.extend((n as u16).to_be_bytes());
                    }
                    _ => {
                        out.push(0xc6);
                        out.extend((n as u32).to_be_bytes());
                    }
                }
                out.extend(b);
            }
            Value::Array(a) => {
                write_len(out, a.len(), 0x90, 0xdc);
                for v in a {
                    v.write(out);
                }
            }
            Value::Map(m) => {
                write_len(out, m.len(), 0x80, 0xde);
                for (k, v) in m {
                    k.write(out);
                    v.write(out);
                }
            }
        }
    }

    /// The value at the start of `b` and the bytes it takes.
    pub fn decode(b: &[u8]) -> Option<(Value, usize)> {
        let mut r = Reader { b, i: 0 };
        let v = r.value(0)?;
        Some((v, r.i))
    }
}

fn write_int(out: &mut Vec<u8>, i: i64) {
    match i {
        0..=0x7f => out.push(i as u8),
        -32..0 => out.push(i as i8 as u8),
        0x80..=0xff => out.extend([0xcc, i as u8]),
        0x100..=0xffff => {
            out.push(0xcd);
            out.extend((i as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(0xce);
            out.extend((i as u32).to_be_bytes());
        }
        i if i > 0 => {
            out.push(0xcf);
            out.extend((i as u64).to_be_bytes());
        }
        -0x80..=-33 => out.extend([0xd0, i as i8 as u8]),
        -0x8000..=-0x81 => {
            out.push(0xd1);
            out.extend((i as i16).to_be_bytes());
        }
        -0x8000_0000..=-0x8001 => {
            out.push(0xd2);
            out.extend((i as i32).to_be_bytes());
        }
        _ => {
            out.push(0xd3);
            out.extend(i.to_be_bytes());
        }
    }
}

/// A container's length: in its fix form (`fix`, under 16) or with a 16-bit (`big`) or
/// 32-bit (`big + 1`) length.
fn write_len(out: &mut Vec<u8>, n: usize, fix: u8, big: u8) {
    match n {
        0..16 => out.push(fix | n as u8),
        16..65536 => {
            out.push(big);
            out.extend((n as u16).to_be_bytes());
        }
        _ => {
            out.push(big + 1);
            out.extend((n as u32).to_be_bytes());
        }
    }
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn bytes(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.i..self.i + n)?;
        self.i += n;
        Some(s)
    }

    fn uint(&mut self, n: usize) -> Option<u64> {
        Some(self.bytes(n)?.iter().fold(0u64, |a, &b| a << 8 | b as u64))
    }

    fn str(&mut self, n: usize) -> Option<Value> {
        Some(Value::Str(String::from_utf8_lossy(self.bytes(n)?).into_owned()))
    }

    fn array(&mut self, n: usize, depth: usize) -> Option<Value> {
        (0..n).map(|_| self.value(depth + 1)).collect::<Option<_>>().map(Value::Array)
    }

    fn map(&mut self, n: usize, depth: usize) -> Option<Value> {
        (0..n).map(|_| Some((self.value(depth + 1)?, self.value(depth + 1)?))).collect::<Option<_>>().map(Value::Map)
    }

    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > 64 {
            return None;
        }
        let t = *self.bytes(1)?.first()?;
        Some(match t {
            0x00..=0x7f => Value::Int(t as i64),
            0x80..=0x8f => self.map((t & 0x0f) as usize, depth)?,
            0x90..=0x9f => self.array((t & 0x0f) as usize, depth)?,
            0xa0..=0xbf => self.str((t & 0x1f) as usize)?,
            0xc0 => Value::Nil,
            0xc2 => Value::Bool(false),
            0xc3 => Value::Bool(true),
            0xc4..=0xc6 => {
                let n = self.uint(1 << (t - 0xc4))? as usize;
                Value::Bin(self.bytes(n)?.to_vec())
            }
            0xca => Value::Float(f32::from_bits(self.uint(4)? as u32) as f64),
            0xcb => Value::Float(f64::from_bits(self.uint(8)?)),
            0xcc..=0xcf => Value::Int(self.uint(1 << (t - 0xcc))? as i64),
            0xd0 => Value::Int(self.uint(1)? as u8 as i8 as i64),
            0xd1 => Value::Int(self.uint(2)? as u16 as i16 as i64),
            0xd2 => Value::Int(self.uint(4)? as u32 as i32 as i64),
            0xd3 => Value::Int(self.uint(8)? as i64),
            0xd9..=0xdb => {
                let n = self.uint(1 << (t - 0xd9))? as usize;
                self.str(n)?
            }
            0xdc | 0xdd => {
                let n = self.uint(2 << (t - 0xdc))? as usize;
                self.array(n, depth)?
            }
            0xde | 0xdf => {
                let n = self.uint(2 << (t - 0xde))? as usize;
                self.map(n, depth)?
            }
            0xe0..=0xff => Value::Int(t as i8 as i64),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let v = Value::Array(vec![
            Value::Map(vec![(Value::str("language"), Value::Int(1)), (Value::str("version"), Value::str("0.0.1"))]),
            Value::Array(vec![Value::Int(-5), Value::Int(300), Value::Int(70000), Value::Bool(true), Value::Nil]),
        ]);
        let b = v.encode();
        assert_eq!(Value::decode(&b), Some((v, b.len())));
    }

    #[test]
    fn decodes_the_game_header() {
        // a captured sys/serveralive request, its hash removed
        let hex = "9285a86c616e677561676501a8706c6174666f726d05a773657373696f6ea0a6757365724964a0a776657273696f6ea5302e302e3190";
        let b: Vec<u8> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        let (v, n) = Value::decode(&b).unwrap();
        assert_eq!(n, b.len());
        assert_eq!(v.as_array()[0].get("platform"), Some(&Value::Int(5)));
        assert_eq!(v.as_array()[0].get("version"), Some(&Value::str("0.0.1")));
    }
}
