//! One native DXGI presentation dispatcher. Game overlays run first; the Sinden frame is
//! composed last. The copied back buffer is restored after Present and never held across resize.
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::log;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, WS_OVERLAPPEDWINDOW,
};
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

static CALLBACKS: Mutex<Vec<fn(P)>> = Mutex::new(Vec::new());
pub fn register(callback: fn(P)) {
    CALLBACKS.lock().unwrap().push(callback);
    init();
}
pub fn init() {
    static START: std::sync::Once = std::sync::Once::new();
    START.call_once(|| {
        std::thread::spawn(hook_present);
    });
}
fn hook_present() {
    let d3d11 = loop {
        let m = unsafe { GetModuleHandleW(windows_sys::w!("d3d11.dll")) };
        if !m.is_null() {
            break m;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let Some(create) = (unsafe {
        GetProcAddress(
            d3d11,
            c"D3D11CreateDeviceAndSwapChain".as_ptr() as *const u8,
        )
    }) else {
        log!("dxgi: D3D11CreateDeviceAndSwapChain not found");
        return;
    };
    type Create = unsafe extern "system" fn(
        P,
        u32,
        P,
        u32,
        P,
        u32,
        u32,
        *const SwapChainDesc,
        *mut P,
        *mut P,
        *mut u32,
        *mut P,
    ) -> HRESULT;
    let create: Create = unsafe { std::mem::transmute(create) };
    let window = unsafe {
        CreateWindowExW(
            0,
            windows_sys::w!("STATIC"),
            windows_sys::w!("wal"),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            64,
            64,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null(),
        )
    };
    let desc = SwapChainDesc {
        width: 64,
        height: 64,
        refresh: [0, 0],
        format: 28,
        scanline: 0,
        scaling: 0,
        sample_count: 1,
        sample_quality: 0,
        usage: 0x20,
        buffer_count: 1,
        window,
        windowed: 1,
        swap_effect: 0,
        flags: 0,
    };
    let (mut swap, mut device, mut context): (P, P, P) = (
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
    );
    let hr = unsafe {
        create(
            std::ptr::null_mut(),
            1,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            0,
            7,
            &desc,
            &mut swap,
            &mut device,
            std::ptr::null_mut(),
            &mut context,
        )
    };
    if hr < 0 || swap.is_null() {
        log!("dxgi: probe swap chain failed ({hr:#x})");
        unsafe { DestroyWindow(window) };
        return;
    }
    let vtable = unsafe { *(swap as *const *mut usize) };
    for (index, hook, orig) in [
        (8, present as *const () as usize, &PRESENT),
        (22, present1 as *const () as usize, &PRESENT1),
    ] {
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
    log!("dxgi: native presentation hook enabled");
}

unsafe extern "system" fn present(swap: P, sync: u32, flags: u32) -> HRESULT {
    if flags & DXGI_PRESENT_TEST == 0 {
        compose(swap);
    }
    let orig: unsafe extern "system" fn(P, u32, u32) -> HRESULT =
        unsafe { std::mem::transmute(PRESENT.load(Ordering::Relaxed)) };
    let result = unsafe { orig(swap, sync, flags) };
    if flags & DXGI_PRESENT_TEST == 0 {
        restore(swap);
    }
    result
}

unsafe extern "system" fn present1(swap: P, sync: u32, flags: u32, params: P) -> HRESULT {
    if flags & DXGI_PRESENT_TEST == 0 {
        compose(swap);
    }
    let orig: unsafe extern "system" fn(P, u32, u32, P) -> HRESULT =
        unsafe { std::mem::transmute(PRESENT1.load(Ordering::Relaxed)) };
    let result = unsafe {
        orig(
            swap,
            sync,
            flags,
            if crate::border::config().is_some() {
                std::ptr::null_mut()
            } else {
                params
            },
        )
    };
    if flags & DXGI_PRESENT_TEST == 0 {
        restore(swap);
    }
    result
}

/// The drawing's objects, for one device: the images' shader resource views.
struct Resources {
    device: P,
    context1: P,
    state: P,
    vs: P,
    ps: P,
    blend: P,
    sampler: P,
    textures: Vec<(P, P)>,
    frame_size: (u32, u32, u32),
    saved: bool,
    backdrop: Option<(P, P)>,
    /// The frame's geometry logged for the current back buffer size.
    logged: bool,
}

unsafe impl Send for Resources {}

static RESOURCES: Mutex<Option<Resources>> = Mutex::new(None);

const SHADER: &str = r#"
Texture2D image : register(t0);
SamplerState linear_clamp : register(s0);
struct V { float4 pos : SV_Position; float2 uv : TEXCOORD0; };
// a triangle covering the viewport (the quad's place), its texture coordinates 0..1 there
V vs_main(uint id : SV_VertexID) {
    V v;
    float2 p = float2((id << 1) & 2, id & 2);
    v.pos = float4(p.x * 2 - 1, 1 - p.y * 2, 0, 1);
    v.uv = p;
    return v;
}
float4 ps_main(V v) : SV_Target {
    return image.Sample(linear_clamp, v.uv);
}
"#;

fn compile(entry: &std::ffi::CStr, target: &std::ffi::CStr) -> Option<Vec<u8>> {
    type Compile = unsafe extern "system" fn(
        *const u8,
        usize,
        *const i8,
        P,
        P,
        *const i8,
        *const i8,
        u32,
        u32,
        *mut P,
        *mut P,
    ) -> HRESULT;
    let module = unsafe { LoadLibraryW(windows_sys::w!("d3dcompiler_47.dll")) };
    let f = unsafe { GetProcAddress(module, c"D3DCompile".as_ptr() as *const u8) }?;
    let compile: Compile = unsafe { std::mem::transmute(f) };
    let (mut code, mut errors): (P, P) = (std::ptr::null_mut(), std::ptr::null_mut());
    let hr = unsafe {
        compile(
            SHADER.as_ptr(),
            SHADER.len(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            entry.as_ptr(),
            target.as_ptr(),
            0,
            0,
            &mut code,
            &mut errors,
        )
    };
    let blob = |b: P| -> Vec<u8> {
        let ptr = com!(b, 3, fn() -> *const u8);
        let len = com!(b, 4, fn() -> usize);
        unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec()
    };
    if hr < 0 || code.is_null() {
        if !errors.is_null() {
            log!(
                "images: shader {entry:?}: {}",
                String::from_utf8_lossy(&blob(errors))
            );
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
    fn new(device: P, context: P, _images: &[()]) -> Option<Resources> {
        let mut r = Resources {
            device,
            context1: std::ptr::null_mut(),
            state: std::ptr::null_mut(),
            vs: std::ptr::null_mut(),
            ps: std::ptr::null_mut(),
            blend: std::ptr::null_mut(),
            sampler: std::ptr::null_mut(),
            textures: Vec::new(),
            frame_size: (0, 0, 0),
            saved: false,
            backdrop: None,
            logged: false,
        };
        let mut device1: P = std::ptr::null_mut();
        if com!(
            device,
            0,
            fn(*const GUID, *mut P) -> HRESULT,
            &IID_DEVICE1,
            &mut device1
        ) < 0
        {
            return None;
        }
        let level = com!(device, 37, fn() -> u32);
        let hr = com!(
            device1,
            47,
            fn(u32, *const u32, u32, u32, *const GUID, *mut u32, *mut P) -> HRESULT,
            0,
            &level,
            1,
            7,
            &IID_DEVICE,
            std::ptr::null_mut(),
            &mut r.state
        );
        release(device1);
        if hr < 0
            || com!(
                context,
                0,
                fn(*const GUID, *mut P) -> HRESULT,
                &IID_CONTEXT1,
                &mut r.context1
            ) < 0
        {
            return None;
        }
        let vs = compile(c"vs_main", c"vs_4_0")?;
        let ps = compile(c"ps_main", c"ps_4_0")?;
        // D3D11_BLEND_DESC: render target 0 blended (src alpha, inverse src alpha), all written
        let mut blend = [0u32; 2 + 8 * 8];
        blend[2..10].copy_from_slice(&[0, 5, 6, 1, 2, 6, 1, 0xF]);
        // D3D11_SAMPLER_DESC: linear, clamped, comparison never, LOD 0..max
        let max = f32::MAX.to_bits();
        let sampler: [u32; 13] = [0x15, 3, 3, 3, 0, 1, 1, 0, 0, 0, 0, 0, max];
        if com!(
            device,
            12,
            fn(*const u8, usize, P, *mut P) -> HRESULT,
            vs.as_ptr(),
            vs.len(),
            std::ptr::null_mut(),
            &mut r.vs
        ) < 0
            || com!(
                device,
                15,
                fn(*const u8, usize, P, *mut P) -> HRESULT,
                ps.as_ptr(),
                ps.len(),
                std::ptr::null_mut(),
                &mut r.ps
            ) < 0
            || com!(
                device,
                20,
                fn(*const u32, *mut P) -> HRESULT,
                blend.as_ptr(),
                &mut r.blend
            ) < 0
            || com!(
                device,
                23,
                fn(*const u32, *mut P) -> HRESULT,
                sampler.as_ptr(),
                &mut r.sampler
            ) < 0
        {
            return None;
        }
        // Two solid pixels for the colored ring and its black interior.
        let rgb = crate::border::config().map_or(0xffffff, |b| b.rgb);
        for pixel in [
            0xff000000 | ((rgb & 255) << 16) | (rgb & 0xff00) | ((rgb >> 16) & 255),
            0xff000000,
        ] {
            let desc = Texture2DDesc {
                width: 1,
                height: 1,
                mip_levels: 1,
                array_size: 1,
                format: 28,
                sample_count: 1,
                bind_flags: 8,
                ..Default::default()
            };
            #[repr(C)]
            struct Data {
                ptr: *const u32,
                pitch: u32,
                slice: u32,
            }
            let data = Data {
                ptr: &pixel,
                pitch: 4,
                slice: 0,
            };
            let (mut tex, mut srv) = (std::ptr::null_mut(), std::ptr::null_mut());
            if com!(
                device,
                5,
                fn(*const Texture2DDesc, *const Data, *mut P) -> HRESULT,
                &desc,
                &data,
                &mut tex
            ) < 0
                || com!(
                    device,
                    7,
                    fn(P, P, *mut P) -> HRESULT,
                    tex,
                    std::ptr::null_mut(),
                    &mut srv
                ) < 0
            {
                release(tex);
                return None;
            }
            r.textures.push((tex, srv));
        }
        if let Some(image) = crate::border::backdrop() {
            let desc = Texture2DDesc {
                width: image.w,
                height: image.h,
                mip_levels: 1,
                array_size: 1,
                format: 87,
                sample_count: 1,
                bind_flags: 8,
                ..Default::default()
            };
            #[repr(C)]
            struct Data {
                ptr: *const u8,
                pitch: u32,
                slice: u32,
            }
            let data = Data {
                ptr: image.bgra.as_ptr(),
                pitch: image.w * 4,
                slice: 0,
            };
            let (mut tex, mut srv) = (std::ptr::null_mut(), std::ptr::null_mut());
            if com!(
                device,
                5,
                fn(*const Texture2DDesc, *const Data, *mut P) -> HRESULT,
                &desc,
                &data,
                &mut tex
            ) >= 0
                && com!(
                    device,
                    7,
                    fn(P, P, *mut P) -> HRESULT,
                    tex,
                    std::ptr::null_mut(),
                    &mut srv
                ) >= 0
            {
                r.backdrop = Some((tex, srv));
            } else {
                release(srv);
                release(tex);
            }
        }
        Some(r)
    }
}

impl Drop for Resources {
    fn drop(&mut self) {
        if let Some((tex, srv)) = self.backdrop {
            release(srv);
            release(tex);
        }
        for p in [
            self.context1,
            self.state,
            self.vs,
            self.ps,
            self.blend,
            self.sampler,
        ] {
            release(p);
        }
        for (tex, srv) in &self.textures {
            release(*srv);
            release(*tex);
        }
    }
}

struct Quad {
    image: usize,
    rect: [f32; 4],
}
fn draw(r: &Resources, context: P, back: P, (w, h): (f32, f32), quads: &[Quad]) {
    let mut rtv: P = std::ptr::null_mut();
    if com!(
        r.device,
        9,
        fn(P, P, *mut P) -> HRESULT,
        back,
        std::ptr::null_mut(),
        &mut rtv
    ) < 0
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| log!("images: no render target view of the back buffer"));
        return;
    }
    let mut previous: P = std::ptr::null_mut();
    com!(r.context1, 131, fn(P, *mut P) -> (), r.state, &mut previous);
    com!(
        context,
        33,
        fn(u32, *const P, P) -> (),
        1,
        &rtv,
        std::ptr::null_mut()
    );
    com!(
        context,
        35,
        fn(P, *const f32, u32) -> (),
        r.blend,
        std::ptr::null(),
        0xFFFF_FFFF
    );
    com!(context, 24, fn(u32) -> (), 4); // TRIANGLELIST
    com!(
        context,
        11,
        fn(P, P, u32) -> (),
        r.vs,
        std::ptr::null_mut(),
        0
    );
    com!(
        context,
        9,
        fn(P, P, u32) -> (),
        r.ps,
        std::ptr::null_mut(),
        0
    );
    com!(context, 10, fn(u32, u32, *const P) -> (), 0, 1, &r.sampler);
    for q in quads {
        let Some((_, srv)) = (if q.image == 3 {
            r.backdrop.as_ref()
        } else {
            r.textures.get(q.image)
        }) else {
            continue;
        };
        // the quad's place: the viewport (the triangle's texture coordinates 0..1 there)
        let viewport = [
            q.rect[0] * w,
            q.rect[1] * h,
            q.rect[2] * w,
            q.rect[3] * h,
            0.0,
            1.0,
        ];
        if viewport[2] <= 0.0 || viewport[3] <= 0.0 {
            continue;
        }
        com!(context, 44, fn(u32, *const f32) -> (), 1, viewport.as_ptr());
        com!(context, 8, fn(u32, u32, *const P) -> (), 0, 1, srv);
        com!(context, 13, fn(u32, u32) -> (), 3, 0);
    }
    let none: P = std::ptr::null_mut();
    com!(context, 8, fn(u32, u32, *const P) -> (), 0, 1, &none);
    com!(
        context,
        33,
        fn(u32, *const P, P) -> (),
        0,
        std::ptr::null(),
        std::ptr::null_mut()
    );
    com!(
        r.context1,
        131,
        fn(P, *mut P) -> (),
        previous,
        std::ptr::null_mut()
    );
    release(previous);
    release(rtv);
}

fn compose(swap: P) {
    // Copy callbacks before calling them: callbacks may initialize another overlay.
    let callbacks = CALLBACKS.lock().unwrap().clone();
    for callback in callbacks {
        callback(swap);
    }
    if crate::border::config().is_none() {
        return;
    }
    with_back(swap, |back, desc, device, context| {
        let mut guard = RESOURCES.lock().unwrap();
        if guard.as_ref().is_none_or(|r| r.device != device) {
            *guard = Resources::new(device, context, &[]);
        }
        let Some(r) = guard.as_mut() else { return };
        r.saved = false;
        let size = (desc.width, desc.height, desc.format);
        if r.frame_size != size {
            if r.textures.len() > 2 {
                let (tex, srv) = r.textures.pop().unwrap();
                release(srv);
                release(tex);
            }
            let copy = Texture2DDesc {
                width: desc.width,
                height: desc.height,
                mip_levels: 1,
                array_size: 1,
                format: desc.format,
                sample_count: 1,
                bind_flags: 8,
                ..Default::default()
            };
            let (mut tex, mut srv) = (std::ptr::null_mut(), std::ptr::null_mut());
            if com!(
                device,
                5,
                fn(*const Texture2DDesc, P, *mut P) -> HRESULT,
                &copy,
                std::ptr::null_mut(),
                &mut tex
            ) < 0
                || com!(
                    device,
                    7,
                    fn(P, P, *mut P) -> HRESULT,
                    tex,
                    std::ptr::null_mut(),
                    &mut srv
                ) < 0
            {
                release(tex);
                return;
            }
            r.textures.push((tex, srv));
            r.frame_size = size;
            r.logged = false;
        }
        let clean = r.textures[2].0;
        if desc.sample_count > 1 {
            com!(
                context,
                57,
                fn(P, u32, P, u32, u32) -> (),
                clean,
                0,
                back,
                0,
                desc.format
            );
        } else {
            com!(context, 47, fn(P, P) -> (), clean, back);
        }
        let Some((rect, bx, by, _)) = crate::border::geometry(desc.width, desc.height) else {
            return;
        };
        if !r.logged {
            r.logged = true;
            log!("sinden: back buffer {}x{} (format {}), picture {:?}", desc.width, desc.height, desc.format, rect);
        }
        let (w, h) = (desc.width as f32, desc.height as f32);
        let q = |image, [l, t, r, b]: [i32; 4]| Quad {
            image,
            rect: [
                l as f32 / w,
                t as f32 / h,
                (r - l) as f32 / w,
                (b - t) as f32 / h,
            ],
        };
        let mut quads = vec![Quad {
            image: 1,
            rect: [0.0, 0.0, 1.0, 1.0],
        }];
        let (ox, oy) = crate::border::outline(desc.width, desc.height);
        quads.extend(
            crate::border::strips(desc.width as i32, desc.height as i32, bx, by, ox, oy)
                .into_iter()
                .map(|rect| q(0, rect)),
        );
        if r.backdrop.is_some() {
            if let Some((outer, _, _, _)) = crate::border::outer_geometry(desc.width, desc.height) {
                quads.push(q(3, outer));
            }
        }
        quads.push(q(2, rect));
        draw(r, context, back, (w, h), &quads);
        r.saved = true;
    });
    crate::pointer_d3d11::capture(swap);
}

fn restore(swap: P) {
    if crate::border::config().is_none() {
        return;
    }
    with_back(swap, |back, desc, device, context| {
        let mut guard = RESOURCES.lock().unwrap();
        if let Some(r) = guard.as_mut().filter(|r| {
            r.saved && r.device == device && r.frame_size == (desc.width, desc.height, desc.format)
        }) {
            draw(
                r,
                context,
                back,
                (desc.width as f32, desc.height as f32),
                &[Quad {
                    image: 2,
                    rect: [0.0, 0.0, 1.0, 1.0],
                }],
            );
            r.saved = false;
        }
    });
}

fn with_back(swap: P, f: impl FnOnce(P, &Texture2DDesc, P, P)) {
    let mut back = std::ptr::null_mut();
    if com!(
        swap,
        9,
        fn(u32, *const GUID, *mut P) -> HRESULT,
        0,
        &IID_TEXTURE2D,
        &mut back
    ) < 0
        || back.is_null()
    {
        return;
    }
    let mut desc = Texture2DDesc::default();
    com!(back, 10, fn(*mut Texture2DDesc) -> (), &mut desc);
    let mut device = std::ptr::null_mut();
    com!(back, 3, fn(*mut P) -> (), &mut device);
    if !device.is_null() {
        let mut context = std::ptr::null_mut();
        com!(device, 40, fn(*mut P) -> (), &mut context);
        if !context.is_null() {
            f(back, &desc, device, context);
            release(context);
        }
        release(device);
    }
    release(back);
}
