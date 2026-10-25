//! Silent Scope: Bone Eater (game code NDD, 2014, 64-bit), as its Widescreen Edition: the dump's
//! `game/BoneEater.exe` is that edition's runtime (natemac's, GPL-3.0, built on spice2x: source at
//! github.com/natemac/SilentScope-BoneEater-WidescreenEdition), which runs `arkndd.dll` /
//! `gamendd.dll` and draws the cabinet's portrait screen, front LCD and scope on one 16:9
//! picture. It emulates the cabinet's boards itself; the payload is its `dxva2.dll` import
//! (loaded before its `main`, the one function forwarded) and feeds it from the virtual sticks:
//!
//! * Its edition launcher's setup (`Bone Eater WS-Edition.exe`, which refuses the run
//!   directory's links): `../user` holds the NVRAM and raw mounts (a copy of the dump's `conf`,
//!   `avs-config.xml` pointed at it), the gun calibration at the full 0-4095 range, the clock set,
//!   free play from `WAL_KONAMI_FREEPLAY`, the cabinet marker `conf/raw/dx` (widescreen aiming).
//! * Its controls (`../user/bone-eater-controls.xml`, rewritten at each start) are spice2x's
//!   "naive" keys, read with `GetAsyncKeyState`: each native input has a key of its own (F13 to
//!   F23, Q the scope), down while the virtual stick's button is. The real keyboard reads as
//!   released, everywhere (the runtime, arkndd.dll, gamendd.dll).
//! * The gun: with no Gun X / Gun Y binding the runtime takes the cursor over the main window
//!   (`GetCursorPos`): it is player 1's left stick (the gun, or the mouse), across that window.
//!   `SetCursorPos` leaves the real cursor alone.
//! * The scope: the edition turns its key's window messages into the scope's tap (2x) and hold
//!   (10x): the scope button posts its key's `WM_KEYDOWN` / `WM_KEYUP` to the main window, marked
//!   (lParam bit 25); real keyboard and mouse button messages are dropped (`PeekMessageA` /
//!   `GetMessageA`), so only ours reach it.
//! * gamendd.dll's XInput pads read as not connected, its TrueType collections are added face by
//!   face for Wine's GDI+ (`ark.rs`: "No font loaded" assert otherwise).
//!
//! Native inputs for `native_map`: `service test coin start up down left right trigger scope
//! scope_left`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log};
use wal_protocol::Axis;
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClientRect, GetSystemMetrics, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, MSG, PostMessageW,
    SM_CXSCREEN, SM_CYSCREEN, WM_KEYDOWN, WM_KEYFIRST, WM_KEYLAST, WM_KEYUP, WM_LBUTTONDOWN, WM_MOUSELAST, WM_NULL,
};

type P = *mut c_void;

/// The runtime's controls file, from the game directory.
const CONTROLS: &str = "../user/bone-eater-controls.xml";
/// The scope's key: in the controls (Scope Right) and the edition's scope bindings (`-scope-bindings
/// Q`, the profile's arguments).
const VK_SCOPE: u8 = b'Q';
/// lParam bit 25 (reserved): a key message of ours.
const OURS: isize = 1 << 25;

#[derive(Clone, Copy, PartialEq)]
struct Native(u8);

/// Native inputs: the spice2x button name of each, its key.
const NATIVES: &[(&str, Native)] = &[
    ("service", Native(0x7c)),
    ("test", Native(0x7d)),
    ("coin", Native(0x7e)),
    ("start", Native(0x7f)),
    ("up", Native(0x80)),
    ("down", Native(0x81)),
    ("left", Native(0x82)),
    ("right", Native(0x83)),
    ("trigger", Native(0x84)),
    ("scope", Native(VK_SCOPE)),
    ("scope_left", Native(0x86)),
];

const BUTTON_NAMES: &[(&str, &str)] = &[
    ("service", "Service"),
    ("test", "Test"),
    ("coin", "Coin Mech"),
    ("start", "Start"),
    ("up", "Up"),
    ("down", "Down"),
    ("left", "Left"),
    ("right", "Right"),
    ("trigger", "Gun Pressed"),
    ("scope", "Scope Right"),
    ("scope_left", "Scope Left"),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("service", "service"),
    ("test", "test"),
    ("coin", "coin"),
    ("start", "start"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "trigger"),
    ("b2", "scope"),
    ("b3", "scope_left"),
];

/// The keys down, a bit per native input (its index in `NATIVES`).
static HELD: AtomicU32 = AtomicU32::new(0);
/// The main window (the largest of the process's visible windows, not a scope output).
static MAIN_WINDOW: AtomicIsize = AtomicIsize::new(0);

fn exe_dir() -> std::path::PathBuf {
    let mut buf = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let exe = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    exe.parent().map(|p| p.to_path_buf()).unwrap_or_default()
}

/// This copy is the `dxva2.dll` of the Widescreen Edition's runtime (`BoneEater.exe`, beside
/// `modules/gamendd.dll`).
pub fn detected(module: crate::HINSTANCE) -> bool {
    crate::file_name(module).eq_ignore_ascii_case("dxva2.dll") && exe_dir().join("modules").join("gamendd.dll").is_file()
}

pub fn init() {
    wal_payload_common::start("konami");
    prepare_user();
    write_controls();
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    hook_input(exe);
    hook_messages(exe);
    for (name, f, orig) in [
        ("LoadLibraryA", load_library_a as *const () as usize, &ORIG_LOAD_A),
        ("LoadLibraryW", load_library_w as *const () as usize, &ORIG_LOAD_W),
        ("LoadLibraryExW", load_library_ex_w as *const () as usize, &ORIG_LOAD_EX_W),
    ] {
        if let Some(o) = unsafe { iat::hook_module(exe, "kernel32.dll", name, f) } {
            orig.store(o, Ordering::Relaxed);
        }
    }
    modules_loaded();
    std::thread::spawn(input_loop);
    log!("konami: keys, gun and scope of the Widescreen Edition runtime fed (Silent Scope: Bone Eater)");
}

// ---- ../user: the edition launcher's setup ----

fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)?.flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if src.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// The `<mount>` section's first `<device>` value replaced by `path`.
fn redirect_mount(config: &mut String, mount: &str, path: &str) -> bool {
    let Some(start) = config.find(&format!("<{mount}>")) else { return false };
    let Some(end) = config[start..].find(&format!("</{mount}>")).map(|e| start + e) else { return false };
    let Some(open) = config[start..end].find("<device").map(|d| start + d) else { return false };
    let Some(value) = config[open..end].find('>').map(|v| open + v + 1) else { return false };
    let Some(stop) = config[value..end].find('<').map(|s| value + s) else { return false };
    config.replace_range(value..stop, path);
    true
}

/// `../user` as the edition's launcher prepares it (its `player_data.cpp`, `calibration.cpp`).
/// The game directory is the current one.
fn prepare_user() {
    let user = std::path::Path::new("../user");
    if !user.join("avs-config.xml").is_file() {
        let Ok(text) = std::fs::read("prop/avs-config.xml") else {
            log!("ndd: no prop/avs-config.xml");
            return;
        };
        if let Err(e) = copy_tree(std::path::Path::new("conf"), &user.join("conf")) {
            log!("ndd: copying conf to ../user/conf: {e}");
        }
        // Shift-JIS, ASCII tags: edited as Latin-1
        let mut config: String = text.iter().map(|&b| b as char).collect();
        if !redirect_mount(&mut config, "nvram", "../user/conf/nvram") || !redirect_mount(&mut config, "raw", "../user/conf/raw") {
            log!("ndd: avs-config.xml has no nvram / raw mount");
        }
        let bytes: Vec<u8> = config.chars().map(|c| c as u8).collect();
        let _ = std::fs::write(user.join("layout-v1.txt"), "Widescreen Edition private nvram/raw mounts v1.\n");
        match std::fs::write(user.join("avs-config.xml"), bytes) {
            Ok(()) => log!("ndd: ../user prepared (NVRAM and raw mounts)"),
            Err(e) => log!("ndd: ../user/avs-config.xml: {e}"),
        }
    }
    let _ = std::fs::create_dir_all(user.join("conf/nvram"));
    let _ = std::fs::create_dir_all(user.join("conf/raw/bookkeeping"));
    // the cabinet marker ARK opens (its contents unused): the widescreen aiming
    let _ = std::fs::OpenOptions::new().create(true).append(true).open(user.join("conf/raw/dx"));
    const XML: &str = "../user/conf/nvram/testmode-v.xml";
    const CRC: &str = "../user/conf/nvram/testmode-v.crc";
    if !std::path::Path::new(XML).is_file() {
        // the dump's template (a cabinet that never ran)
        if let Ok(template) = std::fs::read("prop/testmode-v.xml") {
            let _ = std::fs::write(XML, &template);
            let _ = std::fs::write(CRC, crate::ark::crc32(&template).to_le_bytes());
        }
    }
    let changed = crate::ark::edit_settings(XML, CRC, |xml| {
        let Some(at) = xml.find("<guncontrolleCheck>") else { return };
        let mut rest = xml[at..].to_string();
        crate::ark::set_current(&mut rest, "calibrated", 1);
        for side in ["left", "right"] {
            for (field, value) in [("Min", 0), ("Max", 4095), ("Nut", 2047)] {
                for axis in ["X", "Y"] {
                    crate::ark::set_current(&mut rest, &format!("{side}Calibration{field}{axis}"), value);
                }
            }
        }
        xml.replace_range(at.., &rest);
    });
    if changed {
        log!("ndd: test menu settings set (gun calibration full range, free play {:?})", std::env::var("WAL_KONAMI_FREEPLAY").ok());
    }
}

/// The controls: each native input's key; Gun X / Gun Y unbound (the cursor).
fn write_controls() {
    let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<games>\n  <game name=\"Silent Scope: Bone Eater\">\n    <buttons>\n");
    for (native, label) in BUTTON_NAMES {
        let Some((_, Native(vk))) = NATIVES.iter().find(|(n, _)| n == native) else { continue };
        xml += &format!("      <button name=\"{label}\" vkey=\"{vk}\" analogtype=\"0\" devid=\"\"/>\n");
    }
    xml += "    </buttons>\n    <analogs>\n      <analog name=\"Gun X\" devid=\"\"/>\n      <analog name=\"Gun Y\" devid=\"\"/>\n    </analogs>\n    <lights/>\n    <options/>\n  </game>\n</games>\n";
    if std::fs::read_to_string(CONTROLS).ok().as_deref() != Some(xml.as_str())
        && let Err(e) = std::fs::write(CONTROLS, xml)
    {
        log!("ndd: {CONTROLS}: {e}");
    }
}

// ---- the keys, the gun ----

fn held(vk: i32) -> bool {
    let mask = HELD.load(Ordering::Relaxed);
    NATIVES.iter().enumerate().any(|(i, (_, Native(k)))| *k as i32 == vk && mask & (1 << i) != 0)
}

unsafe extern "system" fn get_async_key_state(vk: i32) -> i16 {
    if held(vk) { i16::MIN } else { 0 }
}

unsafe extern "system" fn get_key_state(vk: i32) -> i16 {
    if held(vk) { i16::MIN } else { 0 }
}

unsafe extern "system" fn get_keyboard_state(keys: *mut u8) -> i32 {
    if keys.is_null() {
        return 0;
    }
    for vk in 0..256 {
        unsafe { *keys.add(vk) = if held(vk as i32) { 0x80 } else { 0 } };
    }
    1
}

/// The gun's aim (player 1's left stick) over the main window, in screen coordinates.
unsafe extern "system" fn get_cursor_pos(point: *mut POINT) -> i32 {
    if point.is_null() {
        return 0;
    }
    let window = MAIN_WINDOW.load(Ordering::Relaxed) as HWND;
    let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let mut origin = POINT { x: 0, y: 0 };
    if window.is_null() || unsafe { GetClientRect(window, &mut client) } == 0 || unsafe { ClientToScreen(window, &mut origin) } == 0 || client.right < 2 {
        client.right = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        client.bottom = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        origin = POINT { x: 0, y: 0 };
    }
    let stick = wal_payload_common::input(0);
    let fraction = |a: Axis| (stick.axis(a) as f32 + 32768.0) / 65535.0;
    unsafe {
        (*point).x = origin.x + (fraction(Axis::LeftX) * (client.right - 1) as f32).round() as i32;
        (*point).y = origin.y + (fraction(Axis::LeftY) * (client.bottom - 1) as f32).round() as i32;
    }
    1
}

unsafe extern "system" fn set_cursor_pos(_x: i32, _y: i32) -> i32 {
    1
}

/// ERROR_DEVICE_NOT_CONNECTED: no XInput pad.
unsafe extern "system" fn xinput_none(_user: u32, _state: P) -> u32 {
    1167
}

unsafe extern "system" fn xinput_caps_none(_user: u32, _flags: u32, _caps: P) -> u32 {
    1167
}

fn hook_input(module: usize) {
    for (dll, name, f) in [
        ("user32.dll", "GetAsyncKeyState", get_async_key_state as *const () as usize),
        ("user32.dll", "GetKeyState", get_key_state as *const () as usize),
        ("user32.dll", "GetKeyboardState", get_keyboard_state as *const () as usize),
        ("user32.dll", "GetCursorPos", get_cursor_pos as *const () as usize),
        ("user32.dll", "SetCursorPos", set_cursor_pos as *const () as usize),
        ("xinput1_3.dll", "XInputGetState", xinput_none as *const () as usize),
        ("xinput1_3.dll", "XInputGetCapabilities", xinput_caps_none as *const () as usize),
    ] {
        unsafe { iat::hook_module(module, dll, name, f) };
    }
}

// ---- window messages ----

type PeekMessage = unsafe extern "system" fn(*mut MSG, HWND, u32, u32, u32) -> i32;
type GetMessage = unsafe extern "system" fn(*mut MSG, HWND, u32, u32) -> i32;
static ORIG_PEEK_EXE: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_EXE: AtomicUsize = AtomicUsize::new(0);
static ORIG_PEEK_GAME: AtomicUsize = AtomicUsize::new(0);

/// A real keyboard or mouse button message becomes `WM_NULL`; our key messages pass.
fn filter(msg: *mut MSG) {
    if msg.is_null() {
        return;
    }
    let m = unsafe { &mut *msg };
    let key = (WM_KEYFIRST..=WM_KEYLAST).contains(&m.message);
    let button = (WM_LBUTTONDOWN..=WM_MOUSELAST).contains(&m.message);
    if (key && m.lParam & OURS == 0) || button {
        m.message = WM_NULL;
    }
}

unsafe extern "system" fn peek_message_exe(msg: *mut MSG, hwnd: HWND, min: u32, max: u32, remove: u32) -> i32 {
    let original: PeekMessage = unsafe { std::mem::transmute(ORIG_PEEK_EXE.load(Ordering::Relaxed)) };
    let r = unsafe { original(msg, hwnd, min, max, remove) };
    if r != 0 {
        filter(msg);
    }
    r
}

unsafe extern "system" fn peek_message_game(msg: *mut MSG, hwnd: HWND, min: u32, max: u32, remove: u32) -> i32 {
    let original: PeekMessage = unsafe { std::mem::transmute(ORIG_PEEK_GAME.load(Ordering::Relaxed)) };
    let r = unsafe { original(msg, hwnd, min, max, remove) };
    if r != 0 {
        filter(msg);
    }
    r
}

unsafe extern "system" fn get_message_exe(msg: *mut MSG, hwnd: HWND, min: u32, max: u32) -> i32 {
    let original: GetMessage = unsafe { std::mem::transmute(ORIG_GET_EXE.load(Ordering::Relaxed)) };
    let r = unsafe { original(msg, hwnd, min, max) };
    if r > 0 {
        filter(msg);
    }
    r
}

fn hook_messages(exe: usize) {
    if let Some(o) = unsafe { iat::hook_module(exe, "user32.dll", "PeekMessageA", peek_message_exe as *const () as usize) } {
        ORIG_PEEK_EXE.store(o, Ordering::Relaxed);
    }
    if let Some(o) = unsafe { iat::hook_module(exe, "user32.dll", "GetMessageA", get_message_exe as *const () as usize) } {
        ORIG_GET_EXE.store(o, Ordering::Relaxed);
    }
}

// ---- the game's modules, as they load ----

type LoadLibraryA = unsafe extern "system" fn(*const u8) -> usize;
type LoadLibraryW = unsafe extern "system" fn(*const u16) -> usize;
type LoadLibraryExW = unsafe extern "system" fn(*const u16, P, u32) -> usize;
static ORIG_LOAD_A: AtomicUsize = AtomicUsize::new(0);
static ORIG_LOAD_W: AtomicUsize = AtomicUsize::new(0);
static ORIG_LOAD_EX_W: AtomicUsize = AtomicUsize::new(0);
static ARK_HOOKED: AtomicBool = AtomicBool::new(false);
static GAME_HOOKED: AtomicBool = AtomicBool::new(false);

fn module(name: &str) -> usize {
    let name: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    unsafe { GetModuleHandleW(name.as_ptr()) as usize }
}

/// arkndd.dll and gamendd.dll hooked once loaded.
fn modules_loaded() {
    let ark = module("arkndd.dll");
    if ark != 0 && !ARK_HOOKED.swap(true, Ordering::Relaxed) {
        hook_input(ark);
        // it may load gamendd.dll itself
        for (name, f) in [
            ("LoadLibraryA", load_library_a as *const () as usize),
            ("LoadLibraryW", load_library_w as *const () as usize),
            ("LoadLibraryExW", load_library_ex_w as *const () as usize),
        ] {
            unsafe { iat::hook_module(ark, "kernel32.dll", name, f) };
        }
    }
    let game = module("gamendd.dll");
    if game != 0 && !GAME_HOOKED.swap(true, Ordering::Relaxed) {
        hook_input(game);
        if let Some(o) = unsafe { iat::hook_module(game, "user32.dll", "PeekMessageA", peek_message_game as *const () as usize) } {
            ORIG_PEEK_GAME.store(o, Ordering::Relaxed);
        }
        crate::ark::split_font_collections(game);
        log!("ndd: gamendd.dll hooked");
    }
}

unsafe extern "system" fn load_library_a(name: *const u8) -> usize {
    let original: LoadLibraryA = unsafe { std::mem::transmute(ORIG_LOAD_A.load(Ordering::Relaxed)) };
    let base = unsafe { original(name) };
    modules_loaded();
    base
}

unsafe extern "system" fn load_library_w(name: *const u16) -> usize {
    let original: LoadLibraryW = unsafe { std::mem::transmute(ORIG_LOAD_W.load(Ordering::Relaxed)) };
    let base = unsafe { original(name) };
    modules_loaded();
    base
}

unsafe extern "system" fn load_library_ex_w(name: *const u16, file: P, flags: u32) -> usize {
    let original: LoadLibraryExW = unsafe { std::mem::transmute(ORIG_LOAD_EX_W.load(Ordering::Relaxed)) };
    let base = unsafe { original(name, file, flags) };
    modules_loaded();
    base
}

// ---- the input thread: keys held, the scope's messages, the main window ----

struct Largest {
    window: HWND,
    area: i64,
}

unsafe extern "system" fn largest_window(window: HWND, data: LPARAM) -> i32 {
    let found = unsafe { &mut *(data as *mut Largest) };
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, &mut pid) };
    if pid != unsafe { GetCurrentProcessId() } || unsafe { IsWindowVisible(window) } == 0 {
        return 1;
    }
    let mut title = [0u16; 64];
    let n = unsafe { GetWindowTextW(window, title.as_mut_ptr(), title.len() as i32) } as usize;
    // the scope outputs ("Aska MultiDisplay[0]...")
    if String::from_utf16_lossy(&title[..n]).contains("MultiDisplay") {
        return 1;
    }
    let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetClientRect(window, &mut client) };
    let area = (client.right - client.left) as i64 * (client.bottom - client.top) as i64;
    if area > found.area {
        *found = Largest { window, area };
    }
    1
}

fn input_loop() {
    let map = ButtonMap::new(NATIVES, DEFAULT_MAP);
    let scope_bit = NATIVES.iter().position(|(n, _)| *n == "scope").unwrap_or(0);
    let mut scope_down = false;
    let mut tick = 0u32;
    loop {
        if tick % 250 == 0 {
            let mut found = Largest { window: std::ptr::null_mut(), area: 0 };
            unsafe { EnumWindows(Some(largest_window), &mut found as *mut Largest as LPARAM) };
            MAIN_WINDOW.store(found.window as isize, Ordering::Relaxed);
        }
        tick = tick.wrapping_add(1);
        let stick = wal_payload_common::input(0);
        let mut mask = 0u32;
        map.for_each_pressed(&stick, |n| {
            if let Some(i) = NATIVES.iter().position(|(_, m)| *m == n) {
                mask |= 1 << i;
            }
        });
        HELD.store(mask, Ordering::Relaxed);
        let scope = mask & (1 << scope_bit) != 0;
        let window = MAIN_WINDOW.load(Ordering::Relaxed) as HWND;
        if scope != scope_down && !window.is_null() {
            scope_down = scope;
            let (message, lparam) = if scope { (WM_KEYDOWN, 1 | OURS) } else { (WM_KEYUP, 1 | OURS | 0xc000_0000) };
            unsafe { PostMessageW(window, message, VK_SCOPE as usize, lparam) };
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

// ---- dxva2.dll ----

static DXVA2_CREATE: AtomicUsize = AtomicUsize::new(0);

/// The runtime's one dxva2 import, forwarded to the system's dxva2.dll.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DXVA2CreateDirect3DDeviceManager9(token: *mut u32, manager: *mut P) -> i32 {
    let mut f = DXVA2_CREATE.load(Ordering::Relaxed);
    if f == 0 {
        let mut dir = [0u16; 260];
        let n = unsafe { GetSystemDirectoryW(dir.as_mut_ptr(), dir.len() as u32) } as usize;
        let path: Vec<u16> = dir[..n].iter().copied().chain("\\dxva2.dll".encode_utf16()).chain([0]).collect();
        let module = unsafe { LoadLibraryW(path.as_ptr()) };
        f = unsafe { GetProcAddress(module, c"DXVA2CreateDirect3DDeviceManager9".as_ptr() as *const u8) }.map_or(0, |f| f as usize);
        DXVA2_CREATE.store(f, Ordering::Relaxed);
    }
    if f == 0 {
        return 0x8000_4005u32 as i32; // E_FAIL
    }
    let real: unsafe extern "system" fn(*mut u32, *mut P) -> i32 = unsafe { std::mem::transmute(f) };
    unsafe { real(token, manager) }
}
