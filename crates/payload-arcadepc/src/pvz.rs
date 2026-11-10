//! Plants vs. Zombies: The Last Stand (`WAL_ARCADEPC_BOARD=pvz`, x86 build): the cabinet's
//! `Shell.exe` (Hollywood Gaming's redemption shell, Direct3D 9) runs the game, `Pvz.exe`
//! (Unity 4.3) only draws it: the Shell sends it every screen, wave and zombie on
//! `127.0.0.1:18888`, the game answers with its events (start pressed, peas fired, objects hit).
//!
//! In the Shell (the profile's executable): the community patches of its dongle, security key,
//! install checks, encrypted game path, I/O board, cleanup and reboot (`WAL_PATCHES`), and its
//! game started through `wal-loader` with this payload ([`create_process`]). The Shell's window
//! stays visible behind the game on the cabinet's single monitor; on a desktop it stayed in front
//! of the game: it is hidden while the game's window is shown ([`shell_window`]).
//!
//! In the game: the pea shooter is the mouse. The Shell's `SetGameMode` (the community
//! `Game.ini`'s `[Debug] UseMouseController=1`) makes `PvzCore` aim at `Input.mousePosition`
//! instead of the cabinet's analog gun (a joystick's axes); every shot, the screens' buttons
//! included, is `Fire1` (the left button). The payload plays that mouse with the virtual sticks:
//! the gun is the cursor (its place on the game's picture, in the game's pixels; Unity's
//! `GetCursorPos` answered with it), the trigger its left button (a message and a raw report:
//! Unity 4 reads the buttons from raw input); the real keyboard and mouse never reach the game
//! (`payload-common`'s `input_filter`). The dump's BepInEx plugin is not loaded (the profile's
//! `winhttp` builtin).
//!
//! The Shell's inputs from its I/O board (not there: its thread patched out) are the payload's:
//! the start button (`START_BUTTON` byte: from the attract, in free play, a credit and the start
//! screen) and the coin meter (`COIN_METER`, a counter: each coin a credit, the main loop reads
//! its increments).
//!
//! Natives: `trigger` (player 1, the game), `start`, `coin` (the Shell).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_mouse, post_raw_buttons};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{desktop, gun, iat, input, log, patches};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW, GetModuleHandleW,
};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetClientRect, GetWindowThreadProcessId, IsWindowVisible, SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE,
};

/// `wParam`'s left button flag.
const MK_LBUTTON: usize = 0x0001;
const RI_MOUSE_LEFT_BUTTON_DOWN: u16 = 0x0001;
const RI_MOUSE_LEFT_BUTTON_UP: u16 = 0x0002;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Start,
    Coin,
}

const NATIVES: &[(&str, Native)] = &[("trigger", Native::Trigger), ("start", Native::Start), ("coin", Native::Coin)];
const DEFAULTS: &[(&str, &str)] = &[("b1", "trigger"), ("start", "start"), ("coin", "coin")];

/// The Shell's I/O board start button (its byte, `FUN_0042ad60`'s).
const START_BUTTON: usize = 0x1120ca;
/// The Shell's I/O board coin meter (`u16`, `FUN_00407a40` adds its increase to the credits).
const COIN_METER: usize = 0x112131;

static ORIG_CREATE_PROCESS: AtomicUsize = AtomicUsize::new(0);

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The Shell's or the game's part, from the executable.
pub fn init() {
    // Unity 4.3: the player is the executable
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    if patches::exe_name().eq_ignore_ascii_case("Pvz.exe") {
        wal_payload_common::input_filter::init(exe, |_| false);
        own_mouse(exe);
        std::thread::spawn(input_thread);
        log!("pvz: the mouse played by the gun");
        return;
    }
    patches::apply();
    wal_payload_common::input_filter::init(exe, |_| false);
    match unsafe { iat::hook("kernel32.dll", "CreateProcessA", create_process as *const () as usize) } {
        Some(orig) => ORIG_CREATE_PROCESS.store(orig, Ordering::Relaxed),
        None => log!("pvz: the Shell does not import CreateProcessA"),
    }
    std::thread::spawn(shell_window);
    std::thread::spawn(move || shell_input(exe));
}

/// The Shell's start button and coin meter, from the virtual sticks.
fn shell_input(exe: usize) {
    let (start_at, meter_at) = ((exe + START_BUTTON) as *mut u8, (exe + COIN_METER) as *mut u16);
    let mut coin_was = false;
    loop {
        std::thread::sleep(Duration::from_millis(10));
        let (mut start, mut coin) = (false, false);
        map().for_each_pressed(&input(0), |n| match n {
            Native::Start => start = true,
            Native::Coin => coin = true,
            Native::Trigger => {}
        });
        unsafe { start_at.write_volatile(start as u8) };
        if coin && !coin_was {
            unsafe { meter_at.write_unaligned(meter_at.read_unaligned().wrapping_add(1)) };
            log!("pvz: coin");
        }
        coin_was = coin;
    }
}

/// The Shell's windows hidden while a Unity window (the game's, another process) is visible,
/// shown again once it is gone.
fn shell_window() {
    unsafe extern "system" fn game_shown(hwnd: HWND, found: isize) -> i32 {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid != unsafe { GetCurrentProcessId() } && unsafe { IsWindowVisible(hwnd) } != 0 {
            let mut class = [0u16; 32];
            let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) } as usize;
            if String::from_utf16_lossy(&class[..n]) == "UnityWndClass" {
                unsafe { *(found as *mut bool) = true };
                return 0;
            }
        }
        1
    }
    unsafe extern "system" fn own(hwnd: HWND, list: isize) -> i32 {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        let mut class = [0u16; 32];
        let n = unsafe { GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32) } as usize;
        // the Shell's own windows, not the payload's (the bezel's strips)
        if pid == unsafe { GetCurrentProcessId() } && !String::from_utf16_lossy(&class[..n]).to_ascii_lowercase().starts_with("wal") {
            unsafe { (*(list as *mut Vec<usize>)).push(hwnd as usize) };
        }
        1
    }
    let mut hidden: Vec<usize> = Vec::new();
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let mut game = false;
        unsafe { EnumWindows(Some(game_shown), &mut game as *mut bool as isize) };
        if game && hidden.is_empty() {
            let mut list: Vec<usize> = Vec::new();
            unsafe { EnumWindows(Some(own), &mut list as *mut Vec<usize> as isize) };
            hidden = list.into_iter().filter(|&w| unsafe { IsWindowVisible(w as HWND) } != 0).collect();
            for &w in &hidden {
                unsafe { ShowWindow(w as HWND, SW_HIDE) };
            }
            log!("pvz: the game's window shown: {} Shell window(s) hidden", hidden.len());
        } else if !game && !hidden.is_empty() {
            for &w in &hidden {
                unsafe { ShowWindow(w as HWND, SW_SHOWNOACTIVATE) };
            }
            log!("pvz: the game's window gone: the Shell's shown again");
            hidden.clear();
        }
    }
}

/// The left button pressed or released at client point `at`, as a message and a raw report.
fn button(hwnd: HWND, down: bool, at: (i32, i32)) {
    if down {
        post_mouse(hwnd, WM_LBUTTONDOWN, at.0, at.1, MK_LBUTTON);
        post_raw_buttons(hwnd, RI_MOUSE_LEFT_BUTTON_DOWN);
    } else {
        post_mouse(hwnd, WM_LBUTTONUP, at.0, at.1, 0);
        post_raw_buttons(hwnd, RI_MOUSE_LEFT_BUTTON_UP);
    }
}

fn input_thread() {
    let (mut trigger_was, mut last) = (false, (i32::MIN, i32::MIN));
    let mut tick = 0u32;
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = gun::find_window();
        if hwnd.is_null() {
            continue;
        }
        let mut trigger = false;
        map().for_each_pressed(&input(0), |n| {
            if n == Native::Trigger {
                trigger = true;
            }
        });
        // the gun: the cursor on the window's picture (kept at its last place off it), in the
        // game's pixels (its portrait desktop's: Unity takes the cursor's client coordinates as
        // its own whatever the fitted window's size)
        let (w, h) = desktop::size().unwrap_or_else(|| {
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            unsafe { GetClientRect(hwnd, &mut r) };
            (r.right, r.bottom)
        });
        let at = match gun::gun_of_player_in_picture(0) {
            Some((x, y)) => ((x * w as f32) as i32, (y * h as f32) as i32),
            None => last,
        };
        let buttons = if trigger { MK_LBUTTON } else { 0 };
        // again every half second: a cursor that did not move still reaches a new screen
        tick = (tick + 1) % 64;
        if (at != last || tick == 0) && at.0 != i32::MIN {
            post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, buttons);
            last = at;
        }
        if trigger != trigger_was && last.0 != i32::MIN {
            button(hwnd, trigger, last);
        }
        trigger_was = trigger;
    }
}

/// This payload's path (its folder holds `wal-loader.exe`).
fn payload_path() -> String {
    let mut module = std::ptr::null_mut();
    let mut buf = [0u16; 520];
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            payload_path as *const () as *const u16,
            &mut module,
        );
        let n = GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) as usize;
        String::from_utf16_lossy(&buf[..n])
    }
}

fn c_str(p: *const u8) -> String {
    if p.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_string_lossy().into_owned() }
}

/// The Shell's `CreateProcessA(game, arguments)` (`c:\sega\pvz\Pvz.exe -popupwindow -adapter 0
/// -screen-height 1920 -screen-width 1080`): the game through `wal-loader`, the rest as asked.
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn create_process(
    app: *const u8,
    cmd: *mut u8,
    pa: usize,
    ta: usize,
    inherit: i32,
    flags: u32,
    env: usize,
    dir: *const u8,
    si: usize,
    pi: usize,
) -> i32 {
    type CreateProcessA = unsafe extern "system" fn(*const u8, *mut u8, usize, usize, i32, u32, usize, *const u8, usize, usize) -> i32;
    let orig: CreateProcessA = unsafe { std::mem::transmute(ORIG_CREATE_PROCESS.load(Ordering::Relaxed)) };
    let (app_s, cmd_s) = (c_str(app), c_str(cmd));
    log!("pvz: the Shell starts {app_s:?} {cmd_s:?} in {:?}", c_str(dir));
    if !app_s.to_ascii_lowercase().ends_with("pvz.exe") {
        return unsafe { orig(app, cmd, pa, ta, inherit, flags, env, dir, si, pi) };
    }
    let payload = payload_path();
    let folder = payload.rsplit_once('\\').map_or("", |(d, _)| d);
    let line = format!("\"{folder}\\wal-loader.exe\" \"{payload}\" \"{app_s}\" {cmd_s}");
    log!("pvz: {line}");
    let mut line = std::ffi::CString::new(line).unwrap_or_default().into_bytes_with_nul();
    // the loader is a console program: no console window over the Shell
    let flags = flags | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    let ok = unsafe { orig(std::ptr::null(), line.as_mut_ptr(), pa, ta, inherit, flags, env, dir, si, pi) };
    if ok == 0 {
        log!("pvz: the game did not start (error {})", unsafe { windows_sys::Win32::Foundation::GetLastError() });
    }
    ok
}
