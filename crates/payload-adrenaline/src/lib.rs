//! Adrenaline Amusements payload: the cabinet hardware of Adrenaline's PC-based arcade games.
//!
//! - Skylanders Cloud Patrol (Unity 4.2, Mono `mono.dll`, 32-bit): a touch screen game.
//!   `pqmt.rs` is its PQ Labs touch frame's client (`PQMTClient.dll`: the players' guns touch
//!   the screen), `kaboom.rs` its "Kaboom" I/O board (`KaboomComDll.dll`: coins, the
//!   operator's keypad, the ticket dispenser).
//! - Rabbids Hollywood (Unity 2017.2, Mono, 64-bit): a gun game. `sixense.rs` is its Sixense
//!   STEM base's driver (`sxCore.dll`: the guns are electromagnetic trackers, posed to aim
//!   where the players' guns point), `adrio.rs` its Kaboom Adrio board
//!   (`KaboomAdrioFxPlusFourFeeders64.dll`: coins, start buttons, the operator's keypad, the
//!   ticket feeders).
//! - Tomb Raider Arcade (Unity 2017.2, Mono, 64-bit): the same guns and board as Rabbids
//!   Hollywood; the trigger is the board's player button 2, the tracker's button reloads.
//! - Rampage (Unity 2017.2, Mono, 64-bit): three players, a joystick and an attack button
//!   each, on the same Adrio board (`adrio.rs`).
//! - Hot Wheels: Kings of the Road (Unity 2018.3, Mono, 64-bit): a six-seat driving game.
//!   `hotwheels.rs` is its three Kaboom Adrio "Hot Wheels" boards
//!   (`KaboomAdrioHotWheels64.dll`: two steering wheels, pedals, keypads and coin doors each).
//! - Infinity Blade FX (Unreal Engine 3, 32-bit): a touch screen sword game, swiped. The
//!   executable imports the PQ Labs client (`pqmt.rs`) and the Matrix dongle, and has the
//!   Kaboom board's library built in (`kaboom.rs`: its serial port).
//! - Spider-Man Homecoming (Unity 5.5, Mono, 32-bit), Crossy Road (Unity 5.1, 32-bit) and
//!   Disney Crossy Road (Unity 5.4, 64-bit): two players, one button each.
//!   `adrio2.rs` is their Kaboom Adrio two-feeder board (`KaboomAdrioFxPlusTwoFeeders.dll`:
//!   coins, the players' buttons on the operator's keypad, the ticket feeders).
//!
//! - Shooty Skies (Unity 5.3, 64-bit) and Flappy Tickets (Unity 4.6, 32-bit): the same board;
//!   Shooty Skies' joysticks are its keyboard keys (`stick_keys.rs`).
//! - Candy Crush Arcade (Unity 4.6, 32-bit, Direct3D 11): the same board; its touch screen
//!   played with the mouse (`mouse.rs`: TouchScript's Windows touches at the cursor, a dot where
//!   it touches); its full screen at the portrait desktop's size (`screen.rs`).
//!
//! - Jetpack Joyride Arcade (Halfbrick's native engine, 32-bit, Direct3D 9): its board is
//!   `KaboomComDll.dll` (`kaboom.rs`: coins, tickets, lamps), resolved through the executable's
//!   own `GetProcAddress`; its dongle imported by name (`jetpack.rs`).
//!
//! - Need for Speed Heat Takedown (Unity 2021, Mono, 64-bit): a driving game. `steering.rs`
//!   is its steering board on a serial port (the wheel and its motor, the pedals, the nitro
//!   button, the operator's keypad, the coin door); `hdrp.rs` its render scale (HDRP's dynamic
//!   resolution, FSR upscaling).
//!
//! - Drakon: Realm Keepers (Unity 2021.3, IL2CPP, 64-bit): two turrets. `drakon.rs` replaces
//!   its input code (the turrets' aim and triggers, the cabinet's keyboard keys) through
//!   IL2CPP's API; no serial port.
//!
//! All have the Matrix dongle (`matrix.rs`: `matrix32.dll`, `matrix64.dll`), all through
//! Mono's or IL2CPP's `DllImport`s (`payload-common`'s `mono_imports.rs`).
//!
//! The payload is `xinput1_3.dll`, which Unity loads at start: no pads (the game's input comes
//! from the emulated hardware), the real keyboard and mouse filtered (`payload-common`'s
//! `input_filter`: the games' own coin, operator and quit keys).

mod adrio;
mod adrio2;
#[cfg(target_pointer_width = "64")]
mod drakon;
#[cfg(target_pointer_width = "64")]
mod hdrp;
mod hotwheels;
#[cfg(target_pointer_width = "32")]
mod jetpack;
#[cfg(target_pointer_width = "32")]
mod kaboom;
mod matrix;
mod mouse;
mod screen;
#[cfg(target_pointer_width = "32")]
mod pqmt;
mod sixense;
mod steering;
mod stick_keys;
mod tombraider;

use std::ffi::c_void;
use std::sync::OnceLock;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, mono_imports};
use windows_sys::Win32::Foundation::HINSTANCE;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

/// The players: one finger (touch point) or gun each.
const PLAYERS: usize = 4;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// The player's finger on the screen, where the gun points.
    Touch,
    /// A finger held on the shield (Infinity Blade FX: its block zone), wherever the gun points.
    Shield,
    /// A finger on the left / right dodge arrow (Infinity Blade FX).
    DodgeLeft,
    DodgeRight,
    /// The gun's trigger (Rabbids Hollywood, Tomb Raider).
    Trigger,
    /// The gun's reload button (Tomb Raider: the tracker's button).
    Reload,
    /// The player's start button (Rabbids Hollywood).
    Start,
    /// The player's button (Spider-Man Homecoming: its only control).
    Button,
    /// Full gas, whatever the pedal (Hot Wheels: Kings of the Road, Need for Speed).
    Gas,
    /// Hot Wheels: Kings of the Road's boost: the pedal pressed twice, quickly.
    Boost,
    /// Full brake, the nitro button (Need for Speed Heat Takedown).
    Brake,
    Nitro,
    /// The player's joystick and attack button (Rampage).
    StickUp,
    StickDown,
    StickLeft,
    StickRight,
    Attack,
    Coin,
    /// The operator's keypad: the operator key (Skylanders: any key opens and closes the
    /// operator menu), up, down and select (Rabbids Hollywood's menus).
    Operator,
    Up,
    Down,
    Select,
}

const NATIVES: &[(&str, Native)] = &[
    ("touch", Native::Touch),
    ("shield", Native::Shield),
    ("dodge_left", Native::DodgeLeft),
    ("dodge_right", Native::DodgeRight),
    ("trigger", Native::Trigger),
    ("reload", Native::Reload),
    ("start", Native::Start),
    ("button", Native::Button),
    ("stick_up", Native::StickUp),
    ("stick_down", Native::StickDown),
    ("stick_left", Native::StickLeft),
    ("stick_right", Native::StickRight),
    ("attack", Native::Attack),
    ("gas", Native::Gas),
    ("boost", Native::Boost),
    ("brake", Native::Brake),
    ("nitro", Native::Nitro),
    ("coin", Native::Coin),
    ("operator", Native::Operator),
    ("up", Native::Up),
    ("down", Native::Down),
    ("select", Native::Select),
];

/// Skylanders Cloud Patrol, Spider-Man Homecoming (32-bit).
#[cfg(target_pointer_width = "32")]
const DEFAULTS: &[(&str, &str)] = &[("b1", "touch+button"), ("coin", "coin"), ("test", "operator"), ("service", "operator"), ("b2", "select")];

/// Rabbids Hollywood (64-bit).
#[cfg(target_pointer_width = "64")]
const DEFAULTS: &[(&str, &str)] =
    &[("b1", "trigger"), ("start", "start"), ("coin", "coin"), ("test", "operator"), ("service", "select"), ("up", "up"), ("down", "down"), ("b2", "select")];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

fn held(player: usize, native: Native) -> bool {
    let mut on = false;
    map().for_each_pressed(&input(player), |n| on |= n == native);
    on
}

/// The emulated hardware's functions.
fn resolve(name: &str, module: &str) -> Option<usize> {
    // Need for Speed Heat Takedown (HDRP): its render scale, before its first frame
    #[cfg(target_pointer_width = "64")]
    hdrp::init();
    // Hot Wheels: Kings of the Road: no webcam pictures, before its operator settings are read
    #[cfg(target_pointer_width = "64")]
    hotwheels::init();
    // Drakon: Realm Keepers (IL2CPP): its methods replaced at its dongle's start
    #[cfg(target_pointer_width = "64")]
    if module == "matrix64.dll" {
        drakon::init();
    }
    match module {
        #[cfg(target_pointer_width = "32")]
        "kaboomcomdll.dll" => kaboom::resolve(name),
        "kaboomadriofxplusfourfeeders.dll" | "kaboomadriofxplusfourfeeders64.dll" => adrio::resolve(name),
        "kaboomadriofxplustwofeeders.dll" | "kaboomadriofxplustwofeeders64.dll" => adrio2::resolve(name),
        "kaboomadriohotwheels64.dll" => hotwheels::resolve(name),
        "matrix32.dll" | "matrix64.dll" => matrix::resolve(name),
        #[cfg(target_pointer_width = "32")]
        "pqmtclient.dll" => pqmt::resolve(name),
        "sxcore.dll" => sixense::resolve(name),
        #[cfg(target_pointer_width = "64")]
        "rs_232_dll.dll" => drakon::resolve(name),
        #[cfg(target_pointer_width = "64")]
        "wlanapi.dll" => drakon::resolve_wlan(name),
        // Mono's registry (`Win32RegistryApi`, CharSet Unicode)
        "advapi32.dll" if name == "RegOpenKeyExW" => Some(reg_open_key_ex_w as *const () as usize),
        // Need for Speed's steering board: its port listed, Mono's `SerialPort` on it
        // Hot Wheels: its boards' ports listed
        "advapi32.dll" if steering::enabled() || hotwheels::enabled() => steering::resolve_advapi32(name),
        "kernel32.dll" if steering::enabled() => steering::resolve_kernel32(name),
        // Candy Crush Arcade: TouchScript's Windows touch input
        "user32.dll" if mouse::enabled() => mouse::resolve(name),
        // another board's library: the real one (logged)
        m if m.starts_with("kaboom") => {
            wal_payload_common::log!("adrenaline: {m}!{name} not emulated");
            None
        }
        _ => None,
    }
}

/// The machine's serial ports hidden (`SerialPort.GetPortNames` reads them from this key):
/// Spider-Man Homecoming's LED controller library (`LedControllerLib.dll`) opens each port
/// listed and waits for its controller's answer; Wine lists the machine's own ports (Batocera:
/// COM1 = /dev/ttyS0, where nothing answers: the game hung before its first frame, a white
/// window). No port, as on a PC without serial ports: no LED controller. Need for Speed Heat
/// Takedown: its steering board's port only; Hot Wheels: Kings of the Road: its three boards'.
unsafe extern "system" fn reg_open_key_ex_w(key: *mut c_void, sub: *const u16, options: u32, access: u32, result: *mut *mut c_void) -> i32 {
    type F = unsafe extern "system" fn(*mut c_void, *const u16, u32, u32, *mut *mut c_void) -> i32;
    static ORIG: OnceLock<usize> = OnceLock::new();
    if !sub.is_null() {
        let len = (0..).take_while(|&i| unsafe { *sub.add(i) } != 0).count();
        let name = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(sub, len) });
        if name.trim_matches('\\').eq_ignore_ascii_case("HARDWARE\\DEVICEMAP\\SERIALCOMM") {
            if steering::enabled() || hotwheels::enabled() {
                if !result.is_null() {
                    unsafe { *result = steering::PORTS_KEY as *mut c_void };
                }
                // listed again at each of the LED strips and spotlight managers' retries
                static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    wal_payload_common::log!("adrenaline: serial port list: the emulated boards'");
                }
                return 0;
            }
            wal_payload_common::log!("adrenaline: serial port list hidden");
            return 2; // ERROR_FILE_NOT_FOUND
        }
    }
    let orig = *ORIG.get_or_init(|| unsafe {
        let advapi32 = windows_sys::Win32::System::LibraryLoader::LoadLibraryW(windows_sys::w!("advapi32.dll"));
        windows_sys::Win32::System::LibraryLoader::GetProcAddress(advapi32, c"RegOpenKeyExW".as_ptr().cast()).map_or(0, |f| f as usize)
    });
    unsafe { std::mem::transmute::<usize, F>(orig)(key, sub, options, access, result) }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("adrenaline");
        // the profile's code patches (WAL_PATCHES)
        wal_payload_common::patches::apply();
        // Unity 2017: the player is UnityPlayer.dll; Unity 4.2: the executable
        let mut unity = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) } as usize;
        if unity == 0 {
            unity = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
        }
        // Shooty Skies' joysticks: the keys the payload posts
        wal_payload_common::input_filter::init(unity, stick_keys::key_held);
        stick_keys::init();
        // Candy Crush Arcade: the touch screen played with the mouse
        if mouse::enabled() {
            mouse::init(unity);
        }
        screen::init(unity);
        // Infinity Blade FX (Unreal Engine 3): the touch frame's client and the dongle imported
        // by the executable, the board on a serial port; no Mono
        #[cfg(target_pointer_width = "32")]
        let native = pqmt::hook_imports();
        #[cfg(target_pointer_width = "64")]
        let native = false;
        #[cfg(target_pointer_width = "32")]
        if native {
            matrix::hook_ordinals();
            kaboom::init_serial();
        }
        // Jetpack Joyride Arcade (native): the dongle imported, the board resolved at run time
        #[cfg(target_pointer_width = "32")]
        let native = native || jetpack::init();
        if !native {
            // Disney Crossy Road (64-bit): the dump's board and dongle libraries are 32-bit;
            // Flappy Tickets: no dongle library in the dump
            mono_imports::stand_in(&["kaboomadriofxplustwofeeders.dll", "matrix32.dll"]);
            mono_imports::hook(resolve);
        }
        if steering::enabled() {
            steering::init();
        }
        // Skylanders Cloud Patrol: the common crosshair option (Direct3D 9; Rabbids Hollywood
        // draws its own), the board's and the touch frame's threads
        #[cfg(target_pointer_width = "32")]
        {
            wal_payload_common::crosshair::init();
            kaboom::init();
            pqmt::init();
        }
    }
    1
}

// XInput: no pads, the cabinet's inputs are the emulated hardware's

const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;

#[unsafe(no_mangle)]
pub extern "system" fn XInputGetState(_index: u32, _state: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

#[unsafe(no_mangle)]
pub extern "system" fn XInputGetStateEx(_index: u32, _state: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

#[unsafe(no_mangle)]
pub extern "system" fn XInputSetState(_index: u32, _vibration: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

#[unsafe(no_mangle)]
pub extern "system" fn XInputGetCapabilities(_index: u32, _flags: u32, _caps: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

#[unsafe(no_mangle)]
pub extern "system" fn XInputEnable(_enable: i32) {}

#[unsafe(no_mangle)]
pub extern "system" fn XInputGetDSoundAudioDeviceGuids(_index: u32, _render: *mut c_void, _capture: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

#[unsafe(no_mangle)]
pub extern "system" fn XInputGetBatteryInformation(_index: u32, _kind: u8, _info: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}

#[unsafe(no_mangle)]
pub extern "system" fn XInputGetKeystroke(_index: u32, _reserved: u32, _keystroke: *mut c_void) -> u32 {
    ERROR_DEVICE_NOT_CONNECTED
}
