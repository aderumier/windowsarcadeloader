//! D3D8 native frame: CopyRects into a cached render-target texture, then a fixed-function
//! textured quad in the inset. State, render targets and the clean frame are restored.
use std::ffi::c_void;
use std::sync::Mutex;
type P = *mut c_void;
macro_rules! com {
    ($obj:expr,$slot:expr,fn($($t:ty),*)->$ret:ty $(,$arg:expr)*) => {{
        let obj=$obj;
        let f:unsafe extern "system" fn(P $(,$t)*)->$ret=unsafe { std::mem::transmute(*(*(obj as *const *const usize)).add($slot)) };
        unsafe { f(obj $(,$arg)*) }
    }};
}
fn release(p: P) {
    if !p.is_null() {
        com!(p, 2, fn() -> u32);
    }
}
struct Frame {
    device: usize,
    size: (u32, u32, u32),
    texture: usize,
    surface: usize,
    bezel: usize,
}
static FRAME: Mutex<Option<Frame>> = Mutex::new(None);
pub fn reset() {
    if let Some(f) = FRAME.lock().unwrap().take() {
        release(f.surface as P);
        release(f.texture as P);
        release(f.bezel as P);
    }
}
#[repr(C)]
#[derive(Default)]
struct Desc {
    format: u32,
    kind: u32,
    usage: u32,
    pool: u32,
    size: u32,
    multi: u32,
    width: u32,
    height: u32,
}

pub fn begin(dev: P) -> bool {
    if crate::border::config().is_none() {
        return false;
    }
    let mut back = std::ptr::null_mut();
    if com!(dev, 16, fn(u32, u32, *mut P) -> i32, 0, 0, &mut back) < 0 {
        return false;
    }
    let mut d = Desc::default();
    if com!(back, 8, fn(*mut Desc) -> i32, &mut d) < 0 {
        release(back);
        return false;
    }
    let Some((rect, bx, by, rgb)) = crate::border::geometry(d.width, d.height) else {
        release(back);
        return false;
    };
    let mut state = FRAME.lock().unwrap();
    if state
        .as_ref()
        .is_some_and(|f| f.device != dev as usize || f.size != (d.width, d.height, d.format))
    {
        let f = state.take().unwrap();
        release(f.surface as P);
        release(f.texture as P);
        release(f.bezel as P);
    }
    if state.is_none() {
        let (mut texture, mut surface) = (std::ptr::null_mut(), std::ptr::null_mut());
        if com!(
            dev,
            20,
            fn(u32, u32, u32, u32, u32, u32, *mut P) -> i32,
            d.width,
            d.height,
            1,
            1,
            d.format,
            0,
            &mut texture
        ) < 0
        {
            release(back);
            return false;
        }
        if com!(texture, 15, fn(u32, *mut P) -> i32, 0, &mut surface) < 0 {
            release(texture);
            release(back);
            return false;
        }
        *state = Some(Frame {
            device: dev as usize,
            size: (d.width, d.height, d.format),
            texture: texture as usize,
            surface: surface as usize,
            bezel: make_bezel(dev) as usize,
        });
    }
    let f = state.as_ref().unwrap();
    if com!(
        dev,
        28,
        fn(P, P, u32, P, P) -> i32,
        back,
        std::ptr::null_mut(),
        0,
        f.surface as P,
        std::ptr::null_mut()
    ) < 0
    {
        release(back);
        return false;
    }
    let mut block = 0;
    if com!(dev, 57, fn(u32, *mut u32) -> i32, 1, &mut block) < 0 {
        release(back);
        return false;
    }
    let (mut target, mut depth) = (std::ptr::null_mut(), std::ptr::null_mut());
    com!(dev, 32, fn(*mut P) -> i32, &mut target);
    com!(dev, 33, fn(*mut P) -> i32, &mut depth);
    if com!(dev, 34, fn() -> i32) < 0 {
        com!(dev, 56, fn(u32) -> i32, block);
        release(target);
        release(depth);
        release(back);
        return false;
    }
    com!(dev, 31, fn(P, P) -> i32, back, std::ptr::null_mut());
    let viewport = [0, 0, d.width, d.height, 0f32.to_bits(), 1f32.to_bits()];
    com!(dev, 40, fn(*const u32) -> i32, viewport.as_ptr());
    // Z, alpha test/blend, fog, lighting off; no culling; all color channels written.
    for (key, value) in [
        (7, 0),
        (14, 0),
        (15, 0),
        (27, 0),
        (28, 0),
        (137, 0),
        (22, 1),
        (168, 15),
    ] {
        com!(dev, 50, fn(u32, u32) -> i32, key, value);
    }
    com!(
        dev,
        36,
        fn(u32, *const i32, u32, u32, f32, u32) -> i32,
        0,
        std::ptr::null(),
        1,
        0xff000000,
        1.0,
        0
    );
    let (ox, oy) = crate::border::outline(d.width, d.height);
    for strip in crate::border::strips(d.width as i32, d.height as i32, bx, by, ox, oy) {
        com!(
            dev,
            36,
            fn(u32, *const i32, u32, u32, f32, u32) -> i32,
            1,
            strip.as_ptr(),
            1,
            0xff000000 | rgb,
            1.0,
            0
        );
    }
    com!(dev, 76, fn(u32) -> i32, 0x104);
    com!(dev, 88, fn(u32) -> i32, 0);
    com!(dev, 61, fn(u32, P) -> i32, 0, f.texture as P);
    for (key, value) in [
        (1, 2),
        (2, 2),
        (4, 2),
        (5, 2),
        (13, 3),
        (14, 3),
        (16, 2),
        (17, 2),
    ] {
        com!(dev, 63, fn(u32, u32, u32) -> i32, 0, key, value);
    }
    com!(dev, 63, fn(u32, u32, u32) -> i32, 1, 1, 1); // stage 1 disabled
    if f.bezel != 0 {
        if let Some((outer, _, _, _)) = crate::border::outer_geometry(d.width, d.height) {
            com!(dev, 61, fn(u32, P) -> i32, 0, f.bezel as P);
            textured_quad(dev, outer);
            com!(dev, 61, fn(u32, P) -> i32, 0, f.texture as P);
        }
    }
    let [l, t, r, b] = rect.map(|v| v as f32 - 0.5);
    let vertices: [[f32; 6]; 4] = [
        [l, t, 0.0, 1.0, 0.0, 0.0],
        [r, t, 0.0, 1.0, 1.0, 0.0],
        [l, b, 0.0, 1.0, 0.0, 1.0],
        [r, b, 0.0, 1.0, 1.0, 1.0],
    ];
    let ok = com!(
        dev,
        72,
        fn(u32, u32, *const f32, u32) -> i32,
        5,
        2,
        vertices.as_ptr().cast(),
        24
    ) >= 0;
    com!(dev, 35, fn() -> i32);
    com!(dev, 31, fn(P, P) -> i32, target, depth);
    com!(dev, 54, fn(u32) -> i32, block);
    com!(dev, 56, fn(u32) -> i32, block);
    release(target);
    release(depth);
    release(back);
    ok
}
pub fn end(dev: P, saved: bool) {
    if !saved {
        return;
    }
    let mut back = std::ptr::null_mut();
    if com!(dev, 16, fn(u32, u32, *mut P) -> i32, 0, 0, &mut back) < 0 {
        return;
    }
    if let Some(f) = FRAME.lock().unwrap().as_ref() {
        com!(
            dev,
            28,
            fn(P, P, u32, P, P) -> i32,
            f.surface as P,
            std::ptr::null_mut(),
            0,
            back,
            std::ptr::null_mut()
        );
    }
    release(back);
}

fn textured_quad(dev: P, rect: [i32; 4]) {
    let [l, t, r, b] = rect.map(|v| v as f32 - 0.5);
    let v: [[f32; 6]; 4] = [
        [l, t, 0.0, 1.0, 0.0, 0.0],
        [r, t, 0.0, 1.0, 1.0, 0.0],
        [l, b, 0.0, 1.0, 0.0, 1.0],
        [r, b, 0.0, 1.0, 1.0, 1.0],
    ];
    com!(
        dev,
        72,
        fn(u32, u32, *const f32, u32) -> i32,
        5,
        2,
        v.as_ptr().cast(),
        24
    );
}
fn make_bezel(dev: P) -> P {
    let Some(image) = crate::border::backdrop() else {
        return std::ptr::null_mut();
    };
    let mut tex = std::ptr::null_mut();
    if com!(
        dev,
        20,
        fn(u32, u32, u32, u32, u32, u32, *mut P) -> i32,
        image.w,
        image.h,
        1,
        0,
        21,
        1,
        &mut tex
    ) < 0
    {
        return std::ptr::null_mut();
    }
    #[repr(C)]
    struct Locked {
        pitch: i32,
        bits: *mut u8,
    }
    let mut locked = Locked {
        pitch: 0,
        bits: std::ptr::null_mut(),
    };
    if com!(
        tex,
        16,
        fn(u32, *mut Locked, P, u32) -> i32,
        0,
        &mut locked,
        std::ptr::null_mut(),
        0
    ) < 0
    {
        release(tex);
        return std::ptr::null_mut();
    }
    for y in 0..image.h as usize {
        unsafe {
            std::ptr::copy_nonoverlapping(
                image.bgra[y * image.w as usize * 4..].as_ptr(),
                locked.bits.offset(y as isize * locked.pitch as isize),
                image.w as usize * 4,
            );
        }
    }
    com!(tex, 17, fn(u32) -> i32, 0);
    tex
}
