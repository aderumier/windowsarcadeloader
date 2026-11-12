//! In The Groove 2 (the OpenITG beta 2 build of the arcade game: `Program\OpenITG.exe`, a
//! StepMania 3.95 fork, OpenGL, 32-bit): `WAL_ARCADEPC_BOARD=itg`.
//!
//! **Inputs.** The cabinet's I/O boards (ITGIO / PIUIO through `libusb0.dll`, the PIU pump pads
//! through HID) are absent; the game then plays on its DirectInput keyboard (`dinput.dll`'s
//! `DirectInputCreateA`, the DirectInput 7 types), mapped by the dump's `Data\KEYMAPS.INI`:
//! player 1's arrows, Enter (start), Escape (back), Delete / Page Down / Home / End (the menu
//! buttons), Right Shift (select), F1 (coin), Scroll Lock (the operator menu); player 2's
//! keypad 4 / 6 / 8 / 2, keypad Enter, Num Lock, keypad `/` `*` `-` `+`, keypad 0. The common
//! fake DirectInput (`WAL_DINPUT_DISABLE`, `WAL_DINPUT_LEGACY`) gives it a system keyboard whose
//! keys come from both players' virtual sticks ([`dinput::set_keyboard`]): the real keyboard
//! never reaches it. StepMania reads a keyboard's buffered data unless its buffer size is
//! refused (`WAL_DINPUT_KEYBOARD_POLLED`: it then polls the key states).
//!
//! **Preferences.** The game reads `Data\StepMania.ini` (then `Data\Static.ini` over it) when it
//! starts, and writes it back. The payload sets there, before: `CoinMode` (the common
//! `WAL_FREEPLAY`: 2 free play, 1 pay), and the window (`WAL_ITG_WINDOWED=1`, the default):
//! `Windowed=1`, `DisplayWidth` / `DisplayHeight` the size its viewport is scaled from
//! (`WAL_GL_SIZE`: the common viewport scaling fits it to the screen-sized window, 4:3, black
//! bars), else 4:3 at the screen's height.
//!
//! **Window.** The game creates its window without a size (0x0: the common popup creation
//! leaves it alone), then sets its captioned style (`SetWindowLongA(GWL_STYLE)`) and its size:
//! with `WAL_WINDOW_POPUP=1` that style stays a borderless popup (the executable's call
//! hooked; on GNOME the title bar showed above a screen-sized window).
//!
//! Natives per player (`p2_` for player 2): `up`, `down`, `left`, `right`, `start`, `back`,
//! `menu_left`, `menu_right`, `menu_up`, `menu_down`, `select`; player 1's `coin`, `operator`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{dinput, iat, input, log};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetParent, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, WS_CAPTION, WS_CHILD, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU,
    WS_THICKFRAME,
};

/// DirectInput key codes (DIK_*) of the dump's `KEYMAPS.INI`.
const NATIVES: &[(&str, u8)] = &[
    ("up", 0xC8),
    ("down", 0xD0),
    ("left", 0xCB),
    ("right", 0xCD),
    ("start", 0x1C),
    ("back", 0x01),
    ("menu_left", 0xD3),
    ("menu_right", 0xD1),
    ("menu_up", 0xC7),
    ("menu_down", 0xCF),
    ("select", 0x36),
    ("coin", 0x3B),
    ("operator", 0x46),
    ("p2_up", 0x48),
    ("p2_down", 0x50),
    ("p2_left", 0x4B),
    ("p2_right", 0x4D),
    ("p2_start", 0x9C),
    ("p2_back", 0x45),
    ("p2_menu_left", 0xB5),
    ("p2_menu_right", 0x37),
    ("p2_menu_up", 0x4A),
    ("p2_menu_down", 0x4E),
    ("p2_select", 0x52),
];

/// A pad's face buttons are the four arrows too, where they lie (X left, Y up, A down, B right:
/// jumps on two of them), the shoulders the menu buttons.
const DEFAULTS: &[(&str, &str)] = &[
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "left"),
    ("b2", "up"),
    ("b5", "down"),
    ("b6", "right"),
    ("b4", "menu_left"),
    ("b3", "menu_right"),
    ("b7", "select"),
    ("b8", "back"),
    ("start", "start"),
    ("coin", "coin"),
    ("service", "coin"),
    ("test", "operator"),
];

const DEFAULTS_P2: &[(&str, &str)] = &[
    ("up", "p2_up"),
    ("down", "p2_down"),
    ("left", "p2_left"),
    ("right", "p2_right"),
    ("b1", "p2_left"),
    ("b2", "p2_up"),
    ("b5", "p2_down"),
    ("b6", "p2_right"),
    ("b4", "p2_menu_left"),
    ("b3", "p2_menu_right"),
    ("b7", "p2_select"),
    ("b8", "p2_back"),
    ("start", "p2_start"),
    // the game counts coins on player 1's key only
    ("coin", "coin"),
    ("service", "coin"),
    ("test", "operator"),
];

fn maps() -> &'static [ButtonMap<u8>; 2] {
    static MAPS: OnceLock<[ButtonMap<u8>; 2]> = OnceLock::new();
    MAPS.get_or_init(|| [ButtonMap::new(NATIVES, DEFAULTS), ButtonMap::with_defaults(NATIVES, DEFAULTS_P2)])
}

/// The DirectInput keyboard's keys: both players' sticks.
fn keyboard(keys: &mut [u8; 256]) {
    for (player, map) in maps().iter().enumerate() {
        map.for_each_pressed(&input(player), |dik| keys[dik as usize] = 0x80);
    }
}

/// `<dump>\Data\StepMania.ini`: the executable's folder's parent.
fn prefs_path() -> Option<std::path::PathBuf> {
    let mut buf = [0u16; 1024];
    let n = unsafe { GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let exe = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n]));
    Some(exe.parent()?.parent()?.join("Data").join("StepMania.ini"))
}

/// `values` set in the `[Options]` section of the ini text (added after its header when absent).
fn set_options(text: &str, values: &[(&str, String)]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let header = lines.iter().position(|l| l.trim().eq_ignore_ascii_case("[Options]"));
    let header = header.unwrap_or_else(|| {
        lines.insert(0, "[Options]".to_string());
        0
    });
    let end = lines[header + 1..].iter().position(|l| l.trim_start().starts_with('[')).map_or(lines.len(), |i| header + 1 + i);
    let mut added = Vec::new();
    for (key, value) in values {
        let line = format!("{key}={value}");
        let found = lines[header + 1..end]
            .iter()
            .position(|l| l.split_once('=').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(key)));
        match found {
            Some(i) => lines[header + 1 + i] = line,
            None => added.push(line),
        }
    }
    for (i, line) in added.into_iter().enumerate() {
        lines.insert(header + 1 + i, line);
    }
    let mut out = lines.join("\r\n");
    out.push_str("\r\n");
    out
}

fn write_prefs() {
    let mut values = Vec::new();
    let free = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY");
    if let Some(free) = free {
        values.push(("CoinMode", if free { "2" } else { "1" }.to_string()));
    }
    let windowed = std::env::var("WAL_ITG_WINDOWED").map_or(true, |v| v.trim() != "0");
    if windowed {
        // the size the profile's viewport scaling draws from (`WAL_GL_SIZE`), else 4:3 at the
        // screen's height
        let gl = std::env::var("WAL_GL_SIZE").ok().and_then(|v| {
            let (w, h) = v.trim().split_once('x')?;
            Some((w.parse::<i32>().ok()?, h.parse::<i32>().ok()?))
        });
        let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let (w, h) = gl.unwrap_or(if sw * 3 > sh * 4 { (sh * 4 / 3, sh) } else { (sw, sw * 3 / 4) });
        values.push(("Windowed", "1".to_string()));
        values.push(("DisplayWidth", w.to_string()));
        values.push(("DisplayHeight", h.to_string()));
    }
    let Some(path) = prefs_path() else { return };
    let text = std::fs::read(&path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    match std::fs::write(&path, set_options(&text, &values)) {
        Ok(()) => log!("itg: {} set: {values:?}", path.display()),
        Err(e) => log!("itg: {} not written: {e}", path.display()),
    }
}

static ORIG_SET_WINDOW_LONG: AtomicUsize = AtomicUsize::new(0);

/// The executable's `SetWindowLongA`: its top-level window's style without caption or frame.
unsafe extern "system" fn set_window_long(hwnd: HWND, index: i32, value: i32) -> i32 {
    let orig: unsafe extern "system" fn(HWND, i32, i32) -> i32 = unsafe { std::mem::transmute(ORIG_SET_WINDOW_LONG.load(Ordering::Relaxed)) };
    let mut v = value as u32;
    if index == GWL_STYLE && v & WS_CHILD == 0 && unsafe { GetParent(hwnd) }.is_null() {
        v = (v & !(WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX)) | WS_POPUP;
        if v != value as u32 {
            log!("itg: window {hwnd:?} style {value:#x} -> {v:#x}");
        }
    }
    unsafe { orig(hwnd, index, v as i32) }
}

pub fn init() {
    dinput::set_keyboard(keyboard);
    write_prefs();
    if std::env::var("WAL_WINDOW_POPUP").is_ok_and(|v| v.trim() == "1") {
        let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
        if let Some(o) = unsafe { iat::hook_module(exe, "user32.dll", "SetWindowLongA", set_window_long as *const () as usize) } {
            ORIG_SET_WINDOW_LONG.store(o, Ordering::Relaxed);
        }
    }
    log!("itg: DirectInput keyboard from players 1 and 2");
}
