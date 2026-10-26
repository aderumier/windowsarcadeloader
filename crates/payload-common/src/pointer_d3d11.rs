//! A touch screen's pointer drawn over a Direct3D 11 game's picture: a dot (a disc with a black
//! outline) where the mouse or gun touches, the Direct3D 11 counterpart of the Konami payload's
//! crosshairs (`WAL_KONAMI_CROSSHAIR_STYLE=disc`).
//!
//! `IDXGISwapChain::Present` / `Present1` are replaced in the swap chains' vtable (found on a
//! swap chain of the payload's own, a hidden window's: DXVK's swap chains share one vtable).
//! Before each frame the dot is drawn in the back buffer, in a device context state of its own
//! (`SwapDeviceContextState`): the game's pipeline state is left untouched. Shaders compiled at
//! the first frame (`d3dcompiler_47.dll`).

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::log;
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

/// The dot's look and where it is.
pub struct Dot {
    /// Red, green, blue (0..1).
    pub color: [f32; 3],
    /// Its diameter, fraction of the picture's height.
    pub size: f32,
    /// Where it is (fractions of the picture from its top left corner); `None`: not shown.
    pub position: fn() -> Option<(f32, f32)>,
}

static DOT: OnceLock<Dot> = OnceLock::new();

/// Draws `dot` over the game's picture from its first frame on.
pub fn init(dot: Dot) {
    if DOT.set(dot).is_ok() {
        std::thread::spawn(hook_present);
    }
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
#[derive(Default)]
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

/// Waits for the game's Direct3D 11, then replaces `Present` in the swap chains' vtable.
fn hook_present() {
    let d3d11 = loop {
        let m = unsafe { GetModuleHandleW(windows_sys::w!("d3d11.dll")) };
        if !m.is_null() {
            break m;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let Some(create) = (unsafe { GetProcAddress(d3d11, c"D3D11CreateDeviceAndSwapChain".as_ptr() as *const u8) }) else {
        log!("pointer: D3D11CreateDeviceAndSwapChain not found, no pointer");
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
        log!("pointer: no swap chain of our own ({hr:#x}), no pointer");
        unsafe { DestroyWindow(window) };
        return;
    }
    let vtable = unsafe { *(swap as *const *mut usize) };
    for (index, hook, orig) in [(8, present as *const () as usize, &PRESENT), (22, present1 as *const () as usize, &PRESENT1)] {
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
    log!("pointer: Present hooked, the touch pointer drawn over the picture");
}

unsafe extern "system" fn present(swap: P, sync: u32, flags: u32) -> HRESULT {
    if flags & DXGI_PRESENT_TEST == 0 {
        overlay(swap);
    }
    let orig: unsafe extern "system" fn(P, u32, u32) -> HRESULT = unsafe { std::mem::transmute(PRESENT.load(Ordering::Relaxed)) };
    unsafe { orig(swap, sync, flags) }
}

unsafe extern "system" fn present1(swap: P, sync: u32, flags: u32, params: P) -> HRESULT {
    if flags & DXGI_PRESENT_TEST == 0 {
        overlay(swap);
    }
    let orig: unsafe extern "system" fn(P, u32, u32, P) -> HRESULT = unsafe { std::mem::transmute(PRESENT1.load(Ordering::Relaxed)) };
    unsafe { orig(swap, sync, flags, params) }
}

/// The objects of the drawing, for one device.
struct Resources {
    device: P,
    context1: P,
    state: P,
    vs: P,
    ps: P,
    blend: P,
    constants: P,
}

unsafe impl Send for Resources {}

static RESOURCES: Mutex<Option<Resources>> = Mutex::new(None);

const SHADER: &str = r#"
cbuffer Dot : register(b0) { float4 dot; float4 color; };
float4 vs_main(uint id : SV_VertexID) : SV_Position {
    float2 p = float2((id << 1) & 2, id & 2);
    return float4(p.x * 2 - 1, 1 - p.y * 2, 0, 1);
}
float4 ps_main(float4 pos : SV_Position) : SV_Target {
    float d = length(pos.xy - dot.xy);
    float a = saturate(dot.z - d + 0.5);
    if (a <= 0) discard;
    float inside = saturate(dot.z - dot.w - d + 0.5);
    return float4(color.rgb * inside, a);
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
            log!("pointer: shader {entry:?}: {}", String::from_utf8_lossy(&blob(errors)));
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
    fn new(device: P, context: P) -> Option<Resources> {
        let mut r = Resources {
            device,
            context1: std::ptr::null_mut(),
            state: std::ptr::null_mut(),
            vs: std::ptr::null_mut(),
            ps: std::ptr::null_mut(),
            blend: std::ptr::null_mut(),
            constants: std::ptr::null_mut(),
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
        let vs = compile(c"vs_main", c"vs_4_0")?;
        let ps = compile(c"ps_main", c"ps_4_0")?;
        // D3D11_BLEND_DESC: render target 0 blended (src alpha, inverse src alpha), all written
        let mut blend = [0u32; 2 + 8 * 8];
        blend[2..10].copy_from_slice(&[1, 5, 6, 1, 2, 6, 1, 0xF]);
        // CONSTANT_BUFFER, 32 bytes
        let buffer: [u32; 6] = [32, 0, 4, 0, 0, 0];
        if com!(device, 12, fn(*const u8, usize, P, *mut P) -> HRESULT, vs.as_ptr(), vs.len(), std::ptr::null_mut(), &mut r.vs) < 0
            || com!(device, 15, fn(*const u8, usize, P, *mut P) -> HRESULT, ps.as_ptr(), ps.len(), std::ptr::null_mut(), &mut r.ps) < 0
            || com!(device, 20, fn(*const u32, *mut P) -> HRESULT, blend.as_ptr(), &mut r.blend) < 0
            || com!(device, 3, fn(*const u32, P, *mut P) -> HRESULT, buffer.as_ptr(), std::ptr::null_mut(), &mut r.constants) < 0
        {
            return None;
        }
        Some(r)
    }
}

impl Drop for Resources {
    fn drop(&mut self) {
        for p in [self.context1, self.state, self.vs, self.ps, self.blend, self.constants] {
            release(p);
        }
    }
}

fn overlay(swap: P) {
    let Some(dot) = DOT.get() else { return };
    let Some((x, y)) = (dot.position)() else { return };
    let mut back: P = std::ptr::null_mut();
    if com!(swap, 9, fn(u32, *const GUID, *mut P) -> HRESULT, 0, &IID_TEXTURE2D, &mut back) < 0 || back.is_null() {
        return;
    }
    let mut desc = Texture2DDesc::default();
    com!(back, 10, fn(*mut Texture2DDesc) -> (), &mut desc);
    let mut device: P = std::ptr::null_mut();
    com!(back, 3, fn(*mut P) -> (), &mut device);
    let mut context: P = std::ptr::null_mut();
    com!(device, 40, fn(*mut P) -> (), &mut context);
    let mut guard = RESOURCES.lock().unwrap();
    if guard.as_ref().is_none_or(|r| r.device != device) {
        *guard = None;
        *guard = Resources::new(device, context);
        if guard.is_none() {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| log!("pointer: the pointer cannot be drawn"));
        }
    }
    if let Some(r) = guard.as_ref() {
        let (w, h) = (desc.width as f32, desc.height as f32);
        let radius = (dot.size * h / 2.0).max(3.0);
        draw(r, context, back, (x * w, y * h), radius, dot.color);
    }
    release(context);
    release(device);
    release(back);
}

fn draw(r: &Resources, context: P, back: P, (x, y): (f32, f32), radius: f32, color: [f32; 3]) {
    let mut rtv: P = std::ptr::null_mut();
    if com!(r.device, 9, fn(P, P, *mut P) -> HRESULT, back, std::ptr::null_mut(), &mut rtv) < 0 {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| log!("pointer: no render target view of the back buffer"));
        return;
    }
    let outline = (radius * 0.3).clamp(1.5, 3.0);
    let constants: [f32; 8] = [x, y, radius, outline, color[0], color[1], color[2], 1.0];
    let mut previous: P = std::ptr::null_mut();
    com!(r.context1, 131, fn(P, *mut P) -> (), r.state, &mut previous);
    com!(context, 48, fn(P, u32, P, *const f32, u32, u32) -> (), r.constants, 0, std::ptr::null_mut(), constants.as_ptr(), 0, 0);
    com!(context, 33, fn(u32, *const P, P) -> (), 1, &rtv, std::ptr::null_mut());
    // only the dot's square drawn
    let side = radius * 2.0 + 2.0;
    let viewport = [x - side / 2.0, y - side / 2.0, side, side, 0.0, 1.0];
    com!(context, 44, fn(u32, *const f32) -> (), 1, viewport.as_ptr());
    com!(context, 35, fn(P, *const f32, u32) -> (), r.blend, std::ptr::null(), 0xFFFF_FFFF);
    com!(context, 24, fn(u32) -> (), 4); // TRIANGLELIST
    com!(context, 11, fn(P, P, u32) -> (), r.vs, std::ptr::null_mut(), 0);
    com!(context, 9, fn(P, P, u32) -> (), r.ps, std::ptr::null_mut(), 0);
    com!(context, 16, fn(u32, u32, *const P) -> (), 0, 1, &r.constants);
    com!(context, 13, fn(u32, u32) -> (), 3, 0);
    com!(context, 33, fn(u32, *const P, P) -> (), 0, std::ptr::null(), std::ptr::null_mut());
    com!(r.context1, 131, fn(P, *mut P) -> (), previous, std::ptr::null_mut());
    release(previous);
    release(rtv);
}
