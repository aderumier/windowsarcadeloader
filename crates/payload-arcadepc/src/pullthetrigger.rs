//! Pull The Trigger (Digital Silkroad, 2003, `PTW-BootTest.exe`: the decrypted executable of
//! the WibuKey-protected `PTW.exe`, OpenGL, 32-bit): `WAL_ARCADEPC_BOARD=pullthetrigger`.
//!
//! A one-gun shooting game (two players: two cabinets linked). The cabinet's gun board talks
//! through COM2 (MarshallSoft `wsc32.dll`: the aim, the trigger, the start button), but the
//! game's window procedure takes the same from the mouse and the keyboard, and goes on without
//! the board ("iolink setup fail"). The payload plays that mouse and keyboard with the virtual
//! sticks (`payload-common`'s `input_filter`: the real keyboard and mouse never reach the game):
//!
//! * the gun is the mouse on the game window's client area (the dump's community
//!   `PttBootTest.dll` stretches the 640x480 window to the screen's 4:3 and scales its mouse
//!   back, once it knows the window: see below), the trigger its left button: a shot, and the
//!   game start after a start press;
//! * `start` S (the start button), `coin` Insert (a credit), `cover` Space held (the cabinet's
//!   pedal: behind cover, no shots), `test` Home (the operator menu, at the title), the menu's
//!   keys Page Up / Page Down / Delete / End from the directions.
//!
//! The community library draws the 4:3 picture in the middle of the screen-sized window (black
//! sides, the dump's bezel `WAL_BEZEL` around it). Its picture's place comes from its globals,
//! built for 1920x1080 (1440x1080 at 240,0): its own measure of the monitor, in its
//! `CreateWindowExA`, never runs (a 1600x1200 screen showed the 1080p picture, zoomed and cut).
//! The payload sets them from the screen: its centered 4:3 rectangle. The library's `glViewport`
//! hook maps the game's 640x480 window onto that picture, but the game sets its viewport from
//! its window's client size, the screen's: the payload brings it back to 640x480 first.
//!
//! That `CreateWindowExA` hook also keeps the game's window (0x10019a10), which its other hooks
//! check: its window procedure scales the mouse, its `GetClientRect` answers 640x480, and its
//! videos (the MP4s through MFPlay, the game's MCI opens) are child windows of it. The payload
//! sets it once the window exists: without it the mouse came in window pixels (the game's
//! pointer, a small white square, and its shots far right and below the gun) and no video
//! played (the MCI open left to Wine: no AVI, the attract's opening black).
//!
//! The common crosshair option (`WAL_CROSSHAIR`): the gun's crosshair drawn before the
//! executable's `SwapBuffers` (the community library's hook of it after, its FREEPLAY over it).
//!
//! The operator menu's exit (its last item, or `menu_left`) rebooted the cabinet (0x42b570:
//! `ExitWindowsEx`, made a `return 0` in this executable): the game then went back to the
//! attract scene its menu had unloaded, without loading it again (a crash on its first frame,
//! its objects freed). The payload loads that scene again there, as the game's scene changes.
//!
//! Free play (`WAL_FREEPLAY`): a start press first inserts the credits a game costs (the price
//! `settings\coinsetting.txt` gives, the game object's +0x120, 0x4cc340; its credits +0x150),
//! then a shot begins the game. The price is kept: with enough credits (a price of 0) the title
//! stays on its "press start" loop and never plays its attract, the opening movie
//! (`AVI\openning.mp4`, 0x4167a8).
//!
//! The community library's own free play (a price of 1 forced, then 0 at the first start: no
//! attract movie after a game) is turned off, the payload's in its place. Free play off: its
//! "FREEPLAY" text, which it always draws, hidden.
//!
//! Natives: `trigger`, `cover`, `start`, `coin`, `test`, `menu_up`, `menu_down`, `menu_left`,
//! `menu_right`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::input_filter::{own_mouse, post_key, post_mouse};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log};
use windows_sys::Win32::Foundation::RECT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};

/// `wParam`'s left button flag.
const MK_LBUTTON: usize = 0x0001;

/// The price of a credit (a 16-bit value: the game object's +0x120).
const PRICE: usize = 0x4cc340;
/// The credits inserted (the game object's +0x150).
const CREDITS: usize = 0x4cc370;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    /// A key: its virtual key.
    Key(u16),
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("start", Native::Key(b'S' as u16)),
    ("coin", Native::Key(0x2D)),
    ("cover", Native::Key(0x20)),
    ("test", Native::Key(0x24)),
    ("menu_up", Native::Key(0x21)),
    ("menu_down", Native::Key(0x22)),
    ("menu_left", Native::Key(0x2E)),
    ("menu_right", Native::Key(0x23)),
];

/// The gun's second button and the pedal (keyboard C: b7) take cover.
const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("b2", "cover"),
    ("b7", "cover"),
    ("start", "start"),
    ("coin", "coin"),
    ("test", "test"),
    ("up", "menu_up"),
    ("down", "menu_down"),
    ("left", "menu_left"),
    ("right", "menu_right"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The keys held (bit n: virtual key n), for the game's key state polls.
static HELD: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

pub fn key_held(vk: i32) -> bool {
    (0..256).contains(&vk) && HELD[vk as usize / 64].load(Ordering::Relaxed) & 1 << (vk % 64) != 0
}

fn input_thread() {
    let (mut keys_was, mut trigger_was, mut last) = ([0u64; 4], false, (i32::MIN, i32::MIN));
    let free_play = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY").unwrap_or(false);
    let mut tick = 0u32;
    // free play: the start button held, a start waiting for the credits (since), the start
    // press kept from the game until released
    let (mut start_held, mut start_pending, mut start_eaten) = (false, None::<Instant>, false);
    let mut scene_was = 0usize;
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = gun::find_window();
        if hwnd.is_null() {
            continue;
        }
        set_video_parent(hwnd);
        let mut keys = [0u64; 4];
        let mut trigger = false;
        map().for_each_pressed(&input(0), |n| match n {
            Native::Trigger => trigger = true,
            Native::Key(vk) => keys[vk as usize / 64] |= 1 << (vk % 64),
        });
        // free play: a start press first inserts the credits a game costs (the coin key), then
        // reaches the game once it took them (a start with the coins was lost: the attract's
        // movie stopping)
        let start = b'S' as usize;
        let pressed = keys[start / 64] >> (start % 64) & 1 != 0;
        let (price, credits) = unsafe { ((PRICE as *const u16).read_volatile() as i32, (CREDITS as *const i32).read_volatile()) };
        if free_play && pressed && !start_held && credits < price {
            for _ in credits..price {
                post_key(hwnd, 0x2D, true);
                post_key(hwnd, 0x2D, false);
            }
            log!("pullthetrigger: free play, {} credit(s) inserted", price - credits);
            start_pending = Some(Instant::now());
        }
        start_held = pressed;
        // free play: the credits it inserted left after a game (the game keeps them, cleared on
        // its title after 19000 time units only: no attract until then) cleared back on the
        // attract scene (the game object's first scene, +0x28 its index)
        let scene = unsafe { ((0x4cc220 + 0x28) as *const usize).read_volatile() };
        if free_play && scene == 0 && scene_was != 0 && credits > 0 {
            unsafe { (CREDITS as *mut i32).write_volatile(0) };
            log!("pullthetrigger: free play, {credits} credit(s) left after the game cleared");
        }
        scene_was = scene;
        if let Some(at) = start_pending {
            if credits >= price && at.elapsed() >= Duration::from_millis(300) {
                post_key(hwnd, start as u16, true);
                post_key(hwnd, start as u16, false);
                start_pending = None;
            } else if at.elapsed() >= Duration::from_secs(3) {
                start_pending = None;
            }
        }
        // the start press itself kept from the game until released (sent above)
        if start_pending.is_some() || start_eaten && pressed {
            start_eaten = true;
            keys[start / 64] &= !(1 << (start % 64));
        } else {
            start_eaten = false;
        }
        for vk in 0..256u16 {
            let (now, was) = (keys[vk as usize / 64] >> (vk % 64) & 1 != 0, keys_was[vk as usize / 64] >> (vk % 64) & 1 != 0);
            if now != was {
                post_key(hwnd, vk, now);
            }
        }
        for (h, k) in HELD.iter().zip(keys) {
            h.store(k, Ordering::Relaxed);
        }
        keys_was = keys;
        // the gun: the mouse on the window (kept at its last place off the picture), which the
        // community library's window procedure scales to the game's 640x480 (once it knows the
        // window, set_video_parent: the window pixels before, taken for the game's)
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe { GetClientRect(hwnd, &mut r) };
        let at = match gun::gun_of_player_in_picture(0) {
            Some((x, y)) => ((x * r.right as f32) as i32, (y * r.bottom as f32) as i32),
            None => last,
        };
        let buttons = if trigger { MK_LBUTTON } else { 0 };
        // again every half second: a pointer that did not move still reaches a new screen
        tick = (tick + 1) % 64;
        if (at != last || tick == 0) && at.0 != i32::MIN {
            post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, buttons);
            last = at;
        }
        if trigger != trigger_was && last.0 != i32::MIN {
            post_mouse(hwnd, if trigger { WM_LBUTTONDOWN } else { WM_LBUTTONUP }, last.0, last.1, buttons);
        }
        trigger_was = trigger;
    }
}

/// The operator menu's exit (the cabinet's reboot, made a `return 0`).
const TEST_EXIT: usize = 0x42b570;

/// The operator menu's exit: the current scene (the game object's scenes, a vector at +0x1c /
/// +0x20, the current one's index +0x28) loaded again (its vtable's +0x10, as the game's scene
/// changes call it), the menu's entry having unloaded it.
unsafe extern "thiscall" fn test_exit(game: usize) -> u32 {
    let (begin, end, index) = unsafe { (*((game + 0x1c) as *const usize), *((game + 0x20) as *const usize), *((game + 0x28) as *const usize)) };
    if begin != 0 && index < end.saturating_sub(begin) / 4 {
        let scene = unsafe { *((begin + index * 4) as *const usize) };
        let load: unsafe extern "thiscall" fn(usize) = unsafe { std::mem::transmute(*((*(scene as *const usize) + 0x10) as *const usize)) };
        log!("pullthetrigger: the operator menu left, scene {index} loaded again");
        unsafe { load(scene) };
    }
    0
}

/// A `jmp` from `at` to `to`, when `at` holds `expect` (this executable's build).
fn jump(at: usize, expect: &[u8], to: usize) -> bool {
    let mut code = [0xe9u8, 0, 0, 0, 0];
    code[1..].copy_from_slice(&(to.wrapping_sub(at + 5) as u32).to_le_bytes());
    patch(at, expect, &code)
}

/// Writes `code` at `at`, when `at` holds `expect` (this build's).
fn patch(at: usize, expect: &[u8], code: &[u8]) -> bool {
    let p = at as *mut u8;
    if unsafe { std::slice::from_raw_parts(p, expect.len()) } != expect {
        log!("pullthetrigger: {at:#x} not this build's: not patched");
        return false;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(p.cast(), code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), p, code.len());
        VirtualProtect(p.cast(), code.len(), old, &mut old);
    }
    true
}

/// The community library's free play: on the attract (the game object's first scene) it forces
/// the price to 1 once (RVA 0x2820, this flag then set), then a start or a click with that price
/// makes it 0 (RVA 0x2750, the click eaten): no coin needed, and the price left at 0 (no attract
/// movie after the first game, the operator's price lost).
const PTT_FREE_PLAY: usize = 0x19a1a;
const PTT_FREE_START: usize = 0x2750;

/// The community library's "FREEPLAY" text, drawn on every frame (its `SwapBuffers` hook,
/// RVA 0x1741: `je` past it without its font) whatever the game's price.
const PTT_FREEPLAY_TEXT: usize = 0x1748;

static ORIG_SWAP: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn swap_buffers(dc: *mut std::ffi::c_void) -> i32 {
    let hwnd = unsafe { windows_sys::Win32::Graphics::Gdi::WindowFromDC(dc) };
    if !hwnd.is_null() {
        unsafe { wal_payload_common::crosshair::draw_gl_window(hwnd) };
    }
    let orig: unsafe extern "system" fn(*mut std::ffi::c_void) -> i32 = unsafe { std::mem::transmute(ORIG_SWAP.load(Ordering::Relaxed)) };
    unsafe { orig(dc) }
}

static ORIG_VIEWPORT: AtomicUsize = AtomicUsize::new(0);

/// `wglGetCurrentDC`.
fn current_dc() -> Option<unsafe extern "system" fn() -> windows_sys::Win32::Graphics::Gdi::HDC> {
    static F: OnceLock<usize> = OnceLock::new();
    let f = *F.get_or_init(|| unsafe {
        let module = GetModuleHandleW(windows_sys::w!("opengl32.dll"));
        windows_sys::Win32::System::LibraryLoader::GetProcAddress(module, c"wglGetCurrentDC".as_ptr().cast()).map_or(0, |p| p as usize)
    });
    (f != 0).then(|| unsafe { std::mem::transmute(f) })
}

/// The game's viewport in its 640x480 window's pixels, as the community library's `glViewport`
/// hook (a 640x480 rectangle onto its picture) takes it: the game sets its window's client size
/// (the screen's, the window stretched), which the hook scaled again (1920x1080: 240,0
/// 4320x2430, the picture zoomed, cut at the top and the right).
unsafe extern "system" fn viewport(x: i32, y: i32, w: i32, h: i32) {
    let orig: unsafe extern "system" fn(i32, i32, i32, i32) = unsafe { std::mem::transmute(ORIG_VIEWPORT.load(Ordering::Relaxed)) };
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let hwnd = match current_dc() {
        Some(get_dc) => unsafe { windows_sys::Win32::Graphics::Gdi::WindowFromDC(get_dc()) },
        None => std::ptr::null_mut(),
    };
    // already in the 640x480 window's pixels: the library's GetClientRect answers 640x480 to the
    // game once it knows its window (set_video_parent)
    if w <= 640 && h <= 480 || hwnd.is_null() || unsafe { GetClientRect(hwnd, &mut r) } == 0 || r.right <= 0 || r.bottom <= 0 || (r.right, r.bottom) == (640, 480) {
        return unsafe { orig(x, y, w, h) };
    }
    let (sx, sy) = (|v: i32| v * 640 / r.right, |v: i32| v * 480 / r.bottom);
    unsafe { orig(sx(x), sy(y), sx(w), sy(h)) }
}

/// The community library's picture globals (RVAs): the monitor (left, top, right, bottom), its
/// width and height, the picture's left, width and height; the picture's top apart.
const PTT_MONITOR: usize = 0x19000;
const PTT_PICTURE_TOP: usize = 0x19a1c;

/// The community library's game window (its videos' parent).
const PTT_WINDOW: usize = 0x19a10;

/// The community library's game window, which its `CreateWindowExA` hook keeps (the payload's
/// window hook in its place).
fn set_video_parent(hwnd: windows_sys::Win32::Foundation::HWND) {
    let ptt = unsafe { GetModuleHandleW(windows_sys::w!("PttBootTest.dll")) } as usize;
    if ptt == 0 {
        return;
    }
    let slot = (ptt + PTT_WINDOW) as *mut usize;
    if unsafe { slot.read_volatile() } != hwnd as usize {
        unsafe { slot.write_volatile(hwnd as usize) };
        log!("pullthetrigger: the community library's window {hwnd:?} (its videos' parent)");
    }
}

static ORIG_CREATE: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_POS: AtomicUsize = AtomicUsize::new(0);

/// A child window's rectangle on the game window `parent` inside the Sinden border, which
/// scales the whole window's picture into its frame (glview's border_frame): the community
/// library's videos are child windows, drawn over the border at the picture's place.
fn framed(parent: windows_sys::Win32::Foundation::HWND, x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    if parent.is_null() || unsafe { GetClientRect(parent, &mut r) } == 0 || r.right <= 0 || r.bottom <= 0 {
        return (x, y, w, h);
    }
    let Some(([l, t, rr, b], ..)) = wal_payload_common::border::geometry(r.right as u32, r.bottom as u32) else { return (x, y, w, h) };
    let (sx, sy) = (|v: i32| (v as i64 * (rr - l) as i64 / r.right as i64) as i32, |v: i32| (v as i64 * (b - t) as i64 / r.bottom as i64) as i32);
    (l + sx(x), t + sy(y), sx(w), sy(h))
}

/// The community library's `CreateWindowExW` (its MP4 video's window, a child of the game's).
unsafe extern "system" fn create_window(
    ex: u32, class: *const u16, name: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32,
    parent: windows_sys::Win32::Foundation::HWND, menu: *mut std::ffi::c_void, instance: *mut std::ffi::c_void, param: *const std::ffi::c_void,
) -> windows_sys::Win32::Foundation::HWND {
    const WS_CHILD: u32 = 0x4000_0000;
    let (x, y, w, h) = if style & WS_CHILD != 0 { framed(parent, x, y, w, h) } else { (x, y, w, h) };
    type F = unsafe extern "system" fn(u32, *const u16, *const u16, u32, i32, i32, i32, i32, windows_sys::Win32::Foundation::HWND, *mut std::ffi::c_void, *mut std::ffi::c_void, *const std::ffi::c_void) -> windows_sys::Win32::Foundation::HWND;
    let orig: F = unsafe { std::mem::transmute(ORIG_CREATE.load(Ordering::Relaxed)) };
    unsafe { orig(ex, class, name, style, x, y, w, h, parent, menu, instance, param) }
}

/// The community library's `SetWindowPos` (its MCI video's window, a child of the game's).
unsafe extern "system" fn set_window_pos(hwnd: windows_sys::Win32::Foundation::HWND, after: windows_sys::Win32::Foundation::HWND, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GWL_STYLE, GetParent, GetWindowLongW};
    const WS_CHILD: i32 = 0x4000_0000;
    let (x, y, w, h) = if unsafe { GetWindowLongW(hwnd, GWL_STYLE) } & WS_CHILD != 0 { framed(unsafe { GetParent(hwnd) }, x, y, w, h) } else { (x, y, w, h) };
    let orig: unsafe extern "system" fn(windows_sys::Win32::Foundation::HWND, windows_sys::Win32::Foundation::HWND, i32, i32, i32, i32, u32) -> i32 =
        unsafe { std::mem::transmute(ORIG_SET_POS.load(Ordering::Relaxed)) };
    unsafe { orig(hwnd, after, x, y, w, h, flags) }
}

/// The community library's picture: the screen's centered 4:3 rectangle (all of a narrower one).
fn set_picture(ptt: usize) {
    let (w, h) = wal_payload_common::desktop::screen_size();
    if w <= 0 || h <= 0 {
        return;
    }
    let (pw, ph) = if w * 3 > h * 4 { (h * 4 / 3, h) } else { (w, w * 3 / 4) };
    let (left, top) = ((w - pw) / 2, (h - ph) / 2);
    let globals = [0, 0, w, h, w, h, left, pw, ph];
    unsafe {
        let p = (ptt + PTT_MONITOR) as *mut i32;
        for (i, v) in globals.into_iter().enumerate() {
            p.add(i).write_volatile(v);
        }
        ((ptt + PTT_PICTURE_TOP) as *mut i32).write_volatile(top);
    }
    log!("pullthetrigger: the picture {pw}x{ph} at {left},{top} of the {w}x{h} screen");
}

pub fn init() {
    // the game's message loop: the executable's
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let ptt = unsafe { GetModuleHandleW(windows_sys::w!("PttBootTest.dll")) } as usize;
    if ptt != 0 {
        set_picture(ptt);
        // the library's own free play off (the payload's, WAL_FREEPLAY, instead): its price of
        // 1 forced on the attract skipped (already done), its free start made a "no" (xor al,al;
        // ret)
        unsafe { ((ptt + PTT_FREE_PLAY) as *mut u8).write_volatile(1) };
        if patch(ptt + PTT_FREE_START, &[0xa1, 0x6c, 0x7d, 0x4c, 0x00], &[0x32, 0xc0, 0xc3]) {
            log!("pullthetrigger: the community library's free play off");
        }
        // free play off: no "FREEPLAY" text (the je made a jmp)
        if !wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY").unwrap_or(false)
            && patch(ptt + PTT_FREEPLAY_TEXT, &[0x0f, 0x84, 0x46, 0x01, 0x00, 0x00], &[0xe9, 0x47, 0x01, 0x00, 0x00, 0x90])
        {
            log!("pullthetrigger: free play off, the community library's FREEPLAY text hidden");
        }
        if let Some(orig) = unsafe { wal_payload_common::iat::hook_module(exe, "opengl32.dll", "glViewport", viewport as *const () as usize) } {
            ORIG_VIEWPORT.store(orig, Ordering::Relaxed);
        }
        if wal_payload_common::border::config().is_some() {
            unsafe {
                if let Some(orig) = wal_payload_common::iat::hook_module(ptt, "user32.dll", "CreateWindowExW", create_window as *const () as usize) {
                    ORIG_CREATE.store(orig, Ordering::Relaxed);
                }
                if let Some(orig) = wal_payload_common::iat::hook_module(ptt, "user32.dll", "SetWindowPos", set_window_pos as *const () as usize) {
                    ORIG_SET_POS.store(orig, Ordering::Relaxed);
                }
            }
        }
    }
    // xor eax,eax; ret (then the reboot's dead code)
    if jump(TEST_EXIT, &[0x33, 0xc0, 0xc3, 0x8d, 0x44], test_exit as *const () as usize) {
        log!("pullthetrigger: the operator menu's exit loads the attract again");
    }
    if wal_payload_common::crosshair::init_gl() {
        if let Some(orig) = unsafe { wal_payload_common::iat::hook_module(exe, "gdi32.dll", "SwapBuffers", swap_buffers as *const () as usize) } {
            ORIG_SWAP.store(orig, Ordering::Relaxed);
        }
    }
    wal_payload_common::input_filter::init(exe, key_held);
    own_mouse(exe);
    std::thread::spawn(input_thread);
    log!("pullthetrigger: the mouse and keys played by the gun");
}

