//! Men In Black (Sega Amusements Europe / SEGA Amusements, 2022, Unity 2020.3 Mono, 64-bit):
//! two guns, the cabinet's shell (`shell_mib.rs`) and its lamp board. `WAL_EUROPAR_BOARD=mib`.
//!
//! * **Guns.** The game reads its gun board as one Unity joystick (`Input.GetAxis("X Axis0")`...:
//!   axes 0 / 1 player 1's gun, 3 / 2 player 2's, inverted; `Input.GetButton("Player1Trigger1")`
//!   button 7, player 2's 9; `PlayerStart1` / `2` buttons 5 / 6), maps the axes onto the screen
//!   with the calibration of `ShellData.ini` (`[Player1Gun]`..., `MIBCore.SetCalibration`), and
//!   takes no input while a joystick has a name (`MIBCore.Update`: the board has none). Unity
//!   2020 reads joysticks with raw input and HID: instead its input functions, registered by
//!   UnityPlayer through Mono's `mono_add_internal_call` (found with `GetProcAddress`), are the
//!   payload's: the axes of those names are the players' guns (the profile's calibration, 0 to
//!   4096 unclamped, maps -1..1 onto the whole screen), the buttons their triggers and starts,
//!   `GetJoystickNames` gives the joysticks no name; the keyboard and mouse (debug keys: Escape
//!   skips the boss) see nothing pressed.
//! * **Lamps.** `RS232` opens the first serial port `SerialPort.GetPortNames` lists (the
//!   registry's `HARDWARE\DEVICEMAP\SERIALCOMM`, read through Mono's `RegOpenKeyExW` import):
//!   none listed, no lamp board (Batocera's real ports and Sinden guns left alone).
//! * **Free play.** `WAL_EUROPAR_FREEPLAY` (else the common `WAL_FREEPLAY`) written to
//!   `C:\Sega\ShellData\ShellData.ini`'s `[Credit] Freeplay` (the profile's copy of the file)
//!   before the game reads it. A player's start sets `CreditsUI.TakingCredit` until the shell
//!   sends a lower credit count, which free play ignores: the second player could not join. In
//!   free play each start pressed clears it first ([`free_play_start`]).

use std::ffi::{CStr, c_char, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use wal_payload_common::{iat, log, mono, mono_imports};
use wal_protocol::button;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

type P = *mut c_void;

const SHELL_DATA: &str = "C:/Sega/ShellData/ShellData.ini";

static UNITY_GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static ADD_INTERNAL_CALL: AtomicUsize = AtomicUsize::new(0);
static FRAME_COUNT: AtomicUsize = AtomicUsize::new(0);
static JOYSTICK_NAMES: AtomicUsize = AtomicUsize::new(0);
static FREE_PLAY: AtomicBool = AtomicBool::new(false);

/// The game's joystick buttons: (name, player, virtual button).
const BUTTONS: [(&str, usize, u32); 6] = [
    ("PlayerStart1", 0, button::START),
    ("PlayerStart2", 1, button::START),
    ("Player1Trigger1", 0, button::B1),
    ("Player1Trigger2", 0, button::B1),
    ("Player2Trigger1", 1, button::B1),
    ("Player2Trigger2", 1, button::B1),
];

/// The text of a managed string (its length after the object header, then UTF-16).
fn managed_string(s: P) -> String {
    if s.is_null() {
        return String::new();
    }
    let s = s as *const u8;
    let len = unsafe { mono::read::<i32>(s, 2 * size_of::<usize>()) }.max(0) as usize;
    let chars: Vec<u16> = (0..len).map(|i| unsafe { mono::read::<u16>(s, 2 * size_of::<usize>() + 4 + 2 * i) }).collect();
    String::from_utf16_lossy(&chars)
}

fn pressed(name: &str) -> bool {
    BUTTONS.iter().any(|&(n, player, b)| n == name && wal_payload_common::input(player).pressed(b))
}

/// The last gun positions on the screen (f32 bits: player 1 x, y, player 2 x, y), kept while a
/// gun points off the screen.
static AIM: [AtomicU32; 4] = [const { AtomicU32::new(0) }; 4];

/// `float Input.GetAxis(string)`, `GetAxisRaw`: the guns, -1..1 (inverted: 1 at the left / top).
unsafe extern "C" fn get_axis(name: P) -> f32 {
    let name = managed_string(name);
    let Some(axis) = ["X Axis0", "Y Axis0", "X Axis1", "Y Axis1"].iter().position(|&a| a == name) else { return 0.0 };
    let player = axis / 2;
    if let Some((x, y)) = wal_payload_common::gun::gun_of_player_in_picture(player) {
        AIM[player * 2].store((1.0 - 2.0 * x).to_bits(), Ordering::Relaxed);
        AIM[player * 2 + 1].store((1.0 - 2.0 * y).to_bits(), Ordering::Relaxed);
    }
    f32::from_bits(AIM[axis].load(Ordering::Relaxed))
}

/// `bool Input.GetButton(string)`
unsafe extern "C" fn get_button(name: P) -> u8 {
    pressed(&managed_string(name)) as u8
}

/// Each button's state at the frames it was read: (frame, previous, current).
static EDGES: Mutex<[(i32, bool, bool); BUTTONS.len()]> = Mutex::new([(-1, false, false); BUTTONS.len()]);

/// Whether the button went down (`down`) or up since the previous frame.
fn edge(name: P, down: bool) -> u8 {
    let name = managed_string(name);
    let Some(i) = BUTTONS.iter().position(|&(n, ..)| n == name) else { return 0 };
    let frame_count = FRAME_COUNT.load(Ordering::Relaxed);
    let frame = if frame_count == 0 { 0 } else { unsafe { std::mem::transmute::<usize, unsafe extern "C" fn() -> i32>(frame_count)() } };
    let mut edges = EDGES.lock().unwrap_or_else(|e| e.into_inner());
    let (last, previous, current) = &mut edges[i];
    if *last != frame {
        *last = frame;
        *previous = *current;
        *current = pressed(&name);
    }
    let went = if down { *current && !*previous } else { !*current && *previous };
    if went && down && name.starts_with("PlayerStart") && FREE_PLAY.load(Ordering::Relaxed) {
        free_play_start();
    }
    went as u8
}

/// `MIBCore.Instance.CreditsController.TakingCredit = false` (the game's main thread: its
/// `CreditsUI.CheckCreditTaken` reads the start button).
fn free_play_start() {
    let Some(m) = mono::mono() else { return };
    let image = m.image("Assembly-CSharp");
    let core = m.class(image, "MIB", "MIBCore");
    let instance = m.static_addr(core, m.field(core, "InstanceVariable"));
    if instance.is_null() {
        return;
    }
    let core_object = unsafe { mono::read::<usize>(instance, 0) } as *mut u8;
    let controller = m.field(core, "CreditsController");
    let taking = m.field(m.class(image, "", "CreditsUI"), "TakingCredit");
    if core_object.is_null() || controller.is_null() || taking.is_null() {
        return;
    }
    let credits_ui = unsafe { mono::read::<usize>(core_object, m.offset(controller)) } as *mut u8;
    if !credits_ui.is_null() {
        unsafe { mono::write::<u8>(credits_ui, m.offset(taking), 0) };
    }
}

unsafe extern "C" fn get_button_down(name: P) -> u8 {
    edge(name, true)
}

unsafe extern "C" fn get_button_up(name: P) -> u8 {
    edge(name, false)
}

/// The keyboard's keys and the mouse's buttons (`GetKey*`, `GetMouseButton*`): none pressed.
unsafe extern "C" fn nothing_pressed(_key: usize) -> u8 {
    0
}

/// `string[] Input.GetJoystickNames()`: the joysticks without names.
unsafe extern "C" fn get_joystick_names() -> P {
    let f: unsafe extern "C" fn() -> P = unsafe { std::mem::transmute(JOYSTICK_NAMES.load(Ordering::Relaxed)) };
    let names = unsafe { f() };
    static EMPTY: AtomicUsize = AtomicUsize::new(0);
    if !names.is_null() && let Some(m) = mono::mono() {
        if EMPTY.load(Ordering::Relaxed) == 0 {
            EMPTY.store(m.pin(m.string("")) as usize, Ordering::Relaxed);
        }
        let len = unsafe { mono::read::<usize>(names as *const u8, 3 * size_of::<usize>()) };
        for i in 0..len {
            unsafe { mono::write::<usize>(names as *mut u8, mono::ARRAY_DATA + i * size_of::<usize>(), EMPTY.load(Ordering::Relaxed)) };
        }
    }
    names
}

unsafe extern "C" fn add_internal_call(name: *const c_char, method: P) {
    let orig: unsafe extern "C" fn(*const c_char, P) = unsafe { std::mem::transmute(ADD_INTERNAL_CALL.load(Ordering::Relaxed)) };
    let call = if name.is_null() { &[][..] } else { unsafe { CStr::from_ptr(name) }.to_bytes() };
    let ours: Option<usize> = match call.strip_prefix(b"UnityEngine.Input::") {
        Some(b"GetAxis" | b"GetAxisRaw") => Some(get_axis as *const () as usize),
        Some(b"GetButton") => Some(get_button as *const () as usize),
        Some(b"GetButtonDown") => Some(get_button_down as *const () as usize),
        Some(b"GetButtonUp") => Some(get_button_up as *const () as usize),
        Some(
            b"GetKeyInt" | b"GetKeyString" | b"GetKeyDownInt" | b"GetKeyDownString" | b"GetKeyUpInt" | b"GetKeyUpString"
            | b"GetMouseButton" | b"GetMouseButtonDown" | b"GetMouseButtonUp",
        ) => Some(nothing_pressed as *const () as usize),
        Some(b"GetJoystickNames") => {
            JOYSTICK_NAMES.store(method as usize, Ordering::Relaxed);
            Some(get_joystick_names as *const () as usize)
        }
        _ => {
            if call == b"UnityEngine.Time::get_frameCount" {
                FRAME_COUNT.store(method as usize, Ordering::Relaxed);
            }
            None
        }
    };
    match ours {
        Some(f) => {
            log!("mib: {} is the payload's", String::from_utf8_lossy(call));
            unsafe { orig(name, f as P) }
        }
        None => unsafe { orig(name, method) },
    }
}

unsafe extern "system" fn unity_get_proc_address(module: P, name: *const u8) -> P {
    let orig: unsafe extern "system" fn(P, *const u8) -> P = unsafe { std::mem::transmute(UNITY_GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    let f = unsafe { orig(module, name) };
    if !f.is_null() && (name as usize) >> 16 != 0 && unsafe { CStr::from_ptr(name.cast()) }.to_bytes() == b"mono_add_internal_call" {
        ADD_INTERNAL_CALL.store(f as usize, Ordering::Relaxed);
        return add_internal_call as *const () as P;
    }
    f
}

/// `RegOpenKeyExW` (Mono's `Microsoft.Win32.Registry`): the serial port list absent.
unsafe extern "system" fn reg_open_key_ex_w(key: P, sub: *const u16, options: u32, access: u32, result: *mut P) -> i32 {
    type F = unsafe extern "system" fn(P, *const u16, u32, u32, *mut P) -> i32;
    static ORIG: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    if !sub.is_null() {
        let len = (0..).take_while(|&i| unsafe { *sub.add(i) } != 0).count();
        let name = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(sub, len) });
        if name.trim_matches('\\').eq_ignore_ascii_case("HARDWARE\\DEVICEMAP\\SERIALCOMM") {
            static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !LOGGED.swap(true, Ordering::Relaxed) {
                log!("mib: serial port list hidden (no lamp board)");
            }
            return 2; // ERROR_FILE_NOT_FOUND
        }
    }
    let orig = *ORIG.get_or_init(|| unsafe {
        let advapi32 = windows_sys::Win32::System::LibraryLoader::LoadLibraryW(windows_sys::w!("advapi32.dll"));
        windows_sys::Win32::System::LibraryLoader::GetProcAddress(advapi32, c"RegOpenKeyExW".as_ptr().cast()).map_or(0, |f| f as usize)
    });
    unsafe { std::mem::transmute::<usize, F>(orig)(key, sub, options, access, result) }
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    (module == "advapi32.dll" && name == "RegOpenKeyExW").then_some(reg_open_key_ex_w as *const () as usize)
}

/// `[Credit] Freeplay` of the shell's settings, set to `on`.
fn write_free_play(on: bool) {
    let Ok(text) = std::fs::read_to_string(SHELL_DATA) else {
        log!("mib: {SHELL_DATA} missing");
        return;
    };
    let mut section = String::new();
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            let t = line.trim();
            if t.starts_with('[') {
                section = t.to_ascii_lowercase();
            } else if section == "[credit]" && t.split('=').next().is_some_and(|k| k.trim().eq_ignore_ascii_case("Freeplay")) {
                return format!("Freeplay={}", on as u8);
            }
            line.to_string()
        })
        .collect();
    let new = lines.join("\r\n") + "\r\n";
    if new != text && let Err(e) = std::fs::write(SHELL_DATA, new) {
        log!("mib: writing {SHELL_DATA}: {e}");
    }
}

pub(crate) fn init() {
    let free_play = wal_payload_common::free_play("WAL_EUROPAR_FREEPLAY").unwrap_or(false);
    write_free_play(free_play);
    FREE_PLAY.store(free_play, Ordering::Relaxed);
    crate::shell_mib::init(free_play);
    mono_imports::hook(resolve);
    let unity = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) } as usize;
    match unsafe { iat::hook_module(unity, "kernel32.dll", "GetProcAddress", unity_get_proc_address as *const () as usize) } {
        Some(o) => UNITY_GET_PROC_ADDRESS.store(o, Ordering::Relaxed),
        None => log!("mib: UnityPlayer does not import GetProcAddress: no gun input"),
    }
    log!("mib: shell, guns, free play {free_play}");
}
