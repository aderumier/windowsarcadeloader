//! NVIDIA-only OpenGL on other GPUs (KanColle Arcade: its renderer draws with NVIDIA's bindless
//! extensions, unchecked; Mesa has none of them).
//!
//! `WAL_NVGL=auto|1|0` (`auto`: only without an NVIDIA GPU, from the Linux side's
//! `/sys/class/drm/*/device/vendor`). The game's `wglGetProcAddress` and `GetProcAddress`
//! (opengl32's functions) hand it this module's functions:
//! * `NV_shader_buffer_load` / `NV_vertex_buffer_unified_memory`: a buffer's GPU address is its
//!   name and an offset (`name << 32 | offset`). The addresses given to the vertex bindings,
//!   the element array, the uniform buffer bindings and the indirect draws bind those buffers
//!   (with the strides the game gave to its bindings without buffer); element draws add the
//!   element address' offset to their indices.
//! * `NV_command_list`: state objects capture the GL state ([`State`]); the token streams
//!   (the game writes them in mapped buffers) are read on the CPU and replayed as GL calls,
//!   the state of each sequence applied first, the GL state restored after.
//! * Bindless indirect draws: their commands read on the CPU, drawn one by one.
//! * `NV_bindless_texture` as `ARB_bindless_texture`; `glTextureBarrierNV`, `glColorMaskIndexedEXT`
//!   as their core functions.
//! * The GLSL sources: `NV_gpu_shader5` as `ARB_gpu_shader5` + `ARB_gpu_shader_int64` +
//!   `ARB_bindless_texture`, `NV_command_list`'s qualifiers dropped ([`shader_source`]).
//!
//! `WAL_NVGL_LOG=1`: the functions asked, the first calls of each translation, the shaders
//! that do not compile (their source in `nvgl\` of the working directory). `WAL_NVGL_LOG=2`
//! with `WAL_NVGL_SHOT`: the two frames after each shot taken too, logged call by call.

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering::Relaxed};

use crate::{iat, log};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA};

static ORIG_WGL: AtomicUsize = AtomicUsize::new(0);
static ORIG_GPA: AtomicUsize = AtomicUsize::new(0);
static OPENGL32: AtomicUsize = AtomicUsize::new(0);
static LOG: AtomicBool = AtomicBool::new(false);

/// An NVIDIA GPU on the machine (PCI vendor 0x10de).
fn nvidia() -> bool {
    let Ok(dir) = std::fs::read_dir("Z:\\sys\\class\\drm") else { return false };
    dir.flatten().any(|e| std::fs::read_to_string(e.path().join("device").join("vendor")).is_ok_and(|v| v.trim() == "0x10de"))
}

/// Installs the translation when `WAL_NVGL` asks for it.
pub fn init() {
    let on = match std::env::var("WAL_NVGL").unwrap_or_default().trim() {
        "auto" => !nvidia(),
        "1" => true,
        _ => false,
    };
    if !on {
        return;
    }
    LOG.store(std::env::var("WAL_NVGL_LOG").is_ok_and(|v| v.trim() == "1"), Relaxed);
    unsafe {
        let module = LoadLibraryA(c"opengl32.dll".as_ptr().cast());
        OPENGL32.store(module as usize, Relaxed);
        let wgl = GetProcAddress(module, c"wglGetProcAddress".as_ptr().cast());
        ORIG_WGL.store(wgl.map_or(0, |f| f as usize), Relaxed);
        iat::hook("opengl32.dll", "wglGetProcAddress", wgl_get_proc as *const () as usize);
        match iat::hook("kernel32.dll", "GetProcAddress", get_proc as *const () as usize) {
            Some(o) => ORIG_GPA.store(o, Relaxed),
            None => ORIG_GPA.store(GetProcAddress(GetModuleHandleA(c"kernel32.dll".as_ptr().cast()), c"GetProcAddress".as_ptr().cast()).map_or(0, |f| f as usize), Relaxed),
        }
    }
    if let Some(secs) = std::env::var("WAL_NVGL_SHOT").ok().and_then(|v| v.trim().parse::<f64>().ok()) {
        SHOT_MS.store((secs * 1000.0).max(1.0) as usize, Relaxed);
        if let Some(o) = unsafe { iat::hook("gdi32.dll", "SwapBuffers", swap_buffers as *const () as usize) } {
            ORIG_SWAP.store(o, Relaxed);
        }
    }
    log!("nvgl: NVIDIA OpenGL extensions translated");
}

static SHOT_MS: AtomicUsize = AtomicUsize::new(0);
/// The draws since the last shot (`WAL_NVGL_SHOT`).
static DRAWS: AtomicUsize = AtomicUsize::new(0);
static ORIG_SWAP: AtomicUsize = AtomicUsize::new(0);

/// `WAL_NVGL_SHOT=<seconds>` (debugging): the back buffer every `<seconds>` as
/// `nvglshot-NNNN.bmp` in the working directory.
unsafe extern "system" fn swap_buffers(hdc: *mut c_void) -> i32 {
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    static FRAMES: AtomicUsize = AtomicUsize::new(0);
    FRAMES.fetch_add(1, Relaxed);
    // `WAL_NVGL_LOG=2`: the two frames after each shot taken and logged call by call too
    static BURST: AtomicUsize = AtomicUsize::new(0);
    let burst = BURST.load(Relaxed);
    let due = burst > 0 || {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        let now = std::time::Instant::now();
        let due = last.is_none_or(|l| now.duration_since(l).as_millis() as usize >= SHOT_MS.load(Relaxed));
        if due {
            *last = Some(now);
        }
        due
    };
    if std::env::var("WAL_NVGL_LOG").is_ok_and(|v| v.trim() == "2") && due {
        let next = if burst > 0 { burst - 1 } else { 2 };
        BURST.store(next, Relaxed);
        FRAME_LOG.store(next > 0, Relaxed);
        log!("nvgl: frame {} ----", FRAMES.load(Relaxed));
    }
    if due {
        let mut vp = [0i32; 4];
        unsafe { gl::glGetIntegerv(0x0BA2, vp.as_mut_ptr()) };
        let window = unsafe { windows_sys::Win32::Graphics::Gdi::WindowFromDC(hdc as _) };
        let mut rect = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::RECT>() };
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(window, &mut rect) };
        let (w, h) = ((rect.right - rect.left).max(1), (rect.bottom - rect.top).max(1));
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        unsafe {
            gl::glBindFramebuffer(0x8CA8, 0);
            gl::glReadBuffer(0x0405);
            gl::glReadPixels(0, 0, w, h, 0x80E1, 0x1401, pixels.as_mut_ptr().cast());
        }
        let row = (w * 4) as usize;
        let flipped: Vec<u8> = pixels.chunks(row).rev().flatten().copied().collect();
        let n = COUNT.fetch_add(1, Relaxed);
        let path = format!("nvglshot-{n:04}.bmp");
        let r = crate::screenshot::write_bmp(&path, w as u32, h as u32, &flipped);
        log!("nvgl: {path} {w}x{h} ({} frames, {} draws since) {r:?}", FRAMES.load(Relaxed), DRAWS.swap(0, Relaxed));
    }
    let orig: unsafe extern "system" fn(*mut c_void) -> i32 = unsafe { std::mem::transmute(ORIG_SWAP.load(Relaxed)) };
    unsafe { orig(hdc) }
}

/// The driver's function `name`: `wglGetProcAddress`'s, else opengl32's export.
unsafe fn real_proc(name: &CStr) -> usize {
    unsafe {
        let wgl: unsafe extern "system" fn(*const c_char) -> usize = std::mem::transmute(ORIG_WGL.load(Relaxed));
        let f = wgl(name.as_ptr());
        if f > 3 && f != usize::MAX {
            return f;
        }
        GetProcAddress(OPENGL32.load(Relaxed) as _, name.as_ptr().cast()).map_or(0, |f| f as usize)
    }
}

unsafe extern "system" fn wgl_get_proc(name: *const c_char) -> usize {
    if name.is_null() {
        return 0;
    }
    let name = unsafe { CStr::from_ptr(name) };
    if let Some(f) = unsafe { replacement(name.to_bytes()) } {
        return f;
    }
    let f = unsafe { real_proc(name) };
    if f == 0 && LOG.load(Relaxed) {
        log!("nvgl: {} absent", name.to_string_lossy());
    }
    f
}

unsafe extern "system" fn get_proc(module: *mut c_void, name: *const c_char) -> usize {
    let orig: unsafe extern "system" fn(*mut c_void, *const c_char) -> usize = unsafe { std::mem::transmute(ORIG_GPA.load(Relaxed)) };
    if module as usize == OPENGL32.load(Relaxed) && name as usize > 0xffff {
        // its own wglGetProcAddress (KanColle Arcade's movie player: its WGL_NV_DX_interop
        // functions from it)
        if unsafe { CStr::from_ptr(name) }.to_bytes() == b"wglGetProcAddress" {
            return wgl_get_proc as *const () as usize;
        }
        if let Some(f) = unsafe { replacement(CStr::from_ptr(name).to_bytes()) } {
            return f;
        }
    }
    unsafe { orig(module, name) }
}

/// This module's function for `name`.
unsafe fn replacement(name: &[u8]) -> Option<usize> {
    // the same functions under their ARB / core names
    let alias: Option<&CStr> = match name {
        b"glIsTextureHandleResidentNV" => Some(c"glIsTextureHandleResidentARB"),
        b"glGetImageHandleNV" => Some(c"glGetImageHandleARB"),
        b"glMakeImageHandleResidentNV" => Some(c"glMakeImageHandleResidentARB"),
        b"glMakeImageHandleNonResidentNV" => Some(c"glMakeImageHandleNonResidentARB"),
        b"glIsImageHandleResidentNV" => Some(c"glIsImageHandleResidentARB"),
        b"glTextureBarrierNV" => Some(c"glTextureBarrier"),
        b"glColorMaskIndexedEXT" => Some(c"glColorMaski"),
        _ => None,
    };
    if let Some(alias) = alias {
        return Some(unsafe { real_proc(alias) });
    }
    let f: usize = match name {
        b"glEnable" => hook_enable as *const () as usize,
        b"glDisable" => hook_disable as *const () as usize,
        b"glIsEnabled" => hook_is_enabled as *const () as usize,
        b"glEnableClientState" => hook_enable_client_state as *const () as usize,
        b"glDisableClientState" => hook_disable_client_state as *const () as usize,
        b"glEnableVertexArrayEXT" => hook_enable_vertex_array as *const () as usize,
        b"glDisableVertexArrayEXT" => hook_disable_vertex_array as *const () as usize,
        b"glNamedBufferStorage" => hook_named_buffer_storage as *const () as usize,
        b"glNamedBufferData" => hook_named_buffer_data as *const () as usize,
        b"glMapNamedBufferRange" => hook_map_named_buffer_range as *const () as usize,
        b"glFlushMappedNamedBufferRange" => hook_flush_mapped_named_buffer_range as *const () as usize,
        b"wglDXSetResourceShareHandleNV" => dx_set_resource_share_handle as *const () as usize,
        b"wglDXOpenDeviceNV" => dx_open_device as *const () as usize,
        b"wglDXCloseDeviceNV" => dx_close_device as *const () as usize,
        b"wglDXRegisterObjectNV" => dx_register_object as *const () as usize,
        b"wglDXUnregisterObjectNV" => dx_unregister_object as *const () as usize,
        b"wglDXObjectAccessNV" => dx_object_access as *const () as usize,
        b"wglDXLockObjectsNV" => dx_lock_objects as *const () as usize,
        b"wglDXUnlockObjectsNV" => dx_unlock_objects as *const () as usize,
        b"glFenceSync" => hook_fence_sync as *const () as usize,
        b"glClientWaitSync" => hook_client_wait_sync as *const () as usize,
        b"glMapNamedBuffer" => hook_map_named_buffer as *const () as usize,
        b"glUnmapNamedBuffer" => hook_unmap_named_buffer as *const () as usize,
        b"glDeleteBuffers" => hook_delete_buffers as *const () as usize,
        b"glGetNamedBufferParameterui64vNV" => hook_get_named_buffer_parameter_ui64 as *const () as usize,
        b"glMakeNamedBufferResidentNV" | b"glMakeBufferResidentNV" => hook_make_resident as *const () as usize,
        b"glMakeNamedBufferNonResidentNV" | b"glMakeBufferNonResidentNV" => hook_make_non_resident as *const () as usize,
        b"glBufferAddressRangeNV" => hook_buffer_address_range as *const () as usize,
        b"glBindVertexArray" => hook_bind_vertex_array as *const () as usize,
        b"glVertexArrayVertexBuffer" => hook_vertex_array_vertex_buffer as *const () as usize,
        b"glBindVertexBuffer" => hook_bind_vertex_buffer as *const () as usize,
        b"glDrawElements" => hook_draw_elements as *const () as usize,
        b"glDrawElementsBaseVertex" => hook_draw_elements_base_vertex as *const () as usize,
        b"glDrawElementsInstanced" => hook_draw_elements_instanced as *const () as usize,
        b"glDrawRangeElements" => hook_draw_range_elements as *const () as usize,
        b"glMultiDrawElementsBaseVertex" => hook_multi_draw_elements_base_vertex as *const () as usize,
        b"glMultiDrawElementsIndirect" => hook_multi_draw_elements_indirect as *const () as usize,
        b"glMultiDrawElementsIndirectBindlessNV" => hook_multi_draw_elements_indirect_bindless as *const () as usize,
        b"glCreateStatesNV" => hook_create_states as *const () as usize,
        b"glDeleteStatesNV" => hook_delete_states as *const () as usize,
        b"glStateCaptureNV" => hook_state_capture as *const () as usize,
        b"glGetCommandHeaderNV" => hook_get_command_header as *const () as usize,
        b"glGetStageIndexNV" => hook_get_stage_index as *const () as usize,
        b"glDrawCommandsStatesAddressNV" => hook_draw_commands_states_address as *const () as usize,
        b"glDrawCommandsAddressNV" => hook_draw_commands_address as *const () as usize,
        b"glShaderSource" => hook_shader_source as *const () as usize,
        b"glProgramUniformHandleui64vNV" | b"glProgramUniformHandleui64vARB" => hook_program_uniform_handles as *const () as usize,
        b"glProgramUniformHandleui64NV" | b"glProgramUniformHandleui64ARB" => hook_program_uniform_handle as *const () as usize,
        b"glUniformHandleui64vNV" | b"glUniformHandleui64vARB" => hook_uniform_handles as *const () as usize,
        b"glUniformHandleui64NV" | b"glUniformHandleui64ARB" => hook_uniform_handle as *const () as usize,
        b"glGetTextureHandleNV" | b"glGetTextureHandleARB" => hook_get_texture_handle as *const () as usize,
        b"glGetTextureSamplerHandleNV" | b"glGetTextureSamplerHandleARB" => hook_get_texture_sampler_handle as *const () as usize,
        b"glMakeTextureHandleResidentNV" | b"glMakeTextureHandleResidentARB" => hook_make_handle_resident as *const () as usize,
        b"glMakeTextureHandleNonResidentNV" | b"glMakeTextureHandleNonResidentARB" => hook_make_handle_non_resident as *const () as usize,
        b"glCompileShader" => hook_compile_shader as *const () as usize,
        b"glLinkProgram" => hook_link_program as *const () as usize,
        b"glUseProgram" => hook_use_program as *const () as usize,
        b"glBindBufferRange" => hook_bind_buffer_range as *const () as usize,
        b"glBindBufferBase" => hook_bind_buffer_base as *const () as usize,
        b"glDrawArrays" => hook_draw_arrays as *const () as usize,
        b"glDrawArraysInstanced" => hook_draw_arrays_instanced as *const () as usize,
        b"glMultiDrawArrays" => hook_multi_draw_arrays as *const () as usize,
        _ => return None,
    };
    Some(f)
}

/// The driver's functions, looked up on first use.
#[allow(non_snake_case, clippy::too_many_arguments)]
mod gl {
    use std::ffi::{CStr, c_void};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

    macro_rules! real {
        ($($name:ident($($a:ident: $t:ty),*) $(-> $r:ty)?;)*) => { $(
            pub unsafe fn $name($($a: $t),*) $(-> $r)? {
                static P: AtomicUsize = AtomicUsize::new(0);
                let mut p = P.load(Relaxed);
                if p == 0 {
                    p = unsafe { super::real_proc(CStr::from_bytes_with_nul(concat!(stringify!($name), "\0").as_bytes()).unwrap()) };
                    if p == 0 {
                        crate::log!("nvgl: {} absent from the driver", stringify!($name));
                    }
                    P.store(p, Relaxed);
                }
                let f: unsafe extern "system" fn($($t),*) $(-> $r)? = unsafe { std::mem::transmute(p) };
                unsafe { f($($a),*) }
            }
        )* };
    }

    real! {
        glEnable(cap: u32);
        glDisable(cap: u32);
        glIsEnabled(cap: u32) -> u8;
        glEnableClientState(cap: u32);
        glDisableClientState(cap: u32);
        glEnableVertexArrayEXT(vao: u32, array: u32);
        glDisableVertexArrayEXT(vao: u32, array: u32);
        glGetIntegerv(pname: u32, data: *mut i32);
        glGetFloatv(pname: u32, data: *mut f32);
        glGetBooleanv(pname: u32, data: *mut u8);
        glNamedBufferStorage(buffer: u32, size: isize, data: *const c_void, flags: u32);
        glNamedBufferData(buffer: u32, size: isize, data: *const c_void, usage: u32);
        glMapNamedBufferRange(buffer: u32, offset: isize, length: isize, access: u32) -> *mut c_void;
        glMapNamedBuffer(buffer: u32, access: u32) -> *mut c_void;
        glUnmapNamedBuffer(buffer: u32) -> u8;
        glDeleteBuffers(n: i32, buffers: *const u32);
        glGetNamedBufferSubData(buffer: u32, offset: isize, size: isize, data: *mut c_void);
        glBindVertexArray(vao: u32);
        glVertexArrayVertexBuffer(vao: u32, binding: u32, buffer: u32, offset: isize, stride: i32);
        glBindVertexBuffer(binding: u32, buffer: u32, offset: isize, stride: i32);
        glBindBuffer(target: u32, buffer: u32);
        glBindBufferRange(target: u32, index: u32, buffer: u32, offset: isize, size: isize);
        glBindBufferBase(target: u32, index: u32, buffer: u32);
        glBindFramebuffer(target: u32, fbo: u32);
        glUseProgram(program: u32);
        glDrawArrays(mode: u32, first: i32, count: i32);
        glDrawArraysInstancedBaseInstance(mode: u32, first: i32, count: i32, instances: i32, base_instance: u32);
        glDrawElements(mode: u32, count: i32, kind: u32, indices: usize);
        glDrawElementsBaseVertex(mode: u32, count: i32, kind: u32, indices: usize, base_vertex: i32);
        glDrawElementsInstanced(mode: u32, count: i32, kind: u32, indices: usize, instances: i32);
        glDrawRangeElements(mode: u32, start: u32, end: u32, count: i32, kind: u32, indices: usize);
        glMultiDrawElementsBaseVertex(mode: u32, counts: *const i32, kind: u32, indices: *const usize, draws: i32, base_vertices: *const i32);
        glMultiDrawElementsIndirect(mode: u32, kind: u32, indirect: usize, draws: i32, stride: i32);
        glDrawElementsInstancedBaseVertexBaseInstance(mode: u32, count: i32, kind: u32, indices: usize, instances: i32, base_vertex: i32, base_instance: u32);
        glDepthFunc(func: u32);
        glDepthMask(flag: u8);
        glBlendFuncSeparate(src_rgb: u32, dst_rgb: u32, src_alpha: u32, dst_alpha: u32);
        glBlendEquationSeparate(rgb: u32, alpha: u32);
        glBlendColor(r: f32, g: f32, b: f32, a: f32);
        glColorMask(r: u8, g: u8, b: u8, a: u8);
        glCullFace(mode: u32);
        glFrontFace(mode: u32);
        glPolygonOffset(factor: f32, units: f32);
        glPolygonMode(face: u32, mode: u32);
        glStencilFuncSeparate(face: u32, func: u32, reference: i32, mask: u32);
        glStencilOpSeparate(face: u32, fail: u32, zfail: u32, zpass: u32);
        glStencilMaskSeparate(face: u32, mask: u32);
        glLineWidth(width: f32);
        glViewport(x: i32, y: i32, w: i32, h: i32);
        glScissor(x: i32, y: i32, w: i32, h: i32);
        glShaderSource(shader: u32, count: i32, strings: *const *const u8, lengths: *const i32);
        glCompileShader(shader: u32);
        glLinkProgram(program: u32);
        glReadBuffer(mode: u32);
        glGetError() -> u32;
        glFinish();
        glGetTextureHandleARB(texture: u32) -> u64;
        glGetTextureSamplerHandleARB(texture: u32, sampler: u32) -> u64;
        glMakeTextureHandleResidentARB(handle: u64);
        glMakeTextureHandleNonResidentARB(handle: u64);
        glIsTexture(texture: u32) -> u8;
        glGetTextureParameteriv(texture: u32, pname: u32, params: *mut i32);
        glGetProgramInterfaceiv(program: u32, interface: u32, pname: u32, params: *mut i32);
        glGetProgramResourceName(program: u32, interface: u32, index: u32, size: i32, length: *mut i32, name: *mut u8);
        glGetAttachedShaders(program: u32, max: i32, count: *mut i32, shaders: *mut u32);
        glProgramUniformHandleui64vARB(program: u32, location: i32, count: i32, values: *const u64);
        glProgramUniform2uiv(program: u32, location: i32, count: i32, values: *const u32);
        glProgramUniform1i(program: u32, location: i32, value: i32);
        glFlushMappedNamedBufferRange(buffer: u32, offset: isize, length: isize);
        glBindTexture(target: u32, texture: u32);
        glTexImage2D(target: u32, level: i32, internal: i32, w: i32, h: i32, border: i32, format: u32, kind: u32, data: *const c_void);
        glTextureSubImage2D(texture: u32, level: i32, x: i32, y: i32, w: i32, h: i32, format: u32, kind: u32, data: *const c_void);
        glPixelStorei(pname: u32, value: i32);
        glTexParameteri(target: u32, pname: u32, value: i32);
        glBindRenderbuffer(target: u32, renderbuffer: u32);
        glRenderbufferStorage(target: u32, internal: u32, w: i32, h: i32);
        glFenceSync(condition: u32, flags: u32) -> usize;
        glClientWaitSync(sync: usize, flags: u32, timeout: u64) -> u32;
        glGetUniformLocation(program: u32, name: *const u8) -> i32;
        glDebugMessageCallback(callback: usize, user: *const c_void);
        glGetIntegeri_v(pname: u32, index: u32, data: *mut i32);
        glGetInteger64i_v(pname: u32, index: u32, data: *mut i64);
        glReadPixels(x: i32, y: i32, w: i32, h: i32, format: u32, kind: u32, pixels: *mut c_void);
        glDrawArraysInstanced(mode: u32, first: i32, count: i32, instances: i32);
        glMultiDrawArrays(mode: u32, first: *const i32, count: *const i32, draws: i32);
        glGetProgramResourceIndex(program: u32, interface: u32, name: *const u8) -> u32;
        glGetProgramResourceiv(program: u32, interface: u32, index: u32, count: i32, props: *const u32, size: i32, length: *mut i32, params: *mut i32);
        glGetNamedBufferParameteriv(buffer: u32, pname: u32, params: *mut i32);
        glGetShaderiv(shader: u32, pname: u32, params: *mut i32);
        glGetShaderInfoLog(shader: u32, size: i32, length: *mut i32, log: *mut u8);
        glGetProgramiv(program: u32, pname: u32, params: *mut i32);
        glGetProgramInfoLog(program: u32, size: i32, length: *mut i32, log: *mut u8);
    }
}

// the NVIDIA enums
const VERTEX_ATTRIB_ARRAY_UNIFIED: u32 = 0x8F1E;
const ELEMENT_ARRAY_UNIFIED: u32 = 0x8F1F;
const VERTEX_ATTRIB_ARRAY_ADDRESS: u32 = 0x8F20;
const ELEMENT_ARRAY_ADDRESS: u32 = 0x8F29;
const BUFFER_GPU_ADDRESS: u32 = 0x8F1D;
const DRAW_INDIRECT_UNIFIED: u32 = 0x8F40;
const DRAW_INDIRECT_ADDRESS: u32 = 0x8F41;
const UNIFORM_BUFFER_UNIFIED: u32 = 0x936E;
const UNIFORM_BUFFER_ADDRESS: u32 = 0x936F;

const ELEMENT_ARRAY_BUFFER: u32 = 0x8893;
const UNIFORM_BUFFER: u32 = 0x8A11;
const DRAW_FRAMEBUFFER: u32 = 0x8CA9;
/// GL_MAX_UNIFORM_BLOCK_SIZE's minimum: the bound size of a uniform buffer address without one.
const UNIFORM_RANGE: u64 = 65536;

/// The client states of `NV_vertex_buffer_unified_memory` (`ELEMENT_ARRAY_UNIFIED`, ...), on.
static UNIFIED: AtomicU32 = AtomicU32::new(0);

fn unified_bit(cap: u32) -> Option<u32> {
    match cap {
        VERTEX_ATTRIB_ARRAY_UNIFIED => Some(1),
        ELEMENT_ARRAY_UNIFIED => Some(2),
        UNIFORM_BUFFER_UNIFIED => Some(4),
        DRAW_INDIRECT_UNIFIED => Some(8),
        _ => None,
    }
}

fn set_unified(cap: u32, on: bool) -> bool {
    let Some(bit) = unified_bit(cap) else { return false };
    if on {
        UNIFIED.fetch_or(bit, Relaxed);
    } else {
        UNIFIED.fetch_and(!bit, Relaxed);
    }
    true
}

fn unified(cap: u32) -> bool {
    unified_bit(cap).is_some_and(|bit| UNIFIED.load(Relaxed) & bit != 0)
}

/// The frames after a shot logged call by call (`WAL_NVGL_SHOT` with `WAL_NVGL_LOG=2`).
static FRAME_LOG: AtomicBool = AtomicBool::new(false);

macro_rules! ftrace {
    ($($arg:tt)*) => {{
        if FRAME_LOG.load(Relaxed) {
            log!($($arg)*);
        }
    }};
}

/// Every draw waited for and logged, with every call ([`sync_trace`]).
static SYNC: AtomicBool = AtomicBool::new(false);

/// Debugging a GPU fault (`WAL_NVGL_SYNC=1`, from the moment a caller chooses): each draw
/// waits for the previous one and is logged, every call logged too; the last draw logged
/// before the fault is the faulting one.
pub fn sync_trace() {
    if std::env::var("WAL_NVGL_SYNC").is_ok_and(|v| v.trim() == "1") && !SYNC.swap(true, Relaxed) {
        log!("nvgl: draws synchronized and logged");
        FRAME_LOG.store(true, Relaxed);
    }
}

/// The first calls of a kind, logged with `WAL_NVGL_LOG=1`.
macro_rules! trace {
    ($n:expr, $($arg:tt)*) => {{
        static N: AtomicU32 = AtomicU32::new(0);
        if LOG.load(Relaxed) && N.fetch_add(1, Relaxed) < $n {
            log!($($arg)*);
        }
    }};
}

#[derive(Default)]
struct Buffer {
    size: u64,
    /// Its mapping: the pointer to offset 0, the mapped range.
    map: Option<(usize, u64, u64)>,
}

#[derive(Default)]
struct Ctx {
    buffers: HashMap<u32, Buffer>,
    vao: u32,
    /// The strides the game gave its vertex bindings, by vertex array and binding.
    strides: HashMap<(u32, u32), i32>,
    /// The GPU addresses of the vertex bindings, by vertex array and binding.
    addresses: HashMap<(u32, u32), u64>,
    /// The element array address' offset and the element size, by vertex array.
    elements: HashMap<u32, (u64, u32)>,
    indirect: u64,
    states: HashMap<u32, State>,
    next_state: u32,
    shaders: HashMap<u32, String>,
    /// The uniform buffer bindings (buffer, offset).
    uniforms: HashMap<u32, (u32, u64)>,
    program: u32,
    /// The pointers of each program: storage buffer binding, uniform buffer binding, offset.
    pointers: HashMap<u32, Vec<(u32, u32, u64)>>,
    /// The location of each program's `nvDrawId` ([`shader_source`]).
    draw_ids: HashMap<u32, i32>,
}

static CTX: Mutex<Option<Ctx>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Ctx) -> R) -> R {
    let mut guard = CTX.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(Default::default))
}

/// The buffer and offset of a GPU address.
fn decode(address: u64) -> (u32, u64) {
    ((address >> 32) as u32, address & 0xffff_ffff)
}

/// `len` bytes at a GPU address: from the buffer's mapping, else read back.
fn read(address: u64, len: usize) -> Vec<u8> {
    let (buffer, offset) = decode(address);
    let mapped = with(|c| c.buffers.get(&buffer).and_then(|b| b.map).filter(|&(_, start, end)| offset >= start && offset + len as u64 <= end).map(|(p, _, _)| p));
    let mut out = vec![0u8; len];
    match mapped {
        Some(p) => unsafe { std::ptr::copy_nonoverlapping((p + offset as usize) as *const u8, out.as_mut_ptr(), len) },
        None if buffer != 0 => unsafe { gl::glGetNamedBufferSubData(buffer, offset as isize, len as isize, out.as_mut_ptr().cast()) },
        None => {}
    }
    out
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    b.get(i..i + 4).map_or(0, |s| u32::from_le_bytes(s.try_into().unwrap()))
}

fn u64_at(b: &[u8], i: usize) -> u64 {
    b.get(i..i + 8).map_or(0, |s| u64::from_le_bytes(s.try_into().unwrap()))
}

fn f32_at(b: &[u8], i: usize) -> f32 {
    f32::from_bits(u32_at(b, i))
}

// --- client states ---

unsafe extern "system" fn hook_enable(cap: u32) {
    if !set_unified(cap, true) {
        unsafe { gl::glEnable(cap) }
    }
}

unsafe extern "system" fn hook_disable(cap: u32) {
    if !set_unified(cap, false) {
        unsafe { gl::glDisable(cap) }
    }
}

unsafe extern "system" fn hook_is_enabled(cap: u32) -> u8 {
    if unified_bit(cap).is_some() {
        return unified(cap) as u8;
    }
    unsafe { gl::glIsEnabled(cap) }
}

unsafe extern "system" fn hook_enable_client_state(cap: u32) {
    if !set_unified(cap, true) {
        unsafe { gl::glEnableClientState(cap) }
    }
}

unsafe extern "system" fn hook_disable_client_state(cap: u32) {
    if !set_unified(cap, false) {
        unsafe { gl::glDisableClientState(cap) }
    }
}

unsafe extern "system" fn hook_enable_vertex_array(vao: u32, array: u32) {
    trace!(8, "nvgl: glEnableVertexArrayEXT({vao}, {array:#x})");
    if !set_unified(array, true) {
        unsafe { gl::glEnableVertexArrayEXT(vao, array) }
    }
}

unsafe extern "system" fn hook_disable_vertex_array(vao: u32, array: u32) {
    if !set_unified(array, false) {
        unsafe { gl::glDisableVertexArrayEXT(vao, array) }
    }
}

// --- buffers and their addresses ---

/// `GL_MAP_PERSISTENT_BIT`, `GL_MAP_COHERENT_BIT`.
const MAP_PERSISTENT: u32 = 0x40;
const MAP_COHERENT: u32 = 0x80;

/// The game's persistent mappings made coherent: it writes its ring buffers (vertices, uniforms)
/// through them without making the writes visible (NVIDIA shows them anyway); on Mesa the GPU
/// read the slot's older data, some frames from a past moment (KanColle Arcade's sprites
/// zoomed, as when they appeared).
unsafe extern "system" fn hook_named_buffer_storage(buffer: u32, size: isize, data: *const c_void, flags: u32) {
    static LOGGED: AtomicU32 = AtomicU32::new(0);
    if LOGGED.fetch_add(1, Relaxed) < 64 {
        log!("nvgl: glNamedBufferStorage({buffer}, {size}, {flags:#x})");
    }
    let flags = if flags & MAP_PERSISTENT != 0 { flags | MAP_COHERENT } else { flags };
    unsafe { gl::glNamedBufferStorage(buffer, size, data, flags) };
    with(|c| c.buffers.insert(buffer, Buffer { size: size as u64, map: None }));
}

unsafe extern "system" fn hook_named_buffer_data(buffer: u32, size: isize, data: *const c_void, usage: u32) {
    unsafe { gl::glNamedBufferData(buffer, size, data, usage) };
    with(|c| c.buffers.insert(buffer, Buffer { size: size as u64, map: None }));
}

unsafe extern "system" fn hook_map_named_buffer_range(buffer: u32, offset: isize, length: isize, access: u32) -> *mut c_void {
    static LOGGED: AtomicU32 = AtomicU32::new(0);
    if LOGGED.fetch_add(1, Relaxed) < 64 {
        log!("nvgl: glMapNamedBufferRange({buffer}, {offset}, {length}, {access:#x})");
    }
    let access = if access & MAP_PERSISTENT != 0 { access | MAP_COHERENT } else { access };
    let p = unsafe { gl::glMapNamedBufferRange(buffer, offset, length, access) };
    trace!(16, "nvgl: glMapNamedBufferRange({buffer}, {offset}, {length}, {access:#x}) = {p:?}");
    if !p.is_null() {
        with(|c| c.buffers.entry(buffer).or_default().map = Some((p as usize - offset as usize, offset as u64, (offset + length) as u64)));
    }
    p
}

unsafe extern "system" fn hook_flush_mapped_named_buffer_range(buffer: u32, offset: isize, length: isize) {
    ftrace!("nvgl: flush {buffer}+{offset} ({length})");
    unsafe { gl::glFlushMappedNamedBufferRange(buffer, offset, length) }
}

unsafe extern "system" fn hook_fence_sync(condition: u32, flags: u32) -> usize {
    let sync = unsafe { gl::glFenceSync(condition, flags) };
    ftrace!("nvgl: fence {sync:#x}");
    sync
}

unsafe extern "system" fn hook_client_wait_sync(sync: usize, flags: u32, timeout: u64) -> u32 {
    let r = unsafe { gl::glClientWaitSync(sync, flags, timeout) };
    ftrace!("nvgl: wait {sync:#x} {timeout} = {r:#x}");
    r
}

unsafe extern "system" fn hook_map_named_buffer(buffer: u32, access: u32) -> *mut c_void {
    let p = unsafe { gl::glMapNamedBuffer(buffer, access) };
    if !p.is_null() {
        with(|c| {
            let b = c.buffers.entry(buffer).or_default();
            b.map = Some((p as usize, 0, b.size));
        });
    }
    p
}

unsafe extern "system" fn hook_unmap_named_buffer(buffer: u32) -> u8 {
    with(|c| {
        if let Some(b) = c.buffers.get_mut(&buffer) {
            b.map = None;
        }
    });
    unsafe { gl::glUnmapNamedBuffer(buffer) }
}

unsafe extern "system" fn hook_delete_buffers(n: i32, buffers: *const u32) {
    if n > 0 && !buffers.is_null() {
        let names = unsafe { std::slice::from_raw_parts(buffers, n as usize) };
        with(|c| names.iter().for_each(|b| _ = c.buffers.remove(b)));
    }
    unsafe { gl::glDeleteBuffers(n, buffers) }
}

unsafe extern "system" fn hook_get_named_buffer_parameter_ui64(buffer: u32, pname: u32, params: *mut u64) {
    if params.is_null() {
        return;
    }
    let v = if pname == BUFFER_GPU_ADDRESS { (buffer as u64) << 32 } else { 0 };
    unsafe { *params = v };
}

unsafe extern "system" fn hook_make_resident(_buffer: u32, _access: u32) {}

unsafe extern "system" fn hook_make_non_resident(_buffer: u32) {}

unsafe extern "system" fn hook_buffer_address_range(pname: u32, index: u32, address: u64, length: isize) {
    let (buffer, offset) = decode(address);
    ftrace!("nvgl: address {pname:#x} {index} = {buffer}+{offset} ({length})");
    match pname {
        VERTEX_ATTRIB_ARRAY_ADDRESS => unsafe { bind_attribute(index, address) },
        ELEMENT_ARRAY_ADDRESS => unsafe { bind_elements(address, 0) },
        UNIFORM_BUFFER_ADDRESS => unsafe { bind_uniforms(index, address, length as u64) },
        DRAW_INDIRECT_ADDRESS => with(|c| c.indirect = address),
        _ => trace!(16, "nvgl: glBufferAddressRangeNV({pname:#x}, {index}, {buffer}+{offset}, {length}) unknown"),
    }
}

/// A vertex binding of the bound vertex array at a GPU address.
unsafe fn bind_attribute(binding: u32, address: u64) {
    let (buffer, offset) = decode(address);
    let stride = with(|c| {
        c.addresses.insert((c.vao, binding), address);
        c.strides.get(&(c.vao, binding)).copied().unwrap_or(0)
    });
    ftrace!("nvgl:  vertex binding {binding} = {buffer}+{offset} stride {stride}");
    unsafe { gl::glBindVertexBuffer(binding, buffer, offset as isize, stride) };
}

/// The element array of the bound vertex array at a GPU address (`size`: the element size, 0
/// keeps the draws' type).
unsafe fn bind_elements(address: u64, size: u32) {
    let (buffer, offset) = decode(address);
    unsafe { gl::glBindBuffer(ELEMENT_ARRAY_BUFFER, buffer) };
    with(|c| {
        let vao = c.vao;
        c.elements.insert(vao, (offset, size));
    });
}

/// A uniform buffer binding at a GPU address.
unsafe fn bind_uniforms(index: u32, address: u64, length: u64) {
    let (buffer, offset) = decode(address);
    with(|c| c.uniforms.insert(index, (buffer, offset)));
    if buffer == 0 {
        unsafe { gl::glBindBufferBase(UNIFORM_BUFFER, index, 0) };
        return;
    }
    let size = with(|c| c.buffers.get(&buffer).map_or(UNIFORM_RANGE, |b| b.size.saturating_sub(offset)));
    let length = if length == 0 { size.min(UNIFORM_RANGE) } else { length.min(size.max(1)) };
    unsafe { gl::glBindBufferRange(UNIFORM_BUFFER, index, buffer, offset as isize, length as isize) };
}

// --- vertex arrays ---

unsafe extern "system" fn hook_bind_vertex_array(vao: u32) {
    ftrace!("nvgl: vertex array {vao}");
    with(|c| c.vao = vao);
    unsafe { gl::glBindVertexArray(vao) }
}

unsafe extern "system" fn hook_vertex_array_vertex_buffer(vao: u32, binding: u32, buffer: u32, offset: isize, stride: i32) {
    ftrace!("nvgl: glVertexArrayVertexBuffer({vao}, {binding}, {buffer}, {offset}, {stride})");
    // the vertex array 0: the bound one (the game sets its unified bindings' strides so)
    let bound = with(|c| {
        let target = if vao == 0 { c.vao } else { vao };
        c.strides.insert((target, binding), stride);
        c.vao
    });
    if vao == 0 {
        trace!(4, "nvgl: glVertexArrayVertexBuffer(0, {binding}, {buffer}, {offset}, {stride}) on vertex array {bound}");
        unsafe { bind_vertex_buffer(binding, buffer, offset, stride) }
    } else {
        unsafe { gl::glVertexArrayVertexBuffer(vao, binding, buffer, offset, stride) }
    }
}

unsafe extern "system" fn hook_bind_vertex_buffer(binding: u32, buffer: u32, offset: isize, stride: i32) {
    with(|c| {
        let vao = c.vao;
        c.strides.insert((vao, binding), stride);
    });
    unsafe { bind_vertex_buffer(binding, buffer, offset, stride) }
}

/// A vertex binding of the bound vertex array: with the vertex attributes' addresses on
/// (`VERTEX_ATTRIB_ARRAY_UNIFIED_NV`) only its stride (NVIDIA ignores the buffer, the game gives
/// its strides so, after the addresses: its models' texture coordinates came from another
/// buffer, parts of them transparent), at the binding's address.
unsafe fn bind_vertex_buffer(binding: u32, buffer: u32, offset: isize, stride: i32) {
    if !unified(VERTEX_ATTRIB_ARRAY_UNIFIED) {
        return unsafe { gl::glBindVertexBuffer(binding, buffer, offset, stride) };
    }
    if let Some(address) = with(|c| c.addresses.get(&(c.vao, binding)).copied()) {
        let (buffer, offset) = decode(address);
        unsafe { gl::glBindVertexBuffer(binding, buffer, offset as isize, stride) };
    }
}

// --- draws ---

/// The bound element array address' offset (element draws' indices are relative to it).
fn element_offset() -> usize {
    if !unified(ELEMENT_ARRAY_UNIFIED) {
        return 0;
    }
    with(|c| c.elements.get(&c.vao).map_or(0, |e| e.0 as usize))
}

fn type_size(kind: u32) -> usize {
    match kind {
        0x1401 => 1,
        0x1403 => 2,
        _ => 4,
    }
}

fn element_type(size: u32) -> u32 {
    match size {
        1 => 0x1401,
        2 => 0x1403,
        _ => 0x1405,
    }
}

unsafe extern "system" fn hook_draw_elements(mode: u32, count: i32, kind: u32, indices: usize) {
    unsafe { update_pointers() };
    unsafe { gl::glDrawElements(mode, count, kind, indices + element_offset()) }
}

unsafe extern "system" fn hook_draw_elements_base_vertex(mode: u32, count: i32, kind: u32, indices: usize, base_vertex: i32) {
    unsafe { update_pointers() };
    unsafe { gl::glDrawElementsBaseVertex(mode, count, kind, indices + element_offset(), base_vertex) }
}

unsafe extern "system" fn hook_draw_elements_instanced(mode: u32, count: i32, kind: u32, indices: usize, instances: i32) {
    unsafe { update_pointers() };
    unsafe { gl::glDrawElementsInstanced(mode, count, kind, indices + element_offset(), instances) }
}

unsafe extern "system" fn hook_draw_range_elements(mode: u32, start: u32, end: u32, count: i32, kind: u32, indices: usize) {
    unsafe { update_pointers() };
    unsafe { gl::glDrawRangeElements(mode, start, end, count, kind, indices + element_offset()) }
}

unsafe extern "system" fn hook_multi_draw_elements_base_vertex(mode: u32, counts: *const i32, kind: u32, indices: *const usize, draws: i32, base_vertices: *const i32) {
    unsafe { update_pointers() };
    let offset = element_offset();
    if offset == 0 || draws <= 0 || indices.is_null() {
        return unsafe { gl::glMultiDrawElementsBaseVertex(mode, counts, kind, indices, draws, base_vertices) };
    }
    let moved: Vec<usize> = unsafe { std::slice::from_raw_parts(indices, draws as usize) }.iter().map(|i| i + offset).collect();
    unsafe { gl::glMultiDrawElementsBaseVertex(mode, counts, kind, moved.as_ptr(), draws, base_vertices) }
}

/// The current program's `nvDrawId` set to a split multi-draw's command index.
fn set_draw_id(id: i32) {
    if let Some((program, location)) = with(|c| c.draw_ids.get(&c.program).map(|&l| (c.program, l))) {
        unsafe { gl::glProgramUniform1i(program, location, id) };
    }
}

/// Indirect element draws: from the indirect address, or with an element address' offset, their
/// commands read and drawn one by one.
unsafe extern "system" fn hook_multi_draw_elements_indirect(mode: u32, kind: u32, indirect: usize, draws: i32, stride: i32) {
    unsafe { update_pointers() };
    let offset = element_offset();
    if !unified(DRAW_INDIRECT_UNIFIED) && offset == 0 {
        return unsafe { gl::glMultiDrawElementsIndirect(mode, kind, indirect, draws, stride) };
    }
    trace!(4, "nvgl: glMultiDrawElementsIndirect({mode:#x}, {kind:#x}, {indirect:#x}, {draws}, {stride})");
    let stride = if stride == 0 { 20 } else { stride as usize };
    let commands = if unified(DRAW_INDIRECT_UNIFIED) {
        read(with(|c| c.indirect) + indirect as u64, stride * draws.max(0) as usize)
    } else {
        unsafe { std::slice::from_raw_parts(indirect as *const u8, stride * draws.max(0) as usize) }.to_vec()
    };
    for i in 0..draws.max(0) as usize {
        let at = i * stride;
        let (count, instances, first, base_vertex, base_instance) = (u32_at(&commands, at), u32_at(&commands, at + 4), u32_at(&commands, at + 8), u32_at(&commands, at + 12), u32_at(&commands, at + 16));
        set_draw_id(i as i32);
        unsafe { gl::glDrawElementsInstancedBaseVertexBaseInstance(mode, count as i32, kind, offset + first as usize * type_size(kind), instances as i32, base_vertex as i32, base_instance) };
    }
    set_draw_id(0);
}

/// `glMultiDrawElementsIndirectBindlessNV`: each command with its element and vertex buffer
/// addresses.
unsafe extern "system" fn hook_multi_draw_elements_indirect_bindless(mode: u32, kind: u32, indirect: usize, draws: i32, stride: i32, vertex_buffers: i32) {
    trace!(4, "nvgl: glMultiDrawElementsIndirectBindlessNV({mode:#x}, {kind:#x}, {indirect:#x}, {draws}, {stride}, {vertex_buffers})");
    unsafe { update_pointers() };
    let n = vertex_buffers.max(0) as usize;
    let stride = if stride == 0 { 48 + 24 * n } else { stride as usize };
    let len = stride * draws.max(0) as usize;
    let commands = if unified(DRAW_INDIRECT_UNIFIED) {
        read(with(|c| c.indirect) + indirect as u64, len)
    } else {
        unsafe { std::slice::from_raw_parts(indirect as *const u8, len) }.to_vec()
    };
    for i in 0..draws.max(0) as usize {
        let at = i * stride;
        let (count, instances, first, base_vertex, base_instance) = (u32_at(&commands, at), u32_at(&commands, at + 4), u32_at(&commands, at + 8), u32_at(&commands, at + 12), u32_at(&commands, at + 16));
        // DrawElementsIndirectCommand, reserved, BindlessPtrNV index buffer, vertex buffers
        let (buffer, offset) = decode(u64_at(&commands, at + 32));
        unsafe { gl::glBindBuffer(ELEMENT_ARRAY_BUFFER, buffer) };
        for v in 0..n {
            let p = at + 48 + 24 * v;
            unsafe { bind_attribute(u32_at(&commands, p), u64_at(&commands, p + 8)) };
        }
        set_draw_id(i as i32);
        unsafe { gl::glDrawElementsInstancedBaseVertexBaseInstance(mode, count as i32, kind, offset as usize + first as usize * type_size(kind), instances as i32, base_vertex as i32, base_instance) };
    }
    set_draw_id(0);
}

// --- NV_command_list ---

/// The GL state a state object captured (`glStateCaptureNV`), applied before its sequences.
#[derive(Clone, Default)]
struct State {
    mode: u32,
    program: i32,
    vao: i32,
    enables: Vec<(u32, bool)>,
    depth_func: i32,
    depth_mask: u8,
    blend: [i32; 6],
    color_mask: [u8; 4],
    cull_face: i32,
    front_face: i32,
    polygon_offset: [f32; 2],
    polygon_mode: [i32; 2],
    stencil: [i32; 14],
}

const CAPS: [u32; 14] = [0x0BE2, 0x0B71, 0x0B44, 0x0B90, 0x8037, 0x809E, 0x864F, 0x8F9D, 0x8D69, 0x809D, 0x8890, 0x8DB9, 0x8C89, 0x0C11];
const STENCIL: [u32; 14] = [0x0B92, 0x0B97, 0x0B93, 0x0B94, 0x0B95, 0x0B96, 0x0B98, 0x8800, 0x8CA3, 0x8CA4, 0x8801, 0x8802, 0x8803, 0x8CA5];

fn get_i(pname: u32) -> i32 {
    let mut v = [0i32; 4];
    unsafe { gl::glGetIntegerv(pname, v.as_mut_ptr()) };
    v[0]
}

unsafe fn capture(mode: u32) -> State {
    let mut s = State { mode, ..Default::default() };
    unsafe {
        s.program = get_i(0x8B8D);
        s.vao = get_i(0x85B5);
        s.enables = CAPS.iter().map(|&cap| (cap, gl::glIsEnabled(cap) != 0)).collect();
        s.depth_func = get_i(0x0B74);
        let mut b = [0u8; 4];
        gl::glGetBooleanv(0x0B72, b.as_mut_ptr());
        s.depth_mask = b[0];
        for (i, p) in [0x80C9, 0x80C8, 0x80CB, 0x80CA, 0x8009, 0x883D].iter().enumerate() {
            s.blend[i] = get_i(*p);
        }
        gl::glGetBooleanv(0x0C23, s.color_mask.as_mut_ptr());
        s.cull_face = get_i(0x0B45);
        s.front_face = get_i(0x0B46);
        gl::glGetFloatv(0x8038, &mut s.polygon_offset[0]);
        gl::glGetFloatv(0x2A00, &mut s.polygon_offset[1]);
        let mut pm = [0i32; 4];
        gl::glGetIntegerv(0x0B40, pm.as_mut_ptr());
        s.polygon_mode = [pm[0], pm[1]];
        for (i, p) in STENCIL.iter().enumerate() {
            s.stencil[i] = get_i(*p);
        }
    }
    s
}

unsafe fn apply(s: &State) {
    unsafe {
        gl::glUseProgram(s.program as u32);
        gl::glBindVertexArray(s.vao as u32);
        for &(cap, on) in &s.enables {
            if on { gl::glEnable(cap) } else { gl::glDisable(cap) }
        }
        gl::glDepthFunc(s.depth_func as u32);
        gl::glDepthMask(s.depth_mask);
        let b = s.blend.map(|v| v as u32);
        gl::glBlendFuncSeparate(b[0], b[1], b[2], b[3]);
        gl::glBlendEquationSeparate(b[4], b[5]);
        gl::glColorMask(s.color_mask[0], s.color_mask[1], s.color_mask[2], s.color_mask[3]);
        gl::glCullFace(s.cull_face as u32);
        gl::glFrontFace(s.front_face as u32);
        gl::glPolygonOffset(s.polygon_offset[0], s.polygon_offset[1]);
        gl::glPolygonMode(0x0408, s.polygon_mode[0] as u32);
        let st = s.stencil.map(|v| v as u32);
        gl::glStencilFuncSeparate(0x0404, st[0], st[1] as i32, st[2]);
        gl::glStencilOpSeparate(0x0404, st[3], st[4], st[5]);
        gl::glStencilMaskSeparate(0x0404, st[6]);
        gl::glStencilFuncSeparate(0x0405, st[7], st[8] as i32, st[9]);
        gl::glStencilOpSeparate(0x0405, st[10], st[11], st[12]);
        gl::glStencilMaskSeparate(0x0405, st[13]);
    }
    with(|c| {
        c.vao = s.vao as u32;
        c.program = s.program as u32;
    });
}

unsafe extern "system" fn hook_create_states(n: i32, states: *mut u32) {
    if n <= 0 || states.is_null() {
        return;
    }
    with(|c| {
        for i in 0..n as usize {
            c.next_state += 1;
            let id = c.next_state;
            c.states.insert(id, State::default());
            unsafe { *states.add(i) = id };
        }
    });
}

unsafe extern "system" fn hook_delete_states(n: i32, states: *const u32) {
    if n <= 0 || states.is_null() {
        return;
    }
    let ids = unsafe { std::slice::from_raw_parts(states, n as usize) };
    with(|c| ids.iter().for_each(|id| _ = c.states.remove(id)));
}

unsafe extern "system" fn hook_state_capture(state: u32, mode: u32) {
    let s = unsafe { capture(mode) };
    trace!(4, "nvgl: glStateCaptureNV({state}, {mode:#x}): program {}, vertex array {}", s.program, s.vao);
    with(|c| c.states.insert(state, s));
}

/// The token headers: the token id under a mark.
const HEADER: u32 = 0x4E56_0000;

unsafe extern "system" fn hook_get_command_header(token: u32, size: u32) -> u32 {
    trace!(24, "nvgl: glGetCommandHeaderNV({token}, {size})");
    HEADER | token
}

unsafe extern "system" fn hook_get_stage_index(shader_type: u32) -> u16 {
    match shader_type {
        0x8B31 => 0,
        0x8E88 => 1,
        0x8E87 => 2,
        0x8DD9 => 3,
        0x8B30 => 4,
        _ => 5,
    }
}

/// A token's size, header included.
fn token_size(id: u32) -> usize {
    match id {
        0 | 1 => 4,
        2 | 4 => 16,
        3 | 5 => 12,
        6 => 28,
        7 => 24,
        8..=10 => 16,
        11 => 20,
        12 | 14 => 12,
        13 | 15 | 18 => 8,
        16 | 17 => 20,
        _ => 4,
    }
}

/// The strip mode of a basic mode (`*_STRIP` tokens).
fn strip(mode: u32) -> u32 {
    match mode {
        0x0004 => 0x0005,
        0x0001 => 0x0003,
        m => m,
    }
}

/// One token sequence replayed.
unsafe fn replay(tokens: &[u8], mode: u32) {
    let mut at = 0;
    let mut element = (0u64, 4u32);
    while at + 4 <= tokens.len() {
        let header = u32_at(tokens, at);
        if header & 0xffff_0000 != HEADER {
            trace!(8, "nvgl: token header {header:#x} at {at}: sequence ended");
            return;
        }
        let id = header & 0xffff;
        let t = &tokens[at..tokens.len().min(at + token_size(id))];
        ftrace!("nvgl:  token {id} {:x?}", &t[4..]);
        if (2..=7).contains(&id) {
            unsafe { update_pointers() };
        }
        unsafe {
            match id {
                0 => return,
                1 => {}
                2 | 4 => {
                    let m = if id == 4 { strip(mode) } else { mode };
                    gl::glDrawElementsBaseVertex(m, u32_at(t, 4) as i32, element_type(element.1), element.0 as usize + u32_at(t, 8) as usize * element.1 as usize, u32_at(t, 12) as i32);
                }
                3 | 5 => {
                    let m = if id == 5 { strip(mode) } else { mode };
                    gl::glDrawArrays(m, u32_at(t, 8) as i32, u32_at(t, 4) as i32);
                }
                6 => gl::glDrawElementsInstancedBaseVertexBaseInstance(u32_at(t, 4), u32_at(t, 8) as i32, element_type(element.1), element.0 as usize + u32_at(t, 16) as usize * element.1 as usize, u32_at(t, 12) as i32, u32_at(t, 20) as i32, u32_at(t, 24)),
                7 => gl::glDrawArraysInstancedBaseInstance(u32_at(t, 4), u32_at(t, 16) as i32, u32_at(t, 8) as i32, u32_at(t, 12) as i32, u32_at(t, 20)),
                8 => {
                    let address = u64_at(t, 4);
                    let size = u32_at(t, 12);
                    gl::glBindBuffer(ELEMENT_ARRAY_BUFFER, decode(address).0);
                    element = (decode(address).1, if size == 0 { 4 } else { size });
                }
                9 => bind_attribute(u32_at(t, 4), u64_at(t, 8)),
                10 => bind_uniforms(u32_at(t, 4) & 0xffff, u64_at(t, 8), 0),
                11 => gl::glBlendColor(f32_at(t, 4), f32_at(t, 8), f32_at(t, 12), f32_at(t, 16)),
                12 => {
                    // the reference values only: the functions and masks of the state
                    let (front, back) = (u32_at(t, 4) as i32, u32_at(t, 8) as i32);
                    let (ff, fm, bf, bm) = (get_i(0x0B92) as u32, get_i(0x0B93) as u32, get_i(0x8800) as u32, get_i(0x8CA4) as u32);
                    gl::glStencilFuncSeparate(0x0404, ff, front, fm);
                    gl::glStencilFuncSeparate(0x0405, bf, back, bm);
                }
                13 => gl::glLineWidth(f32_at(t, 4)),
                14 => gl::glPolygonOffset(f32_at(t, 4), f32_at(t, 8)),
                15 => {}
                16 => gl::glViewport(u32_at(t, 4) as i32, u32_at(t, 8) as i32, u32_at(t, 12) as i32, u32_at(t, 16) as i32),
                17 => gl::glScissor(u32_at(t, 4) as i32, u32_at(t, 8) as i32, u32_at(t, 12) as i32, u32_at(t, 16) as i32),
                18 => gl::glFrontFace(if u32_at(t, 4) == 0 { 0x0900 } else { 0x0901 }),
                _ => trace!(8, "nvgl: token {id} unknown"),
            }
        }
        at += token_size(id);
    }
}

unsafe extern "system" fn hook_draw_commands_states_address(indirects: *const u64, sizes: *const i32, states: *const u32, fbos: *const u32, count: u32) {
    if count == 0 || indirects.is_null() || sizes.is_null() || states.is_null() {
        return;
    }
    trace!(4, "nvgl: glDrawCommandsStatesAddressNV({count} sequences)");
    let current = unsafe { capture(0) };
    let fbo = get_i(0x8CA6);
    let vao = with(|c| c.vao);
    for i in 0..count as usize {
        let (address, size, state, target) = unsafe { (*indirects.add(i), *sizes.add(i), *states.add(i), if fbos.is_null() { 0 } else { *fbos.add(i) }) };
        let Some(s) = with(|c| c.states.get(&state).cloned()) else {
            trace!(8, "nvgl: state {state} unknown");
            continue;
        };
        ftrace!("nvgl: sequence {i}: {address:#x} ({size} bytes), state {state} (program {}, vertex array {}, mode {:#x}), framebuffer {target}", s.program, s.vao, s.mode);
        unsafe {
            gl::glBindFramebuffer(DRAW_FRAMEBUFFER, target);
            apply(&s);
            replay(&read(address, size.max(0) as usize), s.mode);
        }
    }
    unsafe {
        gl::glBindFramebuffer(DRAW_FRAMEBUFFER, fbo as u32);
        apply(&current);
    }
    with(|c| c.vao = vao);
}

unsafe extern "system" fn hook_draw_commands_address(mode: u32, indirects: *const u64, sizes: *const i32, count: u32) {
    if count == 0 || indirects.is_null() || sizes.is_null() {
        return;
    }
    for i in 0..count as usize {
        let (address, size) = unsafe { (*indirects.add(i), *sizes.add(i)) };
        unsafe { replay(&read(address, size.max(0) as usize), mode) };
    }
}

// --- shaders ---

/// The storage buffer bindings of the pointers (`NV_shader_buffer_load`), by name: from
/// [`FIRST_POINTER_BINDING`] on.
static POINTERS: Mutex<Vec<String>> = Mutex::new(Vec::new());
const FIRST_POINTER_BINDING: u32 = 8;
const SHADER_STORAGE_BUFFER: u32 = 0x90D2;

fn pointer_binding(name: &str) -> u32 {
    let mut names = POINTERS.lock().unwrap_or_else(|e| e.into_inner());
    let i = names.iter().position(|n| n == name).unwrap_or_else(|| {
        names.push(name.to_string());
        names.len() - 1
    });
    FIRST_POINTER_BINDING + i as u32
}

fn is_ident(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !s.starts_with(|c: char| c.is_ascii_digit())
}

/// `T* name;`: the type and the name.
fn pointer_member(code: &str) -> Option<(&str, &str)> {
    let (ty, name) = code.trim().strip_suffix(';')?.split_once('*')?;
    let (ty, name) = (ty.trim(), name.trim());
    (is_ident(ty) && is_ident(name)).then_some((ty, name))
}

/// A GLSL source for other GPUs:
/// * `NV_gpu_shader5` as `ARB_gpu_shader5` + `ARB_gpu_shader_int64` + `ARB_bindless_texture`
///   (64-bit integers, samplers from handles); `NV_command_list` and its qualifier dropped.
/// * `NV_shader_buffer_load`'s pointers in uniform blocks (`T* name;`): the member made a
///   `uvec2 nvptr_name` (the same 8 bytes), the data a std430 storage buffer `T name[]` after
///   the block (bound by the payload from the pointer's value, [`update_pointers`]); `name->`
///   as `name[0].`.
/// * `uint64_t` (bindless texture handles) as `uvec2`.
/// * Anonymous structures declared in uniform blocks (`struct{...}name[n];`, NVIDIA's GLSL
///   accepts them): declared before the block, named `NvAnonN`.
/// * `gl_DrawIDARB` plus a uniform `nvDrawId`: the multi-draws drawn one command at a time
///   (each a draw of its own, its `gl_DrawIDARB` 0) give it the command's index (the game's
///   model shaders read their draw's transform and textures at `g_draws[gl_DrawIDARB]`).
pub fn shader_source(src: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    // the uniform block being read: its first line in `out`, its brace depth, its pointers
    let mut block: Option<usize> = None;
    let mut depth = 0i32;
    let mut pointers: Vec<(String, String)> = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut anon: Option<Vec<String>> = None;
    let mut anon_count = 0;
    for line in src.lines() {
        let code = line.split("//").next().unwrap_or("");
        let t = code.trim();
        if t.starts_with("#extension") && t.contains("GL_NV_gpu_shader5") {
            out.push("#extension GL_ARB_gpu_shader5 : enable\n#extension GL_ARB_gpu_shader_int64 : enable\n#extension GL_ARB_bindless_texture : enable".into());
            continue;
        }
        if t.starts_with("#extension") && (t.contains("GL_NV_command_list") || t.contains("GL_NV_shader_buffer_load")) || t.replace(' ', "").starts_with("layout(commandBindableNV)uniform;") {
            out.push(String::new());
            continue;
        }
        let Some(start) = block else {
            if t.contains("uniform") && t.contains('{') && !t.starts_with('#') {
                block = Some(out.len());
                depth = 0;
            } else {
                out.push(line.to_string());
                continue;
            }
            // the block's first line, counted below
            depth += code.matches('{').count() as i32 - code.matches('}').count() as i32;
            out.push(line.to_string());
            continue;
        };
        if let Some(body) = anon.as_mut() {
            if t.starts_with('}') {
                let member = t.trim_start_matches('}').trim();
                let name = format!("NvAnon{anon_count}");
                anon_count += 1;
                let mut def = vec![format!("struct {name}{{")];
                def.append(body);
                def.push("};".into());
                let n = def.len();
                out.splice(start..start, def);
                block = Some(start + n);
                out.push(format!("    {name} {member}"));
                anon = None;
            } else {
                body.push(line.to_string());
            }
            continue;
        }
        if t.replace(' ', "") == "struct{" {
            anon = Some(Vec::new());
            continue;
        }
        if let Some((ty, name)) = pointer_member(code) {
            out.push(format!("    uvec2 nvptr_{name};"));
            pointers.push((ty.to_string(), name.to_string()));
            continue;
        }
        depth += code.matches('{').count() as i32 - code.matches('}').count() as i32;
        out.push(line.to_string());
        if depth <= 0 {
            block = None;
            for (ty, name) in pointers.drain(..) {
                let binding = pointer_binding(&name);
                out.push(format!("layout(std430, binding = {binding}) buffer NvPtr_{name}{{ {ty} {name}[]; }};"));
                names.push(name);
            }
        }
    }
    // 64-bit integers (the bindless handles) as uvec2: Mesa (26.2, radeonsi) crashes compiling a
    // sampler made from a uint64_t uniform block member; the same 8 bytes and alignment
    // the sampler uniforms stay bound ones (the game binds its textures to their units; a
    // bindless sampler's binding read as a handle: GPU faults), samplers made from handles
    let draw_id = out.iter().any(|l| l.contains("gl_DrawIDARB"));
    if let Some(version) = out.iter().position(|l| l.trim_start().starts_with("#version")) {
        out.insert(version + 1, "#extension GL_ARB_bindless_texture : enable".into());
        if draw_id {
            let last = out.iter().rposition(|l| l.trim_start().starts_with("#extension")).unwrap_or(version + 1);
            out.insert(last + 1, "uniform int nvDrawId;".into());
        }
    }
    let mut text = out.join("\n").replace("uint64_t", "uvec2");
    if draw_id {
        text = text.replace("gl_DrawIDARB", "(gl_DrawIDARB + nvDrawId)");
    }
    for name in &names {
        text = text.replace(&format!("{name}->"), &format!("{name}[0]."));
    }
    text.push('\n');
    text
}

unsafe extern "system" fn hook_shader_source(shader: u32, count: i32, strings: *const *const u8, lengths: *const i32) {
    if count <= 0 || strings.is_null() {
        return unsafe { gl::glShaderSource(shader, count, strings, lengths) };
    }
    let mut src = Vec::new();
    for i in 0..count as usize {
        let s = unsafe { *strings.add(i) };
        let len = if lengths.is_null() || unsafe { *lengths.add(i) } < 0 { unsafe { CStr::from_ptr(s.cast()) }.to_bytes().len() } else { (unsafe { *lengths.add(i) }) as usize };
        src.extend_from_slice(unsafe { std::slice::from_raw_parts(s, len) });
    }
    let fixed = shader_source(&String::from_utf8_lossy(&src));
    let one = fixed.as_ptr();
    let len = fixed.len() as i32;
    unsafe { gl::glShaderSource(shader, 1, &one, &len) };
    if LOG.load(Relaxed) {
        with(|c| c.shaders.insert(shader, fixed));
    }
}

static FAILED: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn hook_compile_shader(shader: u32) {
    unsafe { gl::glCompileShader(shader) };
    if !LOG.load(Relaxed) {
        return;
    }
    let mut ok = 0;
    unsafe { gl::glGetShaderiv(shader, 0x8B81, &mut ok) };
    if ok != 0 {
        return;
    }
    let mut buf = vec![0u8; 4096];
    let mut len = 0;
    unsafe { gl::glGetShaderInfoLog(shader, buf.len() as i32, &mut len, buf.as_mut_ptr()) };
    let n = FAILED.fetch_add(1, Relaxed);
    let message = String::from_utf8_lossy(&buf[..len.max(0) as usize]).into_owned();
    log!("nvgl: shader {shader} does not compile (nvgl\\{n}.glsl): {}", message.lines().take(4).collect::<Vec<_>>().join(" | "));
    let _ = std::fs::create_dir_all("nvgl");
    let src = with(|c| c.shaders.get(&shader).cloned()).unwrap_or_default();
    let _ = std::fs::write(format!("nvgl\\{n}.glsl"), format!("{src}\n/* {message} */\n"));
}

unsafe extern "system" fn hook_link_program(program: u32) {
    unsafe { gl::glLinkProgram(program) };
    if std::env::var("WAL_NVGL_SYNC").is_ok_and(|v| v.trim() == "1") {
        let mut shaders = [0u32; 8];
        let mut n = 0;
        unsafe { gl::glGetAttachedShaders(program, 8, &mut n, shaders.as_mut_ptr()) };
        let src: String = with(|c| shaders[..n.max(0) as usize].iter().filter_map(|s| c.shaders.get(s).cloned()).collect::<Vec<_>>().join("\n/* ---- */\n"));
        PROGRAM_SOURCES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(program, src);
    }
    unsafe { program_pointers(program) };
    let location = unsafe { gl::glGetUniformLocation(program, c"nvDrawId".as_ptr().cast()) };
    with(|c| {
        if location >= 0 {
            c.draw_ids.insert(program, location);
        } else {
            c.draw_ids.remove(&program);
        }
    });
    if !LOG.load(Relaxed) {
        return;
    }
    let mut ok = 0;
    unsafe { gl::glGetProgramiv(program, 0x8B82, &mut ok) };
    if ok == 0 {
        let mut buf = vec![0u8; 2048];
        let mut len = 0;
        unsafe { gl::glGetProgramInfoLog(program, buf.len() as i32, &mut len, buf.as_mut_ptr()) };
        log!("nvgl: program {program} does not link: {}", String::from_utf8_lossy(&buf[..len.max(0) as usize]).lines().take(4).collect::<Vec<_>>().join(" | "));
    }
}

static DEBUG_DRAWS: AtomicUsize = AtomicUsize::new(0);

/// The programs' shader sources (`WAL_NVGL_SYNC`), written to `nvgl\program-N.glsl` on
/// their first synchronized draw.
static PROGRAM_SOURCES: Mutex<Option<HashMap<u32, String>>> = Mutex::new(None);

/// The texture handles given: their texture, sampler, residency.
static HANDLES: Mutex<Option<HashMap<u64, (u32, u32, bool)>>> = Mutex::new(None);

unsafe extern "system" fn hook_get_texture_handle(texture: u32) -> u64 {
    let handle = unsafe { gl::glGetTextureHandleARB(texture) };
    HANDLES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).entry(handle).or_insert((texture, 0, false));
    trace!(8, "nvgl: texture {texture} handle {handle:#x}");
    ftrace!("nvgl: texture {texture} handle {handle:#x}");
    handle
}

unsafe extern "system" fn hook_get_texture_sampler_handle(texture: u32, sampler: u32) -> u64 {
    let handle = unsafe { gl::glGetTextureSamplerHandleARB(texture, sampler) };
    HANDLES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).entry(handle).or_insert((texture, sampler, false));
    trace!(8, "nvgl: texture {texture} sampler {sampler} handle {handle:#x}");
    ftrace!("nvgl: texture {texture} sampler {sampler} handle {handle:#x}");
    handle
}

unsafe extern "system" fn hook_make_handle_resident(handle: u64) {
    unsafe { gl::glMakeTextureHandleResidentARB(handle) };
    if let Some(h) = HANDLES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).get_mut(&handle) {
        h.2 = true;
    }
    ftrace!("nvgl: handle {handle:#x} resident");
}

unsafe extern "system" fn hook_make_handle_non_resident(handle: u64) {
    unsafe { gl::glMakeTextureHandleNonResidentARB(handle) };
    if let Some(h) = HANDLES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).get_mut(&handle) {
        h.2 = false;
    }
    ftrace!("nvgl: handle {handle:#x} not resident");
}

/// The texture handles in the bound uniform buffers (`WAL_NVGL_SYNC`), and the program's
/// sources once.
unsafe fn debug_handles() {
    let (program, ubos) = with(|c| (c.program, { let mut u: Vec<_> = c.uniforms.iter().map(|(k, v)| (*k, *v)).collect(); u.sort(); u }));
    if let Some(src) = PROGRAM_SOURCES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).remove(&program) {
        let _ = std::fs::create_dir_all("nvgl");
        let _ = std::fs::write(format!("nvgl\\program-{program}.glsl"), src);
    }
    let handles = HANDLES.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
    let mut found = String::new();
    for (binding, (buffer, offset)) in ubos {
        if buffer == 0 {
            continue;
        }
        let size = with(|c| c.buffers.get(&buffer).map(|b| b.size)).unwrap_or(0);
        let len = size.saturating_sub(offset).min(1024) as usize & !7;
        let data = read(((buffer as u64) << 32) | offset, len);
        for i in (0..len).step_by(8) {
            let v = u64_at(&data, i);
            if let Some(&(texture, sampler, resident)) = handles.get(&v) {
                let valid = unsafe { gl::glIsTexture(texture) } != 0;
                found += &format!(" u{binding}+{i}={v:#x}(t{texture} s{sampler}{}{})", if resident { "" } else { " NOT RESIDENT" }, if valid { "" } else { " DELETED" });
            }
        }
    }
    log!("nvgl:  handles:{found}");
    // the program's samplers: the handles given (else their initial binding), their texture's target
    static SAMPLERS: Mutex<Option<HashMap<u32, Vec<(String, u32, i32)>>>> = Mutex::new(None);
    let samplers = SAMPLERS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).entry(program).or_insert_with(|| unsafe { program_samplers(program) }).clone();
    let given = UNIFORM_HANDLES.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default();
    let mut list = String::new();
    for (name, kind, location) in samplers {
        let describe = |v: u64| match handles.get(&v) {
            Some(&(texture, sampler, resident)) => {
                let mut target = 0;
                unsafe { gl::glGetTextureParameteriv(texture, 0x1006, &mut target) };
                format!("{v:#x}(t{texture} s{sampler} target {target:#x}{}{})", if resident { "" } else { " NOT RESIDENT" }, if unsafe { gl::glIsTexture(texture) } != 0 { "" } else { " DELETED" })
            }
            None => format!("{v:#x}(unknown)"),
        };
        let value = match given.get(&(program, location)) {
            Some(&v) => describe(v),
            None => "never set".into(),
        };
        list += &format!(" {name}[{kind:#x}]@{location}={value}");
    }
    log!("nvgl:  samplers:{list}");
}

/// A program's sampler uniforms: name, type, location.
unsafe fn program_samplers(program: u32) -> Vec<(String, u32, i32)> {
    let mut count = 0;
    unsafe { gl::glGetProgramInterfaceiv(program, 0x92E1, 0x92F5, &mut count) }; // GL_UNIFORM, GL_ACTIVE_RESOURCES
    let mut out = Vec::new();
    for index in 0..count.max(0) as u32 {
        // GL_TYPE, GL_ARRAY_SIZE, GL_LOCATION
        let mut v = [0i32; 3];
        unsafe { gl::glGetProgramResourceiv(program, 0x92E1, index, 3, [0x92FA, 0x92FB, 0x930E].as_ptr(), 3, std::ptr::null_mut(), v.as_mut_ptr()) };
        let kind = v[0] as u32;
        // (0x8DC6-0x8DC9: the unsigned integer vectors, between the sampler types)
        let sampler = (0x8B5D..=0x8B64).contains(&kind) || (0x8DC1..=0x8DC5).contains(&kind) || (0x8DCA..=0x8DD8).contains(&kind) || (0x900C..=0x900F).contains(&kind) || (0x9108..=0x910D).contains(&kind);
        if !sampler || v[2] < 0 {
            continue;
        }
        let mut name = vec![0u8; 128];
        let mut len = 0;
        unsafe { gl::glGetProgramResourceName(program, 0x92E1, index, name.len() as i32, &mut len, name.as_mut_ptr()) };
        let name = String::from_utf8_lossy(&name[..len.max(0) as usize]).into_owned();
        for i in 0..v[1].max(1) {
            out.push((format!("{name}+{i}"), kind, v[2] + i));
        }
    }
    out
}

/// A draw's state, logged (`WAL_NVGL_LOG`): 40 draws once the game runs.
unsafe fn debug_draw() {
    static CALLBACK: AtomicBool = AtomicBool::new(false);
    if !CALLBACK.swap(true, Relaxed) {
        unsafe {
            gl::glEnable(0x92E0); // GL_DEBUG_OUTPUT
            gl::glEnable(0x8242); // GL_DEBUG_OUTPUT_SYNCHRONOUS
            gl::glDebugMessageCallback(debug_message as *const () as usize, std::ptr::null());
        }
    }
    let err = unsafe { gl::glGetError() };
    let (program, vao, fbo) = (get_i(0x8B8D), get_i(0x85B5), get_i(0x8CA6));
    let mut vp = [0i32; 4];
    unsafe { gl::glGetIntegerv(0x0BA2, vp.as_mut_ptr()) };
    let mut bindings = String::new();
    for b in 0..4u32 {
        let mut buf = 0i32;
        let mut off = 0i64;
        let mut stride = 0i32;
        unsafe {
            gl::glGetIntegeri_v(0x8F4F, b, &mut buf);
            gl::glGetInteger64i_v(0x82D7, b, &mut off);
            gl::glGetIntegeri_v(0x82D8, b, &mut stride);
        }
        bindings += &format!(" b{b}={buf}+{off}/{stride}");
    }
    let mut ubos = String::new();
    for b in 0..6u32 {
        let mut buf = 0i32;
        let mut off = 0i64;
        unsafe {
            gl::glGetIntegeri_v(0x8A28, b, &mut buf);
            gl::glGetInteger64i_v(0x8A29, b, &mut off);
        }
        ubos += &format!(" u{b}={buf}+{off}");
    }
    let (elements, unified_bits) = with(|c| (c.elements.get(&c.vao).copied(), UNIFIED.load(Relaxed)));
    log!("nvgl: draw: program {program} vao {vao} fbo {fbo} viewport {vp:?} unified {unified_bits:#x} elements {elements:?} error before {err:#x}{bindings}{ubos}");
}

unsafe extern "system" fn debug_message(_source: u32, kind: u32, id: u32, _severity: u32, len: i32, message: *const u8, _user: *const c_void) {
    static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    if kind != 0x824C || message.is_null() {
        return;
    }
    let text = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(message, len.max(0) as usize) }).into_owned();
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.len() < 64 && !seen.contains(&text) {
        log!("nvgl: GL error {id}: {text}");
        seen.push(text);
    }
}

/// The 64-bit integer uniforms (`uvec2` once rewritten) the game gives handles to, by program
/// and location: set as `uvec2` values (ARB takes handles for samplers only).
static INTEGER_HANDLES: Mutex<Vec<(u32, i32, bool)>> = Mutex::new(Vec::new());

unsafe extern "system" fn hook_program_uniform_handle(program: u32, location: i32, value: u64) {
    unsafe { hook_program_uniform_handles(program, location, 1, &value) }
}

unsafe extern "system" fn hook_uniform_handles(location: i32, count: i32, values: *const u64) {
    unsafe { hook_program_uniform_handles(get_i(0x8B8D) as u32, location, count, values) }
}

unsafe extern "system" fn hook_uniform_handle(location: i32, value: u64) {
    unsafe { hook_program_uniform_handles(get_i(0x8B8D) as u32, location, 1, &value) }
}

/// The handles given to each program's uniforms (`WAL_NVGL_SYNC`), by program and location.
static UNIFORM_HANDLES: Mutex<Option<HashMap<(u32, i32), u64>>> = Mutex::new(None);

unsafe extern "system" fn hook_program_uniform_handles(program: u32, location: i32, count: i32, values: *const u64) {
    if location >= 0 && count > 0 && !values.is_null() {
        let mut set = UNIFORM_HANDLES.lock().unwrap_or_else(|e| e.into_inner());
        let set = set.get_or_insert_with(HashMap::new);
        for (i, &v) in unsafe { std::slice::from_raw_parts(values, count as usize) }.iter().enumerate() {
            set.insert((program, location + i as i32), v);
        }
    }
    let known = INTEGER_HANDLES.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|e| e.0 == program && e.1 == location).map(|e| e.2);
    match known {
        Some(true) => unsafe { gl::glProgramUniform2uiv(program, location, count, values.cast()) },
        Some(false) => unsafe { gl::glProgramUniformHandleui64vARB(program, location, count, values) },
        None => {
            unsafe { gl::glGetError() };
            unsafe { gl::glProgramUniformHandleui64vARB(program, location, count, values) };
            let integer = unsafe { gl::glGetError() } == 0x0502;
            if integer {
                unsafe { gl::glProgramUniform2uiv(program, location, count, values.cast()) };
            }
            INTEGER_HANDLES.lock().unwrap_or_else(|e| e.into_inner()).push((program, location, integer));
        }
    }
}

// --- pointers ---

/// The pointers of a linked program: their uniform block's binding and offset.
unsafe fn program_pointers(program: u32) {
    let names = POINTERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut found = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let member = format!("nvptr_{name}\0");
        let index = unsafe { gl::glGetProgramResourceIndex(program, 0x92E1, member.as_ptr()) };
        if index == u32::MAX {
            continue;
        }
        // GL_OFFSET, GL_BLOCK_INDEX
        let mut v = [0i32; 2];
        unsafe { gl::glGetProgramResourceiv(program, 0x92E1, index, 2, [0x92FC, 0x92FD].as_ptr(), 2, std::ptr::null_mut(), v.as_mut_ptr()) };
        if v[1] < 0 {
            continue;
        }
        // GL_BUFFER_BINDING
        let mut binding = 0i32;
        unsafe { gl::glGetProgramResourceiv(program, 0x92E2, v[1] as u32, 1, [0x9302].as_ptr(), 1, std::ptr::null_mut(), &mut binding) };
        found.push((FIRST_POINTER_BINDING + i as u32, binding as u32, v[0] as u64));
    }
    if !found.is_empty() {
        trace!(16, "nvgl: program {program} pointers {found:?}");
    }
    with(|c| {
        if found.is_empty() {
            c.pointers.remove(&program);
        } else {
            c.pointers.insert(program, found);
        }
    });
}

/// The storage buffers of the current program's pointers bound where they point (their values
/// read in the bound uniform buffers).
unsafe fn update_pointers() {
    let n = DRAWS.fetch_add(1, Relaxed);
    if FRAME_LOG.load(Relaxed) {
        let (vao, program, ubo) = with(|c| (c.vao, c.program, { let mut u: Vec<_> = c.uniforms.iter().map(|(k, v)| (*k, *v)).collect(); u.sort(); u }));
        log!("nvgl:  draw: vertex array {vao}, program {program}, uniforms {ubo:?}, unified {}/{}", unified(0x8F1E) as u8, unified(0x8F1F) as u8);
    }
    if LOG.load(Relaxed) && n == 1000 {
        static ONCE: AtomicBool = AtomicBool::new(false);
        if !ONCE.swap(true, Relaxed) {
            DEBUG_DRAWS.store(40, Relaxed);
        }
    }
    if SYNC.load(Relaxed) {
        unsafe { gl::glFinish() };
        unsafe { debug_draw() };
        unsafe { debug_handles() };
    } else if DEBUG_DRAWS.load(Relaxed) > 0 {
        DEBUG_DRAWS.fetch_sub(1, Relaxed);
        unsafe { debug_draw() };
    }
    let Some((pointers, uniforms)) = with(|c| c.pointers.get(&c.program).map(|p| (p.clone(), p.iter().map(|&(_, u, _)| c.uniforms.get(&u).copied()).collect::<Vec<_>>()))) else { return };
    for (&(storage, _, offset), uniform) in pointers.iter().zip(uniforms) {
        let Some((buffer, base)) = uniform else { continue };
        if buffer == 0 {
            continue;
        }
        let value = u64_at(&read(((buffer as u64) << 32) | (base + offset), 8), 0);
        let (target, at) = decode(value);
        if target == 0 {
            continue;
        }
        let size = with(|c| c.buffers.get(&target).map(|b| b.size)).unwrap_or_else(|| {
            let mut s = 0;
            unsafe { gl::glGetNamedBufferParameteriv(target, 0x8764, &mut s) };
            s as u64
        });
        trace!(16, "nvgl: pointer binding {storage}: buffer {target} + {at} (of {size})");
        ftrace!("nvgl:  pointer {storage} = {target}+{at}");
        unsafe { gl::glBindBufferRange(SHADER_STORAGE_BUFFER, storage, target, at as isize, size.saturating_sub(at).max(4) as isize) };
    }
}

unsafe extern "system" fn hook_use_program(program: u32) {
    ftrace!("nvgl: program {program}");
    with(|c| c.program = program);
    unsafe { gl::glUseProgram(program) }
}

unsafe extern "system" fn hook_bind_buffer_range(target: u32, index: u32, buffer: u32, offset: isize, size: isize) {
    ftrace!("nvgl: glBindBufferRange({target:#x}, {index}, {buffer}, {offset}, {size})");
    if target == UNIFORM_BUFFER {
        with(|c| c.uniforms.insert(index, (buffer, offset as u64)));
    }
    unsafe { gl::glBindBufferRange(target, index, buffer, offset, size) }
}

unsafe extern "system" fn hook_bind_buffer_base(target: u32, index: u32, buffer: u32) {
    ftrace!("nvgl: glBindBufferBase({target:#x}, {index}, {buffer})");
    if target == UNIFORM_BUFFER {
        with(|c| c.uniforms.insert(index, (buffer, 0)));
    }
    unsafe { gl::glBindBufferBase(target, index, buffer) }
}

unsafe extern "system" fn hook_draw_arrays(mode: u32, first: i32, count: i32) {
    unsafe { update_pointers() };
    unsafe { gl::glDrawArrays(mode, first, count) }
}

unsafe extern "system" fn hook_draw_arrays_instanced(mode: u32, first: i32, count: i32, instances: i32) {
    unsafe { update_pointers() };
    unsafe { gl::glDrawArraysInstanced(mode, first, count, instances) }
}

unsafe extern "system" fn hook_multi_draw_arrays(mode: u32, first: *const i32, count: *const i32, draws: i32) {
    unsafe { update_pointers() };
    unsafe { gl::glMultiDrawArrays(mode, first, count, draws) }
}

// --- WGL_NV_DX_interop ---
//
// KanColle Arcade's movies: decoded and converted by DXVA2 into Direct3D 9 render targets, shown
// as OpenGL textures through `WGL_NV_DX_interop` (absent from Mesa; Direct3D 9 is DXVK). A
// registered surface gives its OpenGL texture (or renderbuffer) its size; each lock copies the
// surface's pixels into it (`GetRenderTargetData` into a system memory surface, rows in Direct3D
// order as the shared memory would be).

/// A registered Direct3D surface: its level 0 surface, the OpenGL object, its system memory copy.
struct DxObject {
    surface: usize,
    name: u32,
    target: u32,
    width: u32,
    height: u32,
    format: u32,
    system: usize,
}

unsafe impl Send for DxObject {}

static DX_OBJECTS: Mutex<Vec<Option<DxObject>>> = Mutex::new(Vec::new());

/// A COM method of `object` (its vtable's `index`th entry).
unsafe fn com<F: Copy>(object: usize, index: usize) -> F {
    let vtable = unsafe { *(object as *const *const usize) };
    unsafe { std::mem::transmute_copy(&*vtable.add(index)) }
}

unsafe fn com_release(object: usize) {
    if object != 0 {
        let release: unsafe extern "system" fn(usize) -> u32 = unsafe { com(object, 2) };
        unsafe { release(object) };
    }
}

const IID_DIRECT3D_TEXTURE9: [u8; 16] = [0x27, 0x12, 0xc3, 0x85, 0xe5, 0x3d, 0x00, 0x4f, 0x9b, 0x3a, 0xf1, 0x1a, 0xc3, 0x8c, 0x18, 0xb5];

/// The surface of a Direct3D 9 object (a texture's level 0, or the surface), referenced.
unsafe fn dx_surface(object: usize) -> usize {
    let query: unsafe extern "system" fn(usize, *const u8, *mut usize) -> i32 = unsafe { com(object, 0) };
    let mut texture = 0usize;
    if unsafe { query(object, IID_DIRECT3D_TEXTURE9.as_ptr(), &mut texture) } >= 0 && texture != 0 {
        // IDirect3DTexture9::GetSurfaceLevel
        let level: unsafe extern "system" fn(usize, u32, *mut usize) -> i32 = unsafe { com(texture, 18) };
        let mut surface = 0usize;
        let hr = unsafe { level(texture, 0, &mut surface) };
        unsafe { com_release(texture) };
        return if hr >= 0 { surface } else { 0 };
    }
    let add_ref: unsafe extern "system" fn(usize) -> u32 = unsafe { com(object, 1) };
    unsafe { add_ref(object) };
    object
}

unsafe extern "system" fn dx_set_resource_share_handle(_object: *mut c_void, _share: *mut c_void) -> i32 {
    1
}

unsafe extern "system" fn dx_open_device(device: usize) -> usize {
    log!("nvgl: wglDXOpenDeviceNV({device:#x}): emulated (copies)");
    if device == 0 { 1 } else { device }
}

unsafe extern "system" fn dx_close_device(_device: usize) -> i32 {
    1
}

unsafe extern "system" fn dx_register_object(_device: usize, object: usize, name: u32, target: u32, _access: u32) -> usize {
    if object == 0 {
        return 0;
    }
    let surface = unsafe { dx_surface(object) };
    if surface == 0 {
        log!("nvgl: wglDXRegisterObjectNV({object:#x}): no surface");
        return 0;
    }
    // IDirect3DSurface9::GetDesc: format, type, usage, pool, multisample type and quality, width,
    // height
    let get_desc: unsafe extern "system" fn(usize, *mut [u32; 8]) -> i32 = unsafe { com(surface, 12) };
    let mut desc = [0u32; 8];
    unsafe { get_desc(surface, &mut desc) };
    let (format, width, height) = (desc[0], desc[6], desc[7]);
    unsafe {
        if target == 0x8D41 {
            gl::glBindRenderbuffer(0x8D41, name);
            gl::glRenderbufferStorage(0x8D41, 0x8058, width as i32, height as i32);
        } else {
            let bound = get_i(0x8069); // GL_TEXTURE_BINDING_2D
            gl::glBindTexture(0x0DE1, name);
            gl::glTexImage2D(0x0DE1, 0, 0x8058, width as i32, height as i32, 0, 0x80E1, 0x1401, std::ptr::null());
            // a single level: complete whatever its filters (an incomplete texture samples black)
            gl::glTexParameteri(0x0DE1, 0x813D, 0); // GL_TEXTURE_MAX_LEVEL
            gl::glBindTexture(0x0DE1, bound as u32);
        }
    }
    log!("nvgl: wglDXRegisterObjectNV({object:#x}, {name}, {target:#x}): {width}x{height} format {format}");
    let mut objects = DX_OBJECTS.lock().unwrap_or_else(|e| e.into_inner());
    objects.push(Some(DxObject { surface, name, target, width, height, format, system: 0 }));
    objects.len()
}

unsafe extern "system" fn dx_unregister_object(_device: usize, handle: usize) -> i32 {
    let mut objects = DX_OBJECTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(Some(o)) = objects.get_mut(handle.wrapping_sub(1)).map(Option::take) {
        unsafe {
            com_release(o.system);
            com_release(o.surface);
        }
    }
    1
}

unsafe extern "system" fn dx_object_access(_handle: usize, _access: u32) -> i32 {
    1
}

/// A surface's pixels copied into its OpenGL texture (32-bit formats).
unsafe fn dx_copy(o: &mut DxObject) {
    if o.target == 0x8D41 || !matches!(o.format, 21 | 22) {
        static ONCE: AtomicBool = AtomicBool::new(false);
        if !ONCE.swap(true, Relaxed) {
            log!("nvgl: interop object format {} target {:#x}: not copied", o.format, o.target);
        }
        return;
    }
    unsafe {
        // IDirect3DResource9::GetDevice
        let get_device: unsafe extern "system" fn(usize, *mut usize) -> i32 = com(o.surface, 3);
        let mut device = 0usize;
        if get_device(o.surface, &mut device) < 0 || device == 0 {
            return;
        }
        if o.system == 0 {
            // IDirect3DDevice9::CreateOffscreenPlainSurface (D3DPOOL_SYSTEMMEM)
            let create: unsafe extern "system" fn(usize, u32, u32, u32, u32, *mut usize, *mut c_void) -> i32 = com(device, 36);
            create(device, o.width, o.height, o.format, 2, &mut o.system, std::ptr::null_mut());
        }
        // IDirect3DDevice9::GetRenderTargetData, else the surface itself
        let get_data: unsafe extern "system" fn(usize, usize, usize) -> i32 = com(device, 32);
        let copied = if o.system != 0 { get_data(device, o.surface, o.system) } else { -1 };
        let source = if copied >= 0 { o.system } else { o.surface };
        com_release(device);
        // IDirect3DSurface9::LockRect / UnlockRect (D3DLOCK_READONLY)
        let lock: unsafe extern "system" fn(usize, *mut [usize; 2], *const c_void, u32) -> i32 = com(source, 13);
        let unlock: unsafe extern "system" fn(usize) -> i32 = com(source, 14);
        let mut locked = [0usize; 2];
        let locked_hr = lock(source, &mut locked, std::ptr::null(), 0x10);
        static LOGGED: AtomicU32 = AtomicU32::new(0);
        let n = LOGGED.fetch_add(1, Relaxed);
        if n < 1 {
            let first = if locked_hr >= 0 && locked[1] != 0 {
                let row = std::slice::from_raw_parts((locked[1] + (o.height as usize / 2) * locked[0]) as *const u32, o.width as usize);
                format!("{:08x} {:08x} {:08x}", row[0], row[o.width as usize / 2], row[o.width as usize - 1])
            } else {
                String::new()
            };
            log!("nvgl: interop copy {}: GetRenderTargetData {copied:#x}, lock {locked_hr:#x}, pitch {}, middle row {first}", o.name, locked[0] as u32);
        }
        if locked_hr < 0 {
            return;
        }
        let (pitch, bits) = (locked[0] as u32 as i32, locked[1]);
        let unpack = get_i(0x88EF); // GL_PIXEL_UNPACK_BUFFER_BINDING
        gl::glBindBuffer(0x88EC, 0);
        gl::glPixelStorei(0x0CF2, pitch / 4); // GL_UNPACK_ROW_LENGTH
        gl::glTextureSubImage2D(o.name, 0, 0, 0, o.width as i32, o.height as i32, 0x80E1, 0x1401, bits as *const c_void);
        gl::glPixelStorei(0x0CF2, 0);
        gl::glBindBuffer(0x88EC, unpack as u32);
        unlock(source);
    }
}

unsafe extern "system" fn dx_lock_objects(_device: usize, count: i32, handles: *const usize) -> i32 {
    if count <= 0 || handles.is_null() {
        return 1;
    }
    let handles = unsafe { std::slice::from_raw_parts(handles, count as usize) };
    let mut objects = DX_OBJECTS.lock().unwrap_or_else(|e| e.into_inner());
    for &h in handles {
        if let Some(Some(o)) = objects.get_mut(h.wrapping_sub(1)) {
            unsafe { dx_copy(o) };
        }
    }
    1
}

unsafe extern "system" fn dx_unlock_objects(_device: usize, _count: i32, _handles: *const usize) -> i32 {
    1
}
