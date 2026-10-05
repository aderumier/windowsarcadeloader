//! Code shared by every system payload DLL.
//!
//! A payload calls [`start`] once when it is loaded, then reads the current
//! virtual arcade sticks with [`input`] from the game threads (lock free) and
//! reports game outputs with [`send_output`].

#[cfg(windows)]
pub mod codepage;
#[cfg(windows)]
pub mod crash;
#[cfg(windows)]
pub mod drive;
#[cfg(windows)]
pub mod font;
#[cfg(windows)]
pub mod dshow;
#[cfg(windows)]
pub mod iat;
pub mod jvs;
pub mod log;
pub mod mapping;
pub mod paths;
#[cfg(windows)]
pub mod patches;
#[cfg(windows)]
pub mod serial;
#[cfg(windows)]
pub mod screenshot;
#[cfg(windows)]
pub mod sdl;
#[cfg(windows)]
pub mod window;

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
            dshow::init();
            screenshot::init();
            window::init();
            font::init();
            sdl::init();
            codepage::init();
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
            Message::Output(_) => {}
        }
    }
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

/// Reports a game output to the launcher (dropped while disconnected).
pub fn send_output(player: u8, id: u16, value: i32) {
    if let Some(stream) = WRITER.lock().unwrap().as_mut() {
        let _ = stream.write_all(&Message::Output(Output { player, id, value }).encode());
    }
}
