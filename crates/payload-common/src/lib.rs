//! Code shared by every system payload DLL.
//!
//! A payload calls [`start`] once when it is loaded, then reads the current
//! virtual arcade sticks with [`input`] from the game threads (lock free) and
//! reports game outputs with [`send_output`].

// the ABAAS game server (player data, rankings) of Sega's APM3 and ALLS games
pub mod abaas_gs;
// Sega Nu's AMEX board (JVS, DS, EEPROM, SRAM, GPIO) found by its device interface classes
#[cfg(windows)]
pub mod amex;
pub mod amlog;
pub mod packed;
// AVPro Video's uGUI display played by Unity's VideoPlayer (Android builds without AVPro's backend)
#[cfg(windows)]
pub mod avpro;
#[cfg(windows)]
pub mod codepage;
#[cfg(windows)]
pub mod crash;
#[cfg(windows)]
pub mod memdiff;
#[cfg(windows)]
pub mod crypto;
#[cfg(windows)]
pub mod bezel;
#[cfg(windows)]
pub(crate) mod border;
#[cfg(windows)]
mod border_d3d11;
#[cfg(windows)]
mod border_d3d8;
#[cfg(windows)]
pub mod crosshair;
#[cfg(windows)]
pub mod d3d9_quads;
pub mod d3dx;
pub mod desktop;
#[cfg(windows)]
mod crypto_keys;
#[cfg(windows)]
pub mod dinput;
#[cfg(windows)]
pub mod mouse_stick;
#[cfg(windows)]
pub mod drive;
#[cfg(windows)]
pub mod ini;
#[cfg(windows)]
pub mod joystick;
#[cfg(windows)]
pub mod font;
#[cfg(windows)]
pub mod dshow;
#[cfg(windows)]
pub mod exe_env;
#[cfg(windows)]
pub mod glview;
#[cfg(windows)]
pub(crate) mod late_hooks;
// where the players' guns point in the game's window
#[cfg(windows)]
pub mod gun;
// Initial D's 6-speed H shifter from the virtual stick
pub mod hshifter;
#[cfg(windows)]
pub mod iat;
// Unity IL2CPP games' compiled code (GameAssembly.dll's embedding API)
#[cfg(windows)]
pub mod il2cpp;
#[cfg(windows)]
pub mod input_filter;
// JSON values (servers' requests and answers)
pub mod json;
pub mod jvs;
#[cfg(windows)]
pub mod jvs_board;
pub mod log;
pub mod mapping;
// Unity games' managed code (Mono's embedding API)
#[cfg(windows)]
pub mod mono;
// a Unity game's `DllImport`s resolved to the payload's functions
#[cfg(windows)]
pub mod mono_imports;
// the NESiCAxLive launcher's registry settings (NESiCA payload; Type X games reading them)
#[cfg(windows)]
pub mod nesica_registry;
pub mod paths;
#[cfg(windows)]
pub mod patches;
// images (elevator doors, a bezel) over Direct3D 11 pictures
#[cfg(windows)]
pub mod images_d3d11;
// a touch screen's pointer over Direct3D 11 pictures
#[cfg(windows)]
pub mod pointer_d3d11;
// parallel port force feedback boards (inpout32 Out32)
pub mod port_ffb;
#[cfg(windows)]
pub mod serial;
// Sega's Aime NFC card reader on a serial port
#[cfg(windows)]
pub mod sgreader;
#[cfg(windows)]
pub mod screenshot;
#[cfg(windows)]
pub mod sdl;
pub mod unreal_ini;
#[cfg(windows)]
pub mod vfw;
#[cfg(windows)]
pub mod volume;
#[cfg(windows)]
pub mod wasapi;
#[cfg(windows)]
pub mod wcase;
#[cfg(windows)]
pub mod webcam;
#[cfg(windows)]
pub mod window;
#[cfg(windows)]
pub mod workarea;

use std::io::Write;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicI16, AtomicU32, Ordering};
use std::sync::{Mutex, Once};
use std::time::Duration;

use wal_protocol::{Axis, InputFrame, MAX_PLAYERS, Message, Output, StickState, VERSION, env};

struct SharedStick {
    buttons: AtomicU32,
    axes: [AtomicI16; Axis::COUNT],
}

impl SharedStick {
    const fn new() -> Self {
        SharedStick {
            buttons: AtomicU32::new(0),
            axes: [const { AtomicI16::new(0) }; Axis::COUNT],
        }
    }
}

static STICKS: [SharedStick; MAX_PLAYERS] = [const { SharedStick::new() }; MAX_PLAYERS];
static CONNECTED: AtomicBool = AtomicBool::new(false);
static WRITER: Mutex<Option<TcpStream>> = Mutex::new(None);
static START: Once = Once::new();

/// Starts the background connection to the launcher. `name` identifies the system.
/// The payload in a game's helper process (a daemon the game starts or needs beside it): the
/// log (tagged with the process name) and crash reports only, no connection to the launcher
/// (the game has it).
pub fn start_helper(name: &'static str) {
    START.call_once(|| {
        log::init();
        log!("{name}: payload loaded in a helper process");
        #[cfg(windows)]
        crash::init();
    });
}

pub fn start(name: &'static str) {
    START.call_once(|| {
        log::init();
        let port = std::env::var(env::PORT)
            .ok()
            .and_then(|p| p.parse().ok())
            .unwrap_or(wal_protocol::DEFAULT_PORT);
        log!("{name}: payload loaded, launcher port {port}");
        #[cfg(windows)]
        {
            crash::init();
            // this program's own options, before any is read
            exe_env::apply();
            // before the X11 driver starts (its first window or screen query)
            desktop::unmanaged();
            crash::dump_threads_at();
            #[cfg(target_arch = "x86")]
            crash::trace_exit();
            dshow::init();
            if border::config().is_some() { border_d3d11::init(); }
            screenshot::init();
            window::init();
            workarea::init();
            glview::init();
            desktop::init();
            bezel::init();
            font::init();
            sdl::init();
            codepage::init();
            d3dx::init();
            dinput::init();
            ini::init();
            mouse_stick::init();
            joystick::init();
            wcase::init();
            wasapi::init();
            volume::init();
        }
        std::thread::spawn(move || connection_loop(name, port));
    });
}

fn connection_loop(name: &str, port: u16) {
    loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(mut stream) => {
                let _ = stream.set_nodelay(true);
                let hello = Message::Hello { version: VERSION, name: name.to_string() };
                if hello.write_to(&mut stream).is_ok() {
                    log!("connected to launcher");
                    *WRITER.lock().unwrap() = stream.try_clone().ok();
                    CONNECTED.store(true, Ordering::Release);
                    read_loop(&mut stream);
                    CONNECTED.store(false, Ordering::Release);
                    *WRITER.lock().unwrap() = None;
                    store(&InputFrame::default());
                    log!("launcher connection lost");
                }
            }
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

fn read_loop(stream: &mut TcpStream) {
    while let Ok(msg) = Message::read_from(stream) {
        match msg {
            Message::Input(frame) => {
                if frame.players[0].buttons != input(0).buttons {
                    log!("input: P1 buttons {:#06x}", frame.players[0].buttons);
                }
                store(&frame)
            }
            Message::Hello { version, name } => log!("launcher hello: {name} v{version}"),
            Message::Touch(points) => *TOUCHES.lock().unwrap() = points,
            Message::Output(_) | Message::Relaunch { .. } => {}
        }
    }
}

/// The launcher's latest touch points (`Message::Touch`).
static TOUCHES: std::sync::Mutex<Vec<(u16, u16)>> = std::sync::Mutex::new(Vec::new());

/// The fingers on the player's touch surface (the profile's `input.touch`): (x, y), 0 (left /
/// top) to 65535; empty without one.
pub fn touches() -> Vec<(u16, u16)> {
    TOUCHES.lock().unwrap().clone()
}

fn store(frame: &InputFrame) {
    for (shared, p) in STICKS.iter().zip(frame.players.iter()) {
        shared.buttons.store(p.buttons, Ordering::Relaxed);
        for (a, v) in shared.axes.iter().zip(p.axes) {
            a.store(v, Ordering::Relaxed);
        }
    }
}

/// Current virtual stick of `player` (0 based).
pub fn input(player: usize) -> StickState {
    let shared = &STICKS[player];
    let mut s = StickState { buttons: shared.buttons.load(Ordering::Relaxed), ..Default::default() };
    for (v, a) in s.axes.iter_mut().zip(shared.axes.iter()) {
        *v = a.load(Ordering::Relaxed);
    }
    s
}

pub fn connected() -> bool {
    CONNECTED.load(Ordering::Acquire)
}

/// Asks the launcher to start the game again when it exits, with the profile's arguments
/// plus `args`. Dropped while disconnected.
pub fn request_relaunch(args: &str) {
    if let Some(stream) = WRITER.lock().unwrap().as_mut() {
        let _ = stream.write_all(&Message::Relaunch { args: args.to_string() }.encode());
    }
}

/// Started again at the payload's request ([`request_relaunch`]).
pub fn relaunched() -> bool {
    std::env::var(wal_protocol::env::RELAUNCH).is_ok_and(|v| v == "1")
}

/// The common free play option: the system's own variable (`system`, e.g. `WAL_KONAMI_FREEPLAY`)
/// when set, else `WAL_FREEPLAY` (the launcher's default: on; the GUI's Common options and
/// Batocera's FREE PLAY change it); `None` when neither is set (the game's own setting).
pub fn free_play(system: &str) -> Option<bool> {
    let read = |name: &str| std::env::var(name).ok().map(|v| v.trim().to_ascii_lowercase()).filter(|v| !v.is_empty());
    read(system).or_else(|| read("WAL_FREEPLAY")).map(|v| matches!(v.as_str(), "1" | "true" | "on"))
}

/// The common crosshair option, `WAL_CROSSHAIR` (`1`/`0`): the game profile's default, the
/// GUI's or Batocera's choice. Whether the payload shows the players' crosshairs (`default`
/// when unset).
pub fn crosshairs(default: bool) -> bool {
    std::env::var("WAL_CROSSHAIR").map_or(default, |v| matches!(v.trim(), "1" | "true" | "on"))
}

/// Reports a game output to the launcher (dropped while disconnected).
pub fn send_output(player: u8, id: u16, value: i32) {
    if let Some(stream) = WRITER.lock().unwrap().as_mut() {
        let _ = stream.write_all(&Message::Output(Output { player, id, value }).encode());
    }
}
