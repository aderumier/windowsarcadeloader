//! Konami's binary XML ("kbin", the e-Amusement protocol's payload): reader and writer, for the
//! fake server (`easrv.rs`).
//!
//! Layout: `A0`, `42` (node names packed in 6 bits; `45`: names as bytes), the encoding (`80`:
//! Shift-JIS) and its complement, the node buffer's length (u32 big endian), the node buffer, the
//! data buffer's length, the data buffer. Each node is its type (bit 0x40: an array), its name,
//! then its attributes (type 46 and the name each; values in the data buffer), its children, and
//! `FE`; the document ends with `FF`, the node buffer padded to 4 bytes. The data buffer holds
//! the values in node order: strings, binaries and arrays as their byte length and bytes,
//! padded to 4; other values of 4 bytes and more appended; 1- and 2-byte values packed into
//! shared 4-byte slots (one for bytes, one for words). The same layout as the open-source kbinxml
//! library's. Values are big endian.

const SIXBIT: &[u8] = b"0123456789:ABCDEFGHIJKLMNOPQRSTUVWXYZ_abcdefghijklmnopqrstuvwxyz";

const VOID: u8 = 1;
const BIN: u8 = 10;
const STR: u8 = 11;
const IP4: u8 = 12;
const ATTR: u8 = 46;
const NODE_END: u8 = 0xfe;
const END: u8 = 0xff;

/// A type's (name, element size, element count); `None`: unknown.
fn kind(id: u8) -> Option<(&'static str, usize, usize)> {
    const SCALARS: [(&str, usize); 14] = [
        ("s8", 1), ("u8", 1), ("s16", 2), ("u16", 2), ("s32", 4), ("u32", 4), ("s64", 8), ("u64", 8),
        ("bin", 1), ("str", 1), ("ip4", 4), ("time", 4), ("float", 4), ("double", 8),
    ];
    const VEC_SIZES: [usize; 10] = [1, 1, 2, 2, 4, 4, 8, 8, 4, 8];
    Some(match id {
        1 => ("void", 0, 0),
        2..=15 => {
            let (n, s) = SCALARS[(id - 2) as usize];
            (n, s, if id == BIN || id == STR { 0 } else { 1 })
        }
        16..=45 => {
            let i = ((id - 16) % 10) as usize;
            let count = (id - 16) as usize / 10 + 2;
            let names: [[&str; 10]; 3] = [
                ["2s8", "2u8", "2s16", "2u16", "2s32", "2u32", "2s64", "2u64", "2f", "2d"],
                ["3s8", "3u8", "3s16", "3u16", "3s32", "3u32", "3s64", "3u64", "3f", "3d"],
                ["4s8", "4u8", "4s16", "4u16", "4s32", "4u32", "4s64", "4u64", "4f", "4d"],
            ];
            (names[count - 2][i], VEC_SIZES[i], count)
        }
        48 => ("vs8", 1, 16),
        49 => ("vu8", 1, 16),
        50 => ("vs16", 2, 8),
        51 => ("vu16", 2, 8),
        52 => ("bool", 1, 1),
        53 => ("2b", 1, 2),
        54 => ("3b", 1, 3),
        55 => ("4b", 1, 4),
        56 => ("vb", 1, 16),
        _ => return None,
    })
}

/// A value to build a node with.
#[derive(Clone)]
pub enum Value {
    Void,
    Str(String),
    S8(i8),
    U8(u8),
    U16(u16),
    S32(i32),
    U32(u32),
    S64(i64),
    U64(u64),
    Ip4([u8; 4]),
    Bool(bool),
    Bin(Vec<u8>),
}

#[derive(Clone)]
pub struct Node {
    pub name: String,
    /// The type id (without the array bit).
    pub kind: u8,
    pub array: bool,
    /// The value's bytes (big endian; a string's without its NUL).
    pub raw: Vec<u8>,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Node>,
}

impl Node {
    pub fn new(name: &str) -> Node {
        Node { name: name.to_string(), kind: VOID, array: false, raw: Vec::new(), attrs: Vec::new(), children: Vec::new() }
    }

    /// A leaf node with a value.
    pub fn value(name: &str, value: Value) -> Node {
        let (kind, raw) = match value {
            Value::Void => (VOID, Vec::new()),
            Value::Str(s) => (STR, s.into_bytes()),
            Value::S8(v) => (2, vec![v as u8]),
            Value::U8(v) => (3, vec![v]),
            Value::U16(v) => (5, v.to_be_bytes().to_vec()),
            Value::S32(v) => (6, v.to_be_bytes().to_vec()),
            Value::U32(v) => (7, v.to_be_bytes().to_vec()),
            Value::S64(v) => (8, v.to_be_bytes().to_vec()),
            Value::U64(v) => (9, v.to_be_bytes().to_vec()),
            Value::Ip4(v) => (IP4, v.to_vec()),
            Value::Bool(v) => (52, vec![v as u8]),
            Value::Bin(v) => (BIN, v),
        };
        Node { kind, raw, ..Node::new(name) }
    }

    pub fn str(name: &str, value: &str) -> Node {
        Node::value(name, Value::Str(value.to_string()))
    }

    pub fn attr(mut self, name: &str, value: impl ToString) -> Node {
        self.attrs.push((name.to_string(), value.to_string()));
        self
    }

    pub fn child(mut self, child: Node) -> Node {
        self.children.push(child);
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = Node>) -> Node {
        self.children.extend(children);
        self
    }

    /// The node's attribute `name`.
    pub fn get_attr(&self, name: &str) -> Option<&str> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    /// The first child named `name`.
    pub fn find(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    /// The node at `path` (`a/b/c`, child names) below this one.
    pub fn path(&self, path: &str) -> Option<&Node> {
        path.split('/').try_fold(self, |n, p| n.find(p))
    }

    /// A string value (or the text form of any value).
    pub fn text(&self) -> String {
        if self.kind == STR {
            return String::from_utf8_lossy(&self.raw).into_owned();
        }
        self.value_text()
    }

    /// An array of numbers (`u8`, `s16`, `u16`, `s32`, `u32`), big endian.
    pub fn array(name: &str, kind_name: &str, values: &[i64]) -> Node {
        let (kind, size) = match kind_name {
            "s8" => (2, 1),
            "u8" => (3, 1),
            "s16" => (4, 2),
            "u16" => (5, 2),
            "s32" => (6, 4),
            _ => (7, 4),
        };
        let raw = values.iter().flat_map(|&v| (v as u64).to_be_bytes()[8 - size..].to_vec()).collect();
        Node { kind, array: true, raw, ..Node::new(name) }
    }

    /// Every element of a number value (an array's, or the single one).
    pub fn ints(&self) -> Vec<i64> {
        let Some((_, size, _)) = kind(self.kind) else { return Vec::new() };
        if size == 0 {
            return Vec::new();
        }
        self.raw.chunks(size).map(|c| Node { kind: self.kind, raw: c.to_vec(), ..Node::new("") }.int()).collect()
    }

    /// A number value (the first element), or 0.
    pub fn int(&self) -> i64 {
        let Some((name, size, _)) = kind(self.kind) else { return 0 };
        let b = &self.raw;
        if b.len() < size || size == 0 {
            return 0;
        }
        let unsigned = name.contains('u') || name == "ip4" || name == "time";
        match size {
            1 if unsigned => b[0] as i64,
            1 => b[0] as i8 as i64,
            2 if unsigned => u16::from_be_bytes([b[0], b[1]]) as i64,
            2 => i16::from_be_bytes([b[0], b[1]]) as i64,
            4 if unsigned => u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as i64,
            4 => i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as i64,
            8 => i64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
            _ => 0,
        }
    }

    fn value_text(&self) -> String {
        let Some((name, size, _)) = kind(self.kind) else { return hex(&self.raw) };
        match self.kind {
            VOID => String::new(),
            STR => String::from_utf8_lossy(&self.raw).into_owned(),
            BIN => hex(&self.raw),
            IP4 => self.raw.iter().map(|b| b.to_string()).collect::<Vec<_>>().join("."),
            _ if size == 0 => hex(&self.raw),
            _ => self
                .raw
                .chunks(size)
                .map(|c| {
                    let one = Node { kind: self.kind, raw: c.to_vec(), ..Node::new("") };
                    if name.ends_with('f') || name == "float" {
                        f32::from_be_bytes([c[0], c[1], c[2], c[3]]).to_string()
                    } else if name.ends_with('d') || name == "double" {
                        f64::from_be_bytes(c.try_into().unwrap_or([0; 8])).to_string()
                    } else if name.ends_with("u64") {
                        u64::from_be_bytes(c.try_into().unwrap_or([0; 8])).to_string()
                    } else {
                        one.int().to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(" "),
        }
    }

    /// The XML text form (logs).
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        self.write_text(&mut out, 0);
        out
    }

    fn write_text(&self, out: &mut String, depth: usize) {
        let pad = "  ".repeat(depth);
        out.push_str(&format!("{pad}<{}", self.name));
        if self.kind != VOID {
            let (name, size, _) = kind(self.kind).unwrap_or(("?", 1, 1));
            out.push_str(&format!(" __type=\"{name}\""));
            if self.array && size > 0 {
                out.push_str(&format!(" __count=\"{}\"", self.raw.len() / size));
            }
        }
        for (n, v) in &self.attrs {
            out.push_str(&format!(" {n}=\"{v}\""));
        }
        let value = self.value_text();
        if self.children.is_empty() && value.is_empty() {
            out.push_str("/>\n");
            return;
        }
        out.push('>');
        out.push_str(&value);
        if !self.children.is_empty() {
            out.push('\n');
            for c in &self.children {
                c.write_text(out, depth + 1);
            }
            out.push_str(&pad);
        }
        out.push_str(&format!("</{}>\n", self.name));
    }

    /// The document (this node its root), Shift-JIS.
    pub fn to_binary(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.node(self);
        w.nodes.push(END);
        while w.nodes.len() % 4 != 0 {
            w.nodes.push(0);
        }
        let mut out = vec![0xa0, 0x42, 0x80, 0x7f];
        out.extend((w.nodes.len() as u32).to_be_bytes());
        out.extend(&w.nodes);
        out.extend((w.data.len() as u32).to_be_bytes());
        out.extend(&w.data);
        out
    }

    /// A document's root node, binary or text XML (older games, e.g. Metal Gear Arcade, post
    /// their requests as XML text).
    pub fn parse(data: &[u8]) -> Option<Node> {
        if data.first() == Some(&0xa0) { Node::from_binary(data) } else { Node::from_xml(data) }
    }

    /// A text XML document's root node (`__type` / `__count` give the values' types; the
    /// declaration, comments skipped; strings kept as their bytes).
    pub fn from_xml(data: &[u8]) -> Option<Node> {
        let mut x = XmlReader { data, at: 0 };
        x.skip_misc();
        x.element()
    }

    /// A document's root node.
    pub fn from_binary(data: &[u8]) -> Option<Node> {
        if data.len() < 8 || data[0] != 0xa0 {
            return None;
        }
        let packed = match data[1] {
            0x42 => true,
            0x45 => false,
            _ => return None,
        };
        let node_len = u32::from_be_bytes(data[4..8].try_into().ok()?) as usize;
        let nodes = data.get(8..8 + node_len)?;
        let data_at = 8 + node_len;
        let data_len = u32::from_be_bytes(data.get(data_at..data_at + 4)?.try_into().ok()?) as usize;
        let values = data.get(data_at + 4..data_at + 4 + data_len)?;
        let mut r = Reader { nodes, at: 0, packed, data: values, data_at: 0, byte_at: 0, word_at: 0 };
        // the stack of open nodes; the document's root is the first
        let mut stack: Vec<Node> = Vec::new();
        let mut root = None;
        loop {
            let id = r.byte()?;
            let array = id & 0x40 != 0;
            let id = id & !0x40;
            match id {
                // the end of the document, of a node (0xff, 0xfe without the array bit)
                0xbf => break,
                0xbe => {
                    let node = stack.pop()?;
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(node),
                        None => root = Some(node),
                    }
                }
                ATTR => {
                    let name = r.name()?;
                    let value = r.string()?;
                    stack.last_mut()?.attrs.push((name, value));
                }
                _ => {
                    let name = r.name()?;
                    let (_, size, count) = kind(id)?;
                    let raw = if id == VOID {
                        Vec::new()
                    } else if array || count == 0 {
                        let mut raw = r.chunk()?;
                        if id == STR {
                            while raw.last() == Some(&0) {
                                raw.pop();
                            }
                        }
                        raw
                    } else {
                        r.aligned(size * count)?
                    };
                    stack.push(Node { name, kind: id, array, raw, attrs: Vec::new(), children: Vec::new() });
                }
            }
        }
        root
    }
}

/// The type id of a `__type` name.
fn kind_id(name: &str) -> Option<u8> {
    let alias = match name {
        "string" => "str",
        "binary" => "bin",
        "f" => "float",
        "d" => "double",
        "b" => "bool",
        n => n,
    };
    (1..=56u8).find(|&id| kind(id).is_some_and(|k| k.0 == alias))
}

/// A typed text value as its bytes (big endian; numbers separated by spaces).
fn value_bytes(id: u8, text: &str) -> Vec<u8> {
    let Some((name, size, _)) = kind(id) else { return Vec::new() };
    match id {
        VOID => Vec::new(),
        STR => text.as_bytes().to_vec(),
        BIN => (0..text.len() / 2).filter_map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()).collect(),
        IP4 => text.split('.').filter_map(|p| p.trim().parse::<u8>().ok()).collect(),
        _ => text
            .split_whitespace()
            .flat_map(|v| -> Vec<u8> {
                if name.ends_with('f') || name == "float" {
                    v.parse::<f32>().unwrap_or(0.0).to_be_bytes().to_vec()
                } else if name.ends_with('d') || name == "double" {
                    v.parse::<f64>().unwrap_or(0.0).to_be_bytes().to_vec()
                } else {
                    let n = v.parse::<i128>().unwrap_or(0) as u64;
                    n.to_be_bytes()[8 - size..].to_vec()
                }
            })
            .collect(),
    }
}

/// XML entities of a text or attribute value.
fn unescape(b: &[u8]) -> String {
    // Shift-JIS bytes kept: the text is handled as Latin-1, then written back as its bytes
    let s: String = b.iter().map(|&c| c as char).collect();
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// The text XML reader.
struct XmlReader<'a> {
    data: &'a [u8],
    at: usize,
}

impl XmlReader<'_> {
    fn peek(&self) -> Option<u8> {
        self.data.get(self.at).copied()
    }

    fn skip_space(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    fn starts(&self, s: &[u8]) -> bool {
        self.data[self.at..].starts_with(s)
    }

    /// Skips spaces, `<?...?>` and `<!--...-->`.
    fn skip_misc(&mut self) {
        loop {
            self.skip_space();
            let end: &[u8] = if self.starts(b"<?") {
                b"?>"
            } else if self.starts(b"<!--") {
                b"-->"
            } else {
                return;
            };
            match self.data[self.at..].windows(end.len()).position(|w| w == end) {
                Some(p) => self.at += p + end.len(),
                None => self.at = self.data.len(),
            }
        }
    }

    fn name(&mut self) -> String {
        let start = self.at;
        while self.peek().is_some_and(|c| !c.is_ascii_whitespace() && c != b'>' && c != b'/' && c != b'=') {
            self.at += 1;
        }
        String::from_utf8_lossy(&self.data[start..self.at]).into_owned()
    }

    fn element(&mut self) -> Option<Node> {
        if self.peek()? != b'<' {
            return None;
        }
        self.at += 1;
        let name = self.name();
        let mut node = Node::new(&name);
        let (mut type_name, mut count) = (None, false);
        // attributes
        loop {
            self.skip_space();
            match self.peek()? {
                b'/' => {
                    self.at += 2;
                    node.finish(type_name.as_deref(), count, "");
                    return Some(node);
                }
                b'>' => {
                    self.at += 1;
                    break;
                }
                _ => {
                    let key = self.name();
                    self.skip_space();
                    self.at += 1; // =
                    self.skip_space();
                    let quote = self.peek()?;
                    self.at += 1;
                    let start = self.at;
                    while self.peek()? != quote {
                        self.at += 1;
                    }
                    let value = unescape(&self.data[start..self.at]);
                    self.at += 1;
                    match key.as_str() {
                        "__type" => type_name = Some(value),
                        "__count" => count = true,
                        "__size" => {}
                        _ => node.attrs.push((key, value)),
                    }
                }
            }
        }
        // content: text and children, then the end tag
        let mut text = Vec::new();
        loop {
            if self.starts(b"</") {
                while self.peek()? != b'>' {
                    self.at += 1;
                }
                self.at += 1;
                break;
            }
            if self.starts(b"<!--") || self.starts(b"<?") {
                self.skip_misc();
                continue;
            }
            if self.peek()? == b'<' {
                let child = self.element()?;
                node.children.push(child);
            } else {
                text.push(self.peek()?);
                self.at += 1;
            }
        }
        let text = unescape(&text);
        node.finish(type_name.as_deref(), count, text.trim());
        Some(node)
    }
}

impl Node {
    /// A text XML node's value set from its `__type` (typeless with text: a string).
    fn finish(&mut self, type_name: Option<&str>, count: bool, text: &str) {
        let id = match type_name {
            Some(t) => kind_id(t).unwrap_or(STR),
            None if !text.is_empty() && self.children.is_empty() => STR,
            None => VOID,
        };
        self.kind = id;
        self.array = count;
        // strings: their bytes as sent (Shift-JIS), through the Latin-1 text
        self.raw = if id == STR { text.chars().map(|c| c as u8).collect() } else { value_bytes(id, text) };
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[derive(Default)]
struct Writer {
    nodes: Vec<u8>,
    data: Vec<u8>,
    /// Where the next 1-byte and 2-byte values go (in their shared 4-byte slots).
    byte_at: usize,
    word_at: usize,
}

impl Writer {
    fn name(&mut self, name: &str) {
        let bits: Vec<u8> = name.bytes().map(|c| SIXBIT.iter().position(|&s| s == c).unwrap_or(0) as u8).collect();
        self.nodes.push(bits.len() as u8);
        let mut acc: u32 = 0;
        let mut n = 0;
        for b in bits {
            acc = (acc << 6) | b as u32;
            n += 6;
            while n >= 8 {
                n -= 8;
                self.nodes.push((acc >> n) as u8);
            }
        }
        if n > 0 {
            self.nodes.push((acc << (8 - n)) as u8);
        }
    }

    fn pad(&mut self) {
        while self.data.len() % 4 != 0 {
            self.data.push(0);
        }
    }

    fn chunk(&mut self, bytes: &[u8]) {
        self.data.extend((bytes.len() as u32).to_be_bytes());
        self.data.extend(bytes);
        self.pad();
    }

    fn aligned(&mut self, bytes: &[u8]) {
        if self.byte_at % 4 == 0 {
            self.byte_at = self.data.len();
        }
        if self.word_at % 4 == 0 {
            self.word_at = self.data.len();
        }
        match bytes.len() {
            1 => {
                if self.byte_at % 4 == 0 {
                    self.data.extend([0; 4]);
                }
                self.data[self.byte_at] = bytes[0];
                self.byte_at += 1;
            }
            2 => {
                if self.word_at % 4 == 0 {
                    self.data.extend([0; 4]);
                }
                self.data[self.word_at..self.word_at + 2].copy_from_slice(bytes);
                self.word_at += 2;
            }
            _ => {
                self.data.extend(bytes);
                self.pad();
            }
        }
    }

    fn node(&mut self, node: &Node) {
        self.nodes.push(node.kind | if node.array { 0x40 } else { 0 });
        self.name(&node.name);
        let count = kind(node.kind).map(|k| k.2).unwrap_or(0);
        if node.kind == STR {
            let mut s = node.raw.clone();
            s.push(0);
            self.chunk(&s);
        } else if node.kind != VOID {
            if node.array || count == 0 {
                self.chunk(&node.raw.clone());
            } else {
                self.aligned(&node.raw.clone());
            }
        }
        let mut attrs = node.attrs.clone();
        attrs.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, value) in attrs {
            let mut s = value.into_bytes();
            s.push(0);
            self.chunk(&s);
            self.nodes.push(ATTR);
            self.name(&name);
        }
        for child in &node.children {
            self.node(child);
        }
        self.nodes.push(NODE_END);
    }
}

struct Reader<'a> {
    nodes: &'a [u8],
    at: usize,
    packed: bool,
    data: &'a [u8],
    data_at: usize,
    byte_at: usize,
    word_at: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Option<u8> {
        let b = *self.nodes.get(self.at)?;
        self.at += 1;
        Some(b)
    }

    fn name(&mut self) -> Option<String> {
        let len = self.byte()? as usize;
        if !self.packed {
            let len = (len & !0x40) + 1;
            let s = self.nodes.get(self.at..self.at + len)?;
            self.at += len;
            return Some(String::from_utf8_lossy(s).into_owned());
        }
        let bytes = (len * 6).div_ceil(8);
        let packed = self.nodes.get(self.at..self.at + bytes)?;
        self.at += bytes;
        let mut out = String::new();
        let mut acc: u32 = 0;
        let mut n = 0;
        let mut it = packed.iter();
        for _ in 0..len {
            while n < 6 {
                acc = (acc << 8) | *it.next()? as u32;
                n += 8;
            }
            n -= 6;
            out.push(SIXBIT[((acc >> n) & 0x3f) as usize] as char);
        }
        Some(out)
    }

    fn realign(&mut self) {
        self.data_at = self.data_at.div_ceil(4) * 4;
    }

    fn chunk(&mut self) -> Option<Vec<u8>> {
        let len = u32::from_be_bytes(self.data.get(self.data_at..self.data_at + 4)?.try_into().ok()?) as usize;
        let v = self.data.get(self.data_at + 4..self.data_at + 4 + len)?.to_vec();
        self.data_at += 4 + len;
        self.realign();
        Some(v)
    }

    fn string(&mut self) -> Option<String> {
        let mut v = self.chunk()?;
        while v.last() == Some(&0) {
            v.pop();
        }
        Some(String::from_utf8_lossy(&v).into_owned())
    }

    fn aligned(&mut self, size: usize) -> Option<Vec<u8>> {
        if self.byte_at % 4 == 0 {
            self.byte_at = self.data_at;
        }
        if self.word_at % 4 == 0 {
            self.word_at = self.data_at;
        }
        Some(match size {
            1 => {
                let v = vec![*self.data.get(self.byte_at)?];
                if self.byte_at % 4 == 0 {
                    self.data_at += 4;
                }
                self.byte_at += 1;
                v
            }
            2 => {
                let v = self.data.get(self.word_at..self.word_at + 2)?.to_vec();
                if self.word_at % 4 == 0 {
                    self.data_at += 4;
                }
                self.word_at += 2;
                v
            }
            _ => {
                let v = self.data.get(self.data_at..self.data_at + size)?.to_vec();
                self.data_at += size;
                self.realign();
                v
            }
        })
    }
}

/// RC4 (the e-Amusement payload's cipher).
pub fn rc4(key: &[u8], data: &mut [u8]) {
    let mut s: Vec<u8> = (0..=255).collect();
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let (mut i, mut j) = (0u8, 0u8);
    for b in data {
        i = i.wrapping_add(1);
        j = j.wrapping_add(s[i as usize]);
        s.swap(i as usize, j as usize);
        *b ^= s[s[i as usize].wrapping_add(s[j as usize]) as usize];
    }
}

/// e-Amusement's secret, after the `X-Eamuse-Info` header's 6 bytes in the RC4 key's MD5.
const SECRET: [u8; 26] = [
    0x69, 0xd7, 0x46, 0x27, 0xd9, 0x85, 0xee, 0x21, 0x87, 0x16, 0x15, 0x70, 0xd0, 0x8d, 0x93, 0xb1, 0x24, 0x55, 0x03, 0x5b, 0x6d,
    0xf0, 0xd8, 0x20, 0x5d, 0xf5,
];

/// The RC4 key of an `X-Eamuse-Info` header (`1-xxxxxxxx-xxxx`).
pub fn info_key(info: &str) -> Option<[u8; 16]> {
    let hex: String = info.trim().strip_prefix("1-")?.chars().filter(|c| *c != '-').collect();
    if hex.len() != 12 {
        return None;
    }
    let mut input: Vec<u8> = (0..6).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)).collect::<Result<_, _>>().ok()?;
    input.extend(SECRET);
    Some(md5(&input))
}

/// MD5 (RFC 1321).
pub fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11,
        16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend(bits.to_le_bytes());
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    for block in msg.chunks(64) {
        let m: Vec<u32> = block.chunks(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (i, v) in [a0, b0, c0, d0].into_iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// Konami's LZ77 (`X-Compress: lz77`): a flag byte, then 8 items, each a literal byte (flag bit
/// 1) or a 2-byte back reference (12-bit distance, 4-bit length + 3; distance 0 ends).
pub fn lz77_decompress(data: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0;
    loop {
        let Some(&flags) = data.get(i) else { return out };
        i += 1;
        for bit in 0..8 {
            if flags >> bit & 1 == 1 {
                let Some(&b) = data.get(i) else { return out };
                out.push(b);
                i += 1;
            } else {
                let (Some(&hi), Some(&lo)) = (data.get(i), data.get(i + 1)) else { return out };
                i += 2;
                let distance = ((hi as usize) << 4) | (lo as usize >> 4);
                if distance == 0 {
                    return out;
                }
                let length = (lo & 0x0f) as usize + 3;
                for _ in 0..length {
                    let b = if distance <= out.len() { out[out.len() - distance] } else { 0 };
                    out.push(b);
                }
            }
        }
    }
}
