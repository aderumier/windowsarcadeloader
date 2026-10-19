//! `#include` in shaders compiled from memory without an include handler.
//!
//! The game's `D3DXCompileShader` calls with a NULL `ID3DXInclude` get one that reads the
//! included file from the current directory, as native d3dx9 does: Wine's compiler fails such
//! a shader instead ("#include" without a handler). SEGA Golden Gun compiles its blur shaders
//! (`#include "../fs/shader/PostEffect.fx"`) this way: the effect stayed NULL, crash at boot.
//! Calls with the game's own handler are left alone.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{iat, log};

static ORIG_COMPILE: AtomicUsize = AtomicUsize::new(0);

type CompileShader = unsafe extern "system" fn(
    src: *const u8, len: u32, defines: *const u8, include: *const Include, function: *const u8,
    profile: *const u8, flags: u32, shader: *mut usize, errors: *mut usize, constants: *mut usize,
) -> i32;

/// `ID3DXInclude`: a vtable of `Open` and `Close` (no IUnknown).
#[repr(C)]
struct Include {
    vtbl: *const IncludeVtbl,
}

#[repr(C)]
struct IncludeVtbl {
    open: unsafe extern "system" fn(*const Include, u32, *const i8, *const u8, *mut *const u8, *mut u32) -> i32,
    close: unsafe extern "system" fn(*const Include, *const u8) -> i32,
}

unsafe impl Sync for Include {}

static VTBL: IncludeVtbl = IncludeVtbl { open, close };
static FILE_INCLUDE: Include = Include { vtbl: &VTBL };

const E_FAIL: i32 = 0x80004005u32 as i32;

pub fn init() {
    let f = compile_shader as *const () as usize;
    for v in 24..=43 {
        if let Some(orig) = unsafe { iat::hook(&format!("d3dx9_{v}.dll"), "D3DXCompileShader", f) } {
            ORIG_COMPILE.store(orig, Ordering::Relaxed);
            log!("d3dx: d3dx9_{v}.dll!D3DXCompileShader: includes from the current directory");
            return;
        }
    }
}

unsafe extern "system" fn compile_shader(
    src: *const u8, len: u32, defines: *const u8, include: *const Include, function: *const u8,
    profile: *const u8, flags: u32, shader: *mut usize, errors: *mut usize, constants: *mut usize,
) -> i32 {
    let orig: CompileShader = unsafe { std::mem::transmute(ORIG_COMPILE.load(Ordering::Relaxed)) };
    let include = if include.is_null() { &FILE_INCLUDE as *const Include } else { include };
    unsafe { orig(src, len, defines, include, function, profile, flags, shader, errors, constants) }
}

unsafe extern "system" fn open(
    _this: *const Include, _kind: u32, name: *const i8, _parent: *const u8, data: *mut *const u8,
    bytes: *mut u32,
) -> i32 {
    let name = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned();
    match std::fs::read(&name) {
        Ok(content) => {
            let len = content.len() as u32;
            // freed in close: a boxed slice, its length stored before the data
            let mut buf = Vec::with_capacity(content.len() + 4);
            buf.extend_from_slice(&len.to_le_bytes());
            buf.extend_from_slice(&content);
            let ptr = Box::into_raw(buf.into_boxed_slice()) as *mut u8;
            unsafe {
                *data = ptr.add(4);
                *bytes = len;
            }
            0
        }
        Err(e) => {
            log!("d3dx: #include {name}: {e}");
            E_FAIL
        }
    }
}

unsafe extern "system" fn close(_this: *const Include, data: *const u8) -> i32 {
    if !data.is_null() {
        unsafe {
            let start = data.sub(4) as *mut u8;
            let len = u32::from_le_bytes(*(start as *const [u8; 4])) as usize;
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(start, len + 4)));
        }
    }
    0
}
