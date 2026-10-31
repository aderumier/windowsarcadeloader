//! SiN's virtual reality pod (Global VR, 2002, Ritual's SiN on the Quake II engine): the
//! player stands in the pod with a head-mounted display and a gun; the head tracker aims and
//! turns, the gun's trigger fires, its two thumb buttons walk forward and back.
//!
//! The cabinet's frontend `C:\orion\sin\test.exe` (attract movies, credits, character
//! selection, network link, high scores) starts the game, `sin.exe +enablevr +map ...`, for
//! each play. Both read the pod through Global VR's `ogsdll.dll`: its configuration
//! (`C:\orion\orion.cfg`), credits (kept in its shared section, the same for both programs),
//! encrypted bookkeeping and the pod's hardware. The library is kept; its hardware functions
//! are replaced (`MachineType=3` of the cabinet's configuration, joysticks and the USB GCI
//! board):
//!
//! * `GetButtons(edge)`: the virtual stick's buttons, the cabinet's bits (the library's own
//!   edge detection: with `edge`, the bits that changed when the value dropped);
//! * `InitTracker` / `CloseTracker` / `gettrackerstatus`: the head tracker. `azi_head` (the
//!   heading) and `tilt_head` (the pitch, 0 level, up from 0, down from 0x800 back) are
//!   11-bit angles (0x800 a full turn): the stick turns them at a speed over time;
//! * the renderer's OpenGL extension string reduced to the extensions it knows (a long one
//!   overflows its print buffer);
//! * the gun's recoil solenoid (`KickGun` pulses): the pad's rumble;
//! * the coins: one credit each, added to the library's credit counter (the frontend counts
//!   them, its thread runs for the whole session).
//!
//! In the frontend, the Hardlock dongle check (memory words "GV" and "SIN ") passes, and the
//! game starts with the payload (`beachhead.rs`' `CreateProcessA` through `wal-loader`).
//!
//! | bit | game | game menus | frontend |
//! |---|---|---|---|
//! | 0x1 | trigger: fire | enter | start, select |
//! | 0x2 | forward (with 0x4: jump) | up | choose |
//! | 0x4 | back | down | choose |
//! | 0x8 | use (on press) | escape | |
//!
//! Heavy Gear II's pod (`C:\orion\hgx`) is the same: its frontend `HG2Shell.exe` (Hardlock
//! words "GV" and "HG2 ") starts `hgx.exe`, whose `GCI_*` exports (the game's `hg2sim.dll`
//! reads them) are a front for the library: the buttons (0x1 and 0x8 fire, 0x2 / 0x4 walk
//! forward / back), the heading's change (turn, the arena choice in the frontend), the pitch
//! from where it started (aim up / down), the credits. Its library is another build.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicU32, AtomicUsize, Ordering};
use std::time::Instant;

use wal_payload_common::{log, mapping::ButtonMap};
use wal_protocol::{Axis, button, output};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Native(u32);

const NATIVES: &[(&str, Native)] =
    &[("trigger", Native(0x1)), ("forward", Native(0x2)), ("back", Native(0x4)), ("jump", Native(0x6)), ("use", Native(0x8)), ("button5", Native(0x10))];

/// Virtual stick -> pod, overridden by the profile `native_map`. Start is the trigger (the pod
/// has no start button).
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("b2", "use"),
    ("b3", "jump"),
    ("up", "forward"),
    ("down", "back"),
    ("start", "trigger"),
];

static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();

/// The library's builds this module knows (`TimeDateStamp`), with their internal offsets:
/// the credit counter (shared section), the solenoid's on and off.
struct OgsBuild {
    stamp: u32,
    credits: usize,
    kick_on: usize,
    kick_off: usize,
}

const OGSDLL_BUILDS: &[OgsBuild] = &[
    // SiN
    OgsBuild { stamp: 0x3C79_BCAC, credits: 0x32074, kick_on: 0x3060, kick_off: 0x3080 },
    // Heavy Gear II
    OgsBuild { stamp: 0x3C98_2F2D, credits: 0x32074, kick_on: 0x3090, kick_off: 0x30b0 },
];

/// The frontends (executable name) and their dongle check (`int check()`, 0 the dongle found).
const FRONTENDS: &[(&str, usize)] = &[("test.exe", 0x57d0), ("hg2shell.exe", 0x57c0)];

/// The library's `azi_head`, `tilt_head`, `LastButtonStatus` exports.
static AZI: AtomicUsize = AtomicUsize::new(0);
static TILT: AtomicUsize = AtomicUsize::new(0);
static LAST_BUTTONS: AtomicUsize = AtomicUsize::new(0);

/// The head's angles, 11 bits plus the sub-position remainders (f32 bits), and when they
/// last moved.
static HEADING: AtomicU32 = AtomicU32::new(0);
static PITCH: AtomicU32 = AtomicU32::new(0);
static MOVED: OnceLock<std::sync::Mutex<Option<Instant>>> = OnceLock::new();

/// A full turn of the trackers' angles.
const TURN: f32 = 2048.0;
/// The pitch's limit, either way (about 80 degrees).
const PITCH_LIMIT: f32 = 455.0;

/// Turning speed at full tilt, angle units per second (`WAL_SIN_TURN_SPEED`, a full turn is
/// 2048); the head pitches at half that.
fn turn_speed() -> f32 {
    static SPEED: OnceLock<f32> = OnceLock::new();
    *SPEED.get_or_init(|| std::env::var("WAL_SIN_TURN_SPEED").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(1024.0))
}

fn map() -> &'static ButtonMap<Native> {
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP))
}

/// The pod's buttons now (5 bits).
fn buttons() -> i32 {
    let stick = wal_payload_common::input(0);
    let mut bits = 0;
    map().for_each_pressed(&stick, |Native(bit)| bits |= bit);
    bits as i32
}

/// `int GetButtons(int edge)`: the buttons, or with `edge` the bits that changed when the
/// value dropped (the library's `LastButtonStatus`).
unsafe extern "C" fn get_buttons(edge: i32) -> i32 {
    let now = buttons();
    if edge == 0 {
        return now;
    }
    let last = LAST_BUTTONS.load(Ordering::Relaxed) as *mut i32;
    if last.is_null() {
        return 0;
    }
    unsafe { edge_bits(&mut *last, now) }
}

/// The library's edge detection: the bits that changed, when the value dropped.
fn edge_bits(last: &mut i32, now: i32) -> i32 {
    let changed = *last ^ now;
    if changed == 0 {
        return 0;
    }
    let dropped = *last - now;
    *last = now;
    if dropped > 0 { changed } else { 0 }
}

/// An angle moved by the stick over `seconds`: wrapped to 11 bits (the heading) or clamped
/// to the pitch limit.
fn turn(angle: f32, axis: i16, seconds: f32, speed: f32, wrap: bool) -> f32 {
    let moved = angle + axis as f32 / 32767.0 * speed * seconds;
    if wrap { moved.rem_euclid(TURN) } else { moved.clamp(-PITCH_LIMIT, PITCH_LIMIT) }
}

/// `int gettrackerstatus(int)`: the head's angles from the stick (left stick across or right
/// stick across turns, right stick along pitches). Right turns lower the heading; up is a
/// small `tilt_head`, down one just under 0x800.
unsafe extern "C" fn get_tracker_status(_full: i32) -> i32 {
    let now = Instant::now();
    let seconds = {
        let mut moved = MOVED.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
        let s = moved.map_or(0.0, |t| now.duration_since(t).as_secs_f32().min(0.25));
        *moved = Some(now);
        s
    };
    let stick = wal_payload_common::input(0);
    let across = stick.axis(Axis::LeftX).saturating_add(stick.axis(Axis::RightX));
    let along = stick.axis(Axis::RightY);
    let heading = turn(f32::from_bits(HEADING.load(Ordering::Relaxed)), across.saturating_neg(), seconds, turn_speed(), true);
    let pitch = turn(f32::from_bits(PITCH.load(Ordering::Relaxed)), along, seconds, turn_speed() / 2.0, false);
    HEADING.store(heading.to_bits(), Ordering::Relaxed);
    PITCH.store(pitch.to_bits(), Ordering::Relaxed);
    let (azi, tilt) = (AZI.load(Ordering::Relaxed) as *mut i32, TILT.load(Ordering::Relaxed) as *mut i32);
    if !azi.is_null() && !tilt.is_null() {
        unsafe {
            azi.write(heading as i32 & 0x7ff);
            // the game's pitch is -tilt: up (negative) a small tilt, down wrapped under 0x800
            tilt.write((-(pitch as i32)) & 0x7ff);
        }
    }
    1
}

/// `int InitTracker()`: the head level, looking ahead.
unsafe extern "C" fn init_tracker() -> i32 {
    HEADING.store(0f32.to_bits(), Ordering::Relaxed);
    PITCH.store(0f32.to_bits(), Ordering::Relaxed);
    unsafe { get_tracker_status(1) };
    log!("sin: head tracker");
    0
}

unsafe extern "C" fn close_tracker() {}

/// The gun's solenoid on / off (the library's recoil pulses): the pad's rumble.
static KICK: AtomicI32 = AtomicI32::new(0);

unsafe extern "C" fn kick_on() {
    if KICK.swap(1, Ordering::Relaxed) == 0 {
        wal_payload_common::send_output(0, output::FFB_VIBRATION, output::FFB_MAX / 2);
    }
}

unsafe extern "C" fn kick_off() {
    if KICK.swap(0, Ordering::Relaxed) == 1 {
        wal_payload_common::send_output(0, output::FFB_VIBRATION, 0);
    }
}

/// A jump to `function` written at `at`.
fn patch_jump(at: usize, function: usize) {
    unsafe {
        let mut old = 0;
        VirtualProtect(at as _, 5, PAGE_EXECUTE_READWRITE, &mut old);
        (at as *mut u8).write(0xE9);
        ((at + 1) as *mut i32).write_unaligned(function.wrapping_sub(at + 5) as i32);
        VirtualProtect(at as _, 5, old, &mut old);
    }
}

/// The library's export `name`.
fn export(module: usize, name: &std::ffi::CStr) -> usize {
    unsafe { GetProcAddress(module as _, name.as_ptr().cast()) }.map_or(0, |f| f as usize)
}

/// A module's `TimeDateStamp`.
fn build(module: usize) -> u32 {
    unsafe {
        let nt = module + ((module + 0x3c) as *const u32).read_unaligned() as usize;
        ((nt + 8) as *const u32).read_unaligned()
    }
}

/// A coin: one credit in the library's counter.
fn coin(credits: usize) {
    let counter = unsafe { AtomicI32::from_ptr(credits as *mut i32) };
    let now = counter.fetch_add(1, Ordering::Relaxed) + 1;
    log!("sin: coin ({now} tokens)");
}

/// The coin button watched (the frontend, running for the whole session).
fn watch_coins(credits: usize) {
    std::thread::spawn(move || {
        let mut held = false;
        loop {
            let down = (0..2).any(|p| wal_payload_common::input(p).pressed(button::COIN));
            if down && !held {
                coin(credits);
            }
            held = down;
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });
}

/// The frontend's dongle check passes (its code checked first: the frontend's build).
fn pass_hardlock(check: usize) {
    let exe = wal_payload_common::iat::game_module();
    let at = exe + check;
    if crate::exe_size() <= check + 3 || unsafe { std::slice::from_raw_parts(at as *const u8, 3) } != [0x83, 0xEC, 0x1C] {
        log!("sin: no dongle check at +{check:#x}");
        return;
    }
    unsafe {
        let mut old = 0;
        VirtualProtect(at as _, 3, PAGE_EXECUTE_READWRITE, &mut old);
        // xor eax, eax; ret
        std::ptr::copy_nonoverlapping([0x31u8, 0xC0, 0xC3].as_ptr(), at as *mut u8, 3);
        VirtualProtect(at as _, 3, old, &mut old);
    }
    log!("sin: dongle check passed");
}

/// The OpenGL extensions the game's renderer (`ref_gl.dll`) looks for: its extension string
/// is printed into a 1 KB buffer (a long one overwrote the stack), the rest is dropped.
const GL_EXTENSIONS_USED: &[&str] = &[
    "GL_EXT_compiled_vertex_array",
    "GL_SGI_compiled_vertex_array",
    "GL_EXT_paletted_texture",
    "GL_EXT_point_parameters",
    "GL_EXT_shared_texture_palette",
    "GL_SGIS_multitexture",
    "WGL_EXT_swap_control",
];
const GL_EXTENSIONS: u32 = 0x1F03;

static ORIG_LOAD_LIBRARY: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static ORIG_GL_GET_STRING: AtomicUsize = AtomicUsize::new(0);
static EXTENSIONS: OnceLock<std::ffi::CString> = OnceLock::new();

/// The extensions of `all` the renderer uses, in its order.
fn used_extensions(all: &str) -> String {
    all.split_whitespace().filter(|e| GL_EXTENSIONS_USED.contains(e)).collect::<Vec<_>>().join(" ")
}

/// The renderer's `glGetString`: the extension string reduced to [`GL_EXTENSIONS_USED`].
unsafe extern "system" fn gl_get_string(name: u32) -> *const u8 {
    let orig: unsafe extern "system" fn(u32) -> *const u8 = unsafe { std::mem::transmute(ORIG_GL_GET_STRING.load(Ordering::Relaxed)) };
    let real = unsafe { orig(name) };
    if name != GL_EXTENSIONS || real.is_null() {
        return real;
    }
    EXTENSIONS
        .get_or_init(|| {
            let all = unsafe { std::ffi::CStr::from_ptr(real.cast()) }.to_string_lossy().into_owned();
            let used = used_extensions(&all);
            log!("sin: OpenGL extensions for the renderer: {used} ({} bytes before)", all.len());
            std::ffi::CString::new(used).unwrap_or_default()
        })
        .as_ptr()
        .cast()
}

/// The renderer's `GetProcAddress` (its OpenGL functions): `glGetString` is ours.
unsafe extern "system" fn renderer_get_proc_address(module: usize, name: *const u8) -> usize {
    let orig: unsafe extern "system" fn(usize, *const u8) -> usize = unsafe { std::mem::transmute(ORIG_GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    let f = unsafe { orig(module, name) };
    if f != 0 && name as usize > 0xffff && unsafe { std::ffi::CStr::from_ptr(name.cast()) } == c"glGetString" {
        ORIG_GL_GET_STRING.store(f, Ordering::Relaxed);
        return gl_get_string as *const () as usize;
    }
    f
}

/// The game's `LoadLibraryA`: its renderer's OpenGL calls hooked once loaded.
unsafe extern "system" fn load_library(name: *const u8) -> usize {
    let orig: unsafe extern "system" fn(*const u8) -> usize = unsafe { std::mem::transmute(ORIG_LOAD_LIBRARY.load(Ordering::Relaxed)) };
    let module = unsafe { orig(name) };
    if module != 0 && !name.is_null() {
        let file = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy().to_ascii_lowercase();
        if file.trim_end_matches(".dll").ends_with("ref_gl")
            && let Some(o) = unsafe {
                wal_payload_common::iat::hook_module(module, "kernel32.dll", "GetProcAddress", renderer_get_proc_address as *const () as usize)
            }
        {
            ORIG_GET_PROC_ADDRESS.store(o, Ordering::Relaxed);
        }
    }
    module
}

/// Appended to the game's command line by the frontend's game start: the renderer's OpenGL
/// library is the cabinet's 3dfx MiniGL by default (`3dfxgl.dll`, on Glide), Wine's instead
/// (the later `+set` wins over the game's saved `config.cfg`).
pub(crate) const EXTRA_ARGS: &str = " +set gl_driver opengl32";

/// The game a frontend's `CreateProcessA(app, cmd)` starts, lowercase, if it is a pod game:
/// SiN's `sin.exe` or Heavy Gear II's `hgx.exe`.
pub(crate) fn pod_game(app: &str, cmd: &str) -> Option<&'static str> {
    let exe = if app.is_empty() { cmd.split_whitespace().next().unwrap_or("") } else { app };
    let exe = exe.trim_matches('"').to_ascii_lowercase();
    ["sin.exe", "hgx.exe"].into_iter().find(|game| exe.ends_with(game))
}

pub(crate) fn init() {
    let ogs = unsafe { GetModuleHandleA(c"ogsdll.dll".as_ptr().cast()) } as usize;
    if ogs == 0 {
        return;
    }
    AZI.store(export(ogs, c"azi_head"), Ordering::Relaxed);
    TILT.store(export(ogs, c"tilt_head"), Ordering::Relaxed);
    LAST_BUTTONS.store(export(ogs, c"LastButtonStatus"), Ordering::Relaxed);
    for (name, function) in [
        (c"GetButtons", get_buttons as *const () as usize),
        (c"InitTracker", init_tracker as *const () as usize),
        (c"CloseTracker", close_tracker as *const () as usize),
        (c"gettrackerstatus", get_tracker_status as *const () as usize),
    ] {
        match export(ogs, name) {
            0 => log!("sin: ogsdll.dll has no {name:?}"),
            at => patch_jump(at, function),
        }
    }
    let known = OGSDLL_BUILDS.iter().find(|b| b.stamp == build(ogs));
    match known {
        Some(b) => {
            patch_jump(ogs + b.kick_on, kick_on as *const () as usize);
            patch_jump(ogs + b.kick_off, kick_off as *const () as usize);
        }
        None => log!("sin: unknown ogsdll.dll build {:#x}: no coins, no recoil", build(ogs)),
    }
    let exe = crate::frontend_fit::exe_name();
    if let Some(&(_, check)) = FRONTENDS.iter().find(|(name, _)| *name == exe) {
        pass_hardlock(check);
        crate::beachhead::hook_game_start(wal_payload_common::iat::game_module());
        if let Some(b) = known {
            watch_coins(ogs + b.credits);
        }
        log!("sin: frontend");
    } else {
        if let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "LoadLibraryA", load_library as *const () as usize) } {
            ORIG_LOAD_LIBRARY.store(o, Ordering::Relaxed);
        }
        log!("sin: game");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_on_release() {
        let mut last = 0;
        assert_eq!(edge_bits(&mut last, 1), 0);
        assert_eq!(last, 1);
        assert_eq!(edge_bits(&mut last, 1), 0);
        assert_eq!(edge_bits(&mut last, 0), 1);
        assert_eq!(edge_bits(&mut last, 0), 0);
    }

    #[test]
    fn renderer_extensions() {
        assert_eq!(used_extensions("GL_ARB_multitexture GL_EXT_compiled_vertex_array GL_ARB_foo WGL_EXT_swap_control"), "GL_EXT_compiled_vertex_array WGL_EXT_swap_control");
    }

    #[test]
    fn angles_wrap_and_clamp() {
        assert_eq!(turn(2040.0, 32767, 0.01, 1024.0, true), (2040.0 + 10.24f32).rem_euclid(2048.0));
        assert_eq!(turn(5.0, -32767, 0.01, 1024.0, true), 5.0 - 10.24 + 2048.0);
        assert_eq!(turn(450.0, 32767, 1.0, 512.0, false), PITCH_LIMIT);
        assert_eq!(turn(-450.0, -32767, 1.0, 512.0, false), -PITCH_LIMIT);
    }
}
