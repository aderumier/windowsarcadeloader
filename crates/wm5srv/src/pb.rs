//! Protocol buffers wire format: a message writer and a reader of the fields of a message
//! (the game's `wm5.protobuf` messages, proto2: a response lacking a required field fails to
//! parse in the game).

/// A message being written.
#[derive(Default, Clone)]
pub struct Writer(pub Vec<u8>);

fn varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

impl Writer {
    pub fn new() -> Self {
        Self(Vec::new())
    }

    /// uint32, uint64, int32 (non negative), enum, bool.
    pub fn uint(mut self, field: u32, v: u64) -> Self {
        varint(&mut self.0, (field << 3) as u64);
        varint(&mut self.0, v);
        self
    }

    /// sint32 / sint64 (zigzag).
    pub fn sint(self, field: u32, v: i64) -> Self {
        self.uint(field, ((v << 1) ^ (v >> 63)) as u64)
    }

    pub fn float(mut self, field: u32, v: f32) -> Self {
        varint(&mut self.0, (field << 3 | 5) as u64);
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }

    pub fn bytes(mut self, field: u32, v: &[u8]) -> Self {
        varint(&mut self.0, (field << 3 | 2) as u64);
        varint(&mut self.0, v.len() as u64);
        self.0.extend_from_slice(v);
        self
    }

    pub fn string(self, field: u32, v: &str) -> Self {
        self.bytes(field, v.as_bytes())
    }

    pub fn message(self, field: u32, m: Writer) -> Self {
        self.bytes(field, &m.0)
    }

    /// Fields already encoded (a stored message's).
    pub fn raw(mut self, encoded: &[u8]) -> Self {
        self.0.extend_from_slice(encoded);
        self
    }
}

/// A field value as read.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Varint(u64),
    Fixed64(u64),
    Bytes(Vec<u8>),
    Fixed32(u32),
}

/// The fields of a message, in order (repeated fields repeat).
#[derive(Clone, Debug, Default)]
pub struct Message(pub Vec<(u32, Value)>);

fn read_varint(data: &[u8], pos: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *data.get(*pos)?;
        *pos += 1;
        v |= ((b & 0x7F) as u64) << shift;
        if b & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

impl Message {
    pub fn parse(data: &[u8]) -> Option<Message> {
        let mut fields = Vec::new();
        let mut pos = 0;
        while pos < data.len() {
            let key = read_varint(data, &mut pos)?;
            let field = (key >> 3) as u32;
            let value = match key & 7 {
                0 => Value::Varint(read_varint(data, &mut pos)?),
                1 => {
                    let v = u64::from_le_bytes(data.get(pos..pos + 8)?.try_into().ok()?);
                    pos += 8;
                    Value::Fixed64(v)
                }
                2 => {
                    let len = read_varint(data, &mut pos)? as usize;
                    let v = data.get(pos..pos + len)?.to_vec();
                    pos += len;
                    Value::Bytes(v)
                }
                5 => {
                    let v = u32::from_le_bytes(data.get(pos..pos + 4)?.try_into().ok()?);
                    pos += 4;
                    Value::Fixed32(v)
                }
                _ => return None,
            };
            fields.push((field, value));
        }
        Some(Message(fields))
    }

    /// The last value of `field` (proto2 semantics for a scalar).
    pub fn get(&self, field: u32) -> Option<&Value> {
        self.0.iter().rev().find(|(f, _)| *f == field).map(|(_, v)| v)
    }

    pub fn uint(&self, field: u32) -> Option<u64> {
        match self.get(field)? {
            Value::Varint(v) | Value::Fixed64(v) => Some(*v),
            Value::Fixed32(v) => Some(*v as u64),
            Value::Bytes(_) => None,
        }
    }

    pub fn bytes(&self, field: u32) -> Option<&[u8]> {
        match self.get(field)? {
            Value::Bytes(v) => Some(v),
            _ => None,
        }
    }

    pub fn string(&self, field: u32) -> Option<String> {
        self.bytes(field).map(|b| String::from_utf8_lossy(b).into_owned())
    }

    pub fn message(&self, field: u32) -> Option<Message> {
        Message::parse(self.bytes(field)?)
    }

    /// Every value of a repeated field.
    pub fn all(&self, field: u32) -> impl Iterator<Item = &Value> {
        self.0.iter().filter(move |(f, _)| *f == field).map(|(_, v)| v)
    }

    /// Every element of a repeated uint field, packed or not.
    pub fn uints(&self, field: u32) -> Vec<u64> {
        let mut out = Vec::new();
        for v in self.all(field) {
            match v {
                Value::Varint(v) => out.push(*v),
                Value::Bytes(b) => {
                    let mut pos = 0;
                    while pos < b.len() {
                        match read_varint(b, &mut pos) {
                            Some(v) => out.push(v),
                            None => break,
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// The message encoded again.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        for (field, value) in &self.0 {
            w = match value {
                Value::Varint(v) => w.uint(*field, *v),
                Value::Bytes(b) => w.bytes(*field, b),
                Value::Fixed64(v) => {
                    varint(&mut w.0, (*field << 3 | 1) as u64);
                    w.0.extend_from_slice(&v.to_le_bytes());
                    w
                }
                Value::Fixed32(v) => {
                    varint(&mut w.0, (*field << 3 | 5) as u64);
                    w.0.extend_from_slice(&v.to_le_bytes());
                    w
                }
            };
        }
        w.0
    }

    /// `self` with the scalar fields of `update` replacing its own (the repeated ones of
    /// `update` replace all of its values), the others kept.
    pub fn merge(&mut self, update: &Message) {
        let fields: Vec<u32> = update.0.iter().map(|(f, _)| *f).collect();
        self.0.retain(|(f, _)| !fields.contains(f));
        self.0.extend(update.0.iter().cloned());
        self.0.sort_by_key(|(f, _)| *f);
    }

    /// A readable dump (nested messages guessed), for the log.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        for (field, value) in &self.0 {
            match value {
                Value::Varint(v) => out += &format!("{field}:{v} "),
                Value::Fixed32(v) => out += &format!("{field}:{} ", f32::from_bits(*v)),
                Value::Fixed64(v) => out += &format!("{field}:{v:#x} "),
                Value::Bytes(b) => match std::str::from_utf8(b) {
                    Ok(s) if s.chars().all(|c| !c.is_control()) => out += &format!("{field}:{s:?} "),
                    _ => match Message::parse(b) {
                        Some(m) if !b.is_empty() => out += &format!("{field}:{{ {}}} ", m.dump()),
                        _ => out += &format!("{field}:{b:02x?} "),
                    },
                },
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let w = Writer::new().uint(1, 300).string(2, "abc").message(3, Writer::new().sint(1, -5));
        let m = Message::parse(&w.0).unwrap();
        assert_eq!(m.uint(1), Some(300));
        assert_eq!(m.string(2).as_deref(), Some("abc"));
        assert_eq!(m.message(3).unwrap().uint(1), Some(9));
        assert_eq!(m.encode(), w.0);
    }
}
