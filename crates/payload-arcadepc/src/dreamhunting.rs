//! Dream Hunting (`DreamGun.exe`, DirectDraw / Direct3D immediate mode 640x480, 32-bit):
//! `WAL_ARCADEPC_BOARD=dreamhunting`, x86 build.
//!
//! A one-player hunting game: a gun aims and shoots, an 8-way joystick walks (forward / back)
//! and turns (left / right). The cabinet's board is on COM1 (9600 bauds, opened at
//! `0x42c490`); it only talks: the game's reader thread (`0x42c000`) waits for a character
//! (`WaitCommEvent`), reads what is queued (`ClearCommError`'s count, up to 80 bytes) and hands
//! each read to the parser (`0x42c240`) as one message, which becomes a window message to the
//! game (`0x401`..`0x403`, `0x411c90` / `0x42fc80`):
//!
//! - `XY` + 4 hex digits x + 4 hex digits y + `\r\n`: where a shot landed, the gun's raw sensor
//!   position, sent after the trigger's `KT` (the parser gathers reads until the `\n`; each one
//!   is a shot at that place: a bullet spent, the map's arena chosen, so it is sent only then,
//!   none off the picture: the trigger alone, the empty gun's reload). The game maps it to the
//!   640x480 picture with
//!   `DREAMGUN.INI`'s `[gun]` calibration (`0x42fc80`: x wraps at `xmax`, from `left` to
//!   `right`; y from `min` to `max`; `xcal` / `ycal` offsets): the payload sends the inverse of
//!   the calibration the game holds, so the shot lands where the launcher's gun points.
//! - `KT` the trigger (the title's start, the menus' select), `KE` a coin, `KS` the test menu
//!   (in it: back), `KD` a service credit (and back to the title from the attract), `KB` /
//!   `KC` the zoom on / off (slower turns), `K0`..`K7` the joystick (right, left, back,
//!   forward, back right, back left, forward left, forward right): the game walks while the
//!   last one is under 300 ms old, so a held direction is repeated every 50 ms (in the test
//!   menu, where each message moves the cursor, once per press, repeated after 0.4 s).
//!
//! Each message is given when the game polls an empty port (`serial::install_streaming`'s idle
//! data), one per read: two messages in one read would be one for the parser.
//!
//! The game's own development inputs (window mouse messages, `GetAsyncKeyState` arrows / WASD,
//! Ctrl+Shift+Z the test menu) are kept from the real devices by `payload-common`'s
//! `input_filter`.
//!
//! **Free play** (`WAL_FREEPLAY`, default on): the game has none (its test menu's coins per
//! credit go from 1 to 99): the payload keeps the coin count (`0x674864`) at the price of a game
//! and of a continue (`SERVICE.INI`'s `credits` / `c_credits`), the title then asks for the
//! trigger.
//!
//! Natives: `trigger zoom coin service test forward back left right`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log, serial};
use wal_protocol::Axis;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

const PORT: &str = "COM1";

/// The game's globals (image base 0x400000, no relocations).
const STATE: usize = 0x575630;
const COINS: usize = 0x674864;
const PRICE: usize = 0x456634;
const CONTINUE_PRICE: usize = 0x456638;
/// The picture's size (640x480).
const WIDTH: usize = 0x6638d0;
const HEIGHT: usize = 0x6638d4;
/// `DREAMGUN.INI` `[gun]`: xmax, left, right, min, max, xcal, ycal.
const XMAX: usize = 0x4565a4;
const LEFT: usize = 0x4565a8;
const RIGHT: usize = 0x575678;
const YMIN: usize = 0x4565ac;
const YMAX: usize = 0x4565b0;
const XCAL: usize = 0x57567c;
const YCAL: usize = 0x575680;
/// The last shot's place in the picture.
const SHOT_X: usize = 0x671eb4;
const SHOT_Y: usize = 0x671eb8;
/// The game's states: the test menu.
const STATE_TEST: i32 = 7;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Zoom,
    Coin,
    Service,
    Test,
    Forward,
    Back,
    Left,
    Right,
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("zoom", Native::Zoom),
    ("coin", Native::Coin),
    ("service", Native::Service),
    ("test", Native::Test),
    ("forward", Native::Forward),
    ("back", Native::Back),
    ("left", Native::Left),
    ("right", Native::Right),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("b2", "zoom"),
    ("start", "trigger"),
    ("coin", "coin"),
    ("service", "service"),
    ("test", "test"),
    ("up", "forward"),
    ("down", "back"),
    ("left", "left"),
    ("right", "right"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The real screen's size, before the game's mode change (width << 16 | height).
static SCREEN: AtomicU32 = AtomicU32::new(0);
static FREE_PLAY: AtomicBool = AtomicBool::new(false);

fn read(address: usize) -> i32 {
    unsafe { *(address as *const i32) }
}

/// The board's state between polls.
struct Board {
    queue: VecDeque<Vec<u8>>,
    trigger: bool,
    zoom: bool,
    coin: bool,
    service: bool,
    test: bool,
    /// The direction held, when it was last sent, when it was pressed.
    direction: Option<u8>,
    direction_sent: Option<Instant>,
    direction_pressed: Option<Instant>,
    /// A shot's position sent: the game's logged at the next poll.
    shot: bool,
}

static BOARD: Mutex<Board> = Mutex::new(Board {
    queue: VecDeque::new(),
    trigger: false,
    zoom: false,
    coin: false,
    service: false,
    test: false,
    direction: None,
    direction_sent: None,
    direction_pressed: None,
    shot: false,
});

/// The 4:3 picture's place on a `sw` x `sh` screen (fractions: x, y, width, height): the
/// largest centered rectangle, as the game's 640x480 mode is scaled to the screen.
fn picture(sw: f32, sh: f32) -> [f32; 4] {
    if sw <= 0.0 || sh <= 0.0 {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let (w, h) = if sw * 3.0 > sh * 4.0 { (sh * 4.0 / 3.0 / sw, 1.0) } else { (1.0, sw * 3.0 / 4.0 / sh) };
    [(1.0 - w) / 2.0, (1.0 - h) / 2.0, w, h]
}

/// The gun's place in the picture (fractions), `None` off it.
fn aim() -> Option<(f32, f32)> {
    let stick = input(0);
    let f = |v: i16| (v as f32 + 32768.0) / 65535.0;
    let screen = SCREEN.load(Ordering::Relaxed);
    let [px, py, w, h] = picture((screen >> 16) as f32, (screen & 0xFFFF) as f32);
    let (x, y) = ((f(stick.axis(Axis::LeftX)) - px) / w, (f(stick.axis(Axis::LeftY)) - py) / h);
    ((0.0..1.0).contains(&x) && (0.0..1.0).contains(&y)).then_some((x, y))
}

/// The raw sensor position the game maps to the picture's `(x, y)` (fractions), with the
/// calibration it holds (the inverse of `0x42fc80`).
fn raw((x, y): (f32, f32)) -> (i32, i32) {
    let (w, h) = (read(WIDTH).max(1), read(HEIGHT).max(1));
    let (xmax, left, right) = (read(XMAX), read(LEFT), read(RIGHT));
    let (ymin, ymax) = (read(YMIN), read(YMAX));
    let sx = (x * w as f32) as i32 - read(XCAL);
    let sy = (y * h as f32) as i32 - read(YCAL);
    let span = right - left + xmax;
    let mut rx = sx * span / w + left;
    if rx >= xmax {
        rx -= xmax;
    }
    let ry = sy * (ymax - ymin) / h + ymin;
    (rx.clamp(0, 0xFFFF), ry.clamp(0, 0xFFFF))
}

fn gun_message((x, y): (i32, i32)) -> Vec<u8> {
    format!("XY{x:04X}{y:04X}\r\n").into_bytes()
}

/// The joystick's message digit: right, left, back, forward, back right, back left, forward
/// left, forward right.
fn direction(forward: bool, back: bool, left: bool, right: bool) -> Option<u8> {
    let v = if forward && !back { 1 } else if back && !forward { -1 } else { 0 };
    let h = if right && !left { 1 } else if left && !right { -1 } else { 0 };
    Some(match (v, h) {
        (0, 1) => b'0',
        (0, -1) => b'1',
        (-1, 0) => b'2',
        (1, 0) => b'3',
        (-1, 1) => b'4',
        (-1, -1) => b'5',
        (1, -1) => b'6',
        (1, 1) => b'7',
        _ => return None,
    })
}

/// The board's next message, when the game polls an empty port.
fn idle() -> Vec<u8> {
    let mut b = BOARD.lock().unwrap();
    let now = Instant::now();
    let (mut trigger, mut zoom, mut coin, mut service, mut test) = (false, false, false, false, false);
    let (mut forward, mut back, mut left, mut right) = (false, false, false, false);
    map().for_each_pressed(&input(0), |n| match n {
        Native::Trigger => trigger = true,
        Native::Zoom => zoom = true,
        Native::Coin => coin = true,
        Native::Service => service = true,
        Native::Test => test = true,
        Native::Forward => forward = true,
        Native::Back => back = true,
        Native::Left => left = true,
        Native::Right => right = true,
    });
    if trigger && !b.trigger {
        // the shot, then where the gun points (none off the picture)
        b.queue.push_back(b"KT".to_vec());
        if let Some(a) = aim() {
            let r = raw(a);
            log!("dreamhunting: shot at ({:.3}, {:.3}), raw {r:?}", a.0, a.1);
            b.queue.push_back(gun_message(r));
            b.shot = true;
        }
    }
    b.trigger = trigger;
    if zoom != b.zoom {
        b.queue.push_back(if zoom { b"KB".to_vec() } else { b"KC".to_vec() });
    }
    b.zoom = zoom;
    if coin && !b.coin {
        log!("dreamhunting: coin");
        b.queue.push_back(b"KE".to_vec());
    }
    b.coin = coin;
    if service && !b.service {
        log!("dreamhunting: service");
        b.queue.push_back(b"KD".to_vec());
    }
    b.service = service;
    if test && !b.test {
        log!("dreamhunting: test");
        b.queue.push_back(b"KS".to_vec());
    }
    b.test = test;
    if FREE_PLAY.load(Ordering::Relaxed) {
        let price = read(PRICE).max(read(CONTINUE_PRICE)).max(1);
        if read(COINS) < price {
            unsafe { *(COINS as *mut i32) = price };
        }
    }
    if let Some(message) = b.queue.pop_front() {
        return message;
    }
    if std::mem::take(&mut b.shot) {
        log!("dreamhunting: the game's shot at ({}, {})", read(SHOT_X), read(SHOT_Y));
    }
    // the joystick: repeated while held (the test menu: once per press, then every 0.15 s
    // after 0.4 s)
    let dir = direction(forward, back, left, right);
    if dir != b.direction {
        b.direction = dir;
        b.direction_sent = None;
        b.direction_pressed = Some(now);
    }
    if let Some(d) = dir {
        let since = |t: Option<Instant>| t.map_or(u128::MAX, |t| now.duration_since(t).as_millis());
        let due = if read(STATE) == STATE_TEST {
            b.direction_sent.is_none() || (since(b.direction_pressed) >= 400 && since(b.direction_sent) >= 150)
        } else {
            since(b.direction_sent) >= 50
        };
        if due {
            b.direction_sent = Some(now);
            return vec![b'K', d];
        }
    }
    Vec::new()
}

/// The game writes nothing to the board.
fn handler(_packet: &[u8]) -> Vec<u8> {
    Vec::new()
}

pub fn init() {
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    SCREEN.store((sw as u32) << 16 | (sh as u32 & 0xFFFF), Ordering::Relaxed);
    if unsafe { std::slice::from_raw_parts(0x459090 as *const u8, 6) } != b"COM%d\0" {
        log!("dreamhunting: unknown DreamGun.exe build, no inputs");
        return;
    }
    let free = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") != Some(false);
    FREE_PLAY.store(free, Ordering::Relaxed);
    serial::install_streaming(PORT, handler, idle);
    log!("dreamhunting: the gun board on {PORT} ({sw}x{sh} screen), free play {}", if free { "on" } else { "off" });
}

#[cfg(test)]
mod tests {
    use super::direction;

    #[test]
    fn directions() {
        assert_eq!(direction(true, false, false, false), Some(b'3'));
        assert_eq!(direction(false, true, true, false), Some(b'5'));
        assert_eq!(direction(true, false, false, true), Some(b'7'));
        assert_eq!(direction(false, false, false, false), None);
    }
}
