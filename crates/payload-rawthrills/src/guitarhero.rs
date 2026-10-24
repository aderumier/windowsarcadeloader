//! Guitar Hero Arcade EDITION (GHA 1.0.5): the game does not expose its cabinet controls
//! through its normal XInput polling. Emulate two connected controllers, disable the hardware
//! state writes, and feed its internal controller state from the launcher's two virtual sticks.

use crate::common::{Site, matches, write};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use wal_payload_common::log;
use wal_protocol::{Axis, StickState, button};
use windows_sys::Win32::Foundation::{ERROR_BAD_ARGUMENTS, ERROR_DEVICE_NOT_CONNECTED, ERROR_INVALID_PARAMETER, HINSTANCE, HWND, LPARAM, LRESULT, TRUE, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleA;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegSetValueExW};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetCapture, ReleaseCapture, SetCapture, VK_RBUTTON};
use windows_sys::Win32::UI::Input::XboxController::{XINPUT_CAPABILITIES, XINPUT_CAPS_VOICE_SUPPORTED, XINPUT_DEVTYPE_GAMEPAD, XINPUT_DEVSUBTYPE_GAMEPAD, XINPUT_GAMEPAD, XINPUT_STATE, XINPUT_VIBRATION};
use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowA, GetForegroundWindow, GetSystemMetrics, GetWindowRect, ShowWindow, SM_CXSCREEN, SM_CYSCREEN, SWP_NOZORDER, SWP_NOSIZE, SW_MINIMIZE, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};
use windows_sys::core::GUID;

// GHA's XInput writes, also used by OpenParrot's GHA implementation. These are checked so a
// different executable build is left intact.
const XINPUT_WRITES: [Site; 6] = [
    (0x53215c, &[0xc6, 0x04, 0x08, 0xff]),
    (0x532168, &[0xc6, 0x04, 0x0a, 0x00]),
    (0x5089f0, &[0x89, 0x4e, 0x40]),
    (0x508a01, &[0x89, 0x46, 0x40]),
    (0x508a07, &[0x89, 0x46, 0x30]),
    (0x508aee, &[0x88, 0x46, 0x22]),
];

const INPUT_POINTER: usize = 0x746ab0;
const GAME_ACTIVE: usize = 0x857ae0;
const INPUT_READY: usize = 0x13b3dc4;
const POLL: std::time::Duration = std::time::Duration::from_millis(16);
const IID_GAME_EXPLORER: GUID = GUID::from_u128(0xe7b2fb72_d728_49b3_a5f2_18ebf5f1349e);

static CO_CREATE_INSTANCE: AtomicUsize = AtomicUsize::new(0);
static PATCHED_GAME_EXPLORER_VTABLE: AtomicUsize = AtomicUsize::new(0);
static REG_OPEN_KEY_EX_W: AtomicUsize = AtomicUsize::new(0);
static REG_CREATE_KEY_EX_W: AtomicUsize = AtomicUsize::new(0);
static SET_WINDOW_POS: AtomicUsize = AtomicUsize::new(0);
static SET_WINDOW_TEXT_W: AtomicUsize = AtomicUsize::new(0);
static CREATE_WINDOW_EX_A: AtomicUsize = AtomicUsize::new(0);
static DEF_WINDOW_PROC_A: AtomicUsize = AtomicUsize::new(0);
static CLICK_X: AtomicUsize = AtomicUsize::new(0);
static CLICK_Y: AtomicUsize = AtomicUsize::new(0);
static XINPUT_PACKETS: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];
static XINPUT_DETECTED: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy)]
struct Control {
    input: u32,
    state_offset: usize,
    state_delta: u8,
    menu_offset: Option<usize>,
    menu_value: u8,
    start: bool,
    strum: bool,
}

const P1: [Control; 8] = [
    Control { input: button::START, state_offset: 0x85, state_delta: 0x08, menu_offset: None, menu_value: 0, start: true, strum: false },
    Control { input: button::UP, state_offset: 0x75, state_delta: 0x10, menu_offset: Some(0x84), menu_value: 0x01, start: false, strum: true },
    Control { input: button::DOWN, state_offset: 0x75, state_delta: 0x40, menu_offset: Some(0x84), menu_value: 0x01, start: false, strum: true },
    Control { input: button::B1, state_offset: 0x52, state_delta: 0xff, menu_offset: Some(0x84), menu_value: 0x40, start: false, strum: false },
    Control { input: button::B2, state_offset: 0x50, state_delta: 0xff, menu_offset: Some(0x85), menu_value: 0x01, start: false, strum: false },
    Control { input: button::B3, state_offset: 0x51, state_delta: 0xff, menu_offset: None, menu_value: 0, start: false, strum: false },
    Control { input: button::B4, state_offset: 0x53, state_delta: 0xff, menu_offset: None, menu_value: 0, start: false, strum: false },
    Control { input: button::B5, state_offset: 0x4e, state_delta: 0xff, menu_offset: None, menu_value: 0, start: false, strum: false },
];

const P2: [Control; 8] = [
    Control { input: button::START, state_offset: 0x121, state_delta: 0x08, menu_offset: None, menu_value: 0, start: true, strum: false },
    Control { input: button::UP, state_offset: 0x111, state_delta: 0x10, menu_offset: Some(0x120), menu_value: 0x01, start: false, strum: true },
    Control { input: button::DOWN, state_offset: 0x111, state_delta: 0x40, menu_offset: Some(0x120), menu_value: 0x01, start: false, strum: true },
    Control { input: button::B1, state_offset: 0xee, state_delta: 0xff, menu_offset: Some(0x120), menu_value: 0x40, start: false, strum: false },
    Control { input: button::B2, state_offset: 0xec, state_delta: 0xff, menu_offset: Some(0x121), menu_value: 0x01, start: false, strum: false },
    Control { input: button::B3, state_offset: 0xed, state_delta: 0xff, menu_offset: None, menu_value: 0, start: false, strum: false },
    Control { input: button::B4, state_offset: 0xef, state_delta: 0xff, menu_offset: None, menu_value: 0, start: false, strum: false },
    Control { input: button::B5, state_offset: 0xea, state_delta: 0xff, menu_offset: None, menu_value: 0, start: false, strum: false },
];

pub(crate) fn init(name: &str) -> bool {
    if !name.eq_ignore_ascii_case("guitarhero") {
        return false;
    }
    if let Some((site, _)) = XINPUT_WRITES.iter().find(|s| !matches(**s)) {
        log!("guitarhero: unexpected XInput code at {site:#x}, not this build, left alone");
        return true;
    }
    for (site, expected) in XINPUT_WRITES {
        write(site, &vec![0x90; expected.len()]);
    }
    hook_xinput();
    hook_game_explorer();
    hook_awl_registry();
    force_english();
    configure_game();
    hook_windowing();

    let base = unsafe {
        windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as usize
    };
    std::thread::spawn(move || run(base));
    log!("guitarhero: physical XInput writes disabled; two virtual guitars, Aspyr registry and config initialized; Game Explorer bypassed");
    true
}

fn hook_xinput() {
    // GHA.cpp's memory writes depend on the global XInputEmu.cpp hooks: without
    // connected controllers, GHA skips its controller update and ignores the bytes.
    // This executable imports xinput1_3 by ordinal, rather than by function name.
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as usize };
    for (ordinal, name, replacement) in [
        (1, "XInputEnable", xinput_enable as *const () as usize),
        (2, "XInputGetState", xinput_get_state as *const () as usize),
        (3, "XInputSetState", xinput_set_state as *const () as usize),
        (4, "XInputGetCapabilities", xinput_get_capabilities as *const () as usize),
        (5, "XInputGetDSoundAudioDeviceGuids", xinput_get_audio_guids as *const () as usize),
    ] {
        let hook = unsafe { wal_payload_common::iat::hook_ordinal(base, "xinput1_3.dll", ordinal, replacement) }
            .or_else(|| unsafe { wal_payload_common::iat::hook("xinput1_3.dll", name, replacement) });
        if hook.is_none() && matches!(ordinal, 2 | 4) {
            log!("guitarhero: required {name} import unavailable");
        }
    }
}

unsafe extern "system" fn xinput_enable(_enabled: i32) {}

unsafe extern "system" fn xinput_get_state(index: u32, state: *mut XINPUT_STATE) -> u32 {
    let Some(packet) = XINPUT_PACKETS.get(index as usize) else { return ERROR_DEVICE_NOT_CONNECTED };
    if state.is_null() { return ERROR_INVALID_PARAMETER; }
    // Buttons are injected into the game's state after its hardware writes have
    // been disabled. XInput only supplies the connection and a neutral sample.
    unsafe { state.write(XINPUT_STATE {
        dwPacketNumber: packet.fetch_add(1, Ordering::Relaxed).wrapping_add(1),
        Gamepad: XINPUT_GAMEPAD::default(),
    }) };
    if XINPUT_DETECTED.fetch_or(1 << index, Ordering::Relaxed) & (1 << index) == 0 {
        log!("guitarhero: game polling connected virtual guitar P{}", index + 1);
    }
    0
}

unsafe extern "system" fn xinput_set_state(index: u32, _vibration: *const XINPUT_VIBRATION) -> u32 {
    if index < 2 { 0 } else { ERROR_DEVICE_NOT_CONNECTED }
}

unsafe extern "system" fn xinput_get_capabilities(index: u32, flags: u32, caps: *mut XINPUT_CAPABILITIES) -> u32 {
    if flags > 1 { return ERROR_BAD_ARGUMENTS; }
    if index >= 2 { return ERROR_DEVICE_NOT_CONNECTED; }
    if caps.is_null() { return ERROR_INVALID_PARAMETER; }
    unsafe { caps.write(XINPUT_CAPABILITIES {
        Type: XINPUT_DEVTYPE_GAMEPAD,
        SubType: XINPUT_DEVSUBTYPE_GAMEPAD,
        Flags: XINPUT_CAPS_VOICE_SUPPORTED,
        Gamepad: XINPUT_GAMEPAD {
            wButtons: 0xf3ff,
            bLeftTrigger: 0xff,
            bRightTrigger: 0xff,
            sThumbLX: -64,
            sThumbLY: -64,
            sThumbRX: -64,
            sThumbRY: -64,
        },
        Vibration: XINPUT_VIBRATION { wLeftMotorSpeed: 0xff, wRightMotorSpeed: 0xff },
    }) };
    0
}

unsafe extern "system" fn xinput_get_audio_guids(index: u32, render: *mut GUID, capture: *mut GUID) -> u32 {
    if index >= 2 { return ERROR_DEVICE_NOT_CONNECTED; }
    if render.is_null() || capture.is_null() { return ERROR_INVALID_PARAMETER; }
    unsafe { render.write(GUID::from_u128(0)); capture.write(GUID::from_u128(0)); }
    0
}

const ASPYR_KEY: &[u16] = &[
    83, 79, 70, 84, 87, 65, 82, 69, 92, 65, 115, 112, 121, 114, 92, 71, 117, 105, 116, 97, 114, 32,
    72, 101, 114, 111, 32, 73, 73, 73, 0,
];

fn is_aspyr_key(key: *const u16) -> bool {
    if key.is_null() { return false; }
    let expected = &ASPYR_KEY[..ASPYR_KEY.len() - 1];
    for (index, wanted) in expected.iter().copied().enumerate() {
        let got = unsafe { key.add(index).read() };
        if (got as u8).to_ascii_lowercase() != (wanted as u8).to_ascii_lowercase() { return false; }
    }
    unsafe { key.add(expected.len()).read() == 0 }
}

unsafe extern "system" fn reg_open_key_ex_w(key: HKEY, subkey: *const u16, _options: u32, _access: u32, result: *mut HKEY) -> u32 {
    if is_aspyr_key(subkey) {
        if result.is_null() { return ERROR_INVALID_PARAMETER; }
        let mut disposition = 0u32;
        return unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, ASPYR_KEY.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_QUERY_VALUE | KEY_SET_VALUE, std::ptr::null(), result, &mut disposition) };
    }
    let original = REG_OPEN_KEY_EX_W.load(Ordering::Relaxed);
    if original == 0 { return ERROR_INVALID_PARAMETER; }
    let call: unsafe extern "system" fn(HKEY, *const u16, u32, u32, *mut HKEY) -> u32 = unsafe { std::mem::transmute(original) };
    unsafe { call(key, subkey, _options, _access, result) }
}

unsafe extern "system" fn reg_create_key_ex_w(key: HKEY, subkey: *const u16, reserved: u32, class: *const u16, options: u32, access: u32, security: *const std::ffi::c_void, result: *mut HKEY, disposition: *mut u32) -> u32 {
    let root = if key == HKEY_LOCAL_MACHINE && is_aspyr_key(subkey) { HKEY_CURRENT_USER } else { key };
    let original = REG_CREATE_KEY_EX_W.load(Ordering::Relaxed);
    if original == 0 { return ERROR_INVALID_PARAMETER; }
    let call: unsafe extern "system" fn(HKEY, *const u16, u32, *const u16, u32, u32, *const std::ffi::c_void, *mut HKEY, *mut u32) -> u32 = unsafe { std::mem::transmute(original) };
    unsafe { call(root, subkey, reserved, class, options, access, security, result, disposition) }
}

fn hook_awl_registry() {
    let awl = unsafe { GetModuleHandleA(c"AWL.dll".as_ptr().cast()) };
    if !awl.is_null() && hook_awl_registry_module(awl as usize) { return; }
    std::thread::spawn(|| {
        for _ in 0..10_000 {
            let awl = unsafe { GetModuleHandleA(c"AWL.dll".as_ptr().cast()) };
            if !awl.is_null() && hook_awl_registry_module(awl as usize) { return; }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        log!("guitarhero: AWL registry imports unavailable after 10 seconds");
    });
}

fn hook_awl_registry_module(awl: usize) -> bool {
    let open = unsafe { wal_payload_common::iat::hook_module(awl, "advapi32.dll", "RegOpenKeyExW", reg_open_key_ex_w as *const () as usize) };
    let create = unsafe { wal_payload_common::iat::hook_module(awl, "advapi32.dll", "RegCreateKeyExW", reg_create_key_ex_w as *const () as usize) };
    if let Some(original) = open { let _ = REG_OPEN_KEY_EX_W.compare_exchange(0, original, Ordering::Relaxed, Ordering::Relaxed); }
    if let Some(original) = create { let _ = REG_CREATE_KEY_EX_W.compare_exchange(0, original, Ordering::Relaxed, Ordering::Relaxed); }
    if open.is_some() || create.is_some() { log!("guitarhero: AWL Aspyr HKLM key redirected to per-user registry"); }
    open.is_some() && create.is_some()
}

/// AWL reads the game language from the Aspyr key's `Language` value, else from the Windows UI
/// language: on a French system the game then loads `pak/qb_f.pak.xen`, which the arcade data
/// lacks (English only), and crashes on the missing pak.
fn force_english() {
    let mut key: HKEY = std::ptr::null_mut();
    let mut disposition = 0u32;
    if unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, ASPYR_KEY.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, std::ptr::null(), &mut key, &mut disposition) } != 0 {
        log!("guitarhero: cannot open the Aspyr key to set the language");
        return;
    }
    let name: Vec<u16> = "Language\0".encode_utf16().collect();
    let value: Vec<u16> = "en\0".encode_utf16().collect();
    let result = unsafe { RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, value.as_ptr().cast(), (value.len() * 2) as u32) };
    unsafe { RegCloseKey(key) };
    if result == 0 { log!("guitarhero: language forced to en"); } else { log!("guitarhero: cannot set the language ({result})"); }
}

fn configure_game() {
    let windowed = env_bool("WAL_RAWTHRILLS_GHA_WINDOWED", false);
    let hd = env_bool("WAL_RAWTHRILLS_GHA_HD_PATCH", true);
    let width = if windowed || !hd { 1360 } else { unsafe { GetSystemMetrics(SM_CXSCREEN).max(640) as u32 } };
    let height = if windowed || !hd { 768 } else { unsafe { GetSystemMetrics(SM_CYSCREEN).max(480) as u32 } };
    let xml = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<r>\n\t<s id=\"Video.Width\">{width}</s>\n\t<s id=\"Video.Height\">{height}</s> \n\t<s id=\"Options.GraphicsQuality\">0</s>\n\t<s id=\"Options.Crowd\">1</s>\n\t<s id=\"Options.Physics\">1</s>\n\t<s id=\"Options.Flares\">1</s>\n\t<s id=\"Options.FrontRowCamera\">0</s>\n\t<s id=\"6f1d2b61d5a011cfbfc7444553540000\">328 221 340 343 267 264 999 219 235 331 304 999 310</s>\n</r>\n");
    let cwd = std::env::current_dir();
    if let Ok(cwd) = cwd {
        let config = cwd.join("AspyrConfig.xml");
        if std::fs::write(&config, xml.as_bytes()).is_ok() {
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                let target = std::path::PathBuf::from(local).join("Aspyr").join("Guitar Hero III").join("AspyrConfig.xml");
                if let Some(parent) = target.parent() { let _ = std::fs::create_dir_all(parent); }
                if let Ok(mut permissions) = std::fs::metadata(&target).map(|m| m.permissions()) {
                    permissions.set_readonly(false);
                    let _ = std::fs::set_permissions(&target, permissions);
                }
                let _ = std::fs::copy(&config, &target);
                let mut permissions = std::fs::metadata(&target).ok().map(|m| m.permissions());
                if let Some(ref mut p) = permissions { p.set_readonly(true); let _ = std::fs::set_permissions(&target, p.clone()); }
            }
            log!("guitarhero: Aspyr config written at {width}x{height}");
        } else { log!("guitarhero: cannot write AspyrConfig.xml"); }
        let pak = cwd.join("DATA").join("PAK");
        let unlock = env_bool("WAL_RAWTHRILLS_GHA_UNLOCK_MAIN_MENU", true);
        let variant = if unlock { "MENU" } else { "ATTRACT" };
        for extension in ["pab", "pak"] {
            let target = pak.join(format!("qb.{extension}.xen"));
            let v4 = pak.join(format!("qb.{extension}.{variant}v4.xen"));
            let regular = pak.join(format!("qb.{extension}.{variant}.xen"));
            let source = if v4.exists() { v4 } else { regular };
            if source.exists() && target.exists() && std::fs::copy(&source, &target).is_ok() {
                log!("guitarhero: main-menu data set to {variant}");
            }
        }
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    std::env::var(name).map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "on")).unwrap_or(default)
}

fn hook_windowing() {
    if env_bool("WAL_RAWTHRILLS_GHA_WINDOWED", false) {
        let create = unsafe { wal_payload_common::iat::hook("user32.dll", "CreateWindowExA", create_window_ex_a as *const () as usize) };
        let proc = unsafe { wal_payload_common::iat::hook("user32.dll", "DefWindowProcA", def_window_proc_a as *const () as usize) };
        if let Some(o) = create { CREATE_WINDOW_EX_A.store(o, Ordering::Relaxed); }
        if let Some(o) = proc { DEF_WINDOW_PROC_A.store(o, Ordering::Relaxed); }
        std::thread::spawn(windowed_right_click_loop);
    }
    if let Some(o) = unsafe { wal_payload_common::iat::hook("user32.dll", "SetWindowPos", set_window_pos as *const () as usize) } { SET_WINDOW_POS.store(o, Ordering::Relaxed); }
    if let Some(o) = unsafe { wal_payload_common::iat::hook("user32.dll", "SetWindowTextW", set_window_text_w as *const () as usize) } { SET_WINDOW_TEXT_W.store(o, Ordering::Relaxed); }
    let _ = unsafe { wal_payload_common::iat::hook("user32.dll", "SetCursorPos", set_cursor_pos as *const () as usize) };
}

fn windowed_right_click_loop() {
    loop {
        if unsafe { GetAsyncKeyState(VK_RBUTTON as i32) } < 0 {
            let game = unsafe { FindWindowA(std::ptr::null(), c"Guitar Hero Arcade".as_ptr().cast()) };
            if !game.is_null() && unsafe { GetForegroundWindow() == game } {
                let original = SET_WINDOW_POS.load(Ordering::Relaxed);
                if original != 0 {
                    let set: unsafe extern "system" fn(HWND, HWND, i32, i32, i32, i32, u32) -> i32 = unsafe { std::mem::transmute(original) };
                    let _ = unsafe { set(game, 1usize as HWND, 0, 0, 1360, 768, SWP_NOSIZE) };
                }
                unsafe { ShowWindow(game, SW_MINIMIZE); }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
}

unsafe extern "system" fn set_window_pos(_window: HWND, _insert_after: HWND, _x: i32, _y: i32, _cx: i32, _cy: i32, _flags: u32) -> i32 { TRUE }
unsafe extern "system" fn set_window_text_w(window: HWND, _text: *const u16) -> i32 {
    let original = SET_WINDOW_TEXT_W.load(Ordering::Relaxed);
    if original == 0 { return 0; }
    let call: unsafe extern "system" fn(HWND, *const u16) -> i32 = unsafe { std::mem::transmute(original) };
    unsafe { call(window, "Guitar Hero Arcade\0".encode_utf16().collect::<Vec<_>>().as_ptr()) }
}
unsafe extern "system" fn set_cursor_pos(_x: i32, _y: i32) -> i32 { TRUE }
unsafe extern "system" fn create_window_ex_a(ex: u32, class: *const u8, _title: *const u8, _style: u32, x: i32, y: i32, _width: i32, _height: i32, parent: HWND, menu: *mut std::ffi::c_void, instance: HINSTANCE, param: *mut std::ffi::c_void) -> HWND {
    let original = CREATE_WINDOW_EX_A.load(Ordering::Relaxed);
    if original == 0 { return std::ptr::null_mut(); }
    let call: unsafe extern "system" fn(u32, *const u8, *const u8, u32, i32, i32, i32, i32, HWND, *mut std::ffi::c_void, HINSTANCE, *mut std::ffi::c_void) -> HWND = unsafe { std::mem::transmute(original) };
    unsafe { call(ex, class, c"Guitar Hero Arcade".as_ptr().cast(), 0x9400_0000, x, y, 1360, 768, parent, menu, instance, param) }
}
unsafe extern "system" fn def_window_proc_a(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let original = DEF_WINDOW_PROC_A.load(Ordering::Relaxed);
    if original == 0 { return 0; }
    let call: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = unsafe { std::mem::transmute(original) };
    if message == WM_LBUTTONDOWN {
        unsafe { SetCapture(window); }
        CLICK_X.store((lparam as u32 & 0xffff) as usize, Ordering::Relaxed);
        CLICK_Y.store(((lparam as u32 >> 16) & 0xffff) as usize, Ordering::Relaxed);
    } else if message == WM_LBUTTONUP {
        unsafe { ReleaseCapture(); }
    } else if message == WM_MOUSEMOVE && unsafe { GetCapture() == window } {
        let mut rect = std::mem::MaybeUninit::uninit();
        if unsafe { GetWindowRect(window, rect.as_mut_ptr()) } != 0 {
            let rect = unsafe { rect.assume_init() };
            let mut x = rect.left + (lparam as u32 & 0xffff) as i32 - CLICK_X.load(Ordering::Relaxed) as i32;
            let mut y = rect.top + ((lparam as u32 >> 16) & 0xffff) as i32 - CLICK_Y.load(Ordering::Relaxed) as i32;
            let screen_w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
            let screen_h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
            if x >= screen_w - 100 { x = 0; }
            if y >= screen_h - 100 { y = 0; }
            let original_pos = SET_WINDOW_POS.load(Ordering::Relaxed);
            if original_pos != 0 {
                let set: unsafe extern "system" fn(HWND, HWND, i32, i32, i32, i32, u32) -> i32 = unsafe { std::mem::transmute(original_pos) };
                let _ = unsafe { set(window, std::ptr::null_mut(), x, y, 1360, 768, SWP_NOSIZE | SWP_NOZORDER) };
            }
        }
    }
    unsafe { call(window, message, wparam, lparam) }
}

fn hook_game_explorer() {
    let replacement = co_create_instance as *const () as usize;
    let main = unsafe { wal_payload_common::iat::hook("ole32.dll", "CoCreateInstance", replacement) };
    if let Some(original) = main {
        let _ = CO_CREATE_INSTANCE.compare_exchange(0, original, Ordering::Relaxed, Ordering::Relaxed);
    }
    let awl_name = std::ffi::CString::new("AWL.dll").unwrap();
    let awl = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(awl_name.as_ptr().cast()) };
    let awl_hook = if awl.is_null() {
        None
    } else {
        unsafe { wal_payload_common::iat::hook_module(awl as usize, "ole32.dll", "CoCreateInstance", replacement) }
    };
    if let Some(original) = awl_hook {
        let _ = CO_CREATE_INSTANCE.compare_exchange(0, original, Ordering::Relaxed, Ordering::Relaxed);
    }
    if main.is_none() && awl_hook.is_none() {
        log!("guitarhero: CoCreateInstance import not found; Game Explorer access hook unavailable");
    }
}

unsafe extern "system" fn co_create_instance(
    class: *const GUID,
    outer: *mut std::ffi::c_void,
    context: u32,
    iid: *const GUID,
    out: *mut *mut std::ffi::c_void,
) -> i32 {
    let original = CO_CREATE_INSTANCE.load(Ordering::Relaxed);
    if original == 0 {
        return 0x8000_4005u32 as i32; // E_FAIL
    }
    let call: unsafe extern "system" fn(*const GUID, *mut std::ffi::c_void, u32, *const GUID, *mut *mut std::ffi::c_void) -> i32 =
        unsafe { std::mem::transmute(original) };
    let result = unsafe { call(class, outer, context, iid, out) };
    if result >= 0 && !out.is_null() && !iid.is_null() && !unsafe { (*out).is_null() } && unsafe { same_guid(&*iid, &IID_GAME_EXPLORER) } {
        unsafe { hook_verify_access(*out) };
    }
    result
}

fn same_guid(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

unsafe fn hook_verify_access(interface: *mut std::ffi::c_void) {
    // IUnknown occupies slots 0-2; IGameExplorer::VerifyAccess is slot 6.
    let vtable = unsafe { *(interface as *mut *mut usize) };
    if vtable.is_null() {
        return;
    }
    let table = vtable as usize;
    if PATCHED_GAME_EXPLORER_VTABLE.load(Ordering::Relaxed) == table {
        return;
    }
    let slot = unsafe { vtable.add(6) };
    let mut old = 0;
    if unsafe { VirtualProtect(slot.cast(), size_of::<usize>(), PAGE_EXECUTE_READWRITE, &mut old) } == 0 {
        return;
    }
    unsafe { slot.write(verify_access as *const () as usize) };
    unsafe { VirtualProtect(slot.cast(), size_of::<usize>(), old, &mut old) };
    PATCHED_GAME_EXPLORER_VTABLE.store(table, Ordering::Relaxed);
}

unsafe extern "system" fn verify_access(_this: *mut std::ffi::c_void, _path: *mut std::ffi::c_void, has_access: *mut i32) -> i32 {
    if !has_access.is_null() {
        unsafe { *has_access = 1 };
    }
    0 // S_OK
}

fn run(base: usize) {
    let mut ready = false;
    let mut previous = [0u32; 2];
    let mut last_probe = std::time::Instant::now();
    loop {
        let input_ready = unsafe { std::ptr::read_volatile((base + INPUT_READY) as *const u8) };
        if !ready && input_ready != 0 {
            unsafe { std::ptr::write_volatile((base + GAME_ACTIVE) as *mut u8, 0) };
            ready = true;
            log!("guitarhero: input ready (flag {input_ready:#04x}); cabinet active flag cleared");
        }
        if ready {
            let state = unsafe { std::ptr::read_volatile((base + INPUT_POINTER) as *const u32) } as usize;
            if state != 0 {
                let active = unsafe { std::ptr::read_volatile((base + GAME_ACTIVE) as *const u8) != 0 };
                let sticks = [wal_payload_common::input(0), wal_payload_common::input(1)];
                let mut changed = false;
                for (player, (stick, controls)) in sticks.iter().zip([&P1, &P2]).enumerate() {
                    let mapped = controls.iter().fold(0, |bits, control| bits | control.input) & stick.buttons;
                    if mapped != previous[player] {
                        changed = true;
                        log!("guitarhero: P{} controls {mapped:#06x}, game active={active}", player + 1);
                    }
                }
                apply_player(state, &sticks[0], &P1, &mut previous[0], active);
                apply_player(state, &sticks[1], &P2, &mut previous[1], active);
                if changed {
                    log!("guitarhero: game state start={:#04x}/{:#04x} fret={:#04x}/{:#04x}",
                        unsafe { std::ptr::read_volatile((state + 0x85) as *const u8) },
                        unsafe { std::ptr::read_volatile((state + 0x121) as *const u8) },
                        unsafe { std::ptr::read_volatile((state + 0x52) as *const u8) },
                        unsafe { std::ptr::read_volatile((state + 0xee) as *const u8) });
                }
                unsafe {
                    std::ptr::write_volatile((state + 0x66) as *mut u8, sticks[0].axis_u8(Axis::LeftY));
                    std::ptr::write_volatile((state + 0x102) as *mut u8, sticks[1].axis_u8(Axis::LeftY));
                }
            }
        } else if last_probe.elapsed() >= std::time::Duration::from_secs(2) {
            let pointer = unsafe { std::ptr::read_volatile((base + INPUT_POINTER) as *const u32) };
            log!("guitarhero: waiting for input-ready flag (flag {input_ready:#04x}, state pointer {pointer:#010x})");
            last_probe = std::time::Instant::now();
        }
        std::thread::sleep(POLL);
    }
}

fn apply_player(state: usize, stick: &StickState, controls: &[Control; 8], previous: &mut u32, active: bool) {
    let mut current = 0u32;
    for control in controls {
        if stick.buttons & control.input != 0 {
            current |= control.input;
        }
        let was_down = *previous & control.input != 0;
        let is_down = current & control.input != 0;
        if control.start {
            if !active && is_down && !was_down {
                unsafe { std::ptr::write_volatile((state + control.state_offset) as *mut u8, control.state_delta) };
            } else if !active && (is_down || was_down) {
                unsafe { std::ptr::write_volatile((state + control.state_offset) as *mut u8, 0) };
            }
        } else if is_down && !was_down {
            unsafe {
                let cell = (state + control.state_offset) as *mut u8;
                std::ptr::write_volatile(cell, std::ptr::read_volatile(cell).wrapping_add(control.state_delta));
                if let Some(offset) = control.menu_offset.filter(|_| {
                    if control.strum { active } else { !active }
                }) {
                    std::ptr::write_volatile((state + offset) as *mut u8, control.menu_value);
                }
            }
        } else if !is_down && was_down {
            unsafe {
                let cell = (state + control.state_offset) as *mut u8;
                std::ptr::write_volatile(cell, std::ptr::read_volatile(cell).wrapping_sub(control.state_delta));
                if control.strum == active {
                    if let Some(offset) = control.menu_offset {
                        std::ptr::write_volatile((state + offset) as *mut u8, 0);
                    }
                }
            }
        } else if is_down && control.strum == active {
            if let Some(offset) = control.menu_offset {
                unsafe { std::ptr::write_volatile((state + offset) as *mut u8, 0) };
            }
        }
    }
    *previous = current;
}
