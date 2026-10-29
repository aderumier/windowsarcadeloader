//! The ActiveMovie control (`amovie.ocx`, CLSID `{05589FA1-C356-11CE-BF01-00AA0055595A}`) of
//! Beach Head 2000's frontend: its HTML pages (UniverShell.exe's web browser) play their attract
//! movies with it. Wine has none: the payload registers it for the frontend ([`register`]).
//!
//! What the pages use: the `<object>` parameters (`FileName`, relative to the page, `AutoStart`,
//! `PlayCount`, `Volume`), the methods `Run`, `Pause`, `Stop` and the properties `CurrentState`
//! (0 stopped, 1 paused, 2 running: a page leaves for the next one when its movie stopped),
//! `CurrentPosition`, `Duration`, `Volume`, `PlayCount`, `FileName`.
//!
//! The control is a window inside the page's plugin window (in-place activation), the movie a
//! DirectShow graph whose video window is a child of it.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use wal_payload_common::log;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{BLACK_BRUSH, GetStockObject};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, MoveWindow, RegisterClassW, WNDCLASSW, WS_CHILD, WS_CLIPCHILDREN,
    WS_CLIPSIBLINGS, WS_VISIBLE,
};
use windows_sys::core::GUID;

type P = *mut c_void;
type HRESULT = i32;

const S_OK: HRESULT = 0;
const E_NOTIMPL: HRESULT = 0x8000_4001u32 as i32;
const E_NOINTERFACE: HRESULT = 0x8000_4002u32 as i32;
const E_POINTER: HRESULT = 0x8000_4003u32 as i32;
const E_FAIL: HRESULT = 0x8000_4005u32 as i32;
const CLASS_E_NOAGGREGATION: HRESULT = 0x8004_0110u32 as i32;
const DISP_E_MEMBERNOTFOUND: HRESULT = 0x8002_0003u32 as i32;
const DISP_E_UNKNOWNNAME: HRESULT = 0x8002_0006u32 as i32;
const DISP_E_BADPARAMCOUNT: HRESULT = 0x8002_000Eu32 as i32;

const CLSID_ACTIVE_MOVIE: GUID = GUID::from_u128(0x05589fa1_c356_11ce_bf01_00aa0055595a);
const IID_UNKNOWN: GUID = GUID::from_u128(0x00000000_0000_0000_c000_000000000046);
const IID_CLASS_FACTORY: GUID = GUID::from_u128(0x00000001_0000_0000_c000_000000000046);
const IID_DISPATCH: GUID = GUID::from_u128(0x00020400_0000_0000_c000_000000000046);
const IID_OLE_OBJECT: GUID = GUID::from_u128(0x00000112_0000_0000_c000_000000000046);
const IID_OLE_WINDOW: GUID = GUID::from_u128(0x00000114_0000_0000_c000_000000000046);
const IID_OLE_IN_PLACE_OBJECT: GUID = GUID::from_u128(0x00000113_0000_0000_c000_000000000046);
const IID_PERSIST: GUID = GUID::from_u128(0x0000010c_0000_0000_c000_000000000046);
const IID_PERSIST_PROPERTY_BAG: GUID = GUID::from_u128(0x37d84f60_42cb_11ce_8135_00aa004bb851);
const IID_OBJECT_SAFETY: GUID = GUID::from_u128(0xcb5bdc81_93c1_11cf_8f20_00805f2cd064);
const IID_HTML_DOCUMENT2: GUID = GUID::from_u128(0x332c4425_26cb_11d0_b483_00c04fd90119);
const IID_OLE_IN_PLACE_SITE_EX: GUID = GUID::from_u128(0x9c2cad80_3424_11cf_b670_00aa004cd6d8);
const CLSID_FILTER_GRAPH: GUID = GUID::from_u128(0xe436ebb3_524f_11ce_9f53_0020af0ba770);
const IID_GRAPH_BUILDER: GUID = GUID::from_u128(0x56a868a9_0ad4_11ce_b03a_0020af0ba770);
const IID_MEDIA_CONTROL: GUID = GUID::from_u128(0x56a868b1_0ad4_11ce_b03a_0020af0ba770);
const IID_MEDIA_POSITION: GUID = GUID::from_u128(0x56a868b2_0ad4_11ce_b03a_0020af0ba770);
const IID_BASIC_AUDIO: GUID = GUID::from_u128(0x56a868b3_0ad4_11ce_b03a_0020af0ba770);
const IID_VIDEO_WINDOW: GUID = GUID::from_u128(0x56a868b4_0ad4_11ce_b03a_0020af0ba770);
const IID_MEDIA_EVENT: GUID = GUID::from_u128(0x56a868b6_0ad4_11ce_b03a_0020af0ba770);

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoCreateInstance(clsid: *const GUID, outer: P, ctx: u32, iid: *const GUID, out: *mut P) -> HRESULT;
}

#[link(name = "oleaut32")]
unsafe extern "system" {
    fn SysAllocStringLen(s: *const u16, len: u32) -> *mut u16;
    fn SysFreeString(s: *mut u16);
    fn VariantChangeType(dst: *mut Variant, src: *const Variant, flags: u16, vt: u16) -> HRESULT;
    fn VariantClear(v: *mut Variant) -> HRESULT;
}

const VT_EMPTY: u16 = 0;
const VT_I4: u16 = 3;
const VT_R8: u16 = 5;
const VT_BSTR: u16 = 8;
const VT_BOOL: u16 = 11;

/// `VARIANT` (16 bytes on 32-bit): its type, the value at +8.
#[repr(C)]
#[derive(Clone, Copy)]
struct Variant {
    vt: u16,
    reserved: [u16; 3],
    value: [u32; 2],
}

impl Variant {
    const EMPTY: Variant = Variant { vt: VT_EMPTY, reserved: [0; 3], value: [0; 2] };

    fn i4(v: i32) -> Variant {
        Variant { vt: VT_I4, reserved: [0; 3], value: [v as u32, 0] }
    }

    fn r8(v: f64) -> Variant {
        let bits = v.to_bits();
        Variant { vt: VT_R8, reserved: [0; 3], value: [bits as u32, (bits >> 32) as u32] }
    }

    fn boolean(v: bool) -> Variant {
        Variant { vt: VT_BOOL, reserved: [0; 3], value: [if v { 0xffff } else { 0 }, 0] }
    }

    fn bstr(s: &str) -> Variant {
        let w: Vec<u16> = s.encode_utf16().collect();
        let b = unsafe { SysAllocStringLen(w.as_ptr(), w.len() as u32) };
        Variant { vt: VT_BSTR, reserved: [0; 3], value: [b as usize as u32, 0] }
    }

    /// The value converted to `vt` (`VariantChangeType`).
    fn convert(&self, vt: u16) -> Option<Variant> {
        let mut out = Variant::EMPTY;
        (unsafe { VariantChangeType(&mut out, self, 0, vt) } >= 0).then_some(out)
    }

    fn as_f64(&self) -> Option<f64> {
        self.convert(VT_R8).map(|v| f64::from_bits(v.value[0] as u64 | (v.value[1] as u64) << 32))
    }

    fn as_i32(&self) -> Option<i32> {
        self.as_f64().map(|f| f as i32)
    }

    fn as_string(&self) -> Option<String> {
        let mut v = self.convert(VT_BSTR)?;
        let s = wide_str(v.value[0] as usize as *const u16);
        unsafe { VariantClear(&mut v) };
        Some(s)
    }
}

fn wide_str(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn guid_eq(a: *const GUID, b: &GUID) -> bool {
    if a.is_null() {
        return false;
    }
    let a = unsafe { &*a };
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

/// Address of method `slot` of COM object `obj`.
unsafe fn method(obj: P, slot: usize) -> usize {
    unsafe { *(*(obj as *const *const usize)).add(slot) }
}

unsafe fn com_query(obj: P, iid: &GUID) -> Option<P> {
    let f: unsafe extern "system" fn(P, *const GUID, *mut P) -> HRESULT = unsafe { std::mem::transmute(method(obj, 0)) };
    let mut out: P = std::ptr::null_mut();
    (unsafe { f(obj, iid, &mut out) } >= 0 && !out.is_null()).then_some(out)
}

unsafe fn com_add_ref(obj: P) {
    let f: unsafe extern "system" fn(P) -> u32 = unsafe { std::mem::transmute(method(obj, 1)) };
    unsafe { f(obj) };
}

unsafe fn com_release(obj: P) {
    if !obj.is_null() {
        let f: unsafe extern "system" fn(P) -> u32 = unsafe { std::mem::transmute(method(obj, 2)) };
        unsafe { f(obj) };
    }
}

/// Calls method `slot` of `obj` taking no argument but `this`.
unsafe fn call0(obj: P, slot: usize) -> HRESULT {
    let f: unsafe extern "system" fn(P) -> HRESULT = unsafe { std::mem::transmute(method(obj, slot)) };
    unsafe { f(obj) }
}

unsafe fn call1<A>(obj: P, slot: usize, a: A) -> HRESULT {
    let f: unsafe extern "system" fn(P, A) -> HRESULT = unsafe { std::mem::transmute(method(obj, slot)) };
    unsafe { f(obj, a) }
}

// DirectShow methods (after IDispatch's 7 for the automation interfaces)
const GRAPH_RENDER_FILE: usize = 13;
const CONTROL_RUN: usize = 7;
const CONTROL_PAUSE: usize = 8;
const CONTROL_STOP: usize = 9;
const POSITION_GET_DURATION: usize = 7;
const POSITION_PUT_CURRENT: usize = 8;
const POSITION_GET_CURRENT: usize = 9;
const AUDIO_PUT_VOLUME: usize = 7;
const VIDEO_PUT_WINDOW_STYLE: usize = 9;
const VIDEO_PUT_VISIBLE: usize = 19;
const VIDEO_PUT_OWNER: usize = 29;
const VIDEO_PUT_MESSAGE_DRAIN: usize = 31;
const VIDEO_SET_WINDOW_POSITION: usize = 39;
const EVENT_GET_EVENT: usize = 8;
const EVENT_FREE_PARAMS: usize = 12;
const EC_COMPLETE: i32 = 1;

/// The movie's DirectShow graph and its interfaces.
struct Graph {
    builder: P,
    control: P,
    position: P,
    audio: P,
    video: P,
    event: P,
}

impl Graph {
    fn open(path: &str) -> Option<Graph> {
        let mut builder: P = std::ptr::null_mut();
        let hr = unsafe { CoCreateInstance(&CLSID_FILTER_GRAPH, std::ptr::null_mut(), 1, &IID_GRAPH_BUILDER, &mut builder) };
        if hr < 0 || builder.is_null() {
            log!("amovie: no filter graph: {hr:#x}");
            return None;
        }
        let w = wide(path);
        let f: unsafe extern "system" fn(P, *const u16, *const u16) -> HRESULT =
            unsafe { std::mem::transmute(method(builder, GRAPH_RENDER_FILE)) };
        let hr = unsafe { f(builder, w.as_ptr(), std::ptr::null()) };
        if hr < 0 {
            log!("amovie: cannot render {path}: {hr:#x}");
            unsafe { com_release(builder) };
            return None;
        }
        let q = |iid| unsafe { com_query(builder, iid) }.unwrap_or(std::ptr::null_mut());
        let graph = Graph {
            builder,
            control: q(&IID_MEDIA_CONTROL),
            position: q(&IID_MEDIA_POSITION),
            audio: q(&IID_BASIC_AUDIO),
            video: q(&IID_VIDEO_WINDOW),
            event: q(&IID_MEDIA_EVENT),
        };
        log!("amovie: {path} rendered ({hr:#x})");
        Some(graph)
    }

    fn duration(&self) -> f64 {
        let mut d = 0f64;
        if !self.position.is_null() {
            unsafe { call1(self.position, POSITION_GET_DURATION, &mut d as *mut f64) };
        }
        d
    }

    fn position(&self) -> f64 {
        let mut d = 0f64;
        if !self.position.is_null() {
            unsafe { call1(self.position, POSITION_GET_CURRENT, &mut d as *mut f64) };
        }
        d
    }

    fn seek(&self, t: f64) {
        if !self.position.is_null() {
            unsafe { call1(self.position, POSITION_PUT_CURRENT, t) };
        }
    }

    fn control(&self, slot: usize) {
        if !self.control.is_null() {
            unsafe { call0(self.control, slot) };
        }
    }

    fn volume(&self, v: i32) {
        if !self.audio.is_null() {
            unsafe { call1(self.audio, AUDIO_PUT_VOLUME, v.clamp(-10000, 0)) };
        }
    }

    /// The video in `owner`'s client area `w` x `h`.
    fn place(&self, owner: HWND, w: i32, h: i32) {
        if self.video.is_null() || owner.is_null() {
            return;
        }
        unsafe {
            call1(self.video, VIDEO_PUT_OWNER, owner as usize);
            call1(self.video, VIDEO_PUT_WINDOW_STYLE, (WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN) as i32);
            call1(self.video, VIDEO_PUT_MESSAGE_DRAIN, owner as usize);
            let f: unsafe extern "system" fn(P, i32, i32, i32, i32) -> HRESULT =
                std::mem::transmute(method(self.video, VIDEO_SET_WINDOW_POSITION));
            f(self.video, 0, 0, w, h);
            call1(self.video, VIDEO_PUT_VISIBLE, -1i32);
        }
    }

    /// Whether the graph reported the end of the movie since the last call.
    fn completed(&self) -> bool {
        if self.event.is_null() {
            return false;
        }
        let mut done = false;
        loop {
            let (mut code, mut p1, mut p2) = (0i32, 0isize, 0isize);
            let f: unsafe extern "system" fn(P, *mut i32, *mut isize, *mut isize, i32) -> HRESULT =
                unsafe { std::mem::transmute(method(self.event, EVENT_GET_EVENT)) };
            if unsafe { f(self.event, &mut code, &mut p1, &mut p2, 0) } != S_OK {
                break;
            }
            let free: unsafe extern "system" fn(P, i32, isize, isize) -> HRESULT =
                unsafe { std::mem::transmute(method(self.event, EVENT_FREE_PARAMS)) };
            unsafe { free(self.event, code, p1, p2) };
            done |= code == EC_COMPLETE;
        }
        done
    }
}

impl Drop for Graph {
    fn drop(&mut self) {
        unsafe {
            if !self.control.is_null() {
                call0(self.control, CONTROL_STOP);
            }
            if !self.video.is_null() {
                call1(self.video, VIDEO_PUT_VISIBLE, 0i32);
                call1(self.video, VIDEO_PUT_OWNER, 0usize);
            }
            for p in [self.event, self.video, self.audio, self.position, self.control, self.builder] {
                com_release(p);
            }
        }
    }
}

const STOPPED: i32 = 0;
const PAUSED: i32 = 1;
const RUNNING: i32 = 2;

/// The control: one pointer per interface (their `this`), then its state. Used on the page's
/// thread only.
#[repr(C)]
struct Movie {
    dispatch: &'static Vtable<7>,
    ole: &'static Vtable<24>,
    in_place: &'static Vtable<9>,
    persist: &'static Vtable<7>,
    safety: &'static Vtable<5>,
    refs: Cell<u32>,
    site: Cell<P>,
    hwnd: Cell<HWND>,
    size: Cell<(i32, i32)>,
    file: RefCell<String>,
    /// The page's folder (its moniker's display name), for a relative `FileName`.
    base: RefCell<String>,
    auto_start: Cell<bool>,
    volume: Cell<i32>,
    play_count: Cell<i32>,
    /// Plays done of `play_count`.
    plays: Cell<i32>,
    state: Cell<i32>,
    graph: RefCell<Option<Graph>>,
}

#[repr(C)]
struct Vtable<const N: usize>([*const (); N]);
unsafe impl<const N: usize> Sync for Vtable<N> {}

/// Offset of the interface pointer `field` in the control.
const fn offset(field: usize) -> usize {
    field * size_of::<usize>()
}

/// The control from one of its interface pointers (`field`: the pointer's index).
unsafe fn movie<'a>(this: P, field: usize) -> &'a Movie {
    unsafe { &*((this as usize - offset(field)) as *const Movie) }
}

impl Movie {
    fn interface(&self, field: usize) -> P {
        (self as *const Movie as usize + offset(field)) as P
    }

    fn query(&self, iid: *const GUID, out: *mut P) -> HRESULT {
        if out.is_null() {
            return E_POINTER;
        }
        let field = if guid_eq(iid, &IID_UNKNOWN) || guid_eq(iid, &IID_DISPATCH) {
            Some(0)
        } else if guid_eq(iid, &IID_OLE_OBJECT) {
            Some(1)
        } else if guid_eq(iid, &IID_OLE_IN_PLACE_OBJECT) || guid_eq(iid, &IID_OLE_WINDOW) {
            Some(2)
        } else if guid_eq(iid, &IID_PERSIST_PROPERTY_BAG) || guid_eq(iid, &IID_PERSIST) {
            Some(3)
        } else if guid_eq(iid, &IID_OBJECT_SAFETY) {
            Some(4)
        } else {
            None
        };
        match field {
            Some(f) => {
                self.refs.set(self.refs.get() + 1);
                unsafe { *out = self.interface(f) };
                S_OK
            }
            None => {
                unsafe { *out = std::ptr::null_mut() };
                E_NOINTERFACE
            }
        }
    }

    fn release(&self) -> u32 {
        let n = self.refs.get().saturating_sub(1);
        self.refs.set(n);
        if n == 0 {
            self.close();
            let site = self.site.replace(std::ptr::null_mut());
            unsafe { com_release(site) };
            log!("amovie: released");
            drop(unsafe { Box::from_raw(self as *const Movie as *mut Movie) });
        }
        n
    }

    /// `FileName` as a path: relative to the page's folder.
    fn path(&self) -> String {
        let file = self.file.borrow().replace('/', "\\");
        if file.contains(':') || file.starts_with('\\') {
            return file;
        }
        format!("{}\\{file}", self.base.borrow())
    }

    /// The page's folder from its document's URL (`file:///C:/dir/page.html`): the site's
    /// container, the document (its moniker still names the previous page while the next loads).
    fn find_base(&self) {
        let site = self.site.get();
        if site.is_null() || !self.base.borrow().is_empty() {
            return;
        }
        let mut container: P = std::ptr::null_mut();
        // IOleClientSite::GetContainer
        if unsafe { call1(site, 5, &mut container as *mut P) } < 0 || container.is_null() {
            return;
        }
        let doc = unsafe { com_query(container, &IID_HTML_DOCUMENT2) };
        unsafe { com_release(container) };
        let Some(doc) = doc else {
            return;
        };
        let mut url: *mut u16 = std::ptr::null_mut();
        // IHTMLDocument2::get_URL
        unsafe { call1(doc, 40, &mut url as *mut *mut u16) };
        unsafe { com_release(doc) };
        let page = wide_str(url);
        unsafe { SysFreeString(url) };
        let path = page.strip_prefix("file:///").or_else(|| page.strip_prefix("file://")).unwrap_or(&page);
        let path = unescape(path).replace('/', "\\");
        let dir = path.rsplit_once('\\').map(|(d, _)| d.to_string()).unwrap_or_default();
        log!("amovie: page {page}");
        *self.base.borrow_mut() = dir;
    }

    /// The graph of `FileName`, opened once.
    fn open(&self) -> bool {
        if self.graph.borrow().is_some() {
            return true;
        }
        if self.file.borrow().is_empty() {
            return false;
        }
        self.find_base();
        let Some(graph) = Graph::open(&self.path()) else {
            return false;
        };
        graph.volume(self.volume.get());
        let (w, h) = self.size.get();
        graph.place(self.hwnd.get(), w, h);
        *self.graph.borrow_mut() = Some(graph);
        true
    }

    fn run(&self) {
        if !self.open() {
            return;
        }
        if let Some(g) = self.graph.borrow().as_ref() {
            if self.state.get() == STOPPED {
                g.completed();
                g.seek(0.0);
            }
            g.control(CONTROL_RUN);
        }
        self.state.set(RUNNING);
        log!("amovie: run");
    }

    fn stop(&self) {
        if let Some(g) = self.graph.borrow().as_ref() {
            g.control(CONTROL_STOP);
            g.seek(0.0);
        }
        self.state.set(STOPPED);
        self.plays.set(0);
    }

    fn pause(&self) {
        if let Some(g) = self.graph.borrow().as_ref() {
            g.control(CONTROL_PAUSE);
        }
        self.state.set(PAUSED);
    }

    /// The state, the end of the movie seen: played again until `PlayCount` (<= 0: always).
    fn current_state(&self) -> i32 {
        if self.state.get() == RUNNING
            && let Some(g) = self.graph.borrow().as_ref()
            && g.completed()
        {
            let plays = self.plays.get() + 1;
            self.plays.set(plays);
            let count = self.play_count.get();
            if count <= 0 || plays < count {
                g.seek(0.0);
                g.control(CONTROL_RUN);
            } else {
                g.control(CONTROL_STOP);
                self.state.set(STOPPED);
                self.plays.set(0);
                log!("amovie: movie ended");
            }
        }
        self.state.get()
    }

    fn close(&self) {
        drop(self.graph.borrow_mut().take());
        let hwnd = self.hwnd.replace(std::ptr::null_mut());
        if !hwnd.is_null() {
            unsafe { DestroyWindow(hwnd) };
        }
        self.state.set(STOPPED);
    }

    /// A property or method by name (DISPID).
    fn set(&self, id: i32, v: &Variant) {
        match id {
            ID_FILE_NAME => {
                let file = v.as_string().unwrap_or_default();
                if *self.file.borrow() != file {
                    drop(self.graph.borrow_mut().take());
                    self.state.set(STOPPED);
                    *self.file.borrow_mut() = file;
                }
            }
            ID_AUTO_START => self.auto_start.set(v.as_i32().unwrap_or(0) != 0),
            ID_VOLUME => {
                let vol = v.as_i32().unwrap_or(0);
                self.volume.set(vol);
                if let Some(g) = self.graph.borrow().as_ref() {
                    g.volume(vol);
                }
            }
            ID_PLAY_COUNT => self.play_count.set(v.as_i32().unwrap_or(1)),
            ID_CURRENT_POSITION => {
                if let (Some(g), Some(t)) = (self.graph.borrow().as_ref(), v.as_f64()) {
                    g.seek(t);
                }
            }
            _ => {}
        }
    }

    fn get(&self, id: i32) -> Option<Variant> {
        Some(match id {
            ID_FILE_NAME => Variant::bstr(&self.file.borrow()),
            ID_AUTO_START => Variant::boolean(self.auto_start.get()),
            ID_VOLUME => Variant::i4(self.volume.get()),
            ID_PLAY_COUNT => Variant::i4(self.play_count.get()),
            ID_CURRENT_STATE => Variant::i4(self.current_state()),
            ID_CURRENT_POSITION => Variant::r8(self.graph.borrow().as_ref().map_or(0.0, Graph::position)),
            ID_DURATION => Variant::r8(self.graph.borrow().as_ref().map_or(0.0, Graph::duration)),
            ID_READY_STATE => Variant::i4(4),
            ID_ENABLED => Variant::boolean(true),
            _ => return None,
        })
    }
}

// the control's dispatch ids
const ID_FILE_NAME: i32 = 1;
const ID_AUTO_START: i32 = 2;
const ID_VOLUME: i32 = 3;
const ID_PLAY_COUNT: i32 = 4;
const ID_CURRENT_STATE: i32 = 5;
const ID_CURRENT_POSITION: i32 = 6;
const ID_DURATION: i32 = 7;
const ID_RUN: i32 = 8;
const ID_STOP: i32 = 9;
const ID_PAUSE: i32 = 10;
/// The properties the control accepts and ignores (the other `<object>` parameters).
const ID_IGNORED: i32 = 11;
const ID_READY_STATE: i32 = -525;
const ID_ENABLED: i32 = -514;

const NAMES: &[(&str, i32)] = &[
    ("FileName", ID_FILE_NAME),
    ("AutoStart", ID_AUTO_START),
    ("Volume", ID_VOLUME),
    ("PlayCount", ID_PLAY_COUNT),
    ("CurrentState", ID_CURRENT_STATE),
    ("CurrentPosition", ID_CURRENT_POSITION),
    ("Duration", ID_DURATION),
    ("Run", ID_RUN),
    ("Stop", ID_STOP),
    ("Pause", ID_PAUSE),
    ("Rate", ID_IGNORED),
    ("Balance", ID_IGNORED),
    ("ShowControls", ID_IGNORED),
    ("ShowDisplay", ID_IGNORED),
    ("FullScreenMode", ID_IGNORED),
    ("AutoRewind", ID_IGNORED),
];

/// `%20`-style escapes of a URL decoded.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// IUnknown on each interface
macro_rules! unknown {
    ($field:expr, $q:ident, $a:ident, $r:ident) => {
        unsafe extern "system" fn $q(this: P, iid: *const GUID, out: *mut P) -> HRESULT {
            unsafe { movie(this, $field) }.query(iid, out)
        }
        unsafe extern "system" fn $a(this: P) -> u32 {
            let m = unsafe { movie(this, $field) };
            m.refs.set(m.refs.get() + 1);
            m.refs.get()
        }
        unsafe extern "system" fn $r(this: P) -> u32 {
            unsafe { movie(this, $field) }.release()
        }
    };
}

unknown!(0, d_query, d_add_ref, d_release);
unknown!(1, o_query, o_add_ref, o_release);
unknown!(2, i_query, i_add_ref, i_release);
unknown!(3, p_query, p_add_ref, p_release);
unknown!(4, s_query, s_add_ref, s_release);

unsafe extern "system" fn not_impl(_this: P) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn ok1(_this: P, _a: usize) -> HRESULT {
    S_OK
}

unsafe extern "system" fn ok2(_this: P, _a: usize, _b: usize) -> HRESULT {
    S_OK
}

unsafe extern "system" fn not_impl1(_this: P, _a: usize) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn not_impl2(_this: P, _a: usize, _b: usize) -> HRESULT {
    E_NOTIMPL
}

unsafe extern "system" fn not_impl3(_this: P, _a: usize, _b: usize, _c: usize) -> HRESULT {
    E_NOTIMPL
}

// IDispatch

unsafe extern "system" fn d_type_info_count(_this: P, count: *mut u32) -> HRESULT {
    if !count.is_null() {
        unsafe { *count = 0 };
    }
    S_OK
}

unsafe extern "system" fn d_ids_of_names(_this: P, _iid: P, names: *const *const u16, count: u32, _lcid: u32, ids: *mut i32) -> HRESULT {
    if names.is_null() || ids.is_null() || count == 0 {
        return E_POINTER;
    }
    let name = wide_str(unsafe { *names });
    let id = NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(&name)).map(|&(_, id)| id);
    unsafe { *ids = id.unwrap_or(-1) };
    for i in 1..count as usize {
        unsafe { *ids.add(i) = -1 };
    }
    if id.is_none() {
        log!("amovie: unknown name {name}");
    }
    if id.is_some() { S_OK } else { DISP_E_UNKNOWNNAME }
}

/// `DISPPARAMS`
#[repr(C)]
struct DispParams {
    args: *const Variant,
    named: *const i32,
    count: u32,
    named_count: u32,
}

const DISPATCH_METHOD: u16 = 1;
const DISPATCH_PROPERTYGET: u16 = 2;
const DISPATCH_PROPERTYPUT: u16 = 4;

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn d_invoke(
    this: P,
    id: i32,
    _iid: P,
    _lcid: u32,
    flags: u16,
    params: *const DispParams,
    result: *mut Variant,
    _excep: P,
    _arg_err: *mut u32,
) -> HRESULT {
    let m = unsafe { movie(this, 0) };
    match id {
        ID_RUN | ID_STOP | ID_PAUSE if flags & DISPATCH_METHOD != 0 => {
            match id {
                ID_RUN => m.run(),
                ID_STOP => m.stop(),
                _ => m.pause(),
            }
            if !result.is_null() {
                unsafe { *result = Variant::EMPTY };
            }
            S_OK
        }
        _ if flags & DISPATCH_PROPERTYPUT != 0 => {
            let p = unsafe { params.as_ref() };
            let Some(p) = p.filter(|p| p.count >= 1 && !p.args.is_null()) else {
                return DISP_E_BADPARAMCOUNT;
            };
            m.set(id, unsafe { &*p.args });
            S_OK
        }
        _ if flags & (DISPATCH_PROPERTYGET | DISPATCH_METHOD) != 0 => match m.get(id) {
            Some(v) => {
                if !result.is_null() {
                    unsafe { *result = v };
                } else if v.vt == VT_BSTR {
                    unsafe { SysFreeString(v.value[0] as usize as *mut u16) };
                }
                S_OK
            }
            None if id == ID_IGNORED => {
                if !result.is_null() {
                    unsafe { *result = Variant::i4(0) };
                }
                S_OK
            }
            None => DISP_E_MEMBERNOTFOUND,
        },
        _ => DISP_E_MEMBERNOTFOUND,
    }
}

static DISPATCH_VTABLE: Vtable<7> = Vtable([
    d_query as *const (),
    d_add_ref as *const (),
    d_release as *const (),
    d_type_info_count as *const (),
    not_impl3 as *const (), // GetTypeInfo
    d_ids_of_names as *const (),
    d_invoke as *const (),
]);

// IOleObject

unsafe extern "system" fn o_set_client_site(this: P, site: P) -> HRESULT {
    let m = unsafe { movie(this, 1) };
    if !site.is_null() {
        unsafe { com_add_ref(site) };
    }
    let old = m.site.replace(site);
    unsafe { com_release(old) };
    S_OK
}

unsafe extern "system" fn o_get_client_site(this: P, out: *mut P) -> HRESULT {
    let m = unsafe { movie(this, 1) };
    if out.is_null() {
        return E_POINTER;
    }
    let site = m.site.get();
    if !site.is_null() {
        unsafe { com_add_ref(site) };
    }
    unsafe { *out = site };
    S_OK
}

unsafe extern "system" fn o_close(this: P, _save: u32) -> HRESULT {
    unsafe { movie(this, 1) }.close();
    S_OK
}

/// The control's window class (black, the movie's window inside).
fn window_class() -> *const u16 {
    static CLASS: std::sync::OnceLock<Vec<u16>> = std::sync::OnceLock::new();
    CLASS
        .get_or_init(|| {
            let name = wide("WalActiveMovie");
            let wc = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(DefWindowProcW),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: std::ptr::null_mut(),
                hIcon: std::ptr::null_mut(),
                hCursor: std::ptr::null_mut(),
                hbrBackground: unsafe { GetStockObject(BLACK_BRUSH) },
                lpszMenuName: std::ptr::null(),
                lpszClassName: name.as_ptr(),
            };
            unsafe { RegisterClassW(&wc) };
            name
        })
        .as_ptr()
}

/// `DoVerb(verb, msg, site, index, parent, rect)`: in-place activation, the control's window in
/// the plugin window, the movie started (`AutoStart`).
unsafe extern "system" fn o_do_verb(this: P, verb: i32, _msg: P, site: P, _index: i32, parent: HWND, rect: *const RECT) -> HRESULT {
    let m = unsafe { movie(this, 1) };
    log!("amovie: verb {verb}, parent {parent:p}");
    if !site.is_null() && m.site.get().is_null() {
        unsafe { o_set_client_site(this, site) };
    }
    if m.hwnd.get().is_null() && !parent.is_null() {
        let r = unsafe { rect.as_ref() }.copied().unwrap_or(RECT { left: 0, top: 0, right: 0, bottom: 0 });
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                window_class(),
                std::ptr::null(),
                WS_CHILD | WS_VISIBLE | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
                r.left,
                r.top,
                w,
                h,
                parent,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        m.hwnd.set(hwnd);
        m.size.set((w, h));
        // the site keeps the in-place object (its OnInPlaceActivateEx)
        let site = m.site.get();
        if !site.is_null()
            && let Some(ex) = unsafe { com_query(site, &IID_OLE_IN_PLACE_SITE_EX) }
        {
            let mut no_redraw = 0i32;
            // IOleInPlaceSiteEx::OnInPlaceActivateEx (after IOleInPlaceSite's 15 methods)
            let f: unsafe extern "system" fn(P, *mut i32, u32) -> HRESULT = unsafe { std::mem::transmute(method(ex, 15)) };
            unsafe { f(ex, &mut no_redraw, 0) };
            unsafe { com_release(ex) };
        }
        if let Some(g) = m.graph.borrow().as_ref() {
            g.place(hwnd, w, h);
        }
        if m.auto_start.get() {
            m.run();
        }
    }
    S_OK
}

unsafe extern "system" fn o_get_user_class_id(_this: P, out: *mut GUID) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe { *out = CLSID_ACTIVE_MOVIE };
    S_OK
}

unsafe extern "system" fn o_advise(_this: P, _sink: P, cookie: *mut u32) -> HRESULT {
    if !cookie.is_null() {
        unsafe { *cookie = 1 };
    }
    S_OK
}

unsafe extern "system" fn o_get_misc_status(_this: P, _aspect: u32, status: *mut u32) -> HRESULT {
    // OLEMISC_SETCLIENTSITEFIRST | ACTIVATEWHENVISIBLE | INSIDEOUT | CANTLINKINSIDE | RECOMPOSEONRESIZE
    if !status.is_null() {
        unsafe { *status = 0x0002_0000 | 0x100 | 0x80 | 0x10 | 0x1 };
    }
    S_OK
}

static OLE_VTABLE: Vtable<24> = Vtable([
    o_query as *const (),
    o_add_ref as *const (),
    o_release as *const (),
    o_set_client_site as *const (),
    o_get_client_site as *const (),
    ok2 as *const (), // SetHostNames
    o_close as *const (),
    ok2 as *const (),      // SetMoniker
    not_impl3 as *const (), // GetMoniker
    not_impl3 as *const (), // InitFromData
    not_impl2 as *const (), // GetClipboardData
    o_do_verb as *const (),
    not_impl1 as *const (), // EnumVerbs
    not_impl as *const (),  // Update
    not_impl as *const (),  // IsUpToDate
    o_get_user_class_id as *const (),
    not_impl2 as *const (), // GetUserType
    ok2 as *const (),       // SetExtent
    not_impl2 as *const (), // GetExtent
    o_advise as *const (),
    ok1 as *const (),       // Unadvise
    not_impl1 as *const (), // EnumAdvise
    o_get_misc_status as *const (),
    ok1 as *const (), // SetColorScheme
]);

// IOleInPlaceObject

unsafe extern "system" fn i_get_window(this: P, out: *mut HWND) -> HRESULT {
    let m = unsafe { movie(this, 2) };
    if out.is_null() {
        return E_POINTER;
    }
    unsafe { *out = m.hwnd.get() };
    if m.hwnd.get().is_null() { E_FAIL } else { S_OK }
}

unsafe extern "system" fn i_deactivate(this: P) -> HRESULT {
    unsafe { movie(this, 2) }.close();
    S_OK
}

unsafe extern "system" fn i_set_rects(this: P, pos: *const RECT, _clip: *const RECT) -> HRESULT {
    let m = unsafe { movie(this, 2) };
    if let Some(r) = unsafe { pos.as_ref() } {
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        m.size.set((w, h));
        let hwnd = m.hwnd.get();
        if !hwnd.is_null() {
            unsafe { MoveWindow(hwnd, r.left, r.top, w, h, 1) };
            if let Some(g) = m.graph.borrow().as_ref() {
                g.place(hwnd, w, h);
            }
        }
    }
    S_OK
}

static IN_PLACE_VTABLE: Vtable<9> = Vtable([
    i_query as *const (),
    i_add_ref as *const (),
    i_release as *const (),
    i_get_window as *const (),
    ok1 as *const (), // ContextSensitiveHelp
    i_deactivate as *const (),
    ok0 as *const (), // UIDeactivate
    i_set_rects as *const (),
    not_impl as *const (), // ReactivateAndUndo
]);

unsafe extern "system" fn ok0(_this: P) -> HRESULT {
    S_OK
}

// IPersistPropertyBag

unsafe extern "system" fn p_get_class_id(this: P, out: *mut GUID) -> HRESULT {
    unsafe { o_get_user_class_id(this, out) }
}

/// `Load(bag, log)`: the `<object>` parameters.
unsafe extern "system" fn p_load(this: P, bag: P, _log: P) -> HRESULT {
    let m = unsafe { movie(this, 3) };
    if bag.is_null() {
        return E_POINTER;
    }
    for &(name, id) in NAMES {
        if id == ID_IGNORED || matches!(id, ID_RUN | ID_STOP | ID_PAUSE | ID_CURRENT_STATE | ID_DURATION) {
            continue;
        }
        let w = wide(name);
        // read as a string (Wine's bag converts its text to the variant's type: none for VT_EMPTY)
        let mut v = Variant { vt: VT_BSTR, ..Variant::EMPTY };
        // IPropertyBag::Read(name, variant, errorlog)
        let f: unsafe extern "system" fn(P, *const u16, *mut Variant, P) -> HRESULT = unsafe { std::mem::transmute(method(bag, 3)) };
        if unsafe { f(bag, w.as_ptr(), &mut v, std::ptr::null_mut()) } >= 0 {
            m.set(id, &v);
            unsafe { VariantClear(&mut v) };
        }
    }
    log!("amovie: {} (auto start {}, volume {}, play count {})", m.file.borrow(), m.auto_start.get(), m.volume.get(), m.play_count.get());
    S_OK
}

static PERSIST_VTABLE: Vtable<7> = Vtable([
    p_query as *const (),
    p_add_ref as *const (),
    p_release as *const (),
    p_get_class_id as *const (),
    ok0 as *const (), // InitNew
    p_load as *const (),
    not_impl3 as *const (), // Save
]);

// IObjectSafety: safe for scripting and initializing

unsafe extern "system" fn s_get_options(_this: P, _iid: P, supported: *mut u32, enabled: *mut u32) -> HRESULT {
    for p in [supported, enabled] {
        if !p.is_null() {
            unsafe { *p = 3 };
        }
    }
    S_OK
}

static SAFETY_VTABLE: Vtable<5> = Vtable([
    s_query as *const (),
    s_add_ref as *const (),
    s_release as *const (),
    s_get_options as *const (),
    ok3 as *const (), // SetInterfaceSafetyOptions
]);

unsafe extern "system" fn ok3(_this: P, _a: usize, _b: usize, _c: usize) -> HRESULT {
    S_OK
}

// class factory (static)

#[repr(C)]
struct Factory {
    vtable: &'static Vtable<5>,
}

static FACTORY: Factory = Factory { vtable: &FACTORY_VTABLE };

unsafe extern "system" fn f_query(this: P, iid: *const GUID, out: *mut P) -> HRESULT {
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

unsafe extern "system" fn f_ref(_this: P) -> u32 {
    1
}

unsafe extern "system" fn f_create(_this: P, outer: P, iid: *const GUID, out: *mut P) -> HRESULT {
    if !outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }
    let m = Box::leak(Box::new(Movie {
        dispatch: &DISPATCH_VTABLE,
        ole: &OLE_VTABLE,
        in_place: &IN_PLACE_VTABLE,
        persist: &PERSIST_VTABLE,
        safety: &SAFETY_VTABLE,
        refs: Cell::new(1),
        site: Cell::new(std::ptr::null_mut()),
        hwnd: Cell::new(std::ptr::null_mut()),
        size: Cell::new((0, 0)),
        file: RefCell::new(String::new()),
        base: RefCell::new(String::new()),
        auto_start: Cell::new(false),
        volume: Cell::new(0),
        play_count: Cell::new(1),
        plays: Cell::new(0),
        state: Cell::new(STOPPED),
        graph: RefCell::new(None),
    }));
    let hr = m.query(iid, out);
    m.release();
    log!("amovie: control created");
    hr
}

static FACTORY_VTABLE: Vtable<5> = Vtable([
    f_query as *const (),
    f_ref as *const (),
    f_ref as *const (),
    f_create as *const (),
    ok1 as *const (), // LockServer
]);

// The HTML Help control's "Close" command (`hhctrl.ocx`, Wine's has no such object): the pages'
// `closeWin`, whose `Click()` closed Internet Explorer for the cabinet's watchdog to start it
// again (a periodic restart, or a movie in an unknown state). The frontend keeps running: a
// no-op. Without the object the pages never get their `onload` (the shell's whole logic).

const CLSID_HH_CTRL: GUID = GUID::from_u128(0xadb880a6_d8ff_11cf_9377_00aa003b7a11);

#[repr(C)]
struct HhClose {
    dispatch: &'static Vtable<7>,
    safety: &'static Vtable<5>,
}

static HH_CLOSE: HhClose = HhClose { dispatch: &HH_DISPATCH_VTABLE, safety: &HH_SAFETY_VTABLE };

unsafe extern "system" fn hh_query(_this: P, iid: *const GUID, out: *mut P) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    let base = &HH_CLOSE as *const HhClose as usize;
    let at = if guid_eq(iid, &IID_UNKNOWN) || guid_eq(iid, &IID_DISPATCH) {
        Some(base)
    } else if guid_eq(iid, &IID_OBJECT_SAFETY) {
        Some(base + offset(1))
    } else {
        None
    };
    unsafe { *out = at.map_or(std::ptr::null_mut(), |a| a as P) };
    if at.is_some() { S_OK } else { E_NOINTERFACE }
}

unsafe extern "system" fn hh_ids_of_names(_this: P, _iid: P, names: *const *const u16, count: u32, _lcid: u32, ids: *mut i32) -> HRESULT {
    if names.is_null() || ids.is_null() || count == 0 {
        return E_POINTER;
    }
    let click = wide_str(unsafe { *names }).eq_ignore_ascii_case("Click");
    unsafe { *ids = if click { 1 } else { -1 } };
    if click { S_OK } else { DISP_E_UNKNOWNNAME }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn hh_invoke(_this: P, id: i32, _iid: P, _lcid: u32, _flags: u16, _p: P, result: *mut Variant, _e: P, _a: P) -> HRESULT {
    if id != 1 {
        return DISP_E_MEMBERNOTFOUND;
    }
    log!("amovie: the page closes the browser (the cabinet's watchdog restart): kept running");
    if !result.is_null() {
        unsafe { *result = Variant::EMPTY };
    }
    S_OK
}

static HH_DISPATCH_VTABLE: Vtable<7> = Vtable([
    hh_query as *const (),
    f_ref as *const (),
    f_ref as *const (),
    d_type_info_count as *const (),
    not_impl3 as *const (), // GetTypeInfo
    hh_ids_of_names as *const (),
    hh_invoke as *const (),
]);

static HH_SAFETY_VTABLE: Vtable<5> = Vtable([
    hh_query as *const (),
    f_ref as *const (),
    f_ref as *const (),
    s_get_options as *const (),
    ok3 as *const (), // SetInterfaceSafetyOptions
]);

static HH_FACTORY: Factory = Factory { vtable: &HH_FACTORY_VTABLE };

unsafe extern "system" fn hh_create(_this: P, outer: P, iid: *const GUID, out: *mut P) -> HRESULT {
    if !outer.is_null() {
        return CLASS_E_NOAGGREGATION;
    }
    log!("amovie: HTML Help close control created");
    unsafe { hh_query(std::ptr::null_mut(), iid, out) }
}

static HH_FACTORY_VTABLE: Vtable<5> = Vtable([
    f_query as *const (),
    f_ref as *const (),
    f_ref as *const (),
    hh_create as *const (),
    ok1 as *const (), // LockServer
]);

#[link(name = "ole32")]
unsafe extern "system" {
    fn CoRegisterClassObject(clsid: *const GUID, unk: P, ctx: u32, flags: u32, cookie: *mut u32) -> HRESULT;
}

/// Both controls registered for the frontend's thread (its pages' objects): COM finds them
/// before the registry (no prefix-wide class registration).
pub(crate) fn register() {
    const CLSCTX_INPROC_SERVER: u32 = 1;
    const REGCLS_MULTIPLEUSE: u32 = 1;
    for (clsid, factory, name) in [
        (&CLSID_ACTIVE_MOVIE, &FACTORY as *const Factory as P, "ActiveMovie"),
        (&CLSID_HH_CTRL, &HH_FACTORY as *const Factory as P, "HTML Help close"),
    ] {
        let mut cookie = 0;
        let hr = unsafe { CoRegisterClassObject(clsid, factory, CLSCTX_INPROC_SERVER, REGCLS_MULTIPLEUSE, &mut cookie) };
        log!("amovie: {name} control registered ({hr:#x})");
    }
}
