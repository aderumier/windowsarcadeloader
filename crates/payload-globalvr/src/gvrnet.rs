//! PGA TOUR Golf Championship Edition III's cabinet database: the .NET COM classes of `C:\gvr.net` and
//! `C:\pga.net` (`Interface.CSharp.*`, `Infrastructure.Service.*`, ...) the shell's pages create
//! (`new ActiveXObject(...)`). On the cabinet they keep the configuration, coin and game
//! statistics and the online tournaments in a local SQL Server (MSDE) database the cabinet
//! synchronized by phone; none of it is in the dump. The payload answers them for a cabinet that
//! never registered online (no tournaments, no leaderboards from the network):
//!
//! * the data sets (`...DataSet` classes, the `Xml...` ones) are XML documents: `SetXml(xml)`,
//!   `GetXml()`, their tables as properties (`ds.SoundVolume`: every element of that name with
//!   child elements), `table.Item(i)` / `table.Count`, the row's fields as properties (its simple
//!   child elements, read and written);
//! * the cabinet configuration (`CabinetConfiguration.GetCabinetConfig` / `GetFeesConfig`, the
//!   XML the shell edits in its data sets) starts from the country defaults the cabinet's
//!   installation copies (`C:\gvr.net\Country\US`, with free play), its changes saved next to
//!   them (`C:\gvr.net\*-saved.xml`);
//! * the rest answers "nothing there": `IsError` false, `GetError` "", counts 0, the XML of
//!   tournaments and leaderboards empty (each first call is logged).
//!
//! The classes are registered for the frontend's thread (`CoRegisterClassObject`) under CLSIDs of
//! the payload, their ProgIDs in the machine's classes.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::Mutex;

use wal_payload_common::log;
use windows_sys::Win32::System::Registry::{HKEY, HKEY_LOCAL_MACHINE, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegSetValueExW};
use windows_sys::core::GUID;

type P = *mut c_void;
type HRESULT = i32;

const S_OK: HRESULT = 0;
const E_NOTIMPL: HRESULT = 0x8000_4001u32 as i32;
const E_NOINTERFACE: HRESULT = 0x8000_4002u32 as i32;
const E_POINTER: HRESULT = 0x8000_4003u32 as i32;
const CLASS_E_NOAGGREGATION: HRESULT = 0x8004_0110u32 as i32;
const DISP_E_MEMBERNOTFOUND: HRESULT = 0x8002_0003u32 as i32;

const IID_UNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
const IID_CLASS_FACTORY: GUID = GUID::from_u128(0x00000001_0000_0000_c000_000000000046);
const IID_DISPATCH: GUID = GUID::from_u128(0x00020400_0000_0000_c000_000000000046);
const IID_OBJECT_SAFETY: GUID = GUID::from_u128(0xcb5bdc81_93c1_11cf_8f20_00805f2cd064);

const DISPATCH_PROPERTYPUT: u16 = 4;
const DISPATCH_PROPERTYPUTREF: u16 = 8;

#[link(name = "oleaut32")]
unsafe extern "system" {
    fn SysAllocStringLen(s: *const u16, len: u32) -> *mut u16;
    fn SysStringLen(s: *const u16) -> u32;
    fn VariantChangeType(dst: *mut Variant, src: *const Variant, flags: u16, vt: u16) -> HRESULT;
    fn VariantClear(v: *mut Variant) -> HRESULT;
}

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoRegisterClassObject(clsid: *const GUID, unk: P, ctx: u32, flags: u32, cookie: *mut u32) -> HRESULT;
}

const VT_EMPTY: u16 = 0;
const VT_I4: u16 = 3;
const VT_R8: u16 = 5;
const VT_BSTR: u16 = 8;
const VT_DISPATCH: u16 = 9;
const VT_BOOL: u16 = 11;

/// `VARIANT` (16 bytes on 32-bit): its type, the value at +8.
#[repr(C)]
#[derive(Clone, Copy)]
struct Variant {
    vt: u16,
    reserved: [u16; 3],
    value: [u32; 2],
}

/// `DISPPARAMS`: the arguments last to first.
#[repr(C)]
struct DispParams {
    args: *const Variant,
    named: *const i32,
    count: u32,
    named_count: u32,
}

/// A script value.
#[derive(Clone)]
enum Value {
    Empty,
    Bool(bool),
    Int(i32),
    Real(f64),
    Str(String),
    Obj(Rc<Obj>),
}

impl Value {
    fn text(&self) -> String {
        match self {
            Value::Empty | Value::Obj(_) => String::new(),
            Value::Bool(b) => b.to_string(),
            Value::Int(i) => i.to_string(),
            Value::Real(r) => r.to_string(),
            Value::Str(s) => s.clone(),
        }
    }

    fn int(&self) -> i32 {
        match self {
            Value::Bool(b) => *b as i32,
            Value::Int(i) => *i,
            Value::Real(r) => *r as i32,
            Value::Str(s) => s.trim().parse().unwrap_or(0),
            _ => 0,
        }
    }

    /// An XML field's text as the typed data set returns it: numbers and booleans typed.
    fn from_field(text: &str) -> Value {
        let t = text.trim();
        match t {
            "true" => return Value::Bool(true),
            "false" => return Value::Bool(false),
            _ => {}
        }
        let digits = t.strip_prefix('-').unwrap_or(t);
        if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && (digits.len() == 1 || !digits.starts_with('0')) {
            if let Ok(i) = t.parse::<i32>() {
                return Value::Int(i);
            }
            if let Ok(r) = t.parse::<f64>() {
                return Value::Real(r);
            }
        }
        Value::Str(text.to_string())
    }

    fn debug(&self) -> String {
        match self {
            Value::Empty => "undefined".into(),
            Value::Obj(o) => format!("<{}>", o.kind_name()),
            Value::Str(s) => format!("{:?}", s.chars().take(60).collect::<String>()),
            v => v.text(),
        }
    }

    fn from_variant(v: &Variant) -> Value {
        let v = if v.vt & 0x4000 != 0 && v.vt & 0xfff == 12 {
            // VT_BYREF | VT_VARIANT
            unsafe { *(v.value[0] as usize as *const Variant) }
        } else {
            *v
        };
        match v.vt {
            VT_EMPTY | 1 => Value::Empty,
            VT_BOOL => Value::Bool(v.value[0] & 0xffff != 0),
            VT_I4 | 2 | 16 | 17 | 18 | 19 | 22 | 23 => {
                let mut out = Variant { vt: 0, reserved: [0; 3], value: [0; 2] };
                if unsafe { VariantChangeType(&mut out, &v, 0, VT_I4) } >= 0 { Value::Int(out.value[0] as i32) } else { Value::Empty }
            }
            VT_R8 | 4 => {
                let mut out = Variant { vt: 0, reserved: [0; 3], value: [0; 2] };
                if unsafe { VariantChangeType(&mut out, &v, 0, VT_R8) } >= 0 {
                    let r = f64::from_bits(out.value[0] as u64 | (out.value[1] as u64) << 32);
                    if r.fract() == 0.0 && r.abs() < i32::MAX as f64 { Value::Int(r as i32) } else { Value::Real(r) }
                } else {
                    Value::Empty
                }
            }
            VT_BSTR => {
                let b = v.value[0] as usize as *const u16;
                if b.is_null() {
                    return Value::Str(String::new());
                }
                let len = unsafe { SysStringLen(b) } as usize;
                Value::Str(String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(b, len) }))
            }
            _ => {
                let mut out = Variant { vt: 0, reserved: [0; 3], value: [0; 2] };
                if unsafe { VariantChangeType(&mut out, &v, 0, VT_BSTR) } >= 0 {
                    let s = Value::from_variant(&out);
                    unsafe { VariantClear(&mut out) };
                    s
                } else {
                    Value::Empty
                }
            }
        }
    }

    fn into_variant(self) -> Variant {
        let mut v = Variant { vt: VT_EMPTY, reserved: [0; 3], value: [0; 2] };
        match self {
            Value::Empty => {}
            Value::Bool(b) => {
                v.vt = VT_BOOL;
                v.value[0] = if b { 0xffff } else { 0 };
            }
            Value::Int(i) => {
                v.vt = VT_I4;
                v.value[0] = i as u32;
            }
            Value::Real(r) => {
                v.vt = VT_R8;
                let bits = r.to_bits();
                v.value = [bits as u32, (bits >> 32) as u32];
            }
            Value::Str(s) => {
                let w: Vec<u16> = s.encode_utf16().collect();
                v.vt = VT_BSTR;
                v.value[0] = unsafe { SysAllocStringLen(w.as_ptr(), w.len() as u32) } as usize as u32;
            }
            Value::Obj(o) => {
                v.vt = VT_DISPATCH;
                v.value[0] = Com::wrap(o) as usize as u32;
            }
        }
        v
    }
}

/// An XML element: its name (without prefix), its attributes as written, its text (simple
/// elements) or children.
pub(crate) struct Element {
    pub(crate) name: String,
    attrs: String,
    pub(crate) text: String,
    pub(crate) children: Vec<usize>,
}

/// An XML document, elements by index (0 the root).
#[derive(Default)]
pub(crate) struct Doc {
    pub(crate) elements: Vec<Element>,
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

impl Doc {
    /// Parses `xml` (elements, attributes and text; declarations and comments skipped).
    pub(crate) fn parse(xml: &str) -> Doc {
        let mut doc = Doc::default();
        let mut stack: Vec<usize> = Vec::new();
        let mut rest = xml;
        while let Some(lt) = rest.find('<') {
            let text = &rest[..lt];
            if let Some(&top) = stack.last()
                && doc.elements[top].children.is_empty()
            {
                doc.elements[top].text.push_str(&unescape(text));
            }
            rest = &rest[lt..];
            if rest.starts_with("<?") || rest.starts_with("<!") {
                let end = if rest.starts_with("<!--") { rest.find("-->").map(|e| e + 3) } else { rest.find('>').map(|e| e + 1) };
                rest = &rest[end.unwrap_or(rest.len())..];
                continue;
            }
            let Some(gt) = rest.find('>') else { break };
            let tag = &rest[1..gt];
            rest = &rest[gt + 1..];
            if let Some(name) = tag.strip_prefix('/') {
                let _ = name;
                stack.pop();
                continue;
            }
            let empty = tag.ends_with('/');
            let tag = tag.trim_end_matches('/').trim();
            let (name, attrs) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
            let name = name.rsplit(':').next().unwrap_or(name).to_string();
            let index = doc.elements.len();
            doc.elements.push(Element { name, attrs: attrs.trim().to_string(), text: String::new(), children: Vec::new() });
            if let Some(&top) = stack.last() {
                doc.elements[top].text.clear();
                doc.elements[top].children.push(index);
            }
            if !empty {
                stack.push(index);
            }
        }
        doc
    }

    fn write(&self, index: usize, out: &mut String) {
        let e = &self.elements[index];
        out.push('<');
        out.push_str(&e.name);
        if !e.attrs.is_empty() {
            out.push(' ');
            out.push_str(&e.attrs);
        }
        out.push('>');
        if e.children.is_empty() {
            out.push_str(&escape(&e.text));
        } else {
            for &c in &e.children {
                self.write(c, out);
            }
        }
        out.push_str("</");
        out.push_str(&e.name);
        out.push('>');
    }

    fn xml(&self) -> String {
        let mut out = String::new();
        if !self.elements.is_empty() {
            self.write(0, &mut out);
        }
        out
    }

    /// The rows of table `name`: the elements of that name with children, in document order.
    pub(crate) fn rows(&self, name: &str) -> Vec<usize> {
        (1..self.elements.len())
            .filter(|&i| self.elements[i].name.eq_ignore_ascii_case(name) && !self.elements[i].children.is_empty())
            .collect()
    }

    /// The children of `row` named `name` (a nested table's rows).
    pub(crate) fn children(&self, row: usize, name: &str) -> Vec<usize> {
        self.elements[row].children.iter().copied().filter(|&c| self.elements[c].name.eq_ignore_ascii_case(name)).collect()
    }

    /// `other`'s rows added (a data set's `ReadXml` merges): its root's children appended to this
    /// root's, or `other` itself when this one is empty.
    pub(crate) fn merge(&mut self, other: Doc) {
        if self.elements.is_empty() {
            *self = other;
            return;
        }
        let shift = self.elements.len();
        let top: Vec<usize> = other.elements.first().map(|r| r.children.iter().map(|c| c + shift).collect()).unwrap_or_default();
        for mut e in other.elements {
            e.children.iter_mut().for_each(|c| *c += shift);
            self.elements.push(e);
        }
        self.elements[0].children.extend(top);
    }

    /// Field `name` of row `row`: its simple child element.
    pub(crate) fn field(&self, row: usize, name: &str) -> Option<usize> {
        self.elements[row].children.iter().copied().find(|&c| self.elements[c].name.eq_ignore_ascii_case(name))
    }
}

/// A class of the cabinet's .NET libraries.
struct Class {
    progid: &'static str,
    /// Its instances are data sets (XML documents).
    data_set: bool,
}

/// The classes the shell's pages and scripts create.
const CLASSES: &[Class] = &[
    Class { progid: "Interface.CSharp.CabinetConfiguration.CabinetConfiguration", data_set: false },
    Class { progid: "Interface.CSharp.CabinetConfiguration.CabinetConfigDataSet", data_set: true },
    Class { progid: "Interface.CSharp.CabinetConfiguration.FeesConfigDataSet", data_set: true },
    Class { progid: "Interface.CSharp.CabinetConfiguration.XmlSimpleTypeEnumValues", data_set: true },
    Class { progid: "Interface.CSharp.CabinetRegistration.CabinetRegistration", data_set: false },
    Class { progid: "Interface.CSharp.Dongle.Dongle", data_set: false },
    Class { progid: "Interface.CSharp.FuzzyCompare.FuzzyCompare", data_set: false },
    Class { progid: "Interface.CSharp.GameInfo.GameInfo", data_set: false },
    Class { progid: "Interface.CSharp.GameSession.GameSession", data_set: false },
    Class { progid: "Interface.CSharp.ISPPop.Pop", data_set: false },
    Class { progid: "Interface.CSharp.LeaderBoard.XmlCabinetLeaderBoard", data_set: true },
    Class { progid: "Interface.CSharp.Leaderboard.Leaderboard", data_set: false },
    Class { progid: "Interface.CSharp.LocalHero.LocalHero", data_set: false },
    Class { progid: "Interface.CSharp.PlayerGameSummary.PlayerGameSummary", data_set: false },
    Class { progid: "Interface.CSharp.PlayerGameSummary.XmlWorldTourPlayerCourseGameResult", data_set: true },
    Class { progid: "Interface.CSharp.PlayerRegistration.PlayerRegistration", data_set: false },
    Class { progid: "Interface.CSharp.Stats.CoinStats", data_set: false },
    Class { progid: "Interface.CSharp.Stats.CourseStats", data_set: false },
    Class { progid: "Interface.CSharp.Stats.GameStats", data_set: false },
    Class { progid: "Interface.CSharp.Stats.GolferStats", data_set: false },
    Class { progid: "Interface.CSharp.Stats.PlayerGamesStats", data_set: false },
    Class { progid: "Interface.CSharp.Stats.TimeStats", data_set: false },
    Class { progid: "Interface.CSharp.Stats.XmlTimeStats", data_set: true },
    Class { progid: "Interface.CSharp.Tournament.Tournament", data_set: false },
    Class { progid: "Interface.CSharp.Tournament.TournamentData", data_set: false },
    Class { progid: "Interface.CSharp.TournamentCreation.TournamentCreation", data_set: false },
    Class { progid: "Interface.CSharp.TournamentCreation.TournamentCreationConfigDataSet", data_set: true },
    Class { progid: "Interface.CSharp.Utils.Utils", data_set: false },
    Class { progid: "Infrastructure.Service.Tournament.XML.XmlTournamentInformation", data_set: true },
    Class { progid: "GvrPgaPlayingCondition.PgaPlayingCondition", data_set: false },
    Class { progid: "GvrEncode.GvrInfo", data_set: false },
];

/// The payload's CLSID of class `index`.
fn clsid(index: usize) -> GUID {
    GUID::from_u128(0x7a4c_0000_6a1e_4f47_9d2b_5741_4c47_0000 | (index as u128) << 96 | index as u128)
}

enum Kind {
    Class(&'static Class),
    Table(Rc<RefCell<Doc>>, String),
    Row(Rc<RefCell<Doc>>, usize),
}

/// A script object: its kind, the data set's document, the values the script set.
struct Obj {
    kind: Kind,
    doc: Option<Rc<RefCell<Doc>>>,
    props: RefCell<Vec<(String, Value)>>,
}

impl Obj {
    fn new(kind: Kind) -> Rc<Obj> {
        let doc = match &kind {
            Kind::Class(c) if c.data_set => Some(Rc::new(RefCell::new(Doc::default()))),
            _ => None,
        };
        Rc::new(Obj { kind, doc, props: RefCell::new(Vec::new()) })
    }

    fn kind_name(&self) -> String {
        match &self.kind {
            Kind::Class(c) => c.progid.rsplit('.').next().unwrap_or(c.progid).to_string(),
            Kind::Table(_, t) => format!("table {t}"),
            Kind::Row(d, r) => format!("row {}", d.borrow().elements[*r].name),
        }
    }

    fn prop(&self, name: &str) -> Option<Value> {
        self.props.borrow().iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
    }

    fn set_prop(&self, name: &str, value: Value) {
        let mut props = self.props.borrow_mut();
        match props.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            Some(slot) => slot.1 = value,
            None => props.push((name.to_string(), value)),
        }
    }
}

/// Where the cabinet configuration's changes are kept, and its defaults.
fn config_paths(what: &str) -> (String, String) {
    (format!("C:\\gvr.net\\{what}-saved.xml"), format!("C:\\gvr.net\\Country\\US\\{what}.xml"))
}

/// The configuration `what` the operator saved, else the country defaults with free play on
/// (the cabinet configuration's `PlayMode` "On~0").
pub(crate) fn load_config(what: &str) -> String {
    let (saved, default) = config_paths(what);
    if let Ok(xml) = std::fs::read_to_string(&saved) {
        return xml;
    }
    let xml = std::fs::read_to_string(&default).unwrap_or_default();
    if what != "CabinetConfig" {
        return xml;
    }
    let mut doc = Doc::parse(&xml);
    for row in doc.rows("PlayMode") {
        if let Some(f) = doc.field(row, "Value") {
            doc.elements[f].text = "On~0".into();
        }
    }
    doc.xml()
}

fn save_config(what: &str, xml: &str) {
    let (saved, _) = config_paths(what);
    if let Err(e) = std::fs::write(&saved, xml) {
        log!("gvrnet: {saved}: {e}");
    }
}

/// The values of the XML schema type `name` in `xsd` (the cabinet's `C:\gvr.net\lib\US_*Types.xsd`):
/// its enumeration (not the ones with an id, its default and step), else its range by its step.
fn type_values(xsd: &str, name: &str) -> Vec<String> {
    let Some(start) = xsd.find(&format!("simpleType name=\"{name}\"")) else { return Vec::new() };
    let body = &xsd[start..];
    let body = &body[..body.find("</xs:simpleType>").unwrap_or(body.len())];
    let attr = |tag: &str, key: &str| -> Option<String> {
        let at = tag.find(&format!("{key}=\""))? + key.len() + 2;
        Some(tag[at..at + tag[at..].find('"')?].to_string())
    };
    let tags: Vec<&str> = body.split('<').skip(1).collect();
    let mut values: Vec<String> = tags
        .iter()
        .filter(|t| t.starts_with("xs:enumeration") && attr(t, "id").is_none())
        .filter_map(|t| attr(t, "value"))
        .collect();
    let bound = |kind: &str| tags.iter().find(|t| t.starts_with(kind)).and_then(|t| attr(t, "value")).and_then(|v| v.parse::<i64>().ok());
    if values.is_empty()
        && let (Some(min), Some(max)) = (bound("xs:minInclusive"), bound("xs:maxInclusive"))
    {
        let step = tags
            .iter()
            .find(|t| attr(t, "id").is_some_and(|id| id.ends_with("StepValue")))
            .and_then(|t| attr(t, "value"))
            .and_then(|v| v.parse::<i64>().ok())
            .filter(|&s| s > 0)
            .unwrap_or(1);
        values = (min..=max).step_by(step as usize).map(|v| v.to_string()).collect();
    }
    values
}

/// `GetSimpleTypeEnumValues(config, type)`: the values of a configuration field's type, as the
/// `XmlSimpleTypeEnumValues` data set reads them (table `Enums`, field `Value`).
fn enum_values(config: &str, name: &str) -> String {
    let file = if config.to_ascii_uppercase().contains("FEE") { "FeesConfigTypes" } else { "CabinetConfigTypes" };
    let xsd = std::fs::read_to_string(format!("C:\\gvr.net\\lib\\US_{file}.xsd")).unwrap_or_default();
    let mut xml = String::from("<XmlSimpleTypeEnumValues>");
    for v in type_values(&xsd, name) {
        xml.push_str(&format!("<Enums><Value>{}</Value></Enums>", escape(&v)));
    }
    xml.push_str("</XmlSimpleTypeEnumValues>");
    xml
}

/// The answer of a class's method or property `name` (lower case) to `args`.
fn call(class: &Class, obj: &Obj, name: &str, args: &[Value]) -> Value {
    let short = class.progid.rsplit('.').next().unwrap_or("");
    match (short, name) {
        ("CabinetConfiguration", "getcabinetconfig") => return Value::Str(load_config("CabinetConfig")),
        ("CabinetConfiguration", "getfeesconfig") => return Value::Str(load_config("FeesConfig")),
        ("CabinetConfiguration", "getsimpletypeenumvalues") => {
            let arg = |i: usize| args.get(i).map(Value::text).unwrap_or_default();
            return Value::Str(enum_values(&arg(0), &arg(1)));
        }
        ("CabinetConfiguration", "setcabinetconfig") => {
            save_config("CabinetConfig", &args.first().map(Value::text).unwrap_or_default());
            return Value::Empty;
        }
        ("CabinetConfiguration", "setfeesconfig") => {
            save_config("FeesConfig", &args.first().map(Value::text).unwrap_or_default());
            return Value::Empty;
        }
        _ => {}
    }
    if let Some(v) = obj.prop(name) {
        return v;
    }
    // nothing there
    if name == "iserror" || name.starts_with("is") || name.starts_with("has") {
        Value::Bool(false)
    } else if name.contains("count") || name.contains("size") {
        Value::Int(0)
    } else if name.starts_with("get") {
        Value::Str(String::new())
    } else {
        Value::Empty
    }
}

/// `obj.name(args)` or `obj.name` (`put`: `obj.name = args[0]`).
fn invoke(obj: &Rc<Obj>, name: &str, put: bool, args: &[Value]) -> Result<Value, HRESULT> {
    let lower = name.to_ascii_lowercase();
    match &obj.kind {
        Kind::Class(class) => {
            if let Some(doc) = &obj.doc {
                match lower.as_str() {
                    "setxml" | "readxml" | "loadxml" => {
                        *doc.borrow_mut() = Doc::parse(&args.first().map(Value::text).unwrap_or_default());
                        return Ok(Value::Empty);
                    }
                    "getxml" => return Ok(Value::Str(doc.borrow().xml())),
                    _ if !put => return Ok(Value::Obj(Obj::new(Kind::Table(doc.clone(), name.to_string())))),
                    _ => {}
                }
            }
            if put {
                obj.set_prop(name, args.first().cloned().unwrap_or(Value::Empty));
                return Ok(Value::Empty);
            }
            Ok(call(class, obj, &lower, args))
        }
        Kind::Table(doc, table) => match lower.as_str() {
            "item" => {
                let rows = doc.borrow().rows(table);
                let i = args.first().map(Value::int).unwrap_or(0);
                Ok(rows.get(i as usize).map_or(Value::Empty, |&r| Value::Obj(Obj::new(Kind::Row(doc.clone(), r)))))
            }
            "count" | "length" => Ok(Value::Int(doc.borrow().rows(table).len() as i32)),
            _ => Err(DISP_E_MEMBERNOTFOUND),
        },
        Kind::Row(doc, row) => {
            let field = doc.borrow().field(*row, name);
            if put {
                let text = args.first().map(Value::text).unwrap_or_default();
                let mut d = doc.borrow_mut();
                match field {
                    Some(f) => d.elements[f].text = text,
                    None => {
                        let index = d.elements.len();
                        d.elements.push(Element { name: name.to_string(), attrs: String::new(), text, children: Vec::new() });
                        d.elements[*row].children.push(index);
                    }
                }
                return Ok(Value::Empty);
            }
            let d = doc.borrow();
            Ok(field.map_or(Value::Empty, |f| if d.elements[f].children.is_empty() { Value::from_field(&d.elements[f].text) } else { Value::Empty }))
        }
    }
}

/// The member names, by DISPID - 1 (case-insensitive).
static NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn dispid(name: &str) -> i32 {
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = names.iter().position(|n| n.eq_ignore_ascii_case(name)) {
        return i as i32 + 1;
    }
    names.push(name.to_string());
    names.len() as i32
}

fn name_of(id: i32) -> Option<String> {
    NAMES.lock().unwrap_or_else(|e| e.into_inner()).get((id as usize).wrapping_sub(1)).cloned()
}

/// The first calls of each member, logged.
static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn log_once(obj: &Obj, name: &str, put: bool, args: &[Value], result: &Value) {
    let key = format!("{}.{name}", obj.kind_name());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.iter().filter(|k| **k == key).count() >= 3 {
        return;
    }
    seen.push(key.clone());
    let args: Vec<String> = args.iter().map(Value::debug).collect();
    if put {
        log!("gvrnet: {key} = {}", args.join(", "));
    } else {
        log!("gvrnet: {key}({}) -> {}", args.join(", "), result.debug());
    }
}

/// The COM object of a script object: IDispatch, and IObjectSafety (safe for scripts) next.
#[repr(C)]
struct Com {
    vtable: *const Vtable<7>,
    safety: *const Vtable<5>,
    refs: Cell<u32>,
    obj: Rc<Obj>,
}

impl Com {
    fn wrap(obj: Rc<Obj>) -> *mut Com {
        Box::into_raw(Box::new(Com { vtable: &DISPATCH_VTABLE, safety: &SAFETY_VTABLE, refs: Cell::new(1), obj }))
    }
}

/// A COM vtable.
#[repr(transparent)]
struct Vtable<const N: usize>([*const (); N]);
unsafe impl<const N: usize> Sync for Vtable<N> {}

static DISPATCH_VTABLE: Vtable<7> = Vtable([
    query_interface as *const (),
    add_ref as *const (),
    release as *const (),
    type_info_count as *const (),
    type_info as *const (),
    ids_of_names as *const (),
    invoke_com as *const (),
]);

static SAFETY_VTABLE: Vtable<5> = Vtable([
    safety_query as *const (),
    safety_add_ref as *const (),
    safety_release as *const (),
    safety_get as *const (),
    safety_set as *const (),
]);

fn guid_eq(a: *const GUID, b: &GUID) -> bool {
    !a.is_null() && unsafe { (*a).data1 == b.data1 && (*a).data2 == b.data2 && (*a).data3 == b.data3 && (*a).data4 == b.data4 }
}

unsafe extern "system" fn query_interface(this: *mut Com, iid: *const GUID, out: *mut P) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    let p = if guid_eq(iid, &IID_UNKNOWN) || guid_eq(iid, &IID_DISPATCH) {
        this as P
    } else if guid_eq(iid, &IID_OBJECT_SAFETY) {
        unsafe { &raw mut (*this).safety as P }
    } else {
        unsafe { *out = std::ptr::null_mut() };
        return E_NOINTERFACE;
    };
    unsafe {
        (*this).refs.set((*this).refs.get() + 1);
        *out = p;
    }
    S_OK
}

unsafe extern "system" fn add_ref(this: *mut Com) -> u32 {
    let refs = unsafe { &(*this).refs };
    refs.set(refs.get() + 1);
    refs.get()
}

unsafe extern "system" fn release(this: *mut Com) -> u32 {
    let refs = unsafe { &(*this).refs };
    let n = refs.get().saturating_sub(1);
    refs.set(n);
    if n == 0 {
        drop(unsafe { Box::from_raw(this) });
    }
    n
}

unsafe extern "system" fn type_info_count(_this: *mut Com, count: *mut u32) -> HRESULT {
    if !count.is_null() {
        unsafe { *count = 0 };
    }
    S_OK
}

unsafe extern "system" fn type_info(_this: *mut Com, _index: u32, _lcid: u32, out: *mut P) -> HRESULT {
    if !out.is_null() {
        unsafe { *out = std::ptr::null_mut() };
    }
    E_NOTIMPL
}

unsafe extern "system" fn ids_of_names(_this: *mut Com, _iid: *const GUID, names: *const *const u16, count: u32, _lcid: u32, ids: *mut i32) -> HRESULT {
    for i in 0..count as usize {
        let p = unsafe { *names.add(i) };
        let len = (0..).take_while(|&k| unsafe { *p.add(k) } != 0).count();
        let name = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) });
        unsafe { *ids.add(i) = if i == 0 { dispid(&name) } else { -1 } };
    }
    S_OK
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn invoke_com(
    this: *mut Com,
    id: i32,
    _iid: *const GUID,
    _lcid: u32,
    flags: u16,
    params: *const DispParams,
    result: *mut Variant,
    _excep: P,
    _arg_err: *mut u32,
) -> HRESULT {
    let obj = unsafe { (*this).obj.clone() };
    // DISPID_VALUE: the default member (a table's Item)
    let name = if id == 0 { Some("Item".to_string()) } else { name_of(id) };
    let Some(name) = name else { return DISP_E_MEMBERNOTFOUND };
    let mut args = Vec::new();
    if let Some(p) = unsafe { params.as_ref() } {
        for i in (0..p.count as usize).rev() {
            args.push(Value::from_variant(unsafe { &*p.args.add(i) }));
        }
    }
    let put = flags & (DISPATCH_PROPERTYPUT | DISPATCH_PROPERTYPUTREF) != 0;
    // a property put's value (the first of DISPPARAMS, last here) first
    if put && let Some(value) = args.pop() {
        args.insert(0, value);
    }
    match invoke(&obj, &name, put, &args) {
        Ok(v) => {
            log_once(&obj, &name, put, &args, &v);
            if !result.is_null() {
                unsafe { *result = v.into_variant() };
            }
            S_OK
        }
        Err(hr) => {
            log_once(&obj, &name, put, &args, &Value::Str("<not found>".into()));
            hr
        }
    }
}

unsafe fn safety_base(this: P) -> *mut Com {
    unsafe { (this as *mut u8).sub(std::mem::offset_of!(Com, safety)) as *mut Com }
}

unsafe extern "system" fn safety_query(this: P, iid: *const GUID, out: *mut P) -> HRESULT {
    unsafe { query_interface(safety_base(this), iid, out) }
}

unsafe extern "system" fn safety_add_ref(this: P) -> u32 {
    unsafe { add_ref(safety_base(this)) }
}

unsafe extern "system" fn safety_release(this: P) -> u32 {
    unsafe { release(safety_base(this)) }
}

/// `GetInterfaceSafetyOptions`: safe for untrusted callers and data.
unsafe extern "system" fn safety_get(_this: P, _iid: *const GUID, supported: *mut u32, enabled: *mut u32) -> HRESULT {
    unsafe {
        if !supported.is_null() {
            *supported = 3;
        }
        if !enabled.is_null() {
            *enabled = 3;
        }
    }
    S_OK
}

unsafe extern "system" fn safety_set(_this: P, _iid: *const GUID, _mask: u32, _enabled: u32) -> HRESULT {
    S_OK
}

/// A class factory: its class's index in [`CLASSES`].
#[repr(C)]
struct Factory {
    vtable: *const Vtable<5>,
    class: usize,
}

static FACTORY_VTABLE: Vtable<5> = Vtable([
    factory_query as *const (),
    factory_ref as *const (),
    factory_ref as *const (),
    factory_create as *const (),
    factory_lock as *const (),
]);

unsafe extern "system" fn factory_query(this: P, iid: *const GUID, out: *mut P) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    if guid_eq(iid, &IID_UNKNOWN) || guid_eq(iid, &IID_CLASS_FACTORY) {
        unsafe { *out = this };
        return S_OK;
    }
    unsafe { *out = std::ptr::null_mut() };
    E_NOINTERFACE
}

unsafe extern "system" fn factory_ref(_this: P) -> u32 {
    1
}

unsafe extern "system" fn factory_create(this: *const Factory, outer: P, iid: *const GUID, out: *mut P) -> HRESULT {
    if !outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }
    let class = &CLASSES[unsafe { (*this).class }];
    log!("gvrnet: new {}", class.progid);
    let com = Com::wrap(Obj::new(Kind::Class(class)));
    let hr = unsafe { query_interface(com, iid, out) };
    unsafe { release(com) };
    hr
}

unsafe extern "system" fn factory_lock(_this: P, _lock: i32) -> HRESULT {
    S_OK
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// `HKLM\Software\Classes\<progid>\CLSID` = `clsid` (Wine reads no user classes).
fn register_progid(progid: &str, clsid: &GUID) {
    let text = format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        clsid.data1, clsid.data2, clsid.data3, clsid.data4[0], clsid.data4[1], clsid.data4[2], clsid.data4[3], clsid.data4[4],
        clsid.data4[5], clsid.data4[6], clsid.data4[7]
    );
    let mut key: HKEY = std::ptr::null_mut();
    let path = wide(&format!("Software\\Classes\\{progid}\\CLSID"));
    unsafe {
        if RegCreateKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_WRITE, std::ptr::null(), &mut key, std::ptr::null_mut()) == 0 {
            let value = wide(&text);
            RegSetValueExW(key, std::ptr::null(), 0, REG_SZ, value.as_ptr().cast(), (value.len() * 2) as u32);
            RegCloseKey(key);
        }
    }
}

/// The classes registered for the calling thread (the frontend's), when the dump is PGA TOUR's
/// (its `C:\gvr.net`).
pub(crate) fn register() {
    if !crate::pga::is_dump() {
        return;
    }
    const CLSCTX_INPROC_SERVER: u32 = 1;
    const REGCLS_MULTIPLEUSE: u32 = 1;
    let factories: &'static [Factory] =
        Box::leak((0..CLASSES.len()).map(|class| Factory { vtable: &FACTORY_VTABLE, class }).collect::<Vec<_>>().into_boxed_slice());
    let mut failed = 0;
    for (i, (class, factory)) in CLASSES.iter().zip(factories).enumerate() {
        let id = clsid(i);
        register_progid(class.progid, &id);
        let mut cookie = 0;
        if unsafe { CoRegisterClassObject(&id, factory as *const Factory as P, CLSCTX_INPROC_SERVER, REGCLS_MULTIPLEUSE, &mut cookie) } < 0 {
            failed += 1;
        }
    }
    log!("gvrnet: {} cabinet database classes registered ({failed} failed)", CLASSES.len());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_set_round_trip() {
        let xml = "<?xml version=\"1.0\"?>\n<DS xmlns=\"x\">\n <Cfg>\n  <ID>0</ID>\n  <Vol><Value>Low~2</Value></Vol>\n  <Vol><Value>a &amp; b</Value></Vol>\n </Cfg>\n</DS>";
        let mut doc = Doc::parse(xml);
        let rows = doc.rows("Vol");
        assert_eq!(rows.len(), 2);
        let f = doc.field(rows[1], "value").unwrap();
        assert_eq!(doc.elements[f].text, "a & b");
        doc.elements[f].text = "<c>".into();
        assert_eq!(doc.xml(), "<DS xmlns=\"x\"><Cfg><ID>0</ID><Vol><Value>Low~2</Value></Vol><Vol><Value>&lt;c&gt;</Value></Vol></Cfg></DS>");
        assert_eq!(doc.rows("Cfg").len(), 1);
    }

    #[test]
    fn schema_type_values() {
        let xsd = r#"<xs:simpleType name="PulseType"><xs:restriction base="xs:string"><xs:enumeration value="Pulse" />
            <xs:enumeration value="Tone" /><xs:enumeration id="PulseTypeDefaultValue" value="Tone" /></xs:restriction></xs:simpleType>
            <xs:simpleType name="SkinType"><xs:restriction base="xs:int"><xs:minInclusive value="2" /><xs:maxInclusive value="8" />
            <xs:enumeration id="SkinTypeDefaultValue" value="3" /><xs:enumeration id="SkinTypeStepValue" value="2" /></xs:restriction></xs:simpleType>"#;
        assert_eq!(type_values(xsd, "PulseType"), ["Pulse", "Tone"]);
        assert_eq!(type_values(xsd, "SkinType"), ["2", "4", "6", "8"]);
        assert!(type_values(xsd, "Other").is_empty());
    }

    #[test]
    fn field_types() {
        assert!(matches!(Value::from_field("60"), Value::Int(60)));
        assert!(matches!(Value::from_field("false"), Value::Bool(false)));
        assert!(matches!(Value::from_field("205-624-1005"), Value::Str(_)));
        assert!(matches!(Value::from_field("02"), Value::Str(_)));
        assert!(matches!(Value::from_field("161211622257775"), Value::Real(_)));
    }
}
