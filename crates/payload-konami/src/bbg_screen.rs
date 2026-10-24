//! Rainbow Bomber Girl's two monitors. The cabinet drives them as one 2160x1920 desktop: the
//! main monitor (portrait, 1080x1920) on the left, the touch screen on the right, a landscape
//! 1920x1080 panel mounted on its side (its picture turned a quarter left). The game draws
//! that desktop in one picture: it runs windowed at 2160x1920 (`bbg.rs`), its window the
//! screen's size, the back buffer stretched to it by Direct3D.
//!
//! Before each `Present`, the picture (the 9:8 picture centered in the back buffer, all of it at
//! 2160x1920) is drawn again for the screen (its proportions), as the cabinet
//! stands: the main monitor upright at the screen's height on the right, the touch screen
//! turned back upright in the space left, at the bottom; above it the marquee of the dump's
//! `bezel.png` (its top left corner, the space above the touch screen on a 16:9 screen),
//! stretched to that space, when there is one.
//! `WAL_KONAMI_LAYOUT=main_left`: the main monitor on the left, the touch screen centered on
//! the right, no bezel; `raw`: the game's own picture. The pointer (`accel`, `brake`: a gun or the mouse over the screen) is
//! drawn as a ring on the touch screen; `pointer_on_touch_screen` tells where it is on it
//! (`bbg_io.rs`'s touches).
//!
//! Direct3D 11 (DXVK): `IDXGISwapChain::Present` / `Present1` are replaced in the swap chains'
//! vtable (found on a swap chain of the payload's own); the drawing runs in a device context
//! state of its own (`SwapDeviceContextState`): the game's pipeline state is left untouched.
//! Shaders compiled at the first frame (`d3dcompiler_47.dll`).

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::log;
use wal_protocol::Axis;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
use windows_sys::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_OVERLAPPEDWINDOW};
use windows_sys::core::GUID;

type P = *mut c_void;
type HRESULT = i32;

const IID_TEXTURE2D: GUID = GUID::from_u128(0x6f15aaf2_d208_4e89_9ab4_489535d34f9c);
const IID_DEVICE: GUID = GUID::from_u128(0xdb6f6ddb_ac77_4e88_8253_819df9bbf140);
const IID_DEVICE1: GUID = GUID::from_u128(0xa04bfb29_08ef_43d6_a49c_a9bdbdcbe686);
const IID_CONTEXT1: GUID = GUID::from_u128(0xbb2c6faa_b5fb_4082_8e6b_388b8cfa90e1);

const DXGI_PRESENT_TEST: u32 = 1;

static PRESENT: AtomicUsize = AtomicUsize::new(0);
static PRESENT1: AtomicUsize = AtomicUsize::new(0);
static GET_BUFFER: AtomicUsize = AtomicUsize::new(0);
static RESIZE_BUFFERS: AtomicUsize = AtomicUsize::new(0);
static GET_DESC: AtomicUsize = AtomicUsize::new(0);


/// Layout in screen fractions (x, y, width, height): the main monitor and the touch screen.
static MAIN: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];
static TOUCH: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];

fn load(r: &[AtomicU32; 4]) -> [f32; 4] {
    r.each_ref().map(|v| f32::from_bits(v.load(Ordering::Relaxed)))
}

fn store(r: &[AtomicU32; 4], v: [f32; 4]) {
    for (a, b) in r.iter().zip(v) {
        a.store(b.to_bits(), Ordering::Relaxed);
    }
}

fn main_left() -> bool {
    std::env::var("WAL_KONAMI_LAYOUT").is_ok_and(|v| v.trim().eq_ignore_ascii_case("main_left"))
}

/// The two rectangles on a `width` x `height` screen.
fn layout(width: f32, height: f32) {
    // the main monitor at the screen's height (9:16), the touch screen (16:9) in the rest
    let mw = (height * 9.0 / 16.0).min(width / 2.0);
    let mh = mw * 16.0 / 9.0;
    let tw = width - mw;
    let th = (tw * 9.0 / 16.0).min(height);
    let tw = th * 16.0 / 9.0;
    let (mx, tx, ty) = if main_left() { (0.0, mw + (width - mw - tw) / 2.0, (height - th) / 2.0) } else { (width - mw, (width - mw - tw) / 2.0, height - th) };
    store(&MAIN, [mx / width, (height - mh) / 2.0 / height, mw / width, mh / height]);
    store(&TOUCH, [tx / width, ty / height, tw / width, th / height]);
}

/// The dump's `bezel.png` beside the executable (RGBA rows), with the default layout.
fn bezel() -> Option<(u32, u32, Vec<u8>)> {
    if main_left() {
        return None;
    }
    let mut buf = [0u16; 260];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n])).with_file_name("bezel.png");
    let mut decoder = png::Decoder::new(std::fs::File::open(&path).ok()?);
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut data = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut data).ok()?;
    let data = &data[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => data.to_vec(),
        png::ColorType::Rgb => data.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 0xFF]).collect(),
        png::ColorType::GrayscaleAlpha => data.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g, 0xFF]).collect(),
        _ => return None,
    };
    log!("bbg: bezel.png {}x{}", info.width, info.height);
    Some((info.width, info.height, rgba))
}

/// The screen's size: the game's window covers it, the back buffer stretched to it.
fn screen() -> (f32, f32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    if w > 0 && h > 0 { (w as f32, h as f32) } else { (16.0, 9.0) }
}

/// The pointer over the screen (fractions).
fn pointer() -> (f32, f32) {
    let stick = wal_payload_common::input(0);
    let at = |axis: Axis| stick.axis(axis).max(0) as f32 / 32767.0;
    (at(Axis::Accel), at(Axis::Brake))
}

/// The pointer on the upright touch screen (fractions from its top left corner), when it is
/// over it.
pub fn pointer_on_touch_screen() -> Option<(f32, f32)> {
    if TOUCH[2].load(Ordering::Relaxed) == 0 {
        let (w, h) = screen();
        layout(w, h);
    }
    let [x, y, w, h] = load(&TOUCH);
    let (px, py) = pointer();
    let (u, v) = ((px - x) / w, (py - y) / h);
    ((0.0..1.0).contains(&u) && (0.0..1.0).contains(&v)).then_some((u, v))
}

pub fn init() {
    std::thread::spawn(hook_present);
}

/// Calls the vtable method `index` of the COM object `obj`.
macro_rules! com {
    ($obj:expr, $index:expr, fn($($t:ty),*) -> $r:ty $(, $a:expr)*) => {{
        let obj = $obj;
        let f: unsafe extern "system" fn(P $(, $t)*) -> $r = unsafe { std::mem::transmute(*(*(obj as *const *const usize)).add($index)) };
        unsafe { f(obj $(, $a)*) }
    }};
}

fn release(obj: P) {
    if !obj.is_null() {
        com!(obj, 2, fn() -> u32);
    }
}

#[repr(C)]
struct SwapChainDesc {
    width: u32,
    height: u32,
    refresh: [u32; 2],
    format: u32,
    scanline: u32,
    scaling: u32,
    sample_count: u32,
    sample_quality: u32,
    usage: u32,
    buffer_count: u32,
    window: P,
    windowed: i32,
    swap_effect: u32,
    flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq)]
struct Texture2DDesc {
    width: u32,
    height: u32,
    mip_levels: u32,
    array_size: u32,
    format: u32,
    sample_count: u32,
    sample_quality: u32,
    usage: u32,
    bind_flags: u32,
    cpu_access: u32,
    misc: u32,
}

/// Waits for the game's Direct3D 11, then replaces `Present` in DXVK's swap chain vtable,
/// found on a swap chain of the payload's own (a hidden window).
fn hook_present() {
    let d3d11 = loop {
        let m = unsafe { GetModuleHandleW(windows_sys::w!("d3d11.dll")) };
        if !m.is_null() {
            break m;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let Some(create) = (unsafe { GetProcAddress(d3d11, c"D3D11CreateDeviceAndSwapChain".as_ptr() as *const u8) }) else {
        log!("bbg: D3D11CreateDeviceAndSwapChain not found: one picture");
        return;
    };
    type Create = unsafe extern "system" fn(P, u32, P, u32, P, u32, u32, *const SwapChainDesc, *mut P, *mut P, *mut u32, *mut P) -> HRESULT;
    let create: Create = unsafe { std::mem::transmute(create) };
    let window = unsafe { CreateWindowExW(0, windows_sys::w!("STATIC"), windows_sys::w!("wal"), WS_OVERLAPPEDWINDOW, 0, 0, 64, 64, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null()) };
    let desc = SwapChainDesc {
        width: 64,
        height: 64,
        refresh: [0, 0],
        format: 28, // R8G8B8A8_UNORM
        scanline: 0,
        scaling: 0,
        sample_count: 1,
        sample_quality: 0,
        usage: 0x20, // RENDER_TARGET_OUTPUT
        buffer_count: 1,
        window,
        windowed: 1,
        swap_effect: 0,
        flags: 0,
    };
    let (mut swap, mut device, mut context): (P, P, P) = (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
    // D3D_DRIVER_TYPE_HARDWARE, D3D11_SDK_VERSION
    let hr = unsafe { create(std::ptr::null_mut(), 1, std::ptr::null_mut(), 0, std::ptr::null_mut(), 0, 7, &desc, &mut swap, &mut device, std::ptr::null_mut(), &mut context) };
    if hr < 0 || swap.is_null() {
        log!("bbg: no swap chain of our own ({hr:#x}): one picture");
        unsafe { DestroyWindow(window) };
        return;
    }
    let vtable = unsafe { *(swap as *const *mut usize) };
    let hooks = [
        (8, present as *const () as usize, &PRESENT),
        (22, present1 as *const () as usize, &PRESENT1),
        (9, get_buffer as *const () as usize, &GET_BUFFER),
        (13, resize_buffers as *const () as usize, &RESIZE_BUFFERS),
        (12, get_desc as *const () as usize, &GET_DESC),
    ];
    for (index, hook, orig) in hooks {
        let slot = unsafe { vtable.add(index) };
        let mut old = 0;
        unsafe {
            VirtualProtect(slot.cast(), 8, PAGE_READWRITE, &mut old);
            orig.store(*slot, Ordering::Relaxed);
            *slot = hook;
            VirtualProtect(slot.cast(), 8, old, &mut old);
        }
    }
    release(context);
    release(device);
    release(swap);
    unsafe { DestroyWindow(window) };
    log!("bbg: Present hooked: the two monitors side by side");
}

/// The game's back buffer: a texture of the payload's own at the cabinet's size (2160x1920),
/// the swap chain's real buffer the window's size. `GetBuffer` hands the game the texture
/// (made at its first request, from the real buffer's format), `ResizeBuffers` resizes the real
/// buffers to the window (the texture made again), `GetDesc` tells the cabinet's size; `Present`
/// draws the screen's picture from the texture into the real buffer.
struct Proxy {
    device: P,
    texture: P,
    view: P,
}

unsafe impl Send for Proxy {}

impl Drop for Proxy {
    fn drop(&mut self) {
        release(self.view);
        release(self.texture);
    }
}

static PROXY: Mutex<Option<Proxy>> = Mutex::new(None);

fn real_buffer(swap: P) -> Option<P> {
    let get: unsafe extern "system" fn(P, u32, *const GUID, *mut P) -> HRESULT = unsafe { std::mem::transmute(GET_BUFFER.load(Ordering::Relaxed)) };
    let mut back: P = std::ptr::null_mut();
    (unsafe { get(swap, 0, &IID_TEXTURE2D, &mut back) } >= 0 && !back.is_null()).then_some(back)
}

fn make_proxy(swap: P) -> Option<Proxy> {
    let back = real_buffer(swap)?;
    let mut desc = Texture2DDesc::default();
    com!(back, 10, fn(*mut Texture2DDesc) -> (), &mut desc);
    let mut device: P = std::ptr::null_mut();
    com!(back, 3, fn(*mut P) -> (), &mut device);
    release(back);
    let (w, h) = (crate::bbg::DESKTOP.0 as u32, crate::bbg::DESKTOP.1 as u32);
    // render target, sampled
    let tex_desc = Texture2DDesc { width: w, height: h, mip_levels: 1, array_size: 1, sample_count: 1, sample_quality: 0, usage: 0, bind_flags: 0x20 | 0x8, cpu_access: 0, misc: 0, ..desc };
    let mut p = Proxy { device, texture: std::ptr::null_mut(), view: std::ptr::null_mut() };
    if com!(device, 5, fn(*const Texture2DDesc, P, *mut P) -> HRESULT, &tex_desc, std::ptr::null_mut(), &mut p.texture) < 0
        || com!(device, 7, fn(P, P, *mut P) -> HRESULT, p.texture, std::ptr::null_mut(), &mut p.view) < 0
    {
        release(device);
        return None;
    }
    release(device);
    log!("bbg: the game draws a {w}x{h} picture (the window {}x{})", desc.width, desc.height);
    Some(p)
}

unsafe extern "system" fn get_buffer(swap: P, index: u32, riid: *const GUID, out: *mut P) -> HRESULT {
    let mut guard = PROXY.lock().unwrap();
    if guard.is_none() {
        *guard = make_proxy(swap);
    }
    if let Some(p) = guard.as_ref() {
        return com!(p.texture, 0, fn(*const GUID, *mut P) -> HRESULT, riid, out);
    }
    drop(guard);
    let get: unsafe extern "system" fn(P, u32, *const GUID, *mut P) -> HRESULT = unsafe { std::mem::transmute(GET_BUFFER.load(Ordering::Relaxed)) };
    unsafe { get(swap, index, riid, out) }
}

unsafe extern "system" fn resize_buffers(swap: P, count: u32, _width: u32, _height: u32, format: u32, flags: u32) -> HRESULT {
    *PROXY.lock().unwrap() = None;
    *RESOURCES.lock().unwrap() = None;
    let orig: unsafe extern "system" fn(P, u32, u32, u32, u32, u32) -> HRESULT = unsafe { std::mem::transmute(RESIZE_BUFFERS.load(Ordering::Relaxed)) };
    // the real buffers: the screen's size (the game's window covers it)
    let (w, h) = screen();
    let mut desc = [0u32; 17];
    let get_desc: unsafe extern "system" fn(P, *mut u32) -> HRESULT = unsafe { std::mem::transmute(GET_DESC.load(Ordering::Relaxed)) };
    if unsafe { get_desc(swap, desc.as_mut_ptr()) } >= 0 {
        let window = unsafe { *(desc.as_ptr().add(12) as *const P) };
        let mut rc = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(window, &mut rc) };
        log!("bbg: buffers {w}x{h}, the window's client area {}x{}", rc.right, rc.bottom);
    }
    unsafe { orig(swap, count, w as u32, h as u32, format, flags) }
}

/// DXGI_SWAP_CHAIN_DESC: BufferDesc.Width, Height first: the game's picture.
unsafe extern "system" fn get_desc(swap: P, desc: *mut u32) -> HRESULT {
    let orig: unsafe extern "system" fn(P, *mut u32) -> HRESULT = unsafe { std::mem::transmute(GET_DESC.load(Ordering::Relaxed)) };
    let hr = unsafe { orig(swap, desc) };
    if hr >= 0 && !desc.is_null() {
        unsafe {
            *desc = crate::bbg::DESKTOP.0 as u32;
            *desc.add(1) = crate::bbg::DESKTOP.1 as u32;
        }
    }
    hr
}

unsafe extern "system" fn present(swap: P, sync: u32, flags: u32) -> HRESULT {
    if flags & DXGI_PRESENT_TEST == 0 {
        compose(swap);
    }
    let orig: unsafe extern "system" fn(P, u32, u32) -> HRESULT = unsafe { std::mem::transmute(PRESENT.load(Ordering::Relaxed)) };
    unsafe { orig(swap, sync, flags) }
}

unsafe extern "system" fn present1(swap: P, sync: u32, flags: u32, params: P) -> HRESULT {
    if flags & DXGI_PRESENT_TEST == 0 {
        compose(swap);
    }
    let orig: unsafe extern "system" fn(P, u32, u32, P) -> HRESULT = unsafe { std::mem::transmute(PRESENT1.load(Ordering::Relaxed)) };
    unsafe { orig(swap, sync, flags, params) }
}

/// The objects of the drawing, for one device and back buffer size.
struct Resources {
    device: P,
    context1: P,
    state: P,
    desc: Texture2DDesc,
    copy: P,
    view: P,
    vs: P,
    ps: P,
    sampler: P,
    constants: P,
    /// The bezel's texture view (null: none).
    bezel: P,
}

unsafe impl Send for Resources {}

static RESOURCES: Mutex<Option<Resources>> = Mutex::new(None);

const SHADER: &str = r#"
cbuffer Layout : register(b0) { float4 mainRect; float4 touchRect; float4 cursor; float4 screen; float4 source; float4 marquee; };
Texture2D src : register(t0);
Texture2D bezel : register(t1);
SamplerState smp : register(s0);
float4 vs_main(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p.x * 2 - 1, 1 - p.y * 2, 0, 1);
}
bool inside(float2 o, float4 r) { return all(o >= r.xy) && all(o < r.xy + r.zw); }
float4 ps_main(float4 pos : SV_Position) : SV_Target {
    float2 o = pos.xy * screen.zw;
    if (cursor.z > 0) {
        float d = length(pos.xy - cursor.xy * screen.xy);
        if (d >= 7 && d < 9) return float4(1, 1, 1, 1);
        if (d >= 5 && d < 11) return float4(0, 0, 0, 1);
    }
    if (inside(o, mainRect)) {
        float2 uv = (o - mainRect.xy) / mainRect.zw;
        return src.Sample(smp, source.xy + source.zw * float2(min(uv.x * 0.5, 0.5 - screen.z), uv.y));
    }
    if (inside(o, touchRect)) {
        float2 uv = (o - touchRect.xy) / touchRect.zw;
        return src.Sample(smp, source.xy + source.zw * float2(max(0.5 + uv.y * 0.5, 0.5 + screen.z), 1 - uv.x));
    }
    if (cursor.w > 0 && inside(o, marquee)) {
        float2 uv = (o - marquee.xy) / marquee.zw;
        float4 b = bezel.Sample(smp, uv * float2(1312.5 / 1920, 341.7 / 1080));
        return float4(b.rgb * b.a, 1);
    }
    return float4(0, 0, 0, 1);
}
"#;

/// Compiles `entry` of the shader for `target` (`vs_4_0`, `ps_4_0`).
fn compile(entry: &std::ffi::CStr, target: &std::ffi::CStr) -> Option<Vec<u8>> {
    type Compile = unsafe extern "system" fn(*const u8, usize, *const i8, P, P, *const i8, *const i8, u32, u32, *mut P, *mut P) -> HRESULT;
    let module = unsafe { LoadLibraryW(windows_sys::w!("d3dcompiler_47.dll")) };
    let f = unsafe { GetProcAddress(module, c"D3DCompile".as_ptr() as *const u8) }?;
    let compile: Compile = unsafe { std::mem::transmute(f) };
    let (mut code, mut errors): (P, P) = (std::ptr::null_mut(), std::ptr::null_mut());
    let hr = unsafe { compile(SHADER.as_ptr(), SHADER.len(), std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), entry.as_ptr(), target.as_ptr(), 0, 0, &mut code, &mut errors) };
    let blob = |b: P| -> Vec<u8> {
        let ptr = com!(b, 3, fn() -> *const u8);
        let len = com!(b, 4, fn() -> usize);
        unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()
    };
    if hr < 0 || code.is_null() {
        if !errors.is_null() {
            log!("bbg: shader {entry:?}: {}", String::from_utf8_lossy(&blob(errors)));
            release(errors);
        }
        return None;
    }
    let bytes = blob(code);
    release(code);
    release(errors);
    Some(bytes)
}

impl Resources {
    fn new(device: P, context: P, desc: Texture2DDesc) -> Option<Resources> {
        let mut r = Resources {
            device,
            context1: std::ptr::null_mut(),
            state: std::ptr::null_mut(),
            desc,
            copy: std::ptr::null_mut(),
            view: std::ptr::null_mut(),
            vs: std::ptr::null_mut(),
            ps: std::ptr::null_mut(),
            sampler: std::ptr::null_mut(),
            constants: std::ptr::null_mut(),
            bezel: std::ptr::null_mut(),
        };
        // a context state of our own (its own pipeline state)
        let mut device1: P = std::ptr::null_mut();
        if com!(device, 0, fn(*const GUID, *mut P) -> HRESULT, &IID_DEVICE1, &mut device1) < 0 {
            return None;
        }
        let level = com!(device, 37, fn() -> u32);
        let hr = com!(device1, 47, fn(u32, *const u32, u32, u32, *const GUID, *mut u32, *mut P) -> HRESULT, 0, &level, 1, 7, &IID_DEVICE, std::ptr::null_mut(), &mut r.state);
        release(device1);
        if hr < 0 || com!(context, 0, fn(*const GUID, *mut P) -> HRESULT, &IID_CONTEXT1, &mut r.context1) < 0 {
            return None;
        }
        // the picture's copy, sampled
        let copy_desc = Texture2DDesc { mip_levels: 1, array_size: 1, sample_count: 1, sample_quality: 0, usage: 0, bind_flags: 8, cpu_access: 0, misc: 0, ..desc };
        if com!(device, 5, fn(*const Texture2DDesc, P, *mut P) -> HRESULT, &copy_desc, std::ptr::null_mut(), &mut r.copy) < 0
            || com!(device, 7, fn(P, P, *mut P) -> HRESULT, r.copy, std::ptr::null_mut(), &mut r.view) < 0
        {
            return None;
        }
        let vs = compile(c"vs_main", c"vs_4_0")?;
        let ps = compile(c"ps_main", c"ps_4_0")?;
        if com!(device, 12, fn(*const u8, usize, P, *mut P) -> HRESULT, vs.as_ptr(), vs.len(), std::ptr::null_mut(), &mut r.vs) < 0
            || com!(device, 15, fn(*const u8, usize, P, *mut P) -> HRESULT, ps.as_ptr(), ps.len(), std::ptr::null_mut(), &mut r.ps) < 0
        {
            return None;
        }
        // linear, clamped
        let sampler: [u32; 13] = [0x15, 3, 3, 3, 0, 1, 1, 0, 0, 0, 0, 0, f32::MAX.to_bits()];
        // CONSTANT_BUFFER, 96 bytes
        let buffer: [u32; 6] = [96, 0, 4, 0, 0, 0];
        if com!(device, 23, fn(*const u32, *mut P) -> HRESULT, sampler.as_ptr(), &mut r.sampler) < 0
            || com!(device, 3, fn(*const u32, P, *mut P) -> HRESULT, buffer.as_ptr(), std::ptr::null_mut(), &mut r.constants) < 0
        {
            return None;
        }
        if let Some((w, h, rgba)) = bezel() {
            let tex_desc = Texture2DDesc { width: w, height: h, mip_levels: 1, array_size: 1, format: 28, sample_count: 1, sample_quality: 0, usage: 0, bind_flags: 8, cpu_access: 0, misc: 0 };
            // D3D11_SUBRESOURCE_DATA: pSysMem, SysMemPitch, SysMemSlicePitch
            #[repr(C)]
            struct Data(*const u8, u32, u32);
            let data = Data(rgba.as_ptr(), w * 4, 0);
            let mut tex: P = std::ptr::null_mut();
            if com!(device, 5, fn(*const Texture2DDesc, *const Data, *mut P) -> HRESULT, &tex_desc, &data, &mut tex) >= 0 {
                com!(device, 7, fn(P, P, *mut P) -> HRESULT, tex, std::ptr::null_mut(), &mut r.bezel);
                release(tex);
            }
        }
        let (sw, sh) = (desc.width as f32, desc.height as f32);
        layout(sw, sh);
        log!("bbg: picture {}x{} (format {}, {} samples) composed for a {sw}x{sh} screen", desc.width, desc.height, desc.format, desc.sample_count);
        Some(r)
    }
}

impl Drop for Resources {
    fn drop(&mut self) {
        for p in [self.context1, self.state, self.copy, self.view, self.vs, self.ps, self.sampler, self.constants, self.bezel] {
            release(p);
        }
    }
}

fn compose(swap: P) {
    static RAW: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *RAW.get_or_init(|| std::env::var("WAL_KONAMI_LAYOUT").is_ok_and(|v| v.trim().eq_ignore_ascii_case("raw"))) {
        return;
    }
    let Some(back) = real_buffer(swap) else { return };
    let mut desc = Texture2DDesc::default();
    com!(back, 10, fn(*mut Texture2DDesc) -> (), &mut desc);
    let mut device: P = std::ptr::null_mut();
    com!(back, 3, fn(*mut P) -> (), &mut device);
    let mut context: P = std::ptr::null_mut();
    com!(device, 40, fn(*mut P) -> (), &mut context);
    let mut guard = RESOURCES.lock().unwrap();
    if guard.as_ref().is_none_or(|r| r.device != device || r.desc != desc) {
        *guard = None;
        *guard = Resources::new(device, context, desc);
        if guard.is_none() {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| log!("bbg: the picture cannot be composed: one picture"));
        }
    }
    if let Some(r) = guard.as_ref() {
        // the game's own texture, else (drawn in the real buffer) its copy
        let proxy = PROXY.lock().unwrap();
        let source = proxy.as_ref().filter(|p| p.device == device).map(|p| p.view);
        draw(r, context, back, source);
    }
    release(context);
    release(device);
    release(back);
}

fn draw(r: &Resources, context: P, back: P, source: Option<P>) {
    let mut rtv: P = std::ptr::null_mut();
    if com!(r.device, 9, fn(P, P, *mut P) -> HRESULT, back, std::ptr::null_mut(), &mut rtv) < 0 {
        return;
    }
    // the game's picture
    if source.is_none() {
        if r.desc.sample_count > 1 {
            com!(context, 57, fn(P, u32, P, u32, u32) -> (), r.copy, 0, back, 0, r.desc.format);
        } else {
            com!(context, 47, fn(P, P) -> (), r.copy, back);
        }
    }
    let (w, h) = (r.desc.width as f32, r.desc.height as f32);
    let (px, py) = pointer();
    let cursor = if pointer_on_touch_screen().is_some() { 1.0 } else { 0.0 };
    // the game's texture; in the real buffer: the cabinet's 9:8 picture centered
    let (fw, fh) = if source.is_some() {
        (1.0, 1.0)
    } else if w * 8.0 > h * 9.0 {
        (h * 9.0 / 8.0 / w, 1.0)
    } else {
        (1.0, w * 8.0 / 9.0 / h)
    };
    let mut constants = [0f32; 24];
    constants[0..4].copy_from_slice(&load(&MAIN));
    constants[4..8].copy_from_slice(&load(&TOUCH));
    constants[8..12].copy_from_slice(&[px, py, cursor, if r.bezel.is_null() { 0.0 } else { 1.0 }]);
    constants[12..16].copy_from_slice(&[w, h, 1.0 / w, 1.0 / h]);
    constants[16..20].copy_from_slice(&[(1.0 - fw) / 2.0, (1.0 - fh) / 2.0, fw, fh]);
    // the marquee: the space above the touch screen
    let [tx, ty, tw, _] = load(&TOUCH);
    constants[20..24].copy_from_slice(&[tx, 0.0, tw, ty]);
    let mut previous: P = std::ptr::null_mut();
    com!(r.context1, 131, fn(P, *mut P) -> (), r.state, &mut previous);
    com!(context, 48, fn(P, u32, P, *const f32, u32, u32) -> (), r.constants, 0, std::ptr::null_mut(), constants.as_ptr(), 0, 0);
    com!(context, 33, fn(u32, *const P, P) -> (), 1, &rtv, std::ptr::null_mut());
    let viewport = [0.0f32, 0.0, w, h, 0.0, 1.0];
    com!(context, 44, fn(u32, *const f32) -> (), 1, viewport.as_ptr());
    com!(context, 24, fn(u32) -> (), 4); // TRIANGLELIST
    com!(context, 11, fn(P, P, u32) -> (), r.vs, std::ptr::null_mut(), 0);
    com!(context, 9, fn(P, P, u32) -> (), r.ps, std::ptr::null_mut(), 0);
    let views = [source.unwrap_or(r.view), r.bezel];
    com!(context, 8, fn(u32, u32, *const P) -> (), 0, 2, views.as_ptr());
    com!(context, 10, fn(u32, u32, *const P) -> (), 0, 1, &r.sampler);
    com!(context, 16, fn(u32, u32, *const P) -> (), 0, 1, &r.constants);
    com!(context, 13, fn(u32, u32) -> (), 3, 0);
    // nothing of ours left bound: the copy is written again next frame
    let none: [P; 2] = [std::ptr::null_mut(); 2];
    com!(context, 8, fn(u32, u32, *const P) -> (), 0, 2, none.as_ptr());
    com!(context, 33, fn(u32, *const P, P) -> (), 0, std::ptr::null(), std::ptr::null_mut());
    com!(r.context1, 131, fn(P, *mut P) -> (), previous, std::ptr::null_mut());
    release(previous);
    release(rtv);
}
