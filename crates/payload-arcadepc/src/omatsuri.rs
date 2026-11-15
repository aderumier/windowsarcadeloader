//! Omatsuri Yasan: Kingyo Sukui (2009) and Ganso Takoyaki (2010) (Cave's kids' touch screen
//! games, `cvgame.exe`, Direct3D 9, 32-bit): `WAL_ARCADEPC_BOARD=omatsuri`.
//!
//! The touch screen is the Windows mouse: its position the cursor's (`GetCursorPos`), its
//! touches the DirectInput system mouse's left button, read as buffered events
//! (`GetDeviceData`; the fake DirectInput, `WAL_DINPUT_DISABLE=1`, its keyboard silent: the
//! game's debug keys). The cabinet's I/O board is on COM3 (9600 baud), 4-byte ASCII messages,
//! the game reading one byte at a time:
//!
//! - **Start.** The game writes `&***` and reads 9 bytes: `&` and the board's 8 digit serial
//!   number (`0`-`9`).
//! - **Board to PC**, sent when the inputs change: `#XY ` with `XY` the inputs' byte in hex,
//!   active low (`EF` the coin, `FF` nothing): bit 4 the coin (counted when released), bit 3
//!   the test switch (held 3 s: the test menu, restarted when held there), bit 2 a button, bit 1
//!   the service switch (Kingyo Sukui: a credit when released). In the test menus, on release
//!   (Kingyo Sukui, 0x4228d0): the button enters / confirms, service moves down, test up.
//!   Ganso Takoyaki's board also says `@***`: the prize dispenser paid out.
//! - **PC to board**: `%15n` the coin meter, `%16n` Ganso Takoyaki's prize dispenser (`1` pay
//!   one out, answered `@***` when done, `0` stop). The game's own debug keys (X, Q, A, Z) are
//!   the same messages.
//!
//! Natives: `touch` (the cursor's left button), `button`, `coin`, `service`, `test`.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use wal_payload_common::input_filter::{own_mouse, post_mouse, post_raw_buttons};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{desktop, dinput, gun, input, log, serial};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};

const PORT: &str = "COM3";
/// The board's serial number (8 digits).
const SERIAL: &[u8; 8] = b"20090417";

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// A bit of the inputs' byte (set: pressed).
    Bit(u8),
    /// The test switch and the button: their bits are each game's ([`Game::test`]).
    Test,
    Button,
    Touch,
}

const COIN: u8 = 0x10;

const NATIVES: &[(&str, Native)] = &[
    ("coin", Native::Bit(COIN)),
    ("test", Native::Test),
    ("button", Native::Button),
    ("service", Native::Bit(0x02)),
    ("touch", Native::Touch),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("coin", "coin"),
    ("test", "test"),
    ("service", "service"),
    ("b2", "button"),
    ("b1", "touch"),
    ("start", "touch"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// How often the board looks at its inputs.
const PERIOD: Duration = Duration::from_millis(8);
/// The prize dispenser's pay out.
const PAY_OUT: Duration = Duration::from_millis(500);

struct Board {
    /// The start sequence answered: inputs may be sent.
    started: bool,
    last: Option<Instant>,
    /// The inputs sent (the game's state, all released at first).
    sent: u8,
    /// A prize being paid out since.
    paying: Option<Instant>,
}

static BOARD: Mutex<Board> = Mutex::new(Board { started: false, last: None, sent: 0, paying: None });

fn message(inputs: u8) -> Vec<u8> {
    format!("#{:02X} ", !inputs).into_bytes()
}

/// The messages of the inputs that changed, when the game reads the port.
fn idle() -> Vec<u8> {
    let mut board = BOARD.lock().unwrap();
    if !board.started || board.last.is_some_and(|t| t.elapsed() < PERIOD) {
        return Vec::new();
    }
    board.last = Some(Instant::now());
    let mut out = Vec::new();
    if board.paying.is_some_and(|t| t.elapsed() >= PAY_OUT) {
        board.paying = None;
        log!("omatsuri: prize paid out");
        out.extend(*b"@***");
    }
    let (test, button) = game().map_or((0x08, 0x04), |g| g.test);
    let mut now = 0u8;
    map().for_each_pressed(&input(0), |n| match n {
        Native::Bit(bit) => now |= bit,
        Native::Test => now |= test,
        Native::Button => now |= button,
        Native::Touch => {}
    });
    if now != board.sent {
        out.extend(message(now));
        board.sent = now;
    }
    out
}

fn handler(packet: &[u8]) -> Vec<u8> {
    match packet {
        [b'&', ..] => {
            BOARD.lock().unwrap().started = true;
            log!("omatsuri: board started");
            [&b"&"[..], SERIAL].concat()
        }
        [b'%', b'1', b'6', b'1', ..] => {
            BOARD.lock().unwrap().paying = Some(Instant::now());
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// `wParam`'s left button flag.
const MK_LBUTTON: usize = 0x0001;
const RI_MOUSE_LEFT_BUTTON_DOWN: u16 = 0x0001;
const RI_MOUSE_LEFT_BUTTON_UP: u16 = 0x0002;

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

fn touching() -> bool {
    let mut touch = false;
    map().for_each_pressed(&input(0), |n| touch |= n == Native::Touch);
    touch
}

/// The DirectInput system mouse's buttons: the touch its left button.
fn mouse_buttons() -> u8 {
    touching() as u8
}

/// The touch screen: the gun (or mouse) is the cursor, the touch its left button.
fn touch_thread() {
    let (mut touch_was, mut last) = (false, (i32::MIN, i32::MIN));
    let mut tick = 0u32;
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = gun::find_window();
        if hwnd.is_null() {
            continue;
        }
        let touch = touching();
        let (w, h) = desktop::size().unwrap_or_else(|| {
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            unsafe { GetClientRect(hwnd, &mut r) };
            (r.right, r.bottom)
        });
        let at = match gun::gun_of_player_in_picture(0) {
            Some((x, y)) => ((x * w as f32) as i32, (y * h as f32) as i32),
            None => last,
        };
        let buttons = if touch { MK_LBUTTON } else { 0 };
        // again every half second: a cursor that did not move still reaches a new screen
        tick = (tick + 1) % 64;
        if (at != last || tick == 0) && at.0 != i32::MIN {
            post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, buttons);
            last = at;
        }
        if touch != touch_was && last.0 != i32::MIN {
            button(hwnd, touch, last);
        }
        touch_was = touch;
    }
}

/// Where a game keeps its credits.
enum Credits {
    /// A global.
    At(usize),
    /// A field of the I/O board object (its global pointer, the field's offset).
    Board(usize, usize),
}

/// A game: its machine check (`cmp dword [flag], 1` at its start, and the flag) and its
/// credits (a coin adds one, a game spends them).
struct Game {
    name: &'static str,
    check: usize,
    flag: usize,
    credits: Credits,
    /// The bits of the test switch and of the button: Kingyo Sukui's test menu opens on bit 3
    /// held 3 s, Ganso Takoyaki's on bit 2 at once (its IO TEST: TEST SW bit 2, TOUCHPANEL SW
    /// bit 3, "up" in its menus).
    test: (u8, u8),
}

const GAMES: &[Game] = &[
    // the credits: the game's (the scene's +0x2640, the application's +0x8780), added by the
    // board's coin callback (0x4064f0), spent at the start (0x4204d6)
    Game { name: "Kingyo Sukui", check: 0x4141cc, flag: 0x5988d4, credits: Credits::At(0x5986b8), test: (0x08, 0x04) },
    // the credits: the board's +0xf4, added by its coin callback (0x401600)
    Game { name: "Ganso Takoyaki", check: 0x40b6be, flag: 0x49b6fc, credits: Credits::Board(0x49b6e4, 0xf4), test: (0x04, 0x08) },
];

/// The running game, by the bytes of its machine check (the executable is not relocated).
fn game() -> Option<&'static Game> {
    GAMES.iter().find(|g| {
        let expected = [&[0x83u8, 0x3d][..], &(g.flag as u32).to_le_bytes(), &[1]].concat();
        let found = unsafe { std::slice::from_raw_parts(g.check as *const u8, expected.len()) };
        found == expected.as_slice()
    })
}

/// The machine check passed: its flag set (the check returns at once when it is). The check
/// reads `common.log` (32 bytes: the network adapter's address and the board's serial number,
/// written at the first boot from a blank template the dump lacks): without it, it tries 20
/// times a second apart, then error 11 (a black screen).
fn machine_check(game: &Game) {
    unsafe { (game.flag as *mut u32).write_volatile(1) };
    log!("omatsuri: {}: machine check passed", game.name);
}

/// Free play (the common `WAL_FREEPLAY`; the games have no free play setting): the game's
/// credits kept at 1 at least, so the title always asks for a touch. Off: the coins.
fn free_play(game: &'static Game) {
    if wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") != Some(true) {
        return;
    }
    std::thread::spawn(move || {
        // after the board's start sequence: credits there before it, the game skipped it (no
        // board, no coins)
        while !BOARD.lock().unwrap().started {
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut logged = false;
        loop {
            std::thread::sleep(Duration::from_millis(100));
            let credits = match game.credits {
                Credits::At(at) => at,
                Credits::Board(board, field) => match unsafe { (board as *const usize).read_volatile() } {
                    0 => continue,
                    board => board + field,
                },
            } as *mut i32;
            if unsafe { credits.read_volatile() } <= 0 {
                unsafe { credits.write_volatile(1) };
                if !logged {
                    log!("omatsuri: free play (credits kept at 1)");
                    logged = true;
                }
            }
        }
    });
}

pub fn init() {
    match game() {
        Some(game) => {
            machine_check(game);
            free_play(game);
        }
        None => log!("omatsuri: unknown executable (no machine check, no free play)"),
    }
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    wal_payload_common::input_filter::init(exe, |_| false);
    own_mouse(exe);
    dinput::set_keyboard(|_| {});
    dinput::set_mouse_buttons(mouse_buttons);
    // the touch's pointer (the common crosshair option; the profile: the dump's P1.png)
    wal_payload_common::crosshair::init();
    std::thread::spawn(touch_thread);
    serial::install_streaming(PORT, handler, idle);
    log!("omatsuri: I/O board on {PORT}, the touch screen the mouse");
}
