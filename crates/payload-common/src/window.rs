//! Game window size override: `WAL_WINDOW_SIZE=<w>x<h>` (`screen`: the primary monitor's size).
//!
//! The game's `user32!CreateWindowExA/W` (top-level windows with a size), `SetWindowPos` and
//! `MoveWindow` calls that size a top-level window are replaced by `<w>x<h>` at (0,0). In window mode Direct3D stretches the back buffer to the
//! client area, so this also rescales a game drawing a fixed-size back buffer. Crimzon Clover
//! (DxLib) computes a window height of ~5 million pixels, which X refuses (BadAlloc); Haunted
//! Museum creates its window 1286x5434821 at CW_USEDEFAULT (empty frame, desktop behind).
//!
//! `WAL_WINDOW_SIZE=picture`: the largest centered rectangle of `WAL_D3D9_ASPECT` on the screen,
//! at its place (the whole screen with `WAL_KEEP_ASPECT_RATIO=0`); `aspect`: that rectangle
//! whatever `WAL_KEEP_ASPECT_RATIO`. For a player drawing at its window's size (PGA TOUR Team
//! Challenge's Anark frontend: 4:3 in any window, its sides unpainted), a bezel (`WAL_BEZEL`)
//! around it.
//!
//! `WAL_WINDOW_POPUP=1`: the game's top-level windows are created as borderless popups
//! (`WS_POPUP`, no caption, frame or system menu). Shikigami no Shiro III creates a plain
//! overlapped window (style 0: Windows adds a caption and border) and makes its Direct3D 8
//! device fullscreen on it: the window manager showed an empty frame, desktop behind.
//! Their menu bars are removed too (the window class's or `CreateWindowEx`'s, and the game's
//! later `SetMenu`): Thrill Drive 4's DirectX sample framework window showed its File menu
//! (Go/stop, Single step, Change device, Exit) above the picture. Their later style changes
//! (`SetWindowLong(Ptr)A/W`) keep them popups: Time Crisis 5 (UE3) sets its captioned window
//! style back on its window, a frame around a smaller picture; Hog Wild's `ftu_GameWin.dll`
//! sets style 8 on its shown window (its DLL's calls hooked too, `WS_VISIBLE` kept).
//!
//! `WAL_NO_MODE_CHANGE=1`: the game's (and its directory's DLLs', and wined3d's: DirectDraw)
//! display mode changes (`ChangeDisplaySettings(Ex)A/W`) do
//! nothing and succeed. For games made windowed (`WAL_D3D9_WINDOWED_SIZE`) that also set their
//! mode themselves: Wine emulates the mode by scaling it into the screen, the screen-sized
//! window scaled with it (Blazing Angels: 1280x720 on a 1600x1200 screen, the window's picture
//! 2000x1500 from y=150, cut at the bottom).
//!
//! `WAL_HIDE_CURSOR=1`: no mouse cursor over the game (a cabinet has none; Battle Gear 4 shows
//! the arrow of its window class): `LoadCursorA/W` return no cursor and `SetCursor` sets none.
//!
//! Always: Direct3D (DXVK's d3d9, also behind its d3d8, and wined3d) minimizes a fullscreen
//! window when it is deactivated. A game started while another fullscreen window keeps the
//! focus (a terminal) was minimized at once, out of the taskbar, and stopped presenting. Their
//! `ShowWindow` minimize requests are dropped: the game keeps running behind, switch to it.
//!
//! d7vk (`graphics: d7vk`: a native `ddraw.dll`, DXVK's d3d9 built in, no `d3d9.dll`) and the
//! wined3d it loads later (Wine's ddraw, which d7vk hands the cooperative level and the display
//! mode) get the same: their minimizes dropped, their mode changes not done
//! (`WAL_NO_MODE_CHANGE`), their window sizing too (`WAL_WINDOW_SIZE`). Burning Striker's
//! exclusive full screen window was minimized by wined3d when another window took the focus
//! (a click outside, the bezel's window), and put back over the whole screen.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use crate::{iat, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetDesktopWindow, GetMenu, GetParent, GetWindowLongW, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, SWP_NOMOVE, SWP_NOSIZE, SetMenu,
};

type HWND = *mut c_void;

static WIDTH: AtomicU32 = AtomicU32::new(0);
/// `WAL_WINDOW_SIZE=picture`: the window's place (0, 0 otherwise).
static LEFT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static TOP: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
static HEIGHT: AtomicU32 = AtomicU32::new(0);
/// With the Sinden frame: the size each top-level window asked for before it was forced to
/// the screen (a 0x0 back buffer is the window's size: the game's, not the screen's).
static REQUESTED: std::sync::Mutex<Vec<(usize, i32, i32)>> = std::sync::Mutex::new(Vec::new());
static ORIG_SET_WINDOW_POS: AtomicUsize = AtomicUsize::new(0);
static ORIG_MOVE_WINDOW: AtomicUsize = AtomicUsize::new(0);
static ORIG_SHOW_WINDOW: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_WINDOW_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_WINDOW_W: AtomicUsize = AtomicUsize::new(0);
static POPUP: AtomicBool = AtomicBool::new(false);
static ORIG_SET_MENU: AtomicUsize = AtomicUsize::new(0);
/// `SetWindowLongA`, `SetWindowLongW`, `SetWindowLongPtrA`, `SetWindowLongPtrW`.
static ORIG_SET_WINDOW_LONG: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

pub fn init() {
    init_options();
    if let Some(base) = d7vk() {
        d7vk_hooks(base);
    }
}

fn init_options() {
    keep_unminimized();
    if std::env::var("WAL_HIDE_CURSOR").is_ok_and(|v| v.trim() == "1") {
        hide_cursor();
    }
    if crate::border::config().is_some() || std::env::var("WAL_NO_MODE_CHANGE").is_ok_and(|v| v.trim() == "1") {
        no_mode_change();
    }
    if crate::border::config().is_some() || std::env::var("WAL_WINDOW_POPUP").is_ok_and(|v| v.trim() == "1") {
        POPUP.store(true, Ordering::Relaxed);
        hook_create();
        unsafe {
            if let Some(o) = iat::hook("user32.dll", "SetMenu", set_menu as *const () as usize) {
                ORIG_SET_MENU.store(o, Ordering::Relaxed);
            }
            let hooks: [(&str, usize); 4] = [
                ("SetWindowLongA", set_window_long::<0> as *const () as usize),
                ("SetWindowLongW", set_window_long::<1> as *const () as usize),
                ("SetWindowLongPtrA", set_window_long_ptr::<2> as *const () as usize),
                ("SetWindowLongPtrW", set_window_long_ptr::<3> as *const () as usize),
            ];
            // the game's and its directory's DLLs' (Hog Wild's ftu_GameWin.dll)
            for (i, (name, f)) in hooks.into_iter().enumerate() {
                hook_game(name, f, &ORIG_SET_WINDOW_LONG[i]);
            }
        }
        log!("window: top-level windows created as popups");
    }
    let Some((w, h)) = std::env::var("WAL_WINDOW_SIZE").ok().or_else(|| crate::border::config().map(|_| "screen".into())).and_then(|v| {
        if v.trim() == "screen" {
            // primary monitor size (no display mode change in window mode)
            let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
            return Some((w as u32, h as u32));
        }
        if matches!(v.trim(), "picture" | "aspect") {
            // the WAL_D3D9_ASPECT rectangle centered on the screen (`picture`: the whole screen
            // with WAL_KEEP_ASPECT_RATIO=0)
            let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
            let aspect = if v.trim() == "aspect" { crate::screenshot::aspect_setting() } else { crate::screenshot::d3d9_aspect() };
            let (l, t, w, h) = match aspect {
                Some((aw, ah)) if sw as u64 * ah as u64 > sh as u64 * aw as u64 => {
                    let w = (sh as u64 * aw as u64 / ah as u64) as i32;
                    ((sw - w) / 2, 0, w, sh)
                }
                Some((aw, ah)) => {
                    let h = (sw as u64 * ah as u64 / aw as u64) as i32;
                    (0, (sh - h) / 2, sw, h)
                }
                None => (0, 0, sw, sh),
            };
            LEFT.store(l, Ordering::Relaxed);
            TOP.store(t, Ordering::Relaxed);
            return Some((w as u32, h as u32));
        }
        let (w, h) = v.split_once('x')?;
        Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
    }) else {
        return;
    };
    let (w,h) = if crate::border::config().is_some() {
        LEFT.store(0, Ordering::Relaxed);
        TOP.store(0, Ordering::Relaxed);
        let (w,h) = crate::desktop::screen_size();
        (w as u32,h as u32)
    } else { (w,h) };
    WIDTH.store(w, Ordering::Relaxed);
    HEIGHT.store(h, Ordering::Relaxed);
    hook_create();
    hook_game("SetWindowPos", set_window_pos as *const () as usize, &ORIG_SET_WINDOW_POS);
    hook_game("MoveWindow", move_window as *const () as usize, &ORIG_MOVE_WINDOW);
    SIZED.store(true, Ordering::Relaxed);
    log!("window: top-level windows sized {w}x{h}");
}

static ORIG_SET_CURSOR: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn load_cursor(_instance: *mut c_void, _name: *const c_void) -> *mut c_void {
    std::ptr::null_mut()
}

unsafe extern "system" fn set_cursor(_cursor: *mut c_void) -> *mut c_void {
    let orig: unsafe extern "system" fn(*mut c_void) -> *mut c_void =
        unsafe { std::mem::transmute(ORIG_SET_CURSOR.load(Ordering::Relaxed)) };
    unsafe { orig(std::ptr::null_mut()) }
}

fn hide_cursor() {
    unsafe {
        iat::hook("user32.dll", "LoadCursorA", load_cursor as *const () as usize);
        iat::hook("user32.dll", "LoadCursorW", load_cursor as *const () as usize);
        if let Some(o) = iat::hook("user32.dll", "SetCursor", set_cursor as *const () as usize) {
            ORIG_SET_CURSOR.store(o, Ordering::Relaxed);
        }
    }
    log!("window: mouse cursor hidden");
}

unsafe extern "system" fn change_display_settings(mode: *const c_void, flags: u32) -> i32 {
    unsafe { change_display_settings_ex(std::ptr::null(), mode, std::ptr::null_mut(), flags, std::ptr::null_mut()) }
}

unsafe extern "system" fn change_display_settings_ex(_name: *const c_void, mode: *const c_void, _hwnd: HWND, flags: u32, _param: P) -> i32 {
    static LOGGED: AtomicBool = AtomicBool::new(false);
    if !LOGGED.swap(true, Ordering::Relaxed) {
        log!("window: display mode change (mode {mode:?}, flags {flags:#x}) not done");
    }
    0 // DISP_CHANGE_SUCCESSFUL
}

fn no_mode_change() {
    let hooks: [(&str, usize); 4] = [
        ("ChangeDisplaySettingsA", change_display_settings as *const () as usize),
        ("ChangeDisplaySettingsW", change_display_settings as *const () as usize),
        ("ChangeDisplaySettingsExA", change_display_settings_ex as *const () as usize),
        ("ChangeDisplaySettingsExW", change_display_settings_ex as *const () as usize),
    ];
    // DirectDraw's mode changes (Wine's ddraw.dll through wined3d.dll: PGA TOUR Team Challenge's
    // Anark frontend, its 800x600 mode emulated, its OpenGL picture not scaled with it)
    let wined3d = unsafe { GetModuleHandleW(windows_sys::w!("wined3d.dll")) } as usize;
    // (their replacements never call the original functions)
    static UNUSED: AtomicUsize = AtomicUsize::new(0);
    for (name, f) in hooks {
        hook_game(name, f, &UNUSED);
        if wined3d != 0 {
            unsafe { iat::hook_module(wined3d, "user32.dll", name, f) };
        }
    }
    NO_MODE.store(true, Ordering::Relaxed);
    log!("window: display mode changes not done{}", if wined3d != 0 { " (wined3d's too)" } else { "" });
}

/// `user32!name` of the game module and the DLLs of its directory (also those it loads later)
/// replaced by `f`, the original function in `orig`.
fn hook_game(name: &'static str, f: usize, orig: &'static AtomicUsize) {
    crate::late_hooks::hook("user32.dll", name, f, orig);
}

fn hook_create() {
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        hook_game("CreateWindowExA", create_window_a as *const () as usize, &ORIG_CREATE_WINDOW_A);
        hook_game("CreateWindowExW", create_window_w as *const () as usize, &ORIG_CREATE_WINDOW_W);
    });
}

/// The forced place and size (x, y, w, h) when `hwnd` is a top-level window, logging the
/// replaced geometry.
pub(crate) fn forced(hwnd: HWND, x: i32, y: i32, cx: i32, cy: i32) -> Option<(i32, i32, i32, i32)> {
    if !unsafe { GetParent(hwnd) }.is_null() {
        return None;
    }
    let (w, h) = (WIDTH.load(Ordering::Relaxed) as i32, HEIGHT.load(Ordering::Relaxed) as i32);
    if w == 0 || h == 0 {
        return None; // no WAL_WINDOW_SIZE
    }
    if cx == 0 && cy == 0 {
        // collapsed on purpose (Time Crisis 5 entering its operator menu): left so. A window
        // not shown yet is sized: Mach Storm sizes its new window from the work area less the
        // screen, 0x0 on the Batocera box (Wine's work area 18 pixels taller than the screen)
        if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd) } != 0 {
            log!("window: {hwnd:?} collapsed to 0x0 at {x},{y}: left so");
            return None;
        }
    }
    let (l, t) = (LEFT.load(Ordering::Relaxed), TOP.load(Ordering::Relaxed));
    if cx > 0 && cy > 0 {
        remember(hwnd, cx, cy);
    }
    if (x, y, cx, cy) != (l, t, w, h) {
        log!("window: {hwnd:?} {cx}x{cy} at {x},{y} -> {w}x{h} at {l},{t}");
    }
    Some((l, t, w, h))
}

unsafe extern "system" fn set_window_pos(hwnd: HWND, after: HWND, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32 {
    let orig: unsafe extern "system" fn(HWND, HWND, i32, i32, i32, i32, u32) -> i32 =
        unsafe { std::mem::transmute(ORIG_SET_WINDOW_POS.load(Ordering::Relaxed)) };
    if flags & SWP_NOSIZE == 0 {
        if let Some((l, t, w, h)) = forced(hwnd, x, y, cx, cy) {
            return unsafe { orig(hwnd, after, l, t, w, h, flags & !SWP_NOMOVE) };
        }
    }
    unsafe { orig(hwnd, after, x, y, cx, cy, flags) }
}

unsafe extern "system" fn move_window(hwnd: HWND, x: i32, y: i32, cx: i32, cy: i32, repaint: i32) -> i32 {
    let orig: unsafe extern "system" fn(HWND, i32, i32, i32, i32, i32) -> i32 =
        unsafe { std::mem::transmute(ORIG_MOVE_WINDOW.load(Ordering::Relaxed)) };
    match forced(hwnd, x, y, cx, cy) {
        Some((l, t, w, h)) => unsafe { orig(hwnd, l, t, w, h, repaint) },
        None => unsafe { orig(hwnd, x, y, cx, cy, repaint) },
    }
}

const SW_SHOWMINIMIZED: i32 = 2;
const SW_MINIMIZE: i32 = 6;
const SW_SHOWMINNOACTIVE: i32 = 7;
const SW_FORCEMINIMIZE: i32 = 11;

/// d7vk's `ddraw.dll` when loaded (statically imported by the game): a native DLL, not Wine's
/// builtin one (whose DOS stub says "Wine builtin DLL"; its window work is wined3d's).
fn d7vk() -> Option<usize> {
    let base = unsafe { GetModuleHandleW(windows_sys::w!("ddraw.dll")) } as usize;
    if base == 0 {
        return None;
    }
    let stub = unsafe { std::slice::from_raw_parts((base + 0x40) as *const u8, 16) };
    (stub != b"Wine builtin DLL").then_some(base)
}

/// `WAL_NO_MODE_CHANGE` / `WAL_WINDOW_SIZE` in effect (for the renderer modules hooked later).
static NO_MODE: AtomicBool = AtomicBool::new(false);
static SIZED: AtomicBool = AtomicBool::new(false);
static WINED3D_HOOKED: AtomicBool = AtomicBool::new(false);
static ORIG_LOAD_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_LOAD_W: AtomicUsize = AtomicUsize::new(0);

/// A Direct3D module's window work made the game's: its minimizes dropped, its mode changes not
/// done and its window sizing forced (with the options).
fn hook_renderer(base: usize) {
    hook_show_window(base);
    if NO_MODE.load(Ordering::Relaxed) {
        for (name, f) in [
            ("ChangeDisplaySettingsA", change_display_settings as *const () as usize),
            ("ChangeDisplaySettingsW", change_display_settings as *const () as usize),
            ("ChangeDisplaySettingsExA", change_display_settings_ex as *const () as usize),
            ("ChangeDisplaySettingsExW", change_display_settings_ex as *const () as usize),
        ] {
            unsafe { iat::hook_module(base, "user32.dll", name, f) };
        }
    }
    if SIZED.load(Ordering::Relaxed) {
        for (name, f, orig) in [
            ("SetWindowPos", set_window_pos as *const () as usize, &ORIG_SET_WINDOW_POS),
            ("MoveWindow", move_window as *const () as usize, &ORIG_MOVE_WINDOW),
        ] {
            if let Some(o) = unsafe { iat::hook_module(base, "user32.dll", name, f) }.filter(|&o| o != f) {
                let _ = orig.compare_exchange(0, o, Ordering::Relaxed, Ordering::Relaxed);
            }
        }
    }
}

/// d7vk's window work, and that of the wined3d it loads later (Wine's ddraw, which d7vk hands
/// what it does not implement: the cooperative level, the display mode; wined3d minimized the
/// full screen window when it lost the focus).
fn d7vk_hooks(base: usize) {
    hook_renderer(base);
    for (name, f, orig) in [
        ("LoadLibraryA", d7vk_load_a as *const () as usize, &ORIG_LOAD_A),
        ("LoadLibraryW", d7vk_load_w as *const () as usize, &ORIG_LOAD_W),
    ] {
        if let Some(o) = unsafe { iat::hook_module(base, "kernel32.dll", name, f) }.filter(|&o| o != f) {
            orig.store(o, Ordering::Relaxed);
        }
    }
    log!("window: d7vk's window work hooked");
    wined3d_loaded();
}

fn wined3d_loaded() {
    let base = unsafe { GetModuleHandleW(windows_sys::w!("wined3d.dll")) } as usize;
    if base != 0 && !WINED3D_HOOKED.swap(true, Ordering::Relaxed) {
        hook_renderer(base);
        log!("window: wined3d's window work hooked (loaded by d7vk)");
    }
}

unsafe extern "system" fn d7vk_load_a(name: *const u8) -> *mut c_void {
    let orig: unsafe extern "system" fn(*const u8) -> *mut c_void = unsafe { std::mem::transmute(ORIG_LOAD_A.load(Ordering::Relaxed)) };
    let module = unsafe { orig(name) };
    wined3d_loaded();
    module
}

unsafe extern "system" fn d7vk_load_w(name: *const u16) -> *mut c_void {
    let orig: unsafe extern "system" fn(*const u16) -> *mut c_void = unsafe { std::mem::transmute(ORIG_LOAD_W.load(Ordering::Relaxed)) };
    let module = unsafe { orig(name) };
    wined3d_loaded();
    module
}

fn hook_show_window(base: usize) {
    if let Some(o) = unsafe { iat::hook_module(base, "user32.dll", "ShowWindow", show_window as *const () as usize) }.filter(|&o| o != show_window as *const () as usize) {
        ORIG_SHOW_WINDOW.store(o, Ordering::Relaxed);
    }
}

/// Hooks `ShowWindow` in the Direct3D modules already loaded (statically imported by the game).
fn keep_unminimized() {
    let modules = [windows_sys::w!("d3d9.dll"), windows_sys::w!("wined3d.dll")].map(|dll| unsafe { GetModuleHandleW(dll) } as usize);
    for base in modules.into_iter().filter(|&b| b != 0) {
        hook_show_window(base);
    }
}

unsafe extern "system" fn show_window(hwnd: HWND, cmd: i32) -> i32 {
    if matches!(cmd, SW_SHOWMINIMIZED | SW_MINIMIZE | SW_SHOWMINNOACTIVE | SW_FORCEMINIMIZE) {
        log!("window: Direct3D minimize of {hwnd:?} ignored (focus lost)");
        return 1;
    }
    let orig: unsafe extern "system" fn(HWND, i32) -> i32 = unsafe { std::mem::transmute(ORIG_SHOW_WINDOW.load(Ordering::Relaxed)) };
    unsafe { orig(hwnd, cmd) }
}

const WS_POPUP: u32 = 0x8000_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
/// Caption, frame, system menu, minimize/maximize boxes.
const WS_DECORATIONS: u32 = 0x00C0_0000 | 0x0004_0000 | 0x0008_0000 | 0x0002_0000 | 0x0001_0000;
/// Extended styles adding a border.
const WS_EX_DECORATIONS: u32 = 0x0000_0001 | 0x0000_0100 | 0x0000_0200 | 0x0002_0000;

/// A window created with this parent is top-level: none, or the desktop window (Need for
/// Speed GT's frontend creates its window as a child of `GetDesktopWindow()`).
fn top_level(parent: HWND) -> bool {
    parent.is_null() || parent == unsafe { GetDesktopWindow() }
}

/// Style and geometry of a window being created: popup style (`WAL_WINDOW_POPUP`) and forced
/// size at 0,0 (`WAL_WINDOW_SIZE`) for top-level windows ([`top_level`]) with a size.
pub(crate) fn created(ex: u32, style: u32, x: i32, y: i32, w: i32, h: i32, parent: HWND) -> (u32, u32, i32, i32, i32, i32) {
    // child windows, and hidden helper windows created without a size (CRI's
    // CriDSoundOutput, its DirectSound window: as a popup the game was silent)
    if !top_level(parent) || style & WS_CHILD != 0 || (w == 0 && h == 0) {
        return (ex, style, x, y, w, h);
    }
    let (mut ex2, mut style2, mut geometry) = (ex, style, (x, y, w, h));
    if POPUP.load(Ordering::Relaxed) {
        ex2 = ex & !WS_EX_DECORATIONS;
        style2 = (style & !WS_DECORATIONS) | WS_POPUP;
    }
    let (fw, fh) = (WIDTH.load(Ordering::Relaxed) as i32, HEIGHT.load(Ordering::Relaxed) as i32);
    if fw > 0 && (w != 0 || h != 0) {
        geometry = (LEFT.load(Ordering::Relaxed), TOP.load(Ordering::Relaxed), fw, fh);
    }
    if (ex2, style2, geometry) != (ex, style, (x, y, w, h)) {
        log!("window: created style {style:#x} ex {ex:#x} {w}x{h} at {x},{y} -> style {style2:#x} ex {ex2:#x} {}x{} at {},{}", geometry.2, geometry.3, geometry.0, geometry.1);
    }
    (ex2, style2, geometry.0, geometry.1, geometry.2, geometry.3)
}

/// `SetMenu` on a top-level popup: its menu bar stays removed.
unsafe extern "system" fn set_menu(hwnd: HWND, menu: P) -> i32 {
    let orig: unsafe extern "system" fn(HWND, P) -> i32 = unsafe { std::mem::transmute(ORIG_SET_MENU.load(Ordering::Relaxed)) };
    if !menu.is_null() && unsafe { GetParent(hwnd) }.is_null() {
        return 1;
    }
    unsafe { orig(hwnd, menu) }
}

const GWL_STYLE: i32 = -16;
const GWL_EXSTYLE: i32 = -20;

/// A style or extended style set on a top-level popup, its decorations removed (the windows
/// [`created`] left as they were stay so).
fn kept_popup(hwnd: HWND, index: i32, value: isize) -> isize {
    if !matches!(index, GWL_STYLE | GWL_EXSTYLE) || !unsafe { GetParent(hwnd) }.is_null() {
        return value;
    }
    if unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32 & WS_POPUP == 0 {
        return value;
    }
    let v = value as u32;
    let kept = if index == GWL_STYLE {
        if v & WS_CHILD != 0 {
            return value;
        }
        // a shown window stays shown (Hog Wild's style 8: no longer visible to Wine, the strips
        // of a bezel never found it)
        let current = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
        (v & !WS_DECORATIONS) | WS_POPUP | (current & WS_VISIBLE)
    } else {
        v & !WS_EX_DECORATIONS
    };
    if kept != v {
        log!("window: {hwnd:?} {} {v:#x} -> {kept:#x}", if index == GWL_STYLE { "style" } else { "ex style" });
    }
    kept as i32 as isize
}

unsafe extern "system" fn set_window_long<const I: usize>(hwnd: HWND, index: i32, value: i32) -> i32 {
    let orig: unsafe extern "system" fn(HWND, i32, i32) -> i32 = unsafe { std::mem::transmute(ORIG_SET_WINDOW_LONG[I].load(Ordering::Relaxed)) };
    unsafe { orig(hwnd, index, kept_popup(hwnd, index, value as isize) as i32) }
}

unsafe extern "system" fn set_window_long_ptr<const I: usize>(hwnd: HWND, index: i32, value: isize) -> isize {
    let orig: unsafe extern "system" fn(HWND, i32, isize) -> isize = unsafe { std::mem::transmute(ORIG_SET_WINDOW_LONG[I].load(Ordering::Relaxed)) };
    unsafe { orig(hwnd, index, kept_popup(hwnd, index, value)) }
}

/// A new top-level popup's menu bar (its class's or the one passed) removed.
fn drop_menu(hwnd: HWND, parent: HWND) -> HWND {
    if POPUP.load(Ordering::Relaxed) && !hwnd.is_null() && top_level(parent) && !unsafe { GetMenu(hwnd) }.is_null() {
        unsafe { SetMenu(hwnd, std::ptr::null_mut()) };
        log!("window: {hwnd:?} menu bar removed");
    }
    hwnd
}

fn remember(hwnd: HWND, w: i32, h: i32) {
    if hwnd.is_null() || crate::border::config().is_none() {
        return;
    }
    let mut requested = REQUESTED.lock().unwrap();
    requested.retain(|r| r.0 != hwnd as usize);
    requested.push((hwnd as usize, w, h));
}

/// The size the top-level window `hwnd` asked for (Sinden frame: its real one is the screen's).
/// The profile's `WAL_WINDOW_SIZE` (`WxH`) first: the size the game draws at (Gundam Spirits of
/// Zeon asks for a screen-sized window, its 640x480 picture then in a corner of the back buffer).
pub fn requested(hwnd: HWND) -> Option<(u32, u32)> {
    let profile = std::env::var("WAL_WINDOW_SIZE").ok().and_then(|v| {
        let (w, h) = v.trim().split_once('x')?;
        Some((w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?))
    });
    if profile.is_some_and(|(w, h)| w > 0 && h > 0) {
        return profile;
    }
    let requested = REQUESTED.lock().unwrap();
    requested.iter().find(|r| r.0 == hwnd as usize).map(|r| (r.1 as u32, r.2 as u32))
}

/// A new top-level window forced to another size: the size it asked for remembered.
pub(crate) fn remember_created(hwnd: HWND, parent: HWND, style: u32, w: i32, h: i32, forced: (i32, i32)) {
    if top_level(parent) && style & WS_CHILD == 0 && w > 0 && h > 0 && (w, h) != forced {
        remember(hwnd, w, h);
    }
}

fn created_window(hwnd: HWND, parent: HWND, style: u32, w: i32, h: i32, forced: (i32, i32)) -> HWND {
    remember_created(hwnd, parent, style, w, h, forced);
    drop_menu(hwnd, parent)
}

type CreateWindow<T> = unsafe extern "system" fn(u32, *const T, *const T, u32, i32, i32, i32, i32, HWND, P, P, P) -> HWND;
type P = *mut c_void;

unsafe extern "system" fn create_window_a(
    ex: u32, class: *const u8, title: *const u8, style: u32, x: i32, y: i32, w: i32, h: i32, parent: HWND, menu: P, inst: P, param: P,
) -> HWND {
    let orig: CreateWindow<u8> = unsafe { std::mem::transmute(ORIG_CREATE_WINDOW_A.load(Ordering::Relaxed)) };
    let (ex2, style2, x2, y2, w2, h2) = created(ex, style, x, y, w, h, parent);
    let hwnd = unsafe { orig(ex2, class, title, style2, x2, y2, w2, h2, parent, menu, inst, param) };
    created_window(hwnd, parent, style, w, h, (w2, h2))
}

unsafe extern "system" fn create_window_w(
    ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: HWND, menu: P, inst: P, param: P,
) -> HWND {
    let orig: CreateWindow<u16> = unsafe { std::mem::transmute(ORIG_CREATE_WINDOW_W.load(Ordering::Relaxed)) };
    let (ex2, style2, x2, y2, w2, h2) = created(ex, style, x, y, w, h, parent);
    let hwnd = unsafe { orig(ex2, class, title, style2, x2, y2, w2, h2, parent, menu, inst, param) };
    created_window(hwnd, parent, style, w, h, (w2, h2))
}
