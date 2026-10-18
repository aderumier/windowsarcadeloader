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
//!
//! Debugging: `WAL_RINGEDGE_CG_LOG=1` logs each compile and Cg's listing of the failed ones;
//! `WAL_RINGEDGE_CG_DUMP=1` writes each compiled program to `cgdump\`;
//! `WAL_RINGEDGE_GL_SHOT=<seconds>` writes the back buffer every `<seconds>` (`glshot-NNNN.bmp`,
//! no screen capture of OpenGL games under Wayland) and each program given to GL (`glprog\`).

use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

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
    if shots.is_none() && !scale {
        return;
    }
    let _ = unsafe { windows_sys::Win32::System::LibraryLoader::LoadLibraryA(c"opengl32.dll".as_ptr().cast()) };
    match unsafe { hotpatch(c"opengl32.dll", c"wglSwapBuffers", swap_buffers as *const () as usize) } {
        Some(o) => ORIG_SWAP.store(o, Ordering::Relaxed),
        None => log!("cg: opengl32.dll!wglSwapBuffers not hot patchable"),
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
            | b"glEnableVertexAttribArrayARB" | b"glDisableVertexAttribArray" | b"glDisableVertexAttribArrayARB" => return unsafe { gl_proc(name) },
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
    if f == 0 || name.is_null() {
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
        b"glProgramEnvParameters4fvEXT" if FIX.load(Ordering::Relaxed) => {
            ORIG_ENVS.store(f, Ordering::Relaxed);
            let local = unsafe { orig(c"glProgramLocalParameters4fvEXT".as_ptr()) };
            ORIG_LOCALS.store(local, Ordering::Relaxed);
            env_parameters as *const () as usize
        }
        _ => f,
    }
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
/// (a black screen). Each write also goes to the bound program's locals.
unsafe extern "system" fn env_parameters(target: u32, index: u32, count: i32, params: *const f32) {
    let env: unsafe extern "system" fn(u32, u32, i32, *const f32) = unsafe { std::mem::transmute(ORIG_ENVS.load(Ordering::Relaxed)) };
    unsafe { env(target, index, count, params) };
    let local = ORIG_LOCALS.load(Ordering::Relaxed);
    if local != 0 {
        let local: unsafe extern "system" fn(u32, u32, i32, *const f32) = unsafe { std::mem::transmute(local) };
        unsafe { local(target, index, count, params) };
    }
}

unsafe extern "system" fn bind_program(target: u32, id: u32) {
    BOUND[(target == 0x8804) as usize].store(id as usize, Ordering::Relaxed);
    let orig: unsafe extern "system" fn(u32, u32) = unsafe { std::mem::transmute(ORIG_BIND.load(Ordering::Relaxed)) };
    unsafe { orig(target, id) }
}

unsafe extern "system" fn program_string(target: u32, format: u32, len: i32, text: *const u8) {
    if !DUMP_PROGRAMS.load(Ordering::Relaxed) {
        let orig: unsafe extern "system" fn(u32, u32, i32, *const u8) = unsafe { std::mem::transmute(ORIG_STRING.load(Ordering::Relaxed)) };
        unsafe { orig(target, format, len, text) };
        if *LOG.get().unwrap_or(&false) {
            program_error(text, len);
        }
        return;
    }
    let id = BOUND[(target == 0x8804) as usize].load(Ordering::Relaxed);
    let _ = std::fs::create_dir_all("glprog");
    let kind = if target == 0x8804 { "fp" } else { "vp" };
    let _ = std::fs::write(format!("glprog/{id:04}_{kind}.txt"), unsafe { std::slice::from_raw_parts(text, len.max(0) as usize) });
    let orig: unsafe extern "system" fn(u32, u32, i32, *const u8) = unsafe { std::mem::transmute(ORIG_STRING.load(Ordering::Relaxed)) };
    unsafe { orig(target, format, len, text) }
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

unsafe extern "system" fn swap_buffers(hdc: *mut c_void) -> i32 {
    unsafe { crate::glscale::present(hdc) };
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
    match wal_payload_common::screenshot::write_bmp(&path, w as u32, h as u32, &flipped) {
        Ok(()) => log!("cg: {path} {w}x{h} (framebuffer {fbo})"),
        Err(e) => log!("cg: {path}: {e}"),
    }
}

pub(crate) fn init() {
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
pub(crate) unsafe fn hotpatch(module: &CStr, name: &CStr, hook: usize) -> Option<usize> {
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
