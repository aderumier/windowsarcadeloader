//! The cabinet's GameShell: coins and credits.
//!
//! On the cabinet `GameShell.exe` starts the game with `/h=<its window, hex>`; without that
//! argument the game looks for the window titled "Game Shell - Windowed" (`0x467ba0`), and
//! without one it gives itself 99 credits and free play. The two talk with `WM_COPYDATA`
//! (`dwData` the message, `lpData` its value):
//!
//! * game -> shell (`0x467720`, a sender thread): 8 the game's window (at start), 4 a credit
//!   spent (a game started), 1 acknowledge, 2 unknown message, 5 6 7 10 11 12 game states
//! * shell -> game (`0x467a20`): 3 the credit count (u32; a higher one plays the coin sound),
//!   9 quit, 13 a coin sound (u32 != 0), 1 / 2 counters
//!
//! The game reads free play itself (`[gamesettings] FreePlay` of `data/gamesettings.ini`) but
//! still starts only with credits: in free play the shell gives 99 and never takes them; else
//! each coin is a credit and each game started takes one.
//!
//! The payload plays the shell: its window (hidden, created when the game looks for it), the
//! coins of player 1's virtual stick.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use wal_payload_common::{iat, log};
use wal_protocol::button;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::DataExchange::COPYDATASTRUCT;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExA, DefWindowProcA, DispatchMessageA, GetMessageA, MSG, RegisterClassA, SMTO_ABORTIFHUNG,
    SendMessageTimeoutA, TranslateMessage, WM_COPYDATA, WNDCLASSA, WS_POPUP,
};

const TITLE: &[u8] = b"Game Shell - Windowed\0";
const FREE_PLAY_CREDITS: u32 = 99;

static SHELL: AtomicIsize = AtomicIsize::new(0);
static GAME: AtomicIsize = AtomicIsize::new(0);
static CREDITS: AtomicU32 = AtomicU32::new(0);
static FREE_PLAY: AtomicBool = AtomicBool::new(false);
static EVENTS: OnceLock<Mutex<Sender<Event>>> = OnceLock::new();
static ORIG_FIND_WINDOW: AtomicUsize = AtomicUsize::new(0);

enum Event {
    /// A message from the game: (dwData, its u32 value).
    Message(usize, u32),
    Coin,
}

fn post(e: Event) {
    if let Some(tx) = EVENTS.get() {
        let _ = tx.lock().unwrap().send(e);
    }
}

/// Sends a message to the game's window (the shell's worker thread: the game's window thread
/// answers it).
fn send(id: usize, value: u32) {
    let game = GAME.load(Ordering::Relaxed);
    if game == 0 {
        return;
    }
    let cds = COPYDATASTRUCT { dwData: id, cbData: 4, lpData: &value as *const u32 as *mut _ };
    let mut result = 0usize;
    let ok = unsafe {
        SendMessageTimeoutA(
            game as HWND,
            WM_COPYDATA,
            SHELL.load(Ordering::Relaxed) as WPARAM,
            &cds as *const COPYDATASTRUCT as LPARAM,
            SMTO_ABORTIFHUNG,
            5000,
            &mut result,
        )
    };
    if ok == 0 {
        log!("shell: message {id} ({value}) to the game: no answer");
    }
}

fn set_credits(credits: u32) {
    CREDITS.store(credits, Ordering::Relaxed);
    log!("shell: {credits} credits");
    send(3, credits);
}

/// The worker: the game's messages, the coins.
fn worker(rx: std::sync::mpsc::Receiver<Event>) {
    for e in rx {
        match e {
            Event::Message(8, hwnd) => {
                log!("shell: the game's window {hwnd:#x}");
                GAME.store(hwnd as isize, Ordering::Relaxed);
                let credits = if FREE_PLAY.load(Ordering::Relaxed) { FREE_PLAY_CREDITS } else { CREDITS.load(Ordering::Relaxed) };
                set_credits(credits);
            }
            Event::Message(4, _) => {
                if FREE_PLAY.load(Ordering::Relaxed) {
                    log!("shell: game started (free play)");
                } else {
                    log!("shell: game started");
                    set_credits(CREDITS.load(Ordering::Relaxed).saturating_sub(1));
                }
            }
            Event::Message(1, _) => {}
            Event::Message(id, value) => log!("shell: game message {id} ({value:#x})"),
            Event::Coin => {
                if FREE_PLAY.load(Ordering::Relaxed) {
                    log!("shell: coin (free play)");
                } else {
                    set_credits((CREDITS.load(Ordering::Relaxed) + 1).min(FREE_PLAY_CREDITS));
                }
            }
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_COPYDATA && lparam != 0 {
        let cds = unsafe { &*(lparam as *const COPYDATASTRUCT) };
        let value = if cds.cbData >= 4 && !cds.lpData.is_null() { unsafe { (cds.lpData as *const u32).read_unaligned() } } else { 0 };
        post(Event::Message(cds.dwData, value));
        return 1;
    }
    unsafe { DefWindowProcA(hwnd, msg, wparam, lparam) }
}

/// The shell's window, created by its own thread (which pumps its messages).
fn window() -> isize {
    static CREATE: std::sync::Once = std::sync::Once::new();
    CREATE.call_once(|| {
        let (tx, rx) = channel();
        std::thread::spawn(move || unsafe {
            let class = b"WalGameShell\0";
            let wc = WNDCLASSA { lpfnWndProc: Some(wndproc), lpszClassName: class.as_ptr(), ..std::mem::zeroed() };
            RegisterClassA(&wc);
            let hwnd = CreateWindowExA(0, class.as_ptr(), TITLE.as_ptr(), WS_POPUP, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
            let _ = tx.send(hwnd as isize);
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageA(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageA(&msg);
            }
        });
        let hwnd = rx.recv_timeout(Duration::from_secs(5)).unwrap_or(0);
        SHELL.store(hwnd, Ordering::Relaxed);
        log!("shell: window {hwnd:#x}");
    });
    SHELL.load(Ordering::Relaxed)
}

type FindWindowA = unsafe extern "system" fn(*const u8, *const u8) -> isize;

unsafe extern "system" fn find_window(class: *const u8, title: *const u8) -> isize {
    if class.is_null() && !title.is_null() && unsafe { std::ffi::CStr::from_ptr(title.cast()) }.to_bytes_with_nul() == TITLE {
        return window();
    }
    let f: FindWindowA = unsafe { std::mem::transmute(ORIG_FIND_WINDOW.load(Ordering::Relaxed)) };
    unsafe { f(class, title) }
}

/// The game's free play setting (`settings.rs`).
pub(crate) fn set_free_play(on: bool) {
    FREE_PLAY.store(on, Ordering::Relaxed);
}

pub(crate) fn init() {
    let (tx, rx) = channel();
    let _ = EVENTS.set(Mutex::new(tx));
    std::thread::spawn(move || worker(rx));
    unsafe {
        if let Some(o) = iat::hook("user32.dll", "FindWindowA", find_window as *const () as usize) {
            ORIG_FIND_WINDOW.store(o, Ordering::Relaxed);
        }
    }
    // the coin switch
    std::thread::spawn(|| {
        let mut held = false;
        loop {
            let coin = wal_payload_common::input(0).pressed(button::COIN);
            if coin && !held {
                post(Event::Coin);
            }
            held = coin;
            std::thread::sleep(Duration::from_millis(5));
        }
    });
}
