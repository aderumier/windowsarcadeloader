//! PGA TOUR Golf Championship Edition's game: the .NET data sets of `C:\pga.net` it reads its
//! weather and playing conditions with (`PgaWeatherGame`, `PgaPlayingCondition`,
//! `XmlPgaWindSpeed`: typed `DataSet` classes, created with `CoCreateInstance` by the game and
//! its engine). The cabinet registered them for COM (.NET 1.0, none in the dump): the payload
//! answers their creation with objects of its own over the XML documents (`gvrnet.rs`).
//!
//! The game calls them through their class interfaces (`ClassInterfaceType.AutoDual`: IDispatch,
//! then the members of `Object`, of the .NET base classes, then the class's own public members
//! in declaration order), at fixed vtable slots. A class's own members start at slot 87 for a
//! data set (`SetXml(string)`, then its tables' getters), 78 for a table (`get_Count`,
//! `get_Item(index)`), 60 for a row (its columns' getters and setters by pairs, the parent row's,
//! then `IsXNull` / `SetXNull` by pairs, then the child rows' arrays). The XML elements are the
//! columns' short names (`FN`, `FS`... the assemblies' `DataColumn` names).

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::core::GUID;

use crate::gvrnet::Doc;

type P = *mut c_void;
type HRESULT = i32;

const S_OK: HRESULT = 0;
const E_NOTIMPL: HRESULT = 0x8000_4001u32 as i32;
const E_NOINTERFACE: HRESULT = 0x8000_4002u32 as i32;
const E_INVALIDARG: HRESULT = 0x8007_0057u32 as i32;

#[link(name = "oleaut32")]
unsafe extern "system" {
    fn SysAllocStringLen(s: *const u16, len: u32) -> *mut u16;
    fn SysStringLen(s: *const u16) -> u32;
    fn SafeArrayCreateVector(vt: u16, lower: i32, count: u32) -> *mut SafeArray;
}

/// `SAFEARRAY` of one dimension (the engine reads its elements directly).
#[repr(C)]
struct SafeArray {
    dims: u16,
    features: u16,
    element_size: u32,
    locks: u32,
    data: *mut P,
    count: u32,
    lower: i32,
}

const VT_DISPATCH: u16 = 9;

#[derive(Clone, Copy)]
enum Column {
    Int(&'static str),
    Bool(&'static str),
    Float(&'static str),
    Text(&'static str),
}

/// A row class: its element, its columns, its child rows' arrays (vtable slot, element).
struct RowClass {
    element: &'static str,
    columns: &'static [Column],
    arrays: &'static [(usize, &'static str)],
}

/// A data set class: its CLSID, its tables' elements (getters from slot 88).
struct DataSetClass {
    name: &'static str,
    clsid: GUID,
    tables: &'static [&'static str],
}

const DATA_SETS: &[DataSetClass] = &[
    DataSetClass {
        name: "PgaWeatherGame",
        clsid: GUID::from_u128(0x77510b0d_8475_3b9d_9204_c07756aaa9a3),
        tables: &["WeatherGame", "FG", "RA"],
    },
    DataSetClass {
        name: "PgaPlayingCondition",
        clsid: GUID::from_u128(0x02d3d800_7b02_3186_a9da_746735094f0a),
        tables: &["PC", "TH", "WI"],
    },
    DataSetClass {
        name: "XmlPgaWindSpeed",
        clsid: GUID::from_u128(0x781934b4_9ada_303b_a998_c2f835bcc43f),
        tables: &["PgaWindSpeed", "WS"],
    },
];

use Column::{Bool, Float, Int, Text};

const ROWS: &[RowClass] = &[
    // PgaWeatherGame: WeatherGameRow (GetFogs, GetRains), Fog, Rain
    RowClass { element: "WeatherGame", columns: &[], arrays: &[(60, "FG"), (61, "RA")] },
    RowClass {
        element: "FG",
        columns: &[Bool("FN"), Int("FS"), Int("FE"), Int("FP"), Int("FR"), Int("FF")],
        arrays: &[],
    },
    RowClass { element: "RA", columns: &[Bool("RN"), Float("RD"), Int("RL"), Int("RV")], arrays: &[] },
    // PgaPlayingCondition: PCRow (day time, rain, fog, wind, difficulty; GetHolesRows), Holes
    // (number, order, sky, rain, fog, wind, wind speed, wind direction, pin; GetWindInfosRows),
    // WindInfos
    RowClass { element: "PC", columns: &[Int("TT"), Int("TR"), Int("TF"), Int("TW"), Text("TD")], arrays: &[(80, "TH")] },
    RowClass {
        element: "TH",
        columns: &[Int("N"), Int("O"), Int("WT"), Int("WR"), Int("WF"), Int("WW"), Int("WS"), Int("WD"), Int("PI")],
        arrays: &[(98, "WI")],
    },
    RowClass { element: "WI", columns: &[Int("WS"), Int("WD"), Int("PI")], arrays: &[] },
    // XmlPgaWindSpeed: PgaWindSpeedRow (GetWindSpeeds), WindSpeed (type, difficulty, min, max)
    RowClass { element: "PgaWindSpeed", columns: &[], arrays: &[(60, "WS")] },
    RowClass { element: "WS", columns: &[Int("WT"), Int("DF"), Int("MN"), Int("MX")], arrays: &[] },
];

enum Kind {
    DataSet(&'static DataSetClass),
    Table(&'static str),
    Row(&'static RowClass, usize),
}

/// An object of a data set: its vtable first, its document shared with its tables and rows.
#[repr(C)]
struct NetObj {
    vtable: *const usize,
    refs: Cell<u32>,
    kind: Kind,
    doc: Rc<RefCell<Doc>>,
}

fn new_obj(kind: Kind, doc: Rc<RefCell<Doc>>) -> *mut NetObj {
    let vtable = match kind {
        Kind::DataSet(_) => vtables().0,
        Kind::Table(_) => vtables().1,
        Kind::Row(..) => vtables().2,
    };
    Box::into_raw(Box::new(NetObj { vtable, refs: Cell::new(1), kind, doc }))
}

/// The vtable slots.
const SLOTS: usize = 120;

/// Slot `N` with one argument (getters, setters, arrays, `get_Count`).
unsafe extern "system" fn slot1<const N: usize>(this: *mut NetObj, a: usize) -> HRESULT {
    unsafe { call(this, N, &[a]) }
}

/// Slot `N` with two arguments (`SetXml(string, &result)`, `get_Item(index, &row)`).
unsafe extern "system" fn slot2<const N: usize>(this: *mut NetObj, a: usize, b: usize) -> HRESULT {
    unsafe { call(this, N, &[a, b]) }
}

macro_rules! slots {
    ($f:ident; $($n:literal)*) => { [$($f::<$n> as *const () as usize,)*] };
}

/// The vtables of data sets, tables and rows (made once: their two-argument slots differ).
fn vtables() -> (*const usize, *const usize, *const usize) {
    static MADE: std::sync::OnceLock<[usize; 3]> = std::sync::OnceLock::new();
    let made = MADE.get_or_init(|| {
        let one: [usize; SLOTS] = slots!(slot1; 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29
            30 31 32 33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63 64 65 66 67 68
            69 70 71 72 73 74 75 76 77 78 79 80 81 82 83 84 85 86 87 88 89 90 91 92 93 94 95 96 97 98 99 100 101 102 103 104 105
            106 107 108 109 110 111 112 113 114 115 116 117 118 119);
        let make = |two: usize| -> usize {
            let mut v = one;
            v[0] = query_interface as *const () as usize;
            v[1] = add_ref as *const () as usize;
            v[2] = release as *const () as usize;
            v[3] = type_info_count as *const () as usize;
            v[4] = type_info as *const () as usize;
            v[5] = ids_of_names as *const () as usize;
            v[6] = invoke as *const () as usize;
            v[two] = match two {
                87 => slot2::<87> as *const () as usize,
                _ => slot2::<79> as *const () as usize,
            };
            Box::leak(Box::new(v)).as_ptr() as usize
        };
        // the rows' slot 79 is a setter (one argument): never the table's two-argument one
        let row = {
            let mut v = one;
            v[0] = query_interface as *const () as usize;
            v[1] = add_ref as *const () as usize;
            v[2] = release as *const () as usize;
            v[3] = type_info_count as *const () as usize;
            v[4] = type_info as *const () as usize;
            v[5] = ids_of_names as *const () as usize;
            v[6] = invoke as *const () as usize;
            Box::leak(Box::new(v)).as_ptr() as usize
        };
        [make(87), make(79), row]
    });
    (made[0] as *const usize, made[1] as *const usize, made[2] as *const usize)
}

fn guid_is(a: *const GUID, data1: u32) -> bool {
    !a.is_null() && unsafe { (*a).data1 == data1 && (*a).data2 == 0 && (*a).data3 == 0 && (*a).data4 == [0xc0, 0, 0, 0, 0, 0, 0, 0x46] }
}

/// Any IID the game asks for: IUnknown, IDispatch, the class interfaces (.NET's name-based,
/// version 3 GUIDs).
unsafe extern "system" fn query_interface(this: *mut NetObj, iid: *const GUID, out: *mut P) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let ok = guid_is(iid, 0) || guid_is(iid, 0x0002_0400) || (!iid.is_null() && unsafe { (*iid).data3 } >> 12 == 3);
    if !ok {
        unsafe { *out = std::ptr::null_mut() };
        return E_NOINTERFACE;
    }
    unsafe {
        add_ref(this);
        *out = this as P;
    }
    S_OK
}

unsafe extern "system" fn add_ref(this: *mut NetObj) -> u32 {
    let refs = unsafe { &(*this).refs };
    refs.set(refs.get() + 1);
    refs.get()
}

/// Never freed: the engine releases the rows it reads from arrays and tables once more than it
/// references them (`.NET`'s COM wrappers outlive that); they live as long as the game's process.
unsafe extern "system" fn release(this: *mut NetObj) -> u32 {
    let refs = unsafe { &(*this).refs };
    let n = refs.get().saturating_sub(1);
    refs.set(n);
    n
}

unsafe extern "system" fn type_info_count(_this: P, count: *mut u32) -> HRESULT {
    if !count.is_null() {
        unsafe { *count = 0 };
    }
    S_OK
}

unsafe extern "system" fn type_info(_this: P, _index: u32, _lcid: u32, _out: *mut P) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn ids_of_names(_this: P, _iid: P, _names: P, _count: u32, _lcid: u32, _ids: P) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn invoke(_this: P, _id: i32, _iid: P, _lcid: u32, _flags: u16, _params: P, _result: P, _ex: P, _arg: P) -> HRESULT {
    E_NOTIMPL
}

fn bstr_text(b: *const u16) -> String {
    if b.is_null() {
        return String::new();
    }
    let len = unsafe { SysStringLen(b) } as usize;
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(b, len) })
}

/// The first calls of each slot, logged.
fn log_once(obj: &NetObj, slot: usize) {
    static SEEN: std::sync::Mutex<Vec<(usize, usize)>> = std::sync::Mutex::new(Vec::new());
    let key = match obj.kind {
        Kind::DataSet(c) => c.name.as_ptr() as usize,
        Kind::Table(t) => t.as_ptr() as usize,
        Kind::Row(r, _) => r.element.as_ptr() as usize,
    };
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if !seen.contains(&(key, slot)) {
        seen.push((key, slot));
        let name = match obj.kind {
            Kind::DataSet(c) => c.name,
            Kind::Table(t) => t,
            Kind::Row(r, _) => r.element,
        };
        log!("netds: {name} slot {slot}");
    }
}

unsafe fn call(this: *mut NetObj, slot: usize, args: &[usize]) -> HRESULT {
    let obj = unsafe { &*this };
    log_once(obj, slot);
    let out = |v: usize| args.last().map(|&p| p as *mut usize).filter(|p| !p.is_null()).map(|p| unsafe { *p = v });
    match obj.kind {
        Kind::DataSet(class) => match slot {
            // SetXml(xml): 0, rows added as ReadXml does
            87 => {
                let xml = bstr_text(args[0] as *const u16);
                obj.doc.borrow_mut().merge(Doc::parse(&xml));
                out(0);
                S_OK
            }
            s if (88..88 + class.tables.len()).contains(&s) => {
                out(new_obj(Kind::Table(class.tables[s - 88]), obj.doc.clone()) as usize);
                S_OK
            }
            _ => E_NOTIMPL,
        },
        Kind::Table(table) => match slot {
            78 => {
                out(obj.doc.borrow().rows(table).len());
                S_OK
            }
            79 => {
                let rows = obj.doc.borrow().rows(table);
                match (rows.get(args[0]), ROWS.iter().find(|r| r.element == table)) {
                    (Some(&row), Some(class)) => {
                        out(new_obj(Kind::Row(class, row), obj.doc.clone()) as usize);
                        S_OK
                    }
                    _ => E_INVALIDARG,
                }
            }
            _ => E_NOTIMPL,
        },
        Kind::Row(class, row) => {
            if let Some(&(_, child)) = class.arrays.iter().find(|(s, _)| *s == slot) {
                let rows = obj.doc.borrow().children(row, child);
                let Some(child_class) = ROWS.iter().find(|r| r.element == child) else { return E_NOTIMPL };
                let array = unsafe { SafeArrayCreateVector(VT_DISPATCH, 0, rows.len() as u32) };
                if array.is_null() {
                    return E_NOTIMPL;
                }
                for (i, &r) in rows.iter().enumerate() {
                    unsafe { *(*array).data.add(i) = new_obj(Kind::Row(child_class, r), obj.doc.clone()) as P };
                }
                out(array as usize);
                return S_OK;
            }
            let index = slot.wrapping_sub(60) / 2;
            let Some(&column) = class.columns.get(index).filter(|_| slot >= 60) else { return E_NOTIMPL };
            let name = match column {
                Int(n) | Bool(n) | Float(n) | Text(n) => n,
            };
            let field = obj.doc.borrow().field(row, name);
            if slot % 2 == 1 {
                // a setter: the value as text
                let text = match column {
                    Int(_) => (args[0] as i32).to_string(),
                    Bool(_) => (args[0] as u16 != 0).to_string(),
                    Float(_) => f32::from_bits(args[0] as u32).to_string(),
                    Text(_) => bstr_text(args[0] as *const u16),
                };
                if let Some(f) = field {
                    obj.doc.borrow_mut().elements[f].text = text;
                }
                return S_OK;
            }
            let text = field.map(|f| obj.doc.borrow().elements[f].text.trim().to_string()).unwrap_or_default();
            let Some(&p) = args.first() else { return E_INVALIDARG };
            if p == 0 {
                return E_INVALIDARG;
            }
            unsafe {
                match column {
                    Int(_) => *(p as *mut i32) = text.parse().unwrap_or(0),
                    Bool(_) => *(p as *mut i16) = if text == "1" || text.eq_ignore_ascii_case("true") { -1 } else { 0 },
                    Float(_) => *(p as *mut f32) = text.parse().unwrap_or(0.0),
                    Text(_) => {
                        let w: Vec<u16> = text.encode_utf16().collect();
                        *(p as *mut *mut u16) = SysAllocStringLen(w.as_ptr(), w.len() as u32);
                    }
                }
            }
            S_OK
        }
    }
}

static ORIG_CREATE: AtomicUsize = AtomicUsize::new(0);

/// `CoCreateInstance`: the data sets made by the payload, the other classes by COM.
unsafe extern "system" fn co_create_instance(clsid: *const GUID, outer: P, ctx: u32, iid: *const GUID, out: *mut P) -> HRESULT {
    let class = DATA_SETS.iter().find(|c| {
        !clsid.is_null() && unsafe { (*clsid).data1 == c.clsid.data1 && (*clsid).data2 == c.clsid.data2 && (*clsid).data4 == c.clsid.data4 }
    });
    if let Some(class) = class {
        log!("netds: new {}", class.name);
        let obj = new_obj(Kind::DataSet(class), Rc::new(RefCell::new(Doc::default())));
        let hr = unsafe { query_interface(obj, iid, out) };
        unsafe { release(obj) };
        return hr;
    }
    let f: unsafe extern "system" fn(*const GUID, P, u32, *const GUID, *mut P) -> HRESULT =
        unsafe { std::mem::transmute(ORIG_CREATE.load(Ordering::Relaxed)) };
    unsafe { f(clsid, outer, ctx, iid, out) }
}

/// The game's and its engine's `CoCreateInstance`.
pub(crate) fn init(exe: usize) {
    let engine = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(c"Engine.dll".as_ptr().cast()) } as usize;
    for module in [exe, engine] {
        if module == 0 {
            continue;
        }
        if let Some(o) = unsafe { iat::hook_module(module, "ole32.dll", "CoCreateInstance", co_create_instance as *const () as usize) }
            && o != co_create_instance as *const () as usize
        {
            ORIG_CREATE.store(o, Ordering::Relaxed);
        }
    }
    log!("netds: the game's .NET data sets answered");
}
