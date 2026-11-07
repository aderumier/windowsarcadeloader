//! Pictures drawn over a Direct3D 9 game's back buffer before its `Present`
//! (`screenshot::set_overlay`): the common crosshair, K-On!'s card picker.
//!
//! [`draw_over`] saves the game's states in a state block, renders into the back buffer
//! (pre-transformed textured quads, alpha blended, no depth buffer) and restores the game's
//! render target, depth buffer and states.

use std::ffi::c_void;

type P = *mut c_void;

const DEV_TEST_COOPERATIVE_LEVEL: usize = 3;
const DEV_GET_BACK_BUFFER: usize = 18;
const DEV_CREATE_TEXTURE: usize = 23;
const DEV_SET_RENDER_TARGET: usize = 37;
const DEV_GET_RENDER_TARGET: usize = 38;
const DEV_SET_DEPTH_STENCIL: usize = 39;
const DEV_GET_DEPTH_STENCIL: usize = 40;
const DEV_BEGIN_SCENE: usize = 41;
const DEV_END_SCENE: usize = 42;
const DEV_SET_VIEWPORT: usize = 47;
const DEV_SET_RENDER_STATE: usize = 57;
const DEV_CREATE_STATE_BLOCK: usize = 59;
const DEV_SET_TEXTURE: usize = 65;
const DEV_SET_TEXTURE_STAGE_STATE: usize = 67;
const DEV_SET_SAMPLER_STATE: usize = 69;
const DEV_DRAW_PRIMITIVE_UP: usize = 83;
const DEV_SET_FVF: usize = 89;
const DEV_SET_VERTEX_SHADER: usize = 92;
const DEV_SET_PIXEL_SHADER: usize = 107;
const SURF_GET_DESC: usize = 12;
const TEX_LOCK_RECT: usize = 19;
const TEX_UNLOCK_RECT: usize = 20;
const SB_CAPTURE: usize = 4;
const SB_APPLY: usize = 5;
const D3DFMT_A8R8G8B8: u32 = 21;
const D3DPOOL_MANAGED: u32 = 1;
const D3DSBT_ALL: u32 = 1;
const D3DTEXF_LINEAR: u32 = 2;
const D3DFVF_XYZRHW_TEX1: u32 = 0x104;
const D3DPT_TRIANGLESTRIP: u32 = 5;

/// A picture, BGRA.
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
}

unsafe fn method(obj: P, slot: usize) -> usize {
    unsafe { *(*(obj as *const *const usize)).add(slot) }
}

/// Releases a COM object (null: nothing).
pub unsafe fn release(obj: P) {
    if !obj.is_null() {
        let f: unsafe extern "system" fn(P) -> u32 = unsafe { std::mem::transmute(method(obj, 2)) };
        unsafe { f(obj) };
    }
}

/// Whether the device is lost (the game resets it): draw nothing, keep nothing of it (a state
/// block holds the game's resources: the game's `Reset` would fail again and again).
pub unsafe fn lost(dev: P) -> bool {
    let test: unsafe extern "system" fn(P) -> i32 = unsafe { std::mem::transmute(method(dev, DEV_TEST_COOPERATIVE_LEVEL)) };
    unsafe { test(dev) != 0 }
}

/// A state block of all the device's states (null: none), for [`draw_over`]; released before
/// the device's resets.
pub unsafe fn state_block(dev: P) -> P {
    let create: unsafe extern "system" fn(P, u32, *mut P) -> i32 = unsafe { std::mem::transmute(method(dev, DEV_CREATE_STATE_BLOCK)) };
    let mut states: P = std::ptr::null_mut();
    if unsafe { create(dev, D3DSBT_ALL, &mut states) } < 0 {
        return std::ptr::null_mut();
    }
    states
}

/// A managed A8R8G8B8 texture of `image` (null: none).
pub unsafe fn texture(dev: P, image: &Image) -> P {
    #[repr(C)]
    struct Locked {
        pitch: i32,
        bits: *mut u8,
    }
    unsafe {
        let create: unsafe extern "system" fn(P, u32, u32, u32, u32, u32, u32, *mut P, P) -> i32 = std::mem::transmute(method(dev, DEV_CREATE_TEXTURE));
        let mut tex: P = std::ptr::null_mut();
        if create(dev, image.w, image.h, 1, 0, D3DFMT_A8R8G8B8, D3DPOOL_MANAGED, &mut tex, std::ptr::null_mut()) < 0 || tex.is_null() {
            return std::ptr::null_mut();
        }
        let mut locked = Locked { pitch: 0, bits: std::ptr::null_mut() };
        let lock: unsafe extern "system" fn(P, u32, *mut Locked, P, u32) -> i32 = std::mem::transmute(method(tex, TEX_LOCK_RECT));
        if lock(tex, 0, &mut locked, std::ptr::null_mut(), 0) < 0 {
            release(tex);
            return std::ptr::null_mut();
        }
        let row = image.w as usize * 4;
        for y in 0..image.h as usize {
            std::ptr::copy_nonoverlapping(image.bgra[y * row..].as_ptr(), locked.bits.offset(y as isize * locked.pitch as isize), row);
        }
        let unlock: unsafe extern "system" fn(P, u32) -> i32 = std::mem::transmute(method(tex, TEX_UNLOCK_RECT));
        unlock(tex, 0);
        tex
    }
}

/// Runs `draw(width, height)` rendering into the back buffer, the game's states saved in
/// `states` ([`state_block`]) and restored after.
pub unsafe fn draw_over(dev: P, states: P, draw: impl FnOnce(u32, u32)) {
    unsafe {
        let get_back_buffer: unsafe extern "system" fn(P, u32, u32, u32, *mut P) -> i32 = std::mem::transmute(method(dev, DEV_GET_BACK_BUFFER));
        let mut back: P = std::ptr::null_mut();
        if get_back_buffer(dev, 0, 0, 0, &mut back) < 0 || back.is_null() {
            return;
        }
        draw_on(dev,states,back,draw);
        release(back);
    }
}

/// Draws into a borrowed presentation surface (a compositor's additional swap chain).
pub unsafe fn draw_on(dev:P,states:P,back:P,draw:impl FnOnce(u32,u32)) {
    unsafe {
        // D3DSURFACE_DESC: Format, Type, Usage, Pool, MultiSampleType, MultiSampleQuality, Width, Height
        let mut desc = [0u32; 8];
        let get_desc: unsafe extern "system" fn(P, *mut u32) -> i32 = std::mem::transmute(method(back, SURF_GET_DESC));
        get_desc(back, desc.as_mut_ptr());
        let (w, h) = (desc[6], desc[7]);

        let capture: unsafe extern "system" fn(P) -> i32 = std::mem::transmute(method(states, SB_CAPTURE));
        let apply: unsafe extern "system" fn(P) -> i32 = std::mem::transmute(method(states, SB_APPLY));
        capture(states);
        let get_rt: unsafe extern "system" fn(P, u32, *mut P) -> i32 = std::mem::transmute(method(dev, DEV_GET_RENDER_TARGET));
        let set_rt: unsafe extern "system" fn(P, u32, P) -> i32 = std::mem::transmute(method(dev, DEV_SET_RENDER_TARGET));
        let get_ds: unsafe extern "system" fn(P, *mut P) -> i32 = std::mem::transmute(method(dev, DEV_GET_DEPTH_STENCIL));
        let set_ds: unsafe extern "system" fn(P, P) -> i32 = std::mem::transmute(method(dev, DEV_SET_DEPTH_STENCIL));
        let (mut old_rt, mut old_ds): (P, P) = (std::ptr::null_mut(), std::ptr::null_mut());
        get_rt(dev, 0, &mut old_rt);
        get_ds(dev, &mut old_ds);
        set_rt(dev, 0, back);
        set_ds(dev, std::ptr::null_mut());
        prepare(dev, w, h);
        let begin: unsafe extern "system" fn(P) -> i32 = std::mem::transmute(method(dev, DEV_BEGIN_SCENE));
        let end: unsafe extern "system" fn(P) -> i32 = std::mem::transmute(method(dev, DEV_END_SCENE));
        begin(dev);
        draw(w, h);
        end(dev);
        set_rt(dev, 0, old_rt);
        set_ds(dev, old_ds);
        release(old_rt);
        release(old_ds);
        apply(states);
    }
}

/// Fixed-function states of the quads: pre-transformed textured vertices, alpha blending, no
/// depth, linear sampling.
unsafe fn prepare(dev: P, w: u32, h: u32) {
    unsafe {
        let rs: unsafe extern "system" fn(P, u32, u32) -> i32 = std::mem::transmute(method(dev, DEV_SET_RENDER_STATE));
        let tss: unsafe extern "system" fn(P, u32, u32, u32) -> i32 = std::mem::transmute(method(dev, DEV_SET_TEXTURE_STAGE_STATE));
        let ss: unsafe extern "system" fn(P, u32, u32, u32) -> i32 = std::mem::transmute(method(dev, DEV_SET_SAMPLER_STATE));
        let set_vs: unsafe extern "system" fn(P, P) -> i32 = std::mem::transmute(method(dev, DEV_SET_VERTEX_SHADER));
        let set_ps: unsafe extern "system" fn(P, P) -> i32 = std::mem::transmute(method(dev, DEV_SET_PIXEL_SHADER));
        let set_fvf: unsafe extern "system" fn(P, u32) -> i32 = std::mem::transmute(method(dev, DEV_SET_FVF));
        let set_viewport: unsafe extern "system" fn(P, *const u32) -> i32 = std::mem::transmute(method(dev, DEV_SET_VIEWPORT));
        let viewport: [u32; 6] = [0, 0, w, h, 0, 1.0f32.to_bits()];
        set_viewport(dev, viewport.as_ptr());
        set_vs(dev, std::ptr::null_mut());
        set_ps(dev, std::ptr::null_mut());
        set_fvf(dev, D3DFVF_XYZRHW_TEX1);
        // ZENABLE, FILLMODE solid, ZWRITEENABLE, ALPHATESTENABLE, SRCBLEND srcalpha, DESTBLEND
        // invsrcalpha, CULLMODE none, ALPHABLENDENABLE, FOGENABLE, STENCILENABLE, CLIPPING,
        // LIGHTING, COLORWRITEENABLE, BLENDOP add, SCISSORTESTENABLE, SRGBWRITEENABLE,
        // SEPARATEALPHABLENDENABLE
        for (state, value) in [(7, 0), (8, 3), (14, 0), (15, 0), (19, 5), (20, 6), (22, 1), (27, 1), (28, 0), (52, 0), (136, 1), (137, 0), (168, 0xF), (171, 1), (174, 0), (194, 0), (206, 0)] {
            rs(dev, state, value);
        }
        // stage 0: texture color and alpha; stage 1 off
        for (state, value) in [(1, 2), (2, 2), (4, 2), (5, 2)] {
            tss(dev, 0, state, value);
        }
        tss(dev, 1, 1, 1);
        tss(dev, 1, 4, 1);
        // clamp, linear, no mipmaps
        for (state, value) in [(1, 3), (2, 3), (5, D3DTEXF_LINEAR), (6, D3DTEXF_LINEAR), (7, 0)] {
            ss(dev, 0, state, value);
        }
    }
}

/// Draws `tex` over `x, y, w, h` (in [`draw_over`]).
pub unsafe fn quad(dev: P, tex: P, (x, y, w, h): (f32, f32, f32, f32)) {
    let (x0, y0, x1, y1) = (x - 0.5, y - 0.5, x + w - 0.5, y + h - 0.5);
    let v: [f32; 24] = [
        x0, y0, 0.0, 1.0, 0.0, 0.0, //
        x1, y0, 0.0, 1.0, 1.0, 0.0, //
        x0, y1, 0.0, 1.0, 0.0, 1.0, //
        x1, y1, 0.0, 1.0, 1.0, 1.0,
    ];
    unsafe {
        let set_texture: unsafe extern "system" fn(P, u32, P) -> i32 = std::mem::transmute(method(dev, DEV_SET_TEXTURE));
        let draw_up: unsafe extern "system" fn(P, u32, u32, *const f32, u32) -> i32 = std::mem::transmute(method(dev, DEV_DRAW_PRIMITIVE_UP));
        set_texture(dev, 0, tex);
        draw_up(dev, D3DPT_TRIANGLESTRIP, 2, v.as_ptr(), 24);
    }
}
