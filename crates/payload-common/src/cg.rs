//! NVIDIA Cg shaders on other GPUs (Initial D Arcade Stage 6 AA and 7 AAX, OpenGL games whose shaders
//! are compiled at run time with Cg 1.5, `cg.dll`, or read from caches, loaded as ARB assembly
//! with NVIDIA options).
//!
//! `WAL_RINGEDGE_CG_FIX=auto|1|0` (`auto`: only without an NVIDIA GPU, from the Linux side's
//! `/sys/class/drm/*/device/vendor`; NVIDIA GPUs keep everything as is). Mesa has no NVIDIA
//! program options, and the game relies on NVIDIA driver behaviors:
//! * cgGL reports the vp40 / fp40 profiles supported ([`nv40_profiles`]): the game's NV40
//!   rendering path.
//! * `cgCreateProgram`: the NVIDIA profiles compiled as the ARB ones (fp30/fp40 -> arbfp1,
//!   vp30/vp40 -> arbvp1), the vertex sources adapted ([`vertex_source`]), Cg's limit of 96
//!   constants raised (bone palettes); `cgGetProgramString`: fragment programs without shadow
//!   samplers ([`fragment_text`]).
//! * Precompiled programs and shader caches (`*.asm_gl`, `kernelbase!CreateFileW` hot patched:
//!   every reader): fixed copies in `cgfix\` of the run directory (the dump keeps the
//!   originals), NVIDIA assembly translated ([`crate::nvasm`]); the caches it cannot translate
//!   made stale (`GetFileAttributesExW`), the game compiles their sources (the caches it then
//!   writes go to `cgfix\compiled\`, not into the dump).
//! * Parameters: the game writes environment ones, its programs read local ones
//!   ([`env_parameters`]).
//! * Vertex inputs: the game feeds generic attributes, its programs read the fixed function
//!   ones ([`attrib_pointer`]).
//! * Vertex textures (none on Mesa): the texels read at a constant position given as local
//!   parameters before each draw ([`texel_parameters`]).
//! * Texture reads at a level (`TXL`, a bias in ARB): the last level of each texture given
//!   before each draw ([`level_parameters`]).
//! * NV_parameter_buffer_object (Project DIVA Arcade Future Tone's bone matrices, absent in
//!   Mesa): the buffers of its vertex target kept by the payload, given to the programs reading
//!   them as local parameters before each draw ([`palette_parameters`]).
//!
//! 64-bit games (Nu: no hot patch prologues): the swaps through GLUT's `SwapBuffers` import.
//!
//! Debugging: `WAL_RINGEDGE_CG_LOG=1` logs each compile and Cg's listing of the failed ones;
//! `WAL_RINGEDGE_CG_DUMP=1` writes each compiled program to `cgdump\`;
//! `WAL_RINGEDGE_GL_SHOT=<seconds>` writes the back buffer every `<seconds>` (`glshot-NNNN.bmp`,
//! no screen capture of OpenGL games under Wayland) and each program given to GL (`glprog\`).

use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{iat, log};

static ORIG: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_STRING: AtomicUsize = AtomicUsize::new(0);
static ORIG_ATTRIBUTES: AtomicUsize = AtomicUsize::new(0);
/// The rewritten fragment programs returned to the game, by program.
static FRAGMENTS: std::sync::Mutex<Vec<(usize, std::ffi::CString)>> = std::sync::Mutex::new(Vec::new());
static LOG: OnceLock<bool> = OnceLock::new();

type CreateProgram = unsafe extern "C" fn(*mut c_void, i32, *const c_char, i32, *const c_char, *const *const c_char) -> *mut c_void;
type CreateFileW = unsafe extern "system" fn(*const u16, u32, u32, *mut c_void, u32, u32, *mut c_void) -> *mut c_void;

const ARBVP1: i32 = 6150;
const ARBFP1: i32 = 7000;

/// An NVIDIA GPU on the machine (PCI vendor 0x10de).
fn nvidia() -> bool {
    let Ok(dir) = std::fs::read_dir("Z:\\sys\\class\\drm") else { return false };
    dir.flatten().any(|e| std::fs::read_to_string(e.path().join("device").join("vendor")).is_ok_and(|v| v.trim() == "0x10de"))
}

/// `WAL_RINGEDGE_GL_SHOT=<seconds>` (debugging): the back buffer before each swap, every
/// `<seconds>`, as `glshot-NNNN.bmp` in the run directory (no screen capture of OpenGL games
/// under Wayland).
fn gl_shots() {
    let shots = std::env::var("WAL_RINGEDGE_GL_SHOT").ok().and_then(|v| v.trim().parse::<f64>().ok());
    // the swaps also scale the picture (WAL_RINGEDGE_GL_SCALE)
    let scale = crate::glscale::init();
    // the touch pointer (the common crosshair, Project DIVA Arcade's red dot) over the picture
    CROSSHAIRS.store(crate::crosshair::init_gl(), Ordering::Relaxed);
    if shots.is_none() && !scale && !CROSSHAIRS.load(Ordering::Relaxed) {
        return;
    }
    let _ = unsafe { windows_sys::Win32::System::LibraryLoader::LoadLibraryA(c"opengl32.dll".as_ptr().cast()) };
    match unsafe { hotpatch(c"opengl32.dll", c"wglSwapBuffers", swap_buffers as *const () as usize) } {
        Some(o) => ORIG_SWAP.store(o, Ordering::Relaxed),
        // 64-bit (no hot patch prologues): GLUT's swaps (Project DIVA Arcade Future Tone's
        // glut32.dll, gdi32's SwapBuffers)
        None => {
            let glut = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(c"glut32.dll".as_ptr().cast()) } as usize;
            match (glut != 0).then(|| unsafe { iat::hook_module(glut, "gdi32.dll", "SwapBuffers", swap_buffers as *const () as usize) }).flatten() {
                Some(o) => ORIG_SWAP.store(o, Ordering::Relaxed),
                None => log!("cg: opengl32.dll!wglSwapBuffers not hot patchable"),
            }
        }
    }
    let Some(secs) = shots else { return };
    SHOT_MS.store((secs * 1000.0).max(1.0) as usize, Ordering::Relaxed);
    DUMP_PROGRAMS.store(true, Ordering::Relaxed);
    gl_procs();
}

/// The GL functions the game and cgGL get by name: [`gl_proc`].
fn gl_procs() {
    let real = unsafe { windows_sys::Win32::System::LibraryLoader::GetProcAddress(windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(c"opengl32.dll".as_ptr().cast()), c"wglGetProcAddress".as_ptr().cast()) };
    ORIG_GL_PROC.store(real.map_or(0, |f| f as usize), Ordering::Relaxed);
    let cggl = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(c"cgGL.dll".as_ptr().cast()) } as usize;
    if cggl != 0 {
        unsafe { iat::hook_module(cggl, "opengl32.dll", "wglGetProcAddress", gl_proc as *const () as usize) };
    }
    // the games loading their programs themselves (Project DIVA Arcade's GLEW)
    unsafe { iat::hook("opengl32.dll", "wglGetProcAddress", gl_proc as *const () as usize) };
    if let Some(o) = unsafe { iat::hook("kernel32.dll", "GetProcAddress", get_proc as *const () as usize) } {
        ORIG_GET_PROC.store(o, Ordering::Relaxed);
    }
}

static ORIG_GET_PROC: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn get_proc(module: *mut c_void, name: *const c_char) -> usize {
    let orig: unsafe extern "system" fn(*mut c_void, *const c_char) -> usize = unsafe { std::mem::transmute(ORIG_GET_PROC.load(Ordering::Relaxed)) };
    let f = unsafe { orig(module, name) };
    // names only (not ordinals)
    if f != 0 && name as usize > 0xffff {
        match unsafe { CStr::from_ptr(name) }.to_bytes() {
            b"wglGetProcAddress" => return gl_proc as *const () as usize,
            b"glBindProgramARB" | b"glProgramStringARB" | b"glProgramEnvParameters4fvEXT" | b"glVertexAttribPointer" | b"glVertexAttribPointerARB" | b"glEnableVertexAttribArray"
            | b"glEnableVertexAttribArrayARB" | b"glDisableVertexAttribArray" | b"glDisableVertexAttribArrayARB" | b"glDrawRangeElements" | b"glDrawRangeElementsEXT" => return unsafe { gl_proc(name) },
            _ => {}
        }
    }
    f
}

static ORIG_GL_PROC: AtomicUsize = AtomicUsize::new(0);
static ORIG_BIND: AtomicUsize = AtomicUsize::new(0);
static ORIG_STRING: AtomicUsize = AtomicUsize::new(0);
static BOUND: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(0)];

unsafe extern "system" fn gl_proc(name: *const c_char) -> usize {
    let orig: unsafe extern "system" fn(*const c_char) -> usize = unsafe { std::mem::transmute(ORIG_GL_PROC.load(Ordering::Relaxed)) };
    let f = unsafe { orig(name) };
    if name.is_null() {
        return f;
    }
    if f == 0 {
        let name = unsafe { CStr::from_ptr(name) };
        // NVIDIA's coverage antialiasing (Project DIVA Arcade's buffers, called unchecked): the
        // color samples
        if name.to_bytes() == b"glRenderbufferStorageMultisampleCoverageNV" && FIX.load(Ordering::Relaxed) {
            let ms = unsafe { orig(c"glRenderbufferStorageMultisample".as_ptr()) };
            if ms != 0 {
                ORIG_MULTISAMPLE.store(ms, Ordering::Relaxed);
                return multisample_coverage as *const () as usize;
            }
        }
        // NV_parameter_buffer_object's (Project DIVA Arcade Future Tone's, called unchecked):
        // emulated ([`palette_parameters`])
        if FIX.load(Ordering::Relaxed) {
            match name.to_bytes() {
                b"glBindBufferBaseNV" => return bind_buffer_base_nv as *const () as usize,
                b"glBindBufferRangeNV" => return bind_buffer_range_nv as *const () as usize,
                b"glBindBufferOffsetNV" => return bind_buffer_offset_nv as *const () as usize,
                b"glProgramBufferParametersfvNV" | b"glProgramBufferParametersIivNV" | b"glProgramBufferParametersIuivNV" => return program_buffer_parameters as *const () as usize,
                _ => {}
            }
        }
        // NV_gpu_program4's integer parameters (Border Break Scramble's, called unchecked at its
        // battle's start): the float ones, its programs translated to float ARB programs
        if FIX.load(Ordering::Relaxed)
            && let Some(f) = int_parameters(name.to_bytes(), &orig)
        {
            return f;
        }
        if *LOG.get().unwrap_or(&false) {
            log!("cg: {} absent", name.to_string_lossy());
        }
        return f;
    }
    match unsafe { CStr::from_ptr(name) }.to_bytes() {
        b"glBindProgramARB" => {
            ORIG_BIND.store(f, Ordering::Relaxed);
            bind_program as *const () as usize
        }
        b"glProgramStringARB" => {
            ORIG_STRING.store(f, Ordering::Relaxed);
            program_string as *const () as usize
        }
        b"glDrawRangeElements" | b"glDrawRangeElementsEXT" if FIX.load(Ordering::Relaxed) => {
            ORIG_DRAW_RANGE.store(f, Ordering::Relaxed);
            draw_range as *const () as usize
        }
        b"glVertexAttribPointer" | b"glVertexAttribPointerARB" if FIX.load(Ordering::Relaxed) => {
            ORIG_ATTRIB_POINTER.store(f, Ordering::Relaxed);
            attrib_pointer as *const () as usize
        }
        b"glEnableVertexAttribArray" | b"glEnableVertexAttribArrayARB" if FIX.load(Ordering::Relaxed) => {
            ORIG_ENABLE_ATTRIB.store(f, Ordering::Relaxed);
            enable_attrib as *const () as usize
        }
        b"glDisableVertexAttribArray" | b"glDisableVertexAttribArrayARB" if FIX.load(Ordering::Relaxed) => {
            ORIG_DISABLE_ATTRIB.store(f, Ordering::Relaxed);
            disable_attrib as *const () as usize
        }
        // ... and their constant values (an object's index with its array disabled)
        n if n.starts_with(b"glVertexAttribI") && !n.ends_with(b"PointerEXT") && !n.ends_with(b"Pointer") && std::env::var("WAL_RINGEDGE_GL_INT_ATTRIBS").is_ok_and(|v| v.trim() == "1") => {
            int_attrib_value(n, &orig).unwrap_or(f)
        }
        b"glVertexAttribIPointerEXT" | b"glVertexAttribIPointer" if std::env::var("WAL_RINGEDGE_GL_INT_ATTRIBS").is_ok_and(|v| v.trim() == "1") => {
            let float = unsafe { orig(c"glVertexAttribPointerARB".as_ptr()) };
            if float == 0 {
                return f;
            }
            ORIG_FLOAT_POINTER.store(float, Ordering::Relaxed);
            int_attrib_pointer as *const () as usize
        }
        // instanced draws: ARB programs have no instance (NV_gpu_program4's `vertex.instance`,
        // `nvasm::INSTANCE_LOCAL`): one draw per instance, its number in that local parameter
        // (Border Break Scramble's scenery: else every copy drawn at the first one's place)
        b"glDrawElementsInstanced" | b"glDrawElementsInstancedARB" | b"glDrawElementsInstancedEXT" if FIX.load(Ordering::Relaxed) => draw_elements_instanced as *const () as usize,
        b"glDrawArraysInstanced" | b"glDrawArraysInstancedARB" | b"glDrawArraysInstancedEXT" if FIX.load(Ordering::Relaxed) => draw_arrays_instanced as *const () as usize,
        // NV_parameter_buffer_object (absent in Mesa): the vertex parameter buffers kept by the
        // payload ([`palette_parameters`])
        b"glBindBuffer" | b"glBindBufferARB" if FIX.load(Ordering::Relaxed) => hook(&ORIG_BIND_BUFFER, f, bind_buffer as *const () as usize),
        b"glBufferData" | b"glBufferDataARB" if FIX.load(Ordering::Relaxed) => hook(&ORIG_BUFFER_DATA, f, buffer_data as *const () as usize),
        b"glBufferSubData" | b"glBufferSubDataARB" if FIX.load(Ordering::Relaxed) => hook(&ORIG_BUFFER_SUB_DATA, f, buffer_sub_data as *const () as usize),
        b"glMapBuffer" | b"glMapBufferARB" if FIX.load(Ordering::Relaxed) => hook(&ORIG_MAP_BUFFER, f, map_buffer as *const () as usize),
        b"glMapBufferRange" if FIX.load(Ordering::Relaxed) => hook(&ORIG_MAP_BUFFER_RANGE, f, map_buffer_range as *const () as usize),
        b"glUnmapBuffer" | b"glUnmapBufferARB" if FIX.load(Ordering::Relaxed) => hook(&ORIG_UNMAP_BUFFER, f, unmap_buffer as *const () as usize),
        b"glProgramEnvParameter4fARB" if FIX.load(Ordering::Relaxed) => {
            ORIG_ENV_PARAMETER_4F.store(f, Ordering::Relaxed);
            env_parameter_4f as *const () as usize
        }
        b"glProgramEnvParameter4fvARB" if FIX.load(Ordering::Relaxed) => {
            ORIG_ENV_PARAMETER_4FV.store(f, Ordering::Relaxed);
            env_parameter_4fv as *const () as usize
        }
        b"glProgramEnvParameters4fvEXT" if FIX.load(Ordering::Relaxed) => {
            ORIG_ENVS.store(f, Ordering::Relaxed);
            let local = unsafe { orig(c"glProgramLocalParameters4fvEXT".as_ptr()) };
            ORIG_LOCALS.store(local, Ordering::Relaxed);
            env_parameters as *const () as usize
        }
        _ => f,
    }
}

static ORIG_MULTISAMPLE: AtomicUsize = AtomicUsize::new(0);
static ORIG_FLOAT_POINTER: AtomicUsize = AtomicUsize::new(0);

static ORIG_ATTRIB_4F: AtomicUsize = AtomicUsize::new(0);

/// `glVertexAttribI{1,4}{i,ui}{,v}[EXT]` as `glVertexAttrib4fARB`.
fn int_attrib_value(name: &[u8], orig: &unsafe extern "system" fn(*const c_char) -> usize) -> Option<usize> {
    let rest = name.strip_prefix(b"glVertexAttribI")?;
    let rest = rest.strip_suffix(b"EXT").unwrap_or(rest);
    let hook: usize = match rest {
        b"1i" => attrib_i1i as *const () as usize,
        b"1ui" => attrib_i1ui as *const () as usize,
        b"4i" => attrib_i4i as *const () as usize,
        b"4ui" => attrib_i4ui as *const () as usize,
        b"1iv" => attrib_i1iv as *const () as usize,
        b"1uiv" => attrib_i1uiv as *const () as usize,
        b"4iv" => attrib_i4iv as *const () as usize,
        b"4uiv" => attrib_i4uiv as *const () as usize,
        _ => return None,
    };
    let f = unsafe { orig(c"glVertexAttrib4fARB".as_ptr()) };
    if f == 0 {
        return None;
    }
    ORIG_ATTRIB_4F.store(f, Ordering::Relaxed);
    Some(hook)
}

fn attrib_4f(index: u32, v: [f32; 4]) {
    let f: unsafe extern "system" fn(u32, f32, f32, f32, f32) = unsafe { std::mem::transmute(ORIG_ATTRIB_4F.load(Ordering::Relaxed)) };
    unsafe { f(index, v[0], v[1], v[2], v[3]) }
}

unsafe extern "system" fn attrib_i1i(index: u32, x: i32) {
    attrib_4f(index, [x as f32, 0.0, 0.0, 1.0]);
}
unsafe extern "system" fn attrib_i1ui(index: u32, x: u32) {
    attrib_4f(index, [x as f32, 0.0, 0.0, 1.0]);
}
unsafe extern "system" fn attrib_i4i(index: u32, x: i32, y: i32, z: i32, w: i32) {
    attrib_4f(index, [x as f32, y as f32, z as f32, w as f32]);
}
unsafe extern "system" fn attrib_i4ui(index: u32, x: u32, y: u32, z: u32, w: u32) {
    attrib_4f(index, [x as f32, y as f32, z as f32, w as f32]);
}
unsafe extern "system" fn attrib_i1iv(index: u32, v: *const i32) {
    attrib_4f(index, [unsafe { *v } as f32, 0.0, 0.0, 1.0]);
}
unsafe extern "system" fn attrib_i1uiv(index: u32, v: *const u32) {
    attrib_4f(index, [unsafe { *v } as f32, 0.0, 0.0, 1.0]);
}
unsafe extern "system" fn attrib_i4iv(index: u32, v: *const i32) {
    let v = unsafe { std::slice::from_raw_parts(v, 4) };
    attrib_4f(index, [v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32]);
}
unsafe extern "system" fn attrib_i4uiv(index: u32, v: *const u32) {
    let v = unsafe { std::slice::from_raw_parts(v, 4) };
    attrib_4f(index, [v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32]);
}

/// `glVertexAttribIPointer` as `glVertexAttribPointer`, not normalized.
unsafe extern "system" fn int_attrib_pointer(index: u32, size: i32, kind: u32, stride: i32, pointer: *const c_void) {
    let f: unsafe extern "system" fn(u32, i32, u32, u8, i32, *const c_void) = unsafe { std::mem::transmute(ORIG_FLOAT_POINTER.load(Ordering::Relaxed)) };
    unsafe { f(index, size, kind, 0, stride, pointer) }
}

static ORIG_ENV_4F: AtomicUsize = AtomicUsize::new(0);
static ORIG_LOCAL_4F: AtomicUsize = AtomicUsize::new(0);

/// `glProgram{Env,Local}ParameterI4{i,ui}{,v}NV` (absent on Mesa) as the float
/// `glProgram{Env,Local}Parameter4fARB`.
fn int_parameters(name: &[u8], orig: &unsafe extern "system" fn(*const c_char) -> usize) -> Option<usize> {
    let (local, rest) = match name.strip_prefix(b"glProgram")? {
        r if r.starts_with(b"EnvParameterI4") => (false, &r[14..]),
        r if r.starts_with(b"LocalParameterI4") => (true, &r[16..]),
        _ => return None,
    };
    let (slot, float) = if local { (&ORIG_LOCAL_4F, c"glProgramLocalParameter4fARB") } else { (&ORIG_ENV_4F, c"glProgramEnvParameter4fARB") };
    let f = unsafe { orig(float.as_ptr()) };
    if f == 0 {
        return None;
    }
    slot.store(f, Ordering::Relaxed);
    let hook: usize = match (local, rest) {
        (false, b"iNV") => env_i4i as *const () as usize,
        (false, b"uiNV") => env_i4ui as *const () as usize,
        (false, b"ivNV") => env_i4iv as *const () as usize,
        (false, b"uivNV") => env_i4uiv as *const () as usize,
        (true, b"iNV") => local_i4i as *const () as usize,
        (true, b"uiNV") => local_i4ui as *const () as usize,
        (true, b"ivNV") => local_i4iv as *const () as usize,
        (true, b"uivNV") => local_i4uiv as *const () as usize,
        _ => return None,
    };
    Some(hook)
}

fn parameter_4f(slot: &AtomicUsize, target: u32, index: u32, v: [f32; 4]) {
    let f: unsafe extern "system" fn(u32, u32, f32, f32, f32, f32) = unsafe { std::mem::transmute(slot.load(Ordering::Relaxed)) };
    unsafe { f(target, index, v[0], v[1], v[2], v[3]) }
}

unsafe extern "system" fn env_i4i(target: u32, index: u32, x: i32, y: i32, z: i32, w: i32) {
    parameter_4f(&ORIG_ENV_4F, target, index, [x as f32, y as f32, z as f32, w as f32]);
}
unsafe extern "system" fn env_i4ui(target: u32, index: u32, x: u32, y: u32, z: u32, w: u32) {
    parameter_4f(&ORIG_ENV_4F, target, index, [x as f32, y as f32, z as f32, w as f32]);
}
unsafe extern "system" fn env_i4iv(target: u32, index: u32, v: *const i32) {
    let v = unsafe { std::slice::from_raw_parts(v, 4) };
    parameter_4f(&ORIG_ENV_4F, target, index, [v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32]);
}
unsafe extern "system" fn env_i4uiv(target: u32, index: u32, v: *const u32) {
    let v = unsafe { std::slice::from_raw_parts(v, 4) };
    parameter_4f(&ORIG_ENV_4F, target, index, [v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32]);
}
unsafe extern "system" fn local_i4i(target: u32, index: u32, x: i32, y: i32, z: i32, w: i32) {
    parameter_4f(&ORIG_LOCAL_4F, target, index, [x as f32, y as f32, z as f32, w as f32]);
}
unsafe extern "system" fn local_i4ui(target: u32, index: u32, x: u32, y: u32, z: u32, w: u32) {
    parameter_4f(&ORIG_LOCAL_4F, target, index, [x as f32, y as f32, z as f32, w as f32]);
}
unsafe extern "system" fn local_i4iv(target: u32, index: u32, v: *const i32) {
    let v = unsafe { std::slice::from_raw_parts(v, 4) };
    parameter_4f(&ORIG_LOCAL_4F, target, index, [v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32]);
}
unsafe extern "system" fn local_i4uiv(target: u32, index: u32, v: *const u32) {
    let v = unsafe { std::slice::from_raw_parts(v, 4) };
    parameter_4f(&ORIG_LOCAL_4F, target, index, [v[0] as f32, v[1] as f32, v[2] as f32, v[3] as f32]);
}

/// `glRenderbufferStorageMultisampleCoverageNV` as `glRenderbufferStorageMultisample` with the
/// color samples (the coverage ones without colors when none).
unsafe extern "system" fn multisample_coverage(target: u32, coverage: i32, color: i32, format: u32, width: i32, height: i32) {
    let f: unsafe extern "system" fn(u32, i32, u32, i32, i32) = unsafe { std::mem::transmute(ORIG_MULTISAMPLE.load(Ordering::Relaxed)) };
    unsafe { f(target, if color > 0 { color } else { coverage }, format, width, height) }
}

static ORIG_ENVS: AtomicUsize = AtomicUsize::new(0);
static ORIG_LOCALS: AtomicUsize = AtomicUsize::new(0);

static ORIG_ATTRIB_POINTER: AtomicUsize = AtomicUsize::new(0);
static ORIG_ENABLE_ATTRIB: AtomicUsize = AtomicUsize::new(0);
static ORIG_DISABLE_ATTRIB: AtomicUsize = AtomicUsize::new(0);

/// An `opengl32.dll` export (OpenGL 1.1).
fn gl11(name: &CStr) -> usize {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
    unsafe { GetProcAddress(GetModuleHandleA(c"opengl32.dll".as_ptr().cast()), name.as_ptr().cast()) }.map_or(0, |f| f as usize)
}

/// `glClientActiveTexture`, for the texture coordinate arrays.
fn client_active_texture(unit: u32) {
    static F: AtomicUsize = AtomicUsize::new(0);
    if F.load(Ordering::Relaxed) == 0 {
        let orig: unsafe extern "system" fn(*const c_char) -> usize = unsafe { std::mem::transmute(ORIG_GL_PROC.load(Ordering::Relaxed)) };
        F.store(unsafe { orig(c"glClientActiveTexture".as_ptr()) }, Ordering::Relaxed);
    }
    let f = F.load(Ordering::Relaxed);
    if f != 0 {
        let f: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(f) };
        unsafe { f(0x84C0 + unit) };
    }
}

/// The fixed function array NVIDIA aliases generic attribute `index` with (its client state):
/// 2 normal, 3 color, 8-15 texture coordinates 0-7. Position (0) is aliased everywhere.
fn aliased_array(index: u32) -> Option<u32> {
    match index {
        2 => Some(0x8075),       // GL_NORMAL_ARRAY
        3 => Some(0x8076),       // GL_COLOR_ARRAY
        8..=15 => Some(0x8078),  // GL_TEXTURE_COORD_ARRAY
        _ => None,
    }
}

/// The game feeds its vertex inputs as generic attributes, which NVIDIA aliases onto the fixed
/// function arrays (normal, color, texture coordinates) its programs read, Mesa does not: each
/// generic array also set as the aliased fixed function one.
unsafe extern "system" fn attrib_pointer(index: u32, size: i32, kind: u32, normalized: u8, stride: i32, pointer: *const c_void) {
    let orig: unsafe extern "system" fn(u32, i32, u32, u8, i32, *const c_void) = unsafe { std::mem::transmute(ORIG_ATTRIB_POINTER.load(Ordering::Relaxed)) };
    unsafe { orig(index, size, kind, normalized, stride, pointer) };
    match index {
        2 if size == 3 => {
            let f: unsafe extern "system" fn(u32, i32, *const c_void) = unsafe { std::mem::transmute(gl11(c"glNormalPointer")) };
            unsafe { f(kind, stride, pointer) };
        }
        3 => {
            let f: unsafe extern "system" fn(i32, u32, i32, *const c_void) = unsafe { std::mem::transmute(gl11(c"glColorPointer")) };
            unsafe { f(size, kind, stride, pointer) };
        }
        8..=15 => {
            client_active_texture(index - 8);
            let f: unsafe extern "system" fn(i32, u32, i32, *const c_void) = unsafe { std::mem::transmute(gl11(c"glTexCoordPointer")) };
            unsafe { f(size, kind, stride, pointer) };
            client_active_texture(0);
        }
        _ => {}
    }
}

fn client_state(index: u32, enable: bool) {
    let Some(array) = aliased_array(index) else { return };
    if index >= 8 {
        client_active_texture(index - 8);
    }
    let f: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(gl11(if enable { c"glEnableClientState" } else { c"glDisableClientState" })) };
    unsafe { f(array) };
    if index >= 8 {
        client_active_texture(0);
    }
}

unsafe extern "system" fn enable_attrib(index: u32) {
    let orig: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(ORIG_ENABLE_ATTRIB.load(Ordering::Relaxed)) };
    unsafe { orig(index) };
    client_state(index, true);
}

unsafe extern "system" fn disable_attrib(index: u32) {
    let orig: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(ORIG_DISABLE_ATTRIB.load(Ordering::Relaxed)) };
    unsafe { orig(index) };
    client_state(index, false);
}

/// The game writes its programs' parameters as environment parameters (`program.env`), its
/// programs read local ones (`program.local`, as Cg binds them): fine on NVIDIA, zeros on Mesa
/// (a black screen). Each write also goes to the bound program's locals, unless that program
/// reads environment parameters itself (Project DIVA Arcade's own programs set their locals
/// themselves: its env 4, the fog color, landed on its tone map's local 4, the tone scale).
unsafe extern "system" fn env_parameters(target: u32, index: u32, count: i32, params: *const f32) {
    let env: unsafe extern "system" fn(u32, u32, i32, *const f32) = unsafe { std::mem::transmute(ORIG_ENVS.load(Ordering::Relaxed)) };
    unsafe { env(target, index, count, params) };
    let attrib = crate::nvasm::ATTRIB_ENV;
    if target == 0x8620 && ATTRIB_MIRROR.load(Ordering::Relaxed) && (index..index + count.max(0) as u32).contains(&attrib) {
        unsafe { env(0x8804, attrib, 1, params.add(4 * (attrib - index) as usize)) };
    }
    let local = ORIG_LOCALS.load(Ordering::Relaxed);
    let bound = (target, BOUND[(target == 0x8804) as usize].load(Ordering::Relaxed) as u32);
    if local != 0 && !READS_ENV.lock().unwrap().contains(&bound) {
        let local: unsafe extern "system" fn(u32, u32, i32, *const f32) = unsafe { std::mem::transmute(local) };
        unsafe { local(target, index, count, params) };
    }
}

static ORIG_ENV_PARAMETER_4F: AtomicUsize = AtomicUsize::new(0);
static ORIG_ENV_PARAMETER_4FV: AtomicUsize = AtomicUsize::new(0);
/// Translated NV_gpu_program4 fragment programs read their generic interpolant 0 as the
/// fragment environment parameter `nvasm::ATTRIB_ENV` (Border Break Scramble's object colors):
/// the vertex one of that index copied there.
static ATTRIB_MIRROR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe extern "system" fn env_parameter_4f(target: u32, index: u32, x: f32, y: f32, z: f32, w: f32) {
    let f: unsafe extern "system" fn(u32, u32, f32, f32, f32, f32) = unsafe { std::mem::transmute(ORIG_ENV_PARAMETER_4F.load(Ordering::Relaxed)) };
    unsafe { f(target, index, x, y, z, w) };
    if target == 0x8620 && index == crate::nvasm::ATTRIB_ENV && ATTRIB_MIRROR.load(Ordering::Relaxed) {
        unsafe { f(0x8804, index, x, y, z, w) };
    }
}

unsafe extern "system" fn env_parameter_4fv(target: u32, index: u32, v: *const f32) {
    let f: unsafe extern "system" fn(u32, u32, *const f32) = unsafe { std::mem::transmute(ORIG_ENV_PARAMETER_4FV.load(Ordering::Relaxed)) };
    unsafe { f(target, index, v) };
    if target == 0x8620 && index == crate::nvasm::ATTRIB_ENV && ATTRIB_MIRROR.load(Ordering::Relaxed) {
        unsafe { f(0x8804, index, v) };
    }
}

unsafe extern "system" fn bind_program(target: u32, id: u32) {
    BOUND[(target == 0x8804) as usize].store(id as usize, Ordering::Relaxed);
    if target == 0x8620 {
        let units = TEXELS.lock().unwrap().get(&id).copied().unwrap_or(0);
        BOUND_TEXELS.store(units as usize, Ordering::Relaxed);
    } else if target == 0x8804 {
        let units = LEVELS.lock().unwrap().get(&id).copied().unwrap_or(0);
        BOUND_LEVELS.store(units as usize, Ordering::Relaxed);
    }
    let orig: unsafe extern "system" fn(u32, u32) = unsafe { std::mem::transmute(ORIG_BIND.load(Ordering::Relaxed)) };
    unsafe { orig(target, id) }
}

/// The vertex programs reading texels at a constant position (`nvasm::TEXEL_LOCAL`), by id:
/// their texture units (bits).
static TEXELS: std::sync::Mutex<std::collections::BTreeMap<u32, u16>> = std::sync::Mutex::new(std::collections::BTreeMap::new());
/// The bound vertex program's [`TEXELS`] units.
static BOUND_TEXELS: AtomicUsize = AtomicUsize::new(0);
/// The fragment programs reading textures at a level (`TXL`, `nvasm::LEVELS_LOCAL`), by id:
/// their texture units (bits), the cube map ones in the high 16 bits.
static LEVELS: std::sync::Mutex<std::collections::BTreeMap<u32, u32>> = std::sync::Mutex::new(std::collections::BTreeMap::new());
/// The bound fragment program's [`LEVELS`] units.
static BOUND_LEVELS: AtomicUsize = AtomicUsize::new(0);
static ORIG_DRAW_ARRAYS: AtomicUsize = AtomicUsize::new(0);
static ORIG_DRAW_ELEMENTS: AtomicUsize = AtomicUsize::new(0);
static ORIG_DRAW_RANGE: AtomicUsize = AtomicUsize::new(0);
static ORIG_BEGIN: AtomicUsize = AtomicUsize::new(0);

/// Mesa has no vertex textures (NVIDIA's vp40): the texels the bound vertex program reads at a
/// constant position (Project DIVA Arcade's auto exposure, the luminance its reductions leave
/// at (0, 0) of texture 3, else zero: the brightest exposure, its characters blown out in
/// bloom) read before each draw into its local parameters.
fn texel_parameters() {
    let units = BOUND_TEXELS.load(Ordering::Relaxed) as u16;
    if units == 0 {
        return;
    }
    static PROCS: OnceLock<[usize; 5]> = OnceLock::new();
    let procs = PROCS.get_or_init(|| {
        let orig: unsafe extern "system" fn(*const c_char) -> usize = unsafe { std::mem::transmute(ORIG_GL_PROC.load(Ordering::Relaxed)) };
        [c"glActiveTexture", c"glGenFramebuffersEXT", c"glBindFramebufferEXT", c"glFramebufferTexture2DEXT", c"glProgramLocalParameter4fvARB"].map(|n| unsafe { orig(n.as_ptr()) })
    });
    if procs.contains(&0) {
        return;
    }
    let active_texture: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(procs[0]) };
    let gen_framebuffers: unsafe extern "system" fn(i32, *mut u32) = unsafe { std::mem::transmute(procs[1]) };
    let bind_framebuffer: unsafe extern "system" fn(u32, u32) = unsafe { std::mem::transmute(procs[2]) };
    let framebuffer_texture: unsafe extern "system" fn(u32, u32, u32, u32, i32) = unsafe { std::mem::transmute(procs[3]) };
    let local: unsafe extern "system" fn(u32, u32, *const f32) = unsafe { std::mem::transmute(procs[4]) };
    let get_integer: unsafe extern "system" fn(u32, *mut i32) = unsafe { std::mem::transmute(gl11(c"glGetIntegerv")) };
    let read_buffer: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(gl11(c"glReadBuffer")) };
    let read_pixels: unsafe extern "system" fn(i32, i32, i32, i32, u32, u32, *mut c_void) = unsafe { std::mem::transmute(gl11(c"glReadPixels")) };
    // GL_ACTIVE_TEXTURE, GL_READ_FRAMEBUFFER_BINDING_EXT, GL_PIXEL_PACK_BUFFER_BINDING
    let (mut active, mut read_fb, mut pack) = (0, 0, 0);
    unsafe {
        get_integer(0x84E0, &mut active);
        get_integer(0x8CAA, &mut read_fb);
        get_integer(0x88ED, &mut pack);
    }
    if pack != 0 {
        return;
    }
    static FRAMEBUFFER: AtomicUsize = AtomicUsize::new(0);
    if FRAMEBUFFER.load(Ordering::Relaxed) == 0 {
        let mut fb = 0;
        unsafe { gen_framebuffers(1, &mut fb) };
        FRAMEBUFFER.store(fb as usize, Ordering::Relaxed);
    }
    // GL_READ_FRAMEBUFFER_EXT, its color attachment 0
    unsafe { bind_framebuffer(0x8CA8, FRAMEBUFFER.load(Ordering::Relaxed) as u32) };
    for unit in (0..16).filter(|u| units & (1 << u) != 0) {
        let mut texture = 0;
        unsafe {
            active_texture(0x84C0 + unit);
            // GL_TEXTURE_BINDING_2D
            get_integer(0x8069, &mut texture);
        }
        let mut texel = [0f32; 4];
        if texture != 0 {
            unsafe {
                framebuffer_texture(0x8CA8, 0x8CE0, 0x0DE1, texture as u32, 0);
                read_buffer(0x8CE0);
                // GL_RGBA, GL_FLOAT
                read_pixels(0, 0, 1, 1, 0x1908, 0x1406, texel.as_mut_ptr().cast());
            }
        }
        unsafe { local(0x8620, crate::nvasm::TEXEL_LOCAL + unit, texel.as_ptr()) };
    }
    unsafe {
        framebuffer_texture(0x8CA8, 0x8CE0, 0x0DE1, 0, 0);
        bind_framebuffer(0x8CA8, read_fb as u32);
        active_texture(active as u32);
    }
}

/// The last level of the textures the bound fragment program reads at a level (`TXL`, a bias
/// in ARB, exact for the last level whatever the scale: Project DIVA Arcade's YCbCr sprites,
/// their chroma in level 1 of 2, stretched; read from level 0, every sprite red) into its local
/// parameters before each draw.
fn level_parameters() {
    let units = BOUND_LEVELS.load(Ordering::Relaxed) as u32;
    if units == 0 {
        return;
    }
    static PROCS: OnceLock<[usize; 2]> = OnceLock::new();
    let procs = PROCS.get_or_init(|| {
        let orig: unsafe extern "system" fn(*const c_char) -> usize = unsafe { std::mem::transmute(ORIG_GL_PROC.load(Ordering::Relaxed)) };
        [c"glActiveTexture", c"glProgramLocalParameter4fvARB"].map(|n| unsafe { orig(n.as_ptr()) })
    });
    if procs.contains(&0) {
        return;
    }
    let active_texture: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(procs[0]) };
    let local: unsafe extern "system" fn(u32, u32, *const f32) = unsafe { std::mem::transmute(procs[1]) };
    let get_integer: unsafe extern "system" fn(u32, *mut i32) = unsafe { std::mem::transmute(gl11(c"glGetIntegerv")) };
    let get_parameter: unsafe extern "system" fn(u32, u32, *mut i32) = unsafe { std::mem::transmute(gl11(c"glGetTexParameteriv")) };
    let get_level_parameter: unsafe extern "system" fn(u32, i32, u32, *mut i32) = unsafe { std::mem::transmute(gl11(c"glGetTexLevelParameteriv")) };
    let mut active = 0;
    // GL_ACTIVE_TEXTURE
    unsafe { get_integer(0x84E0, &mut active) };
    for unit in (0..16).filter(|u| units & (1 << u) != 0) {
        // GL_TEXTURE_CUBE_MAP (its first face) or GL_TEXTURE_2D
        let (target, image) = if units & (1 << (16 + unit)) != 0 { (0x8513, 0x8515) } else { (0x0DE1, 0x0DE1) };
        // GL_TEXTURE_MAX_LEVEL, the levels defined below it (GL_TEXTURE_WIDTH)
        let mut max = 0;
        unsafe {
            active_texture(0x84C0 + unit);
            get_parameter(target, 0x813D, &mut max);
        }
        let mut last = 0;
        while last < max.min(15) {
            let mut width = 0;
            unsafe { get_level_parameter(image, last + 1, 0x1000, &mut width) };
            if width == 0 {
                break;
            }
            last += 1;
        }
        let levels = [last as f32; 4];
        unsafe { local(0x8804, crate::nvasm::LEVELS_LOCAL + unit, levels.as_ptr()) };
        if target == 0x0DE1 && last >= 2 {
            unsafe { level1_unit(unit) };
        }
    }
    unsafe { active_texture(active as u32) };
}

/// Unit `nvasm::LEVEL1_UNIT + unit`: the 2D texture of `unit` (the active one) with a sampler
/// holding its level of detail at 1 and its wrap modes, where the programs read its level 1
/// exactly (Border Break Scramble's YCbCr textures, their Cr in level 1 of 3).
unsafe fn level1_unit(unit: u32) {
    static PROCS: OnceLock<[usize; 5]> = OnceLock::new();
    let procs = PROCS.get_or_init(|| {
        let orig: unsafe extern "system" fn(*const c_char) -> usize = unsafe { std::mem::transmute(ORIG_GL_PROC.load(Ordering::Relaxed)) };
        [c"glActiveTexture", c"glGenSamplers", c"glBindSampler", c"glSamplerParameteri", c"glSamplerParameterf"].map(|n| unsafe { orig(n.as_ptr()) })
    });
    if procs.contains(&0) {
        return;
    }
    let active_texture: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(procs[0]) };
    let gen_samplers: unsafe extern "system" fn(i32, *mut u32) = unsafe { std::mem::transmute(procs[1]) };
    let bind_sampler: unsafe extern "system" fn(u32, u32) = unsafe { std::mem::transmute(procs[2]) };
    let sampler_i: unsafe extern "system" fn(u32, u32, i32) = unsafe { std::mem::transmute(procs[3]) };
    let sampler_f: unsafe extern "system" fn(u32, u32, f32) = unsafe { std::mem::transmute(procs[4]) };
    let get_integer: unsafe extern "system" fn(u32, *mut i32) = unsafe { std::mem::transmute(gl11(c"glGetIntegerv")) };
    let get_parameter: unsafe extern "system" fn(u32, u32, *mut i32) = unsafe { std::mem::transmute(gl11(c"glGetTexParameteriv")) };
    let bind_texture: unsafe extern "system" fn(u32, u32) = unsafe { std::mem::transmute(gl11(c"glBindTexture")) };
    static SAMPLERS: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];
    let slot = &SAMPLERS[unit as usize & 15];
    let (mut texture, mut wrap_s, mut wrap_t) = (0, 0, 0);
    unsafe {
        // GL_TEXTURE_BINDING_2D, GL_TEXTURE_WRAP_S / T
        get_integer(0x8069, &mut texture);
        get_parameter(0x0DE1, 0x2802, &mut wrap_s);
        get_parameter(0x0DE1, 0x2803, &mut wrap_t);
        if slot.load(Ordering::Relaxed) == 0 {
            let mut s = 0;
            gen_samplers(1, &mut s);
            // GL_TEXTURE_MIN_FILTER GL_LINEAR_MIPMAP_NEAREST (level 1, filtered in it),
            // GL_TEXTURE_MAG_FILTER GL_LINEAR, GL_TEXTURE_MIN_LOD / MAX_LOD 1
            sampler_i(s, 0x2801, 0x2701);
            sampler_i(s, 0x2800, 0x2601);
            sampler_f(s, 0x813A, 1.0);
            sampler_f(s, 0x813B, 1.0);
            slot.store(s as usize, Ordering::Relaxed);
        }
        let sampler = slot.load(Ordering::Relaxed) as u32;
        sampler_i(sampler, 0x2802, wrap_s);
        sampler_i(sampler, 0x2803, wrap_t);
        let aux = crate::nvasm::LEVEL1_UNIT + unit;
        active_texture(0x84C0 + aux);
        bind_texture(0x0DE1, texture as u32);
        bind_sampler(aux, sampler);
    }
}

/// A GL function replaced: the original kept in `slot`.
fn hook(slot: &AtomicUsize, orig: usize, with: usize) -> usize {
    slot.store(orig, Ordering::Relaxed);
    with
}

/// GL_VERTEX_PROGRAM_PARAMETER_BUFFER_NV: NV_parameter_buffer_object's vertex target.
const PARAMETER_BUFFER: u32 = 0x8DA2;

static ORIG_BIND_BUFFER: AtomicUsize = AtomicUsize::new(0);
static ORIG_BUFFER_DATA: AtomicUsize = AtomicUsize::new(0);
static ORIG_BUFFER_SUB_DATA: AtomicUsize = AtomicUsize::new(0);
static ORIG_MAP_BUFFER: AtomicUsize = AtomicUsize::new(0);
static ORIG_MAP_BUFFER_RANGE: AtomicUsize = AtomicUsize::new(0);
static ORIG_UNMAP_BUFFER: AtomicUsize = AtomicUsize::new(0);

/// NV_parameter_buffer_object, absent in Mesa (its calls an error the game's next `glGetError`
/// check takes for its own: Project DIVA Arcade Future Tone's first render target refused, the
/// game crashed): the buffers bound to its vertex target kept here, by name, never given to GL;
/// the buffer bound to it, the buffer bound to its slot 0 (with its offset), and a count of
/// their changes.
struct ParameterBuffers {
    contents: std::collections::BTreeMap<u32, Vec<u8>>,
    bound: u32,
    slot0: (u32, usize),
    changes: usize,
}

static PARAMETER_BUFFERS: std::sync::Mutex<ParameterBuffers> = std::sync::Mutex::new(ParameterBuffers { contents: std::collections::BTreeMap::new(), bound: 0, slot0: (0, 0), changes: 0 });
/// The vertex programs reading parameter buffer 0 (`nvasm::PALETTE_LOCAL`), by id: the change
/// count their local parameters hold.
static PALETTE_PROGRAMS: std::sync::Mutex<std::collections::BTreeMap<u32, usize>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

unsafe extern "system" fn bind_buffer(target: u32, buffer: u32) {
    if target == PARAMETER_BUFFER {
        PARAMETER_BUFFERS.lock().unwrap().bound = buffer;
        return;
    }
    let orig: unsafe extern "system" fn(u32, u32) = unsafe { std::mem::transmute(ORIG_BIND_BUFFER.load(Ordering::Relaxed)) };
    unsafe { orig(target, buffer) }
}

unsafe extern "system" fn buffer_data(target: u32, size: isize, data: *const u8, usage: u32) {
    if target == PARAMETER_BUFFER {
        let mut buffers = PARAMETER_BUFFERS.lock().unwrap();
        let bound = buffers.bound;
        let bytes = if data.is_null() { vec![0; size.max(0) as usize] } else { unsafe { std::slice::from_raw_parts(data, size.max(0) as usize) }.to_vec() };
        buffers.contents.insert(bound, bytes);
        buffers.changes += 1;
        return;
    }
    let orig: unsafe extern "system" fn(u32, isize, *const u8, u32) = unsafe { std::mem::transmute(ORIG_BUFFER_DATA.load(Ordering::Relaxed)) };
    unsafe { orig(target, size, data, usage) }
}

unsafe extern "system" fn buffer_sub_data(target: u32, offset: isize, size: isize, data: *const u8) {
    if target == PARAMETER_BUFFER {
        let mut buffers = PARAMETER_BUFFERS.lock().unwrap();
        let bound = buffers.bound;
        if let Some(b) = buffers.contents.get_mut(&bound)
            && let (Ok(at), Ok(n)) = (usize::try_from(offset), usize::try_from(size))
            && at + n <= b.len()
            && !data.is_null()
        {
            b[at..at + n].copy_from_slice(unsafe { std::slice::from_raw_parts(data, n) });
        }
        buffers.changes += 1;
        return;
    }
    let orig: unsafe extern "system" fn(u32, isize, isize, *const u8) = unsafe { std::mem::transmute(ORIG_BUFFER_SUB_DATA.load(Ordering::Relaxed)) };
    unsafe { orig(target, offset, size, data) }
}

/// The bound parameter buffer's contents at `offset` (stays in place until the next
/// `glBufferData`), counted as changed.
fn map_parameter_buffer(offset: usize) -> *mut c_void {
    let mut buffers = PARAMETER_BUFFERS.lock().unwrap();
    buffers.changes += 1;
    let bound = buffers.bound;
    match buffers.contents.get_mut(&bound) {
        Some(b) if offset <= b.len() => unsafe { b.as_mut_ptr().add(offset) }.cast(),
        _ => std::ptr::null_mut(),
    }
}

unsafe extern "system" fn map_buffer(target: u32, access: u32) -> *mut c_void {
    if target == PARAMETER_BUFFER {
        return map_parameter_buffer(0);
    }
    let orig: unsafe extern "system" fn(u32, u32) -> *mut c_void = unsafe { std::mem::transmute(ORIG_MAP_BUFFER.load(Ordering::Relaxed)) };
    unsafe { orig(target, access) }
}

unsafe extern "system" fn map_buffer_range(target: u32, offset: isize, length: isize, access: u32) -> *mut c_void {
    if target == PARAMETER_BUFFER {
        return map_parameter_buffer(offset.max(0) as usize);
    }
    let orig: unsafe extern "system" fn(u32, isize, isize, u32) -> *mut c_void = unsafe { std::mem::transmute(ORIG_MAP_BUFFER_RANGE.load(Ordering::Relaxed)) };
    unsafe { orig(target, offset, length, access) }
}

unsafe extern "system" fn unmap_buffer(target: u32) -> u8 {
    if target == PARAMETER_BUFFER {
        PARAMETER_BUFFERS.lock().unwrap().changes += 1;
        return 1;
    }
    let orig: unsafe extern "system" fn(u32) -> u8 = unsafe { std::mem::transmute(ORIG_UNMAP_BUFFER.load(Ordering::Relaxed)) };
    unsafe { orig(target) }
}

/// `glBindBuffer{Base,Range,Offset}NV`: the vertex target's slot 0 (the only one the
/// translated programs read); other targets (transform feedback) dropped.
fn bind_parameter_slot(target: u32, index: u32, buffer: u32, offset: isize) {
    if target != PARAMETER_BUFFER {
        return;
    }
    let mut buffers = PARAMETER_BUFFERS.lock().unwrap();
    buffers.bound = buffer;
    if index == 0 {
        buffers.slot0 = (buffer, offset.max(0) as usize);
        buffers.changes += 1;
    }
}

unsafe extern "system" fn bind_buffer_base_nv(target: u32, index: u32, buffer: u32) {
    bind_parameter_slot(target, index, buffer, 0);
}

unsafe extern "system" fn bind_buffer_range_nv(target: u32, index: u32, buffer: u32, offset: isize, _size: isize) {
    bind_parameter_slot(target, index, buffer, offset);
}

unsafe extern "system" fn bind_buffer_offset_nv(target: u32, index: u32, buffer: u32, offset: isize) {
    bind_parameter_slot(target, index, buffer, offset);
}

/// `glProgramBufferParameters*NV`: `count` vectors written at vector `index` of the buffer in
/// slot `slot` (floats or integers alike: copied as they are).
unsafe extern "system" fn program_buffer_parameters(target: u32, slot: u32, index: u32, count: i32, params: *const u8) {
    if target != PARAMETER_BUFFER || slot != 0 || params.is_null() {
        return;
    }
    let mut buffers = PARAMETER_BUFFERS.lock().unwrap();
    let (buffer, offset) = buffers.slot0;
    if let Some(b) = buffers.contents.get_mut(&buffer) {
        let at = offset + index as usize * 16;
        let n = count.max(0) as usize * 16;
        if at + n <= b.len() {
            b[at..at + n].copy_from_slice(unsafe { std::slice::from_raw_parts(params, n) });
        }
    }
    buffers.changes += 1;
}

/// The bound vertex program reading parameter buffer 0 (Project DIVA Arcade Future Tone's
/// skinning, its bone matrices): the buffer in slot 0 copied into its local parameters
/// (`nvasm::PALETTE_LOCAL`) before the draw, when it changed since that program's last copy.
fn palette_parameters() {
    let program = BOUND[0].load(Ordering::Relaxed) as u32;
    let mut programs = PALETTE_PROGRAMS.lock().unwrap();
    let Some(copied) = programs.get_mut(&program) else { return };
    let buffers = PARAMETER_BUFFERS.lock().unwrap();
    if *copied == buffers.changes {
        return;
    }
    *copied = buffers.changes;
    static LOCALS: OnceLock<usize> = OnceLock::new();
    let f = *LOCALS.get_or_init(|| {
        let orig: unsafe extern "system" fn(*const c_char) -> usize = unsafe { std::mem::transmute(ORIG_GL_PROC.load(Ordering::Relaxed)) };
        unsafe { orig(c"glProgramLocalParameters4fvEXT".as_ptr()) }
    });
    let (buffer, offset) = buffers.slot0;
    let Some(b) = buffers.contents.get(&buffer) else { return };
    let n = (b.len().saturating_sub(offset) / 16).min(crate::nvasm::PALETTE_LEN as usize);
    if f == 0 || n == 0 {
        return;
    }
    let locals: unsafe extern "system" fn(u32, u32, i32, *const u8) = unsafe { std::mem::transmute(f) };
    // GL_VERTEX_PROGRAM_ARB
    unsafe { locals(0x8620, crate::nvasm::PALETTE_LOCAL, n as i32, b[offset..].as_ptr()) };
}

/// The bound vertex program's instance (`nvasm::INSTANCE_LOCAL`).
fn set_instance(instance: i32) {
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(|| log!("cg: instanced draws, one per instance"));
    static LOCAL: OnceLock<usize> = OnceLock::new();
    let f = *LOCAL.get_or_init(|| {
        let orig: unsafe extern "system" fn(*const c_char) -> usize = unsafe { std::mem::transmute(ORIG_GL_PROC.load(Ordering::Relaxed)) };
        unsafe { orig(c"glProgramLocalParameter4fARB".as_ptr()) }
    });
    if f != 0 {
        let local: unsafe extern "system" fn(u32, u32, f32, f32, f32, f32) = unsafe { std::mem::transmute(f) };
        let v = instance as f32;
        // GL_VERTEX_PROGRAM_ARB
        unsafe { local(0x8620, crate::nvasm::INSTANCE_LOCAL, v, v, v, v) };
    }
}

/// `glDrawElementsInstanced*`: one `glDrawElements` per instance ([`set_instance`]).
unsafe extern "system" fn draw_elements_instanced(mode: u32, count: i32, kind: u32, indices: *const c_void, instances: i32) {
    let draw: unsafe extern "system" fn(u32, i32, u32, *const c_void) =
        if ORIG_DRAW_ELEMENTS.load(Ordering::Relaxed) != 0 { draw_elements } else { unsafe { std::mem::transmute(gl11(c"glDrawElements")) } };
    for instance in 0..instances.max(0) {
        set_instance(instance);
        unsafe { draw(mode, count, kind, indices) };
    }
    set_instance(0);
}

/// `glDrawArraysInstanced*`: one `glDrawArrays` per instance ([`set_instance`]).
unsafe extern "system" fn draw_arrays_instanced(mode: u32, first: i32, count: i32, instances: i32) {
    let draw: unsafe extern "system" fn(u32, i32, i32) =
        if ORIG_DRAW_ARRAYS.load(Ordering::Relaxed) != 0 { draw_arrays } else { unsafe { std::mem::transmute(gl11(c"glDrawArrays")) } };
    for instance in 0..instances.max(0) {
        set_instance(instance);
        unsafe { draw(mode, first, count) };
    }
    set_instance(0);
}

unsafe extern "system" fn draw_arrays(mode: u32, first: i32, count: i32) {
    texel_parameters();
    palette_parameters();
    level_parameters();
    let orig: unsafe extern "system" fn(u32, i32, i32) = unsafe { std::mem::transmute(ORIG_DRAW_ARRAYS.load(Ordering::Relaxed)) };
    unsafe { orig(mode, first, count) }
}

unsafe extern "system" fn draw_elements(mode: u32, count: i32, kind: u32, indices: *const c_void) {
    texel_parameters();
    palette_parameters();
    level_parameters();
    let orig: unsafe extern "system" fn(u32, i32, u32, *const c_void) = unsafe { std::mem::transmute(ORIG_DRAW_ELEMENTS.load(Ordering::Relaxed)) };
    unsafe { orig(mode, count, kind, indices) }
}

unsafe extern "system" fn draw_range(mode: u32, start: u32, end: u32, count: i32, kind: u32, indices: *const c_void) {
    texel_parameters();
    palette_parameters();
    level_parameters();
    let orig: unsafe extern "system" fn(u32, u32, u32, i32, u32, *const c_void) = unsafe { std::mem::transmute(ORIG_DRAW_RANGE.load(Ordering::Relaxed)) };
    unsafe { orig(mode, start, end, count, kind, indices) }
}

unsafe extern "system" fn begin(mode: u32) {
    texel_parameters();
    palette_parameters();
    level_parameters();
    let orig: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(ORIG_BEGIN.load(Ordering::Relaxed)) };
    unsafe { orig(mode) }
}

/// The programs (target, id) reading `program.env` themselves: [`env_parameters`].
static READS_ENV: std::sync::Mutex<std::collections::BTreeSet<(u32, u32)>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

unsafe extern "system" fn program_string(target: u32, format: u32, len: i32, text: *const u8) {
    let original = unsafe { std::slice::from_raw_parts(text, len.max(0) as usize) };
    {
        let program = (target, BOUND[(target == 0x8804) as usize].load(Ordering::Relaxed) as u32);
        let mut reads = READS_ENV.lock().unwrap();
        if original.windows(11).any(|w| w == b"program.env") {
            reads.insert(program);
        } else {
            reads.remove(&program);
        }
    }
    if DUMP_PROGRAMS.load(Ordering::Relaxed) {
        let id = BOUND[(target == 0x8804) as usize].load(Ordering::Relaxed);
        let _ = std::fs::create_dir_all("glprog");
        let kind = if target == 0x8804 { "fp" } else { "vp" };
        let _ = std::fs::write(format!("glprog/{id:04}_{kind}.txt"), original);
    }
    // the programs the game gives GL itself (Project DIVA Arcade's), NVIDIA assembly translated
    let fixed = match std::str::from_utf8(original) {
        Ok(t) if FIX.load(Ordering::Relaxed) && crate::nvasm::nvidia(t) => {
            let fixed = crate::nvasm::translate(t);
            if fixed.is_none() {
                static UNTRANSLATED: AtomicUsize = AtomicUsize::new(0);
                if UNTRANSLATED.fetch_add(1, Ordering::Relaxed) < 20 {
                    log!("cg: program not translated: {}", t.lines().next().unwrap_or(""));
                }
            }
            // a fragment program not translated (Project DIVA Arcade's blurs without
            // WAL_RINGEDGE_CG_REPEAT, loops of a parameter's length): its first texture
            // as is (a blur without radius), not an invalid program (nothing drawn)
            fixed.or_else(|| (target == 0x8804).then(|| "!!ARBfp1.0\nTEMP c;\nTEX c, fragment.texcoord[0], texture[0], 2D;\nMOV result.color, c;\nEND\n".to_string()))
        }
        _ => None,
    };
    if target == 0x8804 && fixed.as_deref().is_some_and(|f| f.contains(&format!("program.env[{}]", crate::nvasm::ATTRIB_ENV))) && original.windows(15).any(|w| w == b"fragment.attrib") {
        ATTRIB_MIRROR.store(true, Ordering::Relaxed);
    }
    if target == 0x8620 {
        let units = fixed.as_deref().map_or(0, |f| (0..16u32).filter(|u| f.contains(&format!("program.local[{}]", crate::nvasm::TEXEL_LOCAL + u))).fold(0u16, |m, u| m | 1 << u));
        let id = BOUND[0].load(Ordering::Relaxed) as u32;
        let mut texels = TEXELS.lock().unwrap();
        if units != 0 {
            texels.insert(id, units);
        } else {
            texels.remove(&id);
        }
        BOUND_TEXELS.store(units as usize, Ordering::Relaxed);
        let palette = fixed.as_deref().is_some_and(|f| f.contains(&format!("program.local[{}..", crate::nvasm::PALETTE_LOCAL)));
        let mut programs = PALETTE_PROGRAMS.lock().unwrap();
        if palette {
            programs.insert(id, usize::MAX);
        } else {
            programs.remove(&id);
        }
    } else if target == 0x8804 {
        let units = fixed.as_deref().map_or(0, |f| {
            (0..16u32)
                .filter(|u| f.contains(&format!("program.local[{}]", crate::nvasm::LEVELS_LOCAL + u)))
                .fold(0u32, |m, u| m | 1 << u | (f.contains(&format!("texture[{u}], CUBE")) as u32) << (16 + u))
        });
        let id = BOUND[1].load(Ordering::Relaxed) as u32;
        let mut levels = LEVELS.lock().unwrap();
        if units != 0 {
            levels.insert(id, units);
        } else {
            levels.remove(&id);
        }
        BOUND_LEVELS.store(units as usize, Ordering::Relaxed);
    }
    let (text, len) = match &fixed {
        Some(f) => (f.as_ptr(), f.len() as i32),
        None => (text, len),
    };
    let orig: unsafe extern "system" fn(u32, u32, i32, *const u8) = unsafe { std::mem::transmute(ORIG_STRING.load(Ordering::Relaxed)) };
    unsafe { orig(target, format, len, text) };
    if *LOG.get().unwrap_or(&false) {
        program_error(text, len);
    }
}

/// WAL_RINGEDGE_CG_LOG: a program GL refused, with its error and the program.
fn program_error(text: *const u8, len: i32) {
    let get_error: unsafe extern "system" fn() -> u32 = unsafe { std::mem::transmute(gl11(c"glGetError")) };
    let error = unsafe { get_error() };
    if error == 0 {
        return;
    }
    let get_string: unsafe extern "system" fn(u32) -> *const c_char = unsafe { std::mem::transmute(gl11(c"glGetString")) };
    // GL_PROGRAM_ERROR_STRING_ARB
    let message = unsafe { get_string(0x8874) };
    let message = if message.is_null() { String::new() } else { unsafe { CStr::from_ptr(message) }.to_string_lossy().into_owned() };
    let program = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(text, len.max(0) as usize) });
    log!("cg: GL error {error:#x} loading a program: {message}\n{program}");
}

/// Each program given to GL written as `glprog\<id>_<target>.txt` (with the shots).
static DUMP_PROGRAMS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static FIX: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static SHOT_MS: AtomicUsize = AtomicUsize::new(0);
static ORIG_SWAP: AtomicUsize = AtomicUsize::new(0);

static CROSSHAIRS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe extern "system" fn swap_buffers(hdc: *mut c_void) -> i32 {
    unsafe { crate::glscale::present(hdc) };
    if CROSSHAIRS.load(Ordering::Relaxed) {
        let window = unsafe { windows_sys::Win32::Graphics::Gdi::WindowFromDC(hdc as _) };
        if !window.is_null() {
            unsafe { crate::crosshair::draw_gl_window(window) };
        }
    }
    if SHOT_MS.load(Ordering::Relaxed) == 0 {
        let orig: unsafe extern "system" fn(*mut c_void) -> i32 = unsafe { std::mem::transmute(ORIG_SWAP.load(Ordering::Relaxed)) };
        return unsafe { orig(hdc) };
    }
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    let due = {
        let mut last = LAST.lock().unwrap();
        let now = std::time::Instant::now();
        let due = last.is_none_or(|l| now.duration_since(l).as_millis() as usize >= SHOT_MS.load(Ordering::Relaxed));
        if due {
            *last = Some(now);
        }
        due
    };
    if due {
        unsafe { gl_shot(hdc, COUNT.fetch_add(1, Ordering::Relaxed)) };
    }
    let orig: unsafe extern "system" fn(*mut c_void) -> i32 = unsafe { std::mem::transmute(ORIG_SWAP.load(Ordering::Relaxed)) };
    unsafe { orig(hdc) }
}

unsafe fn gl_shot(hdc: *mut c_void, n: usize) {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
    let gl = unsafe { GetModuleHandleA(c"opengl32.dll".as_ptr().cast()) };
    let procs = unsafe { [c"glGetIntegerv", c"glReadPixels", c"glReadBuffer", c"glGetError"].map(|n| GetProcAddress(gl, n.as_ptr().cast())) };
    let [Some(get), Some(read), Some(buffer), Some(error)] = procs else { return };
    let error: unsafe extern "system" fn() -> u32 = unsafe { std::mem::transmute(error) };
    let get: unsafe extern "system" fn(u32, *mut i32) = unsafe { std::mem::transmute(get) };
    let read: unsafe extern "system" fn(i32, i32, i32, i32, u32, u32, *mut u8) = unsafe { std::mem::transmute(read) };
    let buffer: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(buffer) };
    let window = unsafe { windows_sys::Win32::Graphics::Gdi::WindowFromDC(hdc as _) };
    let mut rect = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::RECT>() };
    unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(window, &mut rect) };
    let (w, h) = ((rect.right - rect.left).max(1), (rect.bottom - rect.top).max(1));
    let mut fbo = 0;
    unsafe { get(0x8CA6, &mut fbo) }; // GL_DRAW_FRAMEBUFFER_BINDING
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    let before = unsafe { error() };
    unsafe {
        buffer(0x0405); // GL_BACK
        read(0, 0, w, h, 0x80E1, 0x1401, pixels.as_mut_ptr()); // GL_BGRA, GL_UNSIGNED_BYTE
    }
    let after = unsafe { error() };
    if before != 0 || after != 0 {
        log!("cg: shot: GL errors {before:#x} {after:#x}");
    }
    // bottom-up rows
    let row = (w * 4) as usize;
    let flipped: Vec<u8> = pixels.chunks(row).rev().flatten().copied().collect();
    let path = format!("glshot-{n:04}.bmp");
    match crate::screenshot::write_bmp(&path, w as u32, h as u32, &flipped) {
        Ok(()) => log!("cg: {path} {w}x{h} (framebuffer {fbo})"),
        Err(e) => log!("cg: {path}: {e}"),
    }
}

pub fn init() {
    gl_shots();
    let mode = std::env::var("WAL_RINGEDGE_CG_FIX").unwrap_or_default();
    let log = std::env::var("WAL_RINGEDGE_CG_LOG").is_ok_and(|v| v.trim() == "1");
    let _ = LOG.set(log);
    let fix = match mode.trim() {
        "1" => true,
        "auto" => !nvidia(),
        _ => false,
    };
    if !mode.is_empty() {
        log!("cg: shader fix {} ({mode})", if fix { "on" } else { "off" });
    }
    if !fix && !log && std::env::var_os("WAL_RINGEDGE_CG_DUMP").is_none() {
        return;
    }
    if let Some(o) = unsafe { iat::hook("cg.dll", "cgCreateProgram", if fix { create_program_fixed as *const () as usize } else { create_program_logged as *const () as usize }) } {
        ORIG.store(o, Ordering::Relaxed);
    }
    if fix {
        FIX.store(true, Ordering::Relaxed);
        if !DUMP_PROGRAMS.load(Ordering::Relaxed) {
            gl_procs();
        }
        // the vertex programs' texels (texel_parameters)
        for (name, hook, orig) in [
            ("glDrawArrays", draw_arrays as *const () as usize, &ORIG_DRAW_ARRAYS),
            ("glDrawElements", draw_elements as *const () as usize, &ORIG_DRAW_ELEMENTS),
            ("glBegin", begin as *const () as usize, &ORIG_BEGIN),
        ] {
            if let Some(o) = unsafe { iat::hook("opengl32.dll", name, hook) } {
                orig.store(o, Ordering::Relaxed);
            }
        }
        unsafe { nv40_profiles() };
        if let Some(o) = unsafe { iat::hook("cg.dll", "cgGetProgramString", get_program_string as *const () as usize) } {
            ORIG_GET_STRING.store(o, Ordering::Relaxed);
        }
        // the precompiled programs and the shader caches, read by the game, its C runtime and
        // Wine's: the functions themselves
        for (name, hook, orig) in [
            (c"CreateFileW", create_file_w as *const () as usize, &ORIG_CREATE_W),
            (c"GetFileAttributesExW", get_file_attributes_ex_w as *const () as usize, &ORIG_ATTRIBUTES),
        ] {
            match unsafe { hotpatch(c"kernelbase.dll", name, hook) } {
                Some(o) => orig.store(o, Ordering::Relaxed),
                None => log!("cg: kernelbase.dll!{} not hot patchable", name.to_string_lossy()),
            }
        }
    }
}

/// cgGL's vp40 and fp40 profiles registered without the NVIDIA extensions (its checks
/// inverted): the game then takes its NV40 rendering path, the one its shaders are written for
/// (the other one draws nothing), and asks for these profiles, compiled as the ARB ones.
///
/// cgGL registers each profile with `test eax,eax; je skip; push edi; mov edi, <profile>`.
unsafe fn nv40_profiles() {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(c"cgGL.dll".as_ptr().cast()) } as usize;
    if base == 0 {
        return;
    }
    let size = unsafe { std::ptr::read_unaligned((base + std::ptr::read_unaligned((base + 0x3c) as *const u32) as usize + 0x50) as *const u32) } as usize;
    let image = unsafe { std::slice::from_raw_parts(base as *const u8, size) };
    for profile in [7001u32, 6151] {
        let mut pattern = vec![0x85, 0xC0, 0x74, 0x1A, 0x57, 0xBF];
        pattern.extend_from_slice(&profile.to_le_bytes());
        let Some(at) = image.windows(pattern.len()).position(|w| w == pattern.as_slice()) else {
            log!("cg: cgGL.dll: profile {profile} check not found");
            continue;
        };
        let jump = (base + at + 2) as *mut u8;
        let mut old = 0;
        unsafe {
            VirtualProtect(jump.cast(), 1, PAGE_EXECUTE_READWRITE, &mut old);
            *jump = 0x75;
            VirtualProtect(jump.cast(), 1, old, &mut old);
        }
        log!("cg: cgGL.dll: profile {profile} registered (+{:#x})", at + 2);
    }
}

/// Hot patch of `module!name` (`mov edi,edi` or a 2 byte nop after 5 padding bytes, as Wine
/// builds its exports): a jump to `hook` in the padding, a short jump to it at the entry.
/// Returns the function past its 2 byte prologue (the original).
pub unsafe fn hotpatch(module: &CStr, name: &CStr, hook: usize) -> Option<usize> {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    let f = unsafe { GetProcAddress(GetModuleHandleA(module.as_ptr().cast()), name.as_ptr().cast()) }? as usize;
    let code = unsafe { std::slice::from_raw_parts((f - 5) as *const u8, 7) };
    if !code[..5].iter().all(|&b| b == 0xCC || b == 0x90) || !(code[5..] == [0x8B, 0xFF] || code[5..] == [0x66, 0x90]) {
        return None;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect((f - 5) as *const _, 7, PAGE_EXECUTE_READWRITE, &mut old);
        *((f - 5) as *mut u8) = 0xE9;
        std::ptr::write_unaligned((f - 4) as *mut i32, hook.wrapping_sub(f) as i32);
        // jmp $-5, written last: one 2 byte store
        std::ptr::write_unaligned(f as *mut u16, 0xF9EB);
        VirtualProtect((f - 5) as *const _, 7, old, &mut old);
    }
    Some(f + 2)
}

fn cg_proc(name: &CStr) -> Option<unsafe extern "system" fn() -> isize> {
    unsafe {
        let module = windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(c"cg.dll".as_ptr().cast());
        windows_sys::Win32::System::LibraryLoader::GetProcAddress(module, name.as_ptr().cast())
    }
}

/// Cg's last error and listing, for the log.
fn failure(ctx: *mut c_void) -> String {
    unsafe {
        let mut out = String::new();
        if let (Some(get), Some(text)) = (cg_proc(c"cgGetError"), cg_proc(c"cgGetErrorString")) {
            let get: unsafe extern "C" fn() -> i32 = std::mem::transmute(get);
            let text: unsafe extern "C" fn(i32) -> *const c_char = std::mem::transmute(text);
            let s = text(get());
            if !s.is_null() {
                out += &CStr::from_ptr(s).to_string_lossy();
            }
        }
        if let Some(f) = cg_proc(c"cgGetLastListing") {
            let f: unsafe extern "C" fn(*mut c_void) -> *const c_char = std::mem::transmute(f);
            let s = f(ctx);
            if !s.is_null() {
                out += ": ";
                out += CStr::from_ptr(s).to_string_lossy().trim();
            }
        }
        out
    }
}

fn logged(ctx: *mut c_void, asked: i32, profile: i32, p: *mut c_void) -> *mut c_void {
    // WAL_RINGEDGE_CG_DUMP: each compiled program in cgdump\ (debugging)
    if !p.is_null() && std::env::var_os("WAL_RINGEDGE_CG_DUMP").is_some()
        && let Some(f) = cg_proc(c"cgGetProgramString")
    {
        static N: AtomicUsize = AtomicUsize::new(0);
        let f: unsafe extern "C" fn(*mut c_void, i32) -> *const c_char = unsafe { std::mem::transmute(f) };
        let s = unsafe { f(p, 4106) };
        if !s.is_null() {
            let n = N.fetch_add(1, Ordering::Relaxed);
            let _ = std::fs::create_dir_all("cgdump");
            let _ = std::fs::write(format!("cgdump/{n:04}_{asked}_{profile}.asm"), unsafe { CStr::from_ptr(s) }.to_bytes());
        }
    }
    if *LOG.get().unwrap_or(&false) {
        if p.is_null() {
            log!("cg: compile profile {asked}->{profile} failed: {}", failure(ctx));
        } else {
            log!("cg: compile profile {asked}->{profile}");
        }
    }
    p
}

unsafe extern "C" fn create_program_logged(ctx: *mut c_void, kind: i32, program: *const c_char, profile: i32, entry: *const c_char, args: *const *const c_char) -> *mut c_void {
    let orig: CreateProgram = unsafe { std::mem::transmute(ORIG.load(Ordering::Relaxed)) };
    let p = unsafe { orig(ctx, kind, program, profile, entry, args) };
    logged(ctx, profile, profile, p)
}

unsafe extern "C" fn create_program_fixed(ctx: *mut c_void, kind: i32, program: *const c_char, profile: i32, entry: *const c_char, args: *const *const c_char) -> *mut c_void {
    let orig: CreateProgram = unsafe { std::mem::transmute(ORIG.load(Ordering::Relaxed)) };
    let asked = profile;
    let profile = match profile {
        6149 | 6151 => ARBFP1, // fp30, fp40
        6148 | 7001 => ARBVP1, // vp30, vp40
        p => p,
    };
    if profile != ARBVP1 {
        let p = unsafe { orig(ctx, kind, program, profile, entry, args) };
        return logged(ctx, asked, profile, p);
    }
    let src = std::ffi::CString::new(vertex_source(unsafe { CStr::from_ptr(program) }.to_bytes())).unwrap_or_default();
    let mut list: Vec<*const c_char> = Vec::new();
    if !args.is_null() {
        let mut i = 0;
        while !unsafe { *args.add(i) }.is_null() {
            list.push(unsafe { *args.add(i) });
            i += 1;
        }
    }
    list.extend_from_slice(&[c"-po".as_ptr(), c"MaxLocalParams=256".as_ptr(), std::ptr::null()]);
    let p = unsafe { orig(ctx, kind, src.as_ptr(), profile, entry, list.as_ptr()) };
    logged(ctx, asked, profile, p)
}

/// Constants the game writes at register 0 (NVIDIA's vp40 layout): bound there.
const AT_C0: [&str; 4] = ["mtxWorldViewProj", "g_mtxWorldViewProj", "g_m4WorldViewProj", "f2UV_Offset"];

/// Loops bounded by a uniform (vp40 branches, arbvp1 has none: "for could not be unrolled"),
/// with the constant maximum they get: the loop runs to the maximum, its body conditional.
const LOOP_BOUNDS: [(&str, &str); 1] = [("sLightNumber", "S_MODEL_LIGHT_MAX")];

/// A vertex program's source for arbvp1: `BLENDWEIGHT` (the fixed function vertex weight,
/// `ARB_vertex_blend`, absent) as generic attribute 1 (where NVIDIA's vp40 reads it and the game
/// feeds it), the [`AT_C0`] declarations bound to `C0`, the [`LOOP_BOUNDS`] loops unrollable.
fn vertex_source(src: &[u8]) -> Vec<u8> {
    // the sources are Shift-JIS: bytes, ASCII patterns
    let mut out = Vec::with_capacity(src.len() + 64);
    for line in src.split_inclusive(|&b| b == b'\n') {
        let text = String::from_utf8_lossy(line);
        let t = text.trim_start();
        let mut line = replace(line, b"BLENDWEIGHT;", b"ATTR1;");
        if t.starts_with("for") {
            line = bounded_loop(&line);
        }
        if (t.starts_with("const") || t.starts_with("uniform") || t.starts_with("float")) && t.contains("float") {
            for name in AT_C0 {
                if let Some(at) = declared(&line, name.as_bytes()) {
                    line.splice(at..at, *b" : C0");
                    break;
                }
            }
        }
        out.extend_from_slice(&line);
    }
    out
}

/// `for( i=0; i<bound; i++ )` as `for( i=0; i<MAX; i++ ) if( i<bound )` for a [`LOOP_BOUNDS`]
/// bound (the loop's body follows, braced or not).
fn bounded_loop(line: &[u8]) -> Vec<u8> {
    // (Shift-JIS comments: such a line left as is)
    let Ok(text) = std::str::from_utf8(line) else { return line.to_vec() };
    for (bound, max) in LOOP_BOUNDS {
        // the condition: `<var>\s*<\s*<bound>`, then the loop header's closing parenthesis
        let Some(at) = text.find(bound) else { continue };
        let Some(lt) = text[..at].rfind('<') else { continue };
        let var_end = text[..lt].trim_end().len();
        let var_start = text[..var_end].rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).map_or(0, |i| i + 1);
        let Some(close) = text[at..].rfind(')').map(|i| at + i) else { continue };
        let var = &text[var_start..var_end];
        if var.is_empty() || !text[lt + 1..at].trim().is_empty() {
            continue;
        }
        return format!("{}{max}{} if( {var}<{bound} ){}", &text[..lt + 1], &text[at + bound.len()..=close], &text[close + 1..]).into_bytes();
    }
    line.to_vec()
}

/// End of the declared name `name` in a declaration line (followed by `;`, `=` or `:` not
/// already bound).
fn declared(line: &[u8], name: &[u8]) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = line[from..].windows(name.len()).position(|w| w == name) {
        let start = from + i;
        let end = start + name.len();
        let before = start == 0 || !(line[start - 1].is_ascii_alphanumeric() || line[start - 1] == b'_');
        let next = line[end..].iter().find(|b| !b.is_ascii_whitespace());
        if before && matches!(next, Some(b';' | b'=')) {
            return Some(end);
        }
        from = end;
    }
    None
}

fn replace(text: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(text[i]);
            i += 1;
        }
    }
    out
}

/// A fragment program without shadow samplers (Mesa refuses the draws with a shadow
/// sampler on a texture without depth comparison).
fn fragment_text(text: &[u8]) -> Vec<u8> {
    let text = replace(text, b"OPTION ARB_fragment_program_shadow;", b"#OPTION ARB_fragment_program_shadow;");
    replace(&text, b" SHADOW2D;", b" 2D;")
}

unsafe extern "C" fn get_program_string(program: *mut c_void, pname: i32) -> *const c_char {
    let orig: unsafe extern "C" fn(*mut c_void, i32) -> *const c_char = unsafe { std::mem::transmute(ORIG_GET_STRING.load(Ordering::Relaxed)) };
    let s = unsafe { orig(program, pname) };
    // CG_COMPILED_PROGRAM of a fragment program
    if pname != 4106 || s.is_null() {
        return s;
    }
    let text = unsafe { CStr::from_ptr(s) }.to_bytes();
    if !text.starts_with(b"!!ARBfp") || !text.windows(8).any(|w| w == b"SHADOW2D") {
        return s;
    }
    let fixed = std::ffi::CString::new(fragment_text(text)).unwrap_or_default();
    // kept for good: the game may still use a string returned earlier (a few hundred programs)
    let mut list = FRAGMENTS.lock().unwrap();
    if let Some((_, kept)) = list.iter().find(|(p, kept)| *p == program as usize && *kept == fixed) {
        return kept.as_ptr();
    }
    let ptr = fixed.as_ptr();
    list.push((program as usize, fixed));
    ptr
}

/// A precompiled program's text for Mesa: NVIDIA assembly translated ([`crate::nvasm`]), the
/// ARB programs with the vertex weight as generic attribute 1 and no shadow samplers. `None`:
/// unchanged.
fn precompiled_text(text: &str) -> Option<String> {
    let fixed = if crate::nvasm::nvidia(text) {
        crate::nvasm::translate(text)?
    } else {
        let text = text.replace("vertex.weight", "vertex.attrib[1]");
        String::from_utf8(fragment_text(text.as_bytes())).unwrap_or(text)
    };
    (fixed != text).then_some(fixed)
}

/// `fixed`, as long as `orig` (the game reads the file with the size of the original): its
/// `#` comment lines shortened, or one padded, by the difference.
fn same_length(orig: &str, fixed: &str) -> Option<String> {
    if fixed.len() < orig.len() {
        // a comment line padded with spaces
        let at = fixed.find("\n#")? + 2;
        let line_end = at + fixed[at..].find(['\r', '\n'])?;
        return Some(format!("{}{}{}", &fixed[..line_end], " ".repeat(orig.len() - fixed.len()), &fixed[line_end..]));
    }
    let mut extra = fixed.len() - orig.len();
    let mut out = String::with_capacity(orig.len());
    for line in fixed.split_inclusive('\n') {
        if extra > 0 && line.starts_with('#') {
            let body = line.trim_end_matches(['\r', '\n']);
            let cut = extra.min(body.len() - 1);
            out.push_str(&body[..body.len() - cut]);
            out.push_str(&line[body.len()..]);
            extra -= cut;
        } else {
            out.push_str(line);
        }
    }
    (extra == 0).then_some(out)
}

/// A shader cache of `data\V5SHADER\` compiled for NVIDIA GPUs (`OPTION NV_...`) that
/// [`crate::nvasm`] cannot translate: dated 1601, older than its source, the game compiles the source
/// again (with the ARB profiles).
fn stale_cache(path: &str) -> bool {
    static SEEN: std::sync::Mutex<Vec<(String, bool)>> = std::sync::Mutex::new(Vec::new());
    if !path.to_ascii_lowercase().contains("v5shader") {
        return false;
    }
    if let Some((_, stale)) = SEEN.lock().unwrap().iter().find(|(p, _)| p == path) {
        return *stale;
    }
    let stale = std::fs::read(path).is_ok_and(|t| {
        let t = String::from_utf8_lossy(&t);
        crate::nvasm::nvidia(&t) && crate::nvasm::translate(&t).is_none()
    });
    if stale && *LOG.get().unwrap_or(&false) {
        log!("cg: {path}: NVIDIA cache made stale");
    }
    SEEN.lock().unwrap().push((path.to_string(), stale));
    stale
}

/// A precompiled program changed by [`precompiled_text`]: its fixed copy in `cgfix\` (made once),
/// as long as the original.
fn fixed_copy(path: &str) -> Option<Vec<u16>> {
    // Shift-JIS comments
    let text = String::from_utf8_lossy(&std::fs::read(path).ok()?).into_owned();
    let original = std::fs::metadata(path).ok()?.len() as usize;
    let fixed = precompiled_text(&text)?;
    let name = path.rsplit(['\\', '/']).next()?;
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.join("cgfix");
    let _ = std::fs::create_dir_all(&dir);
    let out = dir.join(name);
    if !out.exists() {
        let fixed = same_length(&" ".repeat(original), &fixed)?;
        std::fs::write(&out, fixed).ok()?;
        log!("cg: {path}: fixed in {}", out.display());
    }
    Some(out.to_string_lossy().encode_utf16().chain([0]).collect())
}

/// Where a cache the game writes goes: `cgfix\compiled\` of the run directory.
fn compiled_copy(path: &str) -> Option<Vec<u16>> {
    let name = path.rsplit(['\\', '/']).next()?;
    let dir = std::env::current_exe().ok()?.parent()?.join("cgfix").join("compiled");
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join(name).to_string_lossy().encode_utf16().chain([0]).collect())
}

unsafe extern "system" fn get_file_attributes_ex_w(name: *const u16, level: i32, info: *mut c_void) -> i32 {
    let orig: unsafe extern "system" fn(*const u16, i32, *mut c_void) -> i32 = unsafe { std::mem::transmute(ORIG_ATTRIBUTES.load(Ordering::Relaxed)) };
    let ok = unsafe { orig(name, level, info) };
    // GetFileExInfoStandard: WIN32_FILE_ATTRIBUTE_DATA, ftLastWriteTime at +20
    if ok != 0 && level == 0 && !name.is_null() {
        let path = wide_str(name);
        if path.to_ascii_lowercase().ends_with(".asm_gl") && reentry(|| stale_cache(&path)) == Some(true) {
            unsafe { std::ptr::write_unaligned(info.cast::<u8>().add(20).cast::<u64>(), 1) };
        }
    }
    ok
}

/// `f` unless already running on this thread: the payload's own file reads come back through
/// the hooks.
fn reentry<T>(f: impl FnOnce() -> T) -> Option<T> {
    thread_local!(static BUSY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if BUSY.with(|b| b.replace(true)) {
        return None;
    }
    let r = f();
    BUSY.with(|b| b.set(false));
    Some(r)
}

fn wide_str(name: *const u16) -> String {
    let mut len = 0;
    while unsafe { *name.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) })
}

unsafe extern "system" fn create_file_w(name: *const u16, a: u32, s: u32, sa: *mut c_void, d: u32, f: u32, t: *mut c_void) -> *mut c_void {
    let orig: CreateFileW = unsafe { std::mem::transmute(ORIG_CREATE_W.load(Ordering::Relaxed)) };
    if !name.is_null() {
        let path = wide_str(name);
        // CREATE_NEW / CREATE_ALWAYS / TRUNCATE_EXISTING: the game writing the program it
        // compiled as the new cache, kept out of the dump (its NVIDIA cache stays stale,
        // compiled again at each start)
        if *LOG.get().unwrap_or(&false) && path.to_ascii_lowercase().ends_with(".asm_gl") {
            reentry(|| log!("cg: open {path} access {a:#x} disposition {d}"));
        }
        if path.to_ascii_lowercase().ends_with(".asm_gl")
            && matches!(d, 1 | 2 | 5)
            && let Some(out) = compiled_copy(&path)
        {
            return unsafe { orig(out.as_ptr(), a, s, sa, d, f, t) };
        }
        if path.to_ascii_lowercase().ends_with(".asm_gl")
            && let Some(Some(fixed)) = reentry(|| fixed_copy(&path))
        {
            return unsafe { orig(fixed.as_ptr(), a, s, sa, d, f, t) };
        }
    }
    unsafe { orig(name, a, s, sa, d, f, t) }
}
