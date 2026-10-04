//! DirectShow compatibility shims for games built against Windows' DirectShow behaviour.
//!
//! * `WAL_DSHOW_FIND_FILTER=1`: `IFilterGraph::FindFilterByName` also matches the name with a
//!   Wine duplicate suffix. Wine stores autoplugged filters as e.g. "WMVideo Decoder DMO 0001"
//!   where Windows keeps "WMVideo Decoder DMO"; games looking the decoder up by name to splice
//!   their own filters (KOF XIII Climax: SampleGrabber on the decoder `out0`) fall back to other
//!   paths and crash.
//! * `WAL_DSHOW_ASF_READER=1`: `IGraphBuilder::RenderFile` builds Windows' ASF topology (WM ASF
//!   Reader named "Reader", pins `out0`/`out1`) instead of Wine's async reader + FFmpeg ASF
//!   splitter. Note: Wine's WM ASF Reader stalls on some files (WMA Lossless audio never
//!   delivered), so prefer the find-filter shim when it is enough.
//!
//! Both are installed by an IAT hook of `ole32!CoCreateInstance` in the game executable: graphs
//! (`CLSID_FilterGraph[NoThread]`) get the methods of their shared vtable wrapped.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use windows_sys::core::GUID;
use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};

use crate::{iat, log};

type HRESULT = i32;
type P = *mut c_void;

const S_OK: HRESULT = 0;
const VFW_S_PARTIAL_RENDER: HRESULT = 0x0004_0242;
const E_FAIL: HRESULT = 0x8000_4005_u32 as i32;
const CLSCTX_INPROC_SERVER: u32 = 1;
const PINDIR_OUTPUT: i32 = 1;

const CLSID_FILTER_GRAPH: GUID = GUID::from_u128(0xe436ebb3_524f_11ce_9f53_0020af0ba770);
const CLSID_FILTER_GRAPH_NO_THREAD: GUID = GUID::from_u128(0xe436ebb8_524f_11ce_9f53_0020af0ba770);
const CLSID_WM_ASF_READER: GUID = GUID::from_u128(0x187463a0_5bb7_11d3_acbe_0080c75e246e);
const IID_IGRAPH_BUILDER: GUID = GUID::from_u128(0x56a868a9_0ad4_11ce_b03a_0020af0ba770);
const IID_IBASE_FILTER: GUID = GUID::from_u128(0x56a86895_0ad4_11ce_b03a_0020af0ba770);
const IID_IFILE_SOURCE_FILTER: GUID = GUID::from_u128(0x56a868a6_0ad4_11ce_b03a_0020af0ba770);

// vtable slots
const QUERY_INTERFACE: usize = 0;
const RELEASE: usize = 2;
const GRAPH_ADD_FILTER: usize = 3;
const GRAPH_ENUM_FILTERS: usize = 5;
const GRAPH_FIND_FILTER_BY_NAME: usize = 6;
const GRAPH_CONNECT: usize = 11;
const PIN_QUERY_PIN_INFO: usize = 8;
const FILTER_QUERY_FILTER_INFO: usize = 12;
const GRAPH_REMOVE_FILTER: usize = 4;
const GRAPH_RENDER: usize = 12;
const GRAPH_RENDER_FILE: usize = 13;
const FILTER_ENUM_PINS: usize = 10;
const ENUM_NEXT: usize = 3;
const PIN_QUERY_DIRECTION: usize = 9;
const FILE_SOURCE_LOAD: usize = 3;

static ORIG_CO_CREATE: AtomicUsize = AtomicUsize::new(0);
static ORIG_RENDER_FILE: AtomicUsize = AtomicUsize::new(0);
static ORIG_FIND_FILTER: AtomicUsize = AtomicUsize::new(0);
static ASF_READER: AtomicBool = AtomicBool::new(false);
static FIND_FILTER: AtomicBool = AtomicBool::new(false);
static TRACE: AtomicBool = AtomicBool::new(false);
static ORIG_CONNECT: AtomicUsize = AtomicUsize::new(0);

type CoCreateFn = unsafe extern "system" fn(*const GUID, P, u32, *const GUID, *mut P) -> HRESULT;
type RenderFileFn = unsafe extern "system" fn(P, *const u16, *const u16) -> HRESULT;

/// Address of method `slot` of COM object `obj`.
unsafe fn method(obj: P, slot: usize) -> usize {
    unsafe { *(*(obj as *const *const usize)).add(slot) }
}

unsafe fn release(obj: P) {
    if !obj.is_null() {
        let f: unsafe extern "system" fn(P) -> u32 = unsafe { std::mem::transmute(method(obj, RELEASE)) };
        unsafe { f(obj) };
    }
}

unsafe fn query(obj: P, iid: &GUID) -> Option<P> {
    let f: unsafe extern "system" fn(P, *const GUID, *mut P) -> HRESULT =
        unsafe { std::mem::transmute(method(obj, QUERY_INTERFACE)) };
    let mut out: P = std::ptr::null_mut();
    (unsafe { f(obj, iid, &mut out) } >= 0 && !out.is_null()).then_some(out)
}

fn guid_eq(a: *const GUID, b: &GUID) -> bool {
    if a.is_null() {
        return false;
    }
    let a = unsafe { &*a };
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

pub fn init() {
    let on = |var: &str| std::env::var(var).is_ok_and(|v| v == "1");
    ASF_READER.store(on("WAL_DSHOW_ASF_READER"), Ordering::Relaxed);
    FIND_FILTER.store(on("WAL_DSHOW_FIND_FILTER"), Ordering::Relaxed);
    TRACE.store(on("WAL_DSHOW_TRACE"), Ordering::Relaxed);
    if !ASF_READER.load(Ordering::Relaxed) && !FIND_FILTER.load(Ordering::Relaxed) && !TRACE.load(Ordering::Relaxed) {
        return;
    }
    if let Some(o) = unsafe { iat::hook("ole32.dll", "CoCreateInstance", co_create_instance as *const () as usize) } {
        ORIG_CO_CREATE.store(o, Ordering::Relaxed);
        log!(
            "dshow: shims enabled (asf reader: {}, find filter: {})",
            ASF_READER.load(Ordering::Relaxed),
            FIND_FILTER.load(Ordering::Relaxed)
        );
    }
}

unsafe extern "system" fn co_create_instance(clsid: *const GUID, outer: P, ctx: u32, iid: *const GUID, out: *mut P) -> HRESULT {
    let orig: CoCreateFn = unsafe { std::mem::transmute(ORIG_CO_CREATE.load(Ordering::Relaxed)) };
    let hr = unsafe { orig(clsid, outer, ctx, iid, out) };
    if hr >= 0
        && !out.is_null()
        && (guid_eq(clsid, &CLSID_FILTER_GRAPH) || guid_eq(clsid, &CLSID_FILTER_GRAPH_NO_THREAD))
    {
        unsafe { patch_graph(*out) };
    }
    hr
}

/// Wraps methods in the (shared) vtable of the graph's IGraphBuilder interface, once.
unsafe fn patch_graph(graph: P) {
    if ORIG_RENDER_FILE.load(Ordering::Relaxed) != 0
        || ORIG_FIND_FILTER.load(Ordering::Relaxed) != 0
        || ORIG_CONNECT.load(Ordering::Relaxed) != 0
    {
        return;
    }
    let Some(builder) = (unsafe { query(graph, &IID_IGRAPH_BUILDER) }) else { return };
    let vtable = unsafe { *(builder as *const *mut usize) };
    if ASF_READER.load(Ordering::Relaxed) {
        unsafe { patch_slot(vtable, GRAPH_RENDER_FILE, render_file as *const () as usize, &ORIG_RENDER_FILE) };
        log!("dshow: IGraphBuilder::RenderFile wrapped");
    }
    if FIND_FILTER.load(Ordering::Relaxed) || TRACE.load(Ordering::Relaxed) {
        unsafe { patch_slot(vtable, GRAPH_FIND_FILTER_BY_NAME, find_filter_by_name as *const () as usize, &ORIG_FIND_FILTER) };
        log!("dshow: IFilterGraph::FindFilterByName wrapped");
    }
    if TRACE.load(Ordering::Relaxed) {
        unsafe { patch_slot(vtable, GRAPH_CONNECT, connect as *const () as usize, &ORIG_CONNECT) };
    }
    unsafe { release(builder) };
}

unsafe fn patch_slot(vtable: *mut usize, index: usize, replacement: usize, orig: &AtomicUsize) {
    let slot = unsafe { vtable.add(index) };
    let mut old = 0;
    unsafe {
        VirtualProtect(slot.cast(), size_of::<usize>(), PAGE_READWRITE, &mut old);
        orig.store(*slot, Ordering::Relaxed);
        *slot = replacement;
        VirtualProtect(slot.cast(), size_of::<usize>(), old, &mut old);
    }
}

/// `name` followed by Wine's duplicate suffix (" 0001").
fn is_suffixed(candidate: &[u16], name: &[u16]) -> bool {
    candidate.len() > name.len() + 1
        && candidate.starts_with(name)
        && candidate[name.len()] == b' ' as u16
        && candidate[name.len() + 1..].iter().all(|c| (b'0' as u16..=b'9' as u16).contains(c))
}

unsafe extern "system" fn find_filter_by_name(graph: P, name: *const u16, out: *mut P) -> HRESULT {
    let orig: unsafe extern "system" fn(P, *const u16, *mut P) -> HRESULT =
        unsafe { std::mem::transmute(ORIG_FIND_FILTER.load(Ordering::Relaxed)) };
    if FIND_FILTER.load(Ordering::Relaxed) && !name.is_null() && !out.is_null() {
        let found = unsafe { find_by_filter_info(graph, name) };
        if !found.is_null() {
            if TRACE.load(Ordering::Relaxed) {
                log!("dshow: FindFilterByName({}) -> {}", wide_to_string(name), unsafe { filter_name(found) });
            }
            unsafe { *out = found };
            return S_OK;
        }
    }
    let hr = unsafe { orig(graph, name, out) };
    if TRACE.load(Ordering::Relaxed) {
        let found = if hr >= 0 && !out.is_null() { unsafe { filter_name(*out) } } else { String::new() };
        log!("dshow: FindFilterByName({}) -> {hr:#x} {found} (wine)", wide_to_string(name));
    }
    hr
}

/// Looks a filter up by the name it reports (`QueryFilterInfo`): exact, or with Wine's
/// duplicate suffix. Returns an AddRef'd filter or null.
unsafe fn find_by_filter_info(graph: P, name: *const u16) -> P {
    let wanted: Vec<u16> = wide_to_string(name).encode_utf16().collect();
    unsafe {
        let enum_filters: unsafe extern "system" fn(P, *mut P) -> HRESULT =
            std::mem::transmute(method(graph, GRAPH_ENUM_FILTERS));
        let mut filters: P = std::ptr::null_mut();
        if enum_filters(graph, &mut filters) < 0 || filters.is_null() {
            return std::ptr::null_mut();
        }
        let next: unsafe extern "system" fn(P, u32, *mut P, *mut u32) -> HRESULT = std::mem::transmute(method(filters, ENUM_NEXT));
        let mut found: P = std::ptr::null_mut();
        let mut suffixed: P = std::ptr::null_mut();
        loop {
            let mut filter: P = std::ptr::null_mut();
            let mut fetched = 0u32;
            if next(filters, 1, &mut filter, &mut fetched) != S_OK || fetched == 0 {
                break;
            }
            // FILTER_INFO { WCHAR achName[128]; IFilterGraph *pGraph; }
            let mut info = FilterInfo { name: [0; 128], graph: std::ptr::null_mut() };
            let query_info: unsafe extern "system" fn(P, *mut FilterInfo) -> HRESULT =
                std::mem::transmute(method(filter, FILTER_QUERY_FILTER_INFO));
            if query_info(filter, &mut info) >= 0 {
                release(info.graph);
                let len = info.name.iter().position(|c| *c == 0).unwrap_or(128);
                if info.name[..len] == wanted[..] {
                    found = filter; // keeps the reference for the caller
                    break;
                }
                if suffixed.is_null() && is_suffixed(&info.name[..len], &wanted) {
                    suffixed = filter;
                    continue;
                }
            }
            release(filter);
        }
        release(filters);
        if found.is_null() {
            found = suffixed;
        } else {
            release(suffixed);
        }
        found
    }
}

#[repr(C)]
struct FilterInfo {
    name: [u16; 128],
    graph: P,
}

unsafe fn filter_name(filter: P) -> String {
    if filter.is_null() {
        return "(null)".into();
    }
    let mut info = FilterInfo { name: [0; 128], graph: std::ptr::null_mut() };
    let query_info: unsafe extern "system" fn(P, *mut FilterInfo) -> HRESULT =
        unsafe { std::mem::transmute(method(filter, FILTER_QUERY_FILTER_INFO)) };
    if unsafe { query_info(filter, &mut info) } < 0 {
        return "?".into();
    }
    unsafe { release(info.graph) };
    let len = info.name.iter().position(|c| *c == 0).unwrap_or(128);
    String::from_utf16_lossy(&info.name[..len])
}

/// "filter:pin" of a pin, for traces.
unsafe fn pin_name(pin: P) -> String {
    if pin.is_null() {
        return "(null)".into();
    }
    // PIN_INFO { IBaseFilter *pFilter; PIN_DIRECTION dir; WCHAR achName[128]; }
    #[repr(C)]
    struct PinInfo {
        filter: P,
        dir: i32,
        name: [u16; 128],
    }
    let mut info = PinInfo { filter: std::ptr::null_mut(), dir: 0, name: [0; 128] };
    let query: unsafe extern "system" fn(P, *mut PinInfo) -> HRESULT = unsafe { std::mem::transmute(method(pin, PIN_QUERY_PIN_INFO)) };
    if unsafe { query(pin, &mut info) } < 0 {
        return "?".into();
    }
    let filter = unsafe { filter_name(info.filter) };
    unsafe { release(info.filter) };
    let len = info.name.iter().position(|c| *c == 0).unwrap_or(128);
    format!("{filter}:{}", String::from_utf16_lossy(&info.name[..len]))
}

unsafe extern "system" fn connect(graph: P, out: P, input: P) -> HRESULT {
    let orig: unsafe extern "system" fn(P, P, P) -> HRESULT = unsafe { std::mem::transmute(ORIG_CONNECT.load(Ordering::Relaxed)) };
    let hr = unsafe { orig(graph, out, input) };
    log!("dshow: Connect({} -> {}) -> {hr:#x}", unsafe { pin_name(out) }, unsafe { pin_name(input) });
    hr
}

fn wide_to_string(s: *const u16) -> String {
    if s.is_null() {
        return String::new();
    }
    let mut len = 0;
    while unsafe { *s.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(s, len) })
}

unsafe extern "system" fn render_file(graph: P, file: *const u16, playlist: *const u16) -> HRESULT {
    let orig: RenderFileFn = unsafe { std::mem::transmute(ORIG_RENDER_FILE.load(Ordering::Relaxed)) };
    let name = wide_to_string(file);
    let lower = name.to_ascii_lowercase();
    if [".wmv", ".asf", ".wma"].iter().any(|e| lower.ends_with(e)) {
        let hr = unsafe { render_asf(graph, file) };
        log!("dshow: RenderFile({name}) through the WM ASF Reader -> {hr:#x}");
        if hr >= 0 {
            return hr;
        }
    }
    unsafe { orig(graph, file, playlist) }
}

/// Windows topology: WM ASF Reader named "Reader", every output pin rendered.
unsafe fn render_asf(graph: P, file: *const u16) -> HRESULT {
    let co_create: CoCreateFn = unsafe { std::mem::transmute(ORIG_CO_CREATE.load(Ordering::Relaxed)) };
    let mut reader: P = std::ptr::null_mut();
    let hr = unsafe { co_create(&CLSID_WM_ASF_READER, std::ptr::null_mut(), CLSCTX_INPROC_SERVER, &IID_IBASE_FILTER, &mut reader) };
    if hr < 0 || reader.is_null() {
        return if hr < 0 { hr } else { E_FAIL };
    }
    unsafe {
        let add_filter: unsafe extern "system" fn(P, P, *const u16) -> HRESULT =
            std::mem::transmute(method(graph, GRAPH_ADD_FILTER));
        let remove_filter: unsafe extern "system" fn(P, P) -> HRESULT = std::mem::transmute(method(graph, GRAPH_REMOVE_FILTER));
        let render: unsafe extern "system" fn(P, P) -> HRESULT = std::mem::transmute(method(graph, GRAPH_RENDER));

        let reader_name: Vec<u16> = "Reader\0".encode_utf16().collect();
        let hr = add_filter(graph, reader, reader_name.as_ptr());
        if hr < 0 {
            release(reader);
            return hr;
        }
        // Load after joining the graph, as Windows' RenderFile does
        let loaded = match query(reader, &IID_IFILE_SOURCE_FILTER) {
            Some(src) => {
                let load: unsafe extern "system" fn(P, *const u16, P) -> HRESULT =
                    std::mem::transmute(method(src, FILE_SOURCE_LOAD));
                let hr = load(src, file, std::ptr::null_mut());
                release(src);
                hr
            }
            None => E_FAIL,
        };
        if loaded < 0 {
            remove_filter(graph, reader);
            release(reader);
            return loaded;
        }

        let enum_pins: unsafe extern "system" fn(P, *mut P) -> HRESULT = std::mem::transmute(method(reader, FILTER_ENUM_PINS));
        let mut pins: P = std::ptr::null_mut();
        let (mut rendered, mut failed) = (0, 0);
        if enum_pins(reader, &mut pins) >= 0 && !pins.is_null() {
            let next: unsafe extern "system" fn(P, u32, *mut P, *mut u32) -> HRESULT = std::mem::transmute(method(pins, ENUM_NEXT));
            loop {
                let mut pin: P = std::ptr::null_mut();
                let mut fetched = 0u32;
                if next(pins, 1, &mut pin, &mut fetched) != S_OK || fetched == 0 {
                    break;
                }
                let dir_fn: unsafe extern "system" fn(P, *mut i32) -> HRESULT =
                    std::mem::transmute(method(pin, PIN_QUERY_DIRECTION));
                let mut dir = 0;
                if dir_fn(pin, &mut dir) >= 0 && dir == PINDIR_OUTPUT {
                    if render(graph, pin) >= 0 {
                        rendered += 1;
                    } else {
                        failed += 1;
                    }
                }
                release(pin);
            }
            release(pins);
        }
        log!("dshow: {rendered} stream(s) rendered, {failed} failed");
        let hr = match (rendered, failed) {
            (0, _) => {
                remove_filter(graph, reader);
                E_FAIL
            }
            (_, 0) => S_OK,
            _ => VFW_S_PARTIAL_RENDER,
        };
        release(reader);
        hr
    }
}
