//! Bug Busters (MIZI Production, 2001; `BBPC.exe`, the PC release of the arcade game, DirectDraw
//! 640x480, 32-bit): `WAL_ARCADEPC_BOARD=bugbusters`, x86 build.
//!
//! - **Inputs.** The PC build plays each player's gun from the mouse (the window's mouse
//!   messages: `0x4554e0`'s player), the DirectInput keyboard or a joystick (`control.cfg`'s
//!   device per player): they all end in the players' records, `0x45fba8` + 0x1c per player
//!   (x 0..640, y 0..480, the trigger held), the spray's tank (`0x45509c`, a float per player,
//!   1.0 full: a reload press adds 0.25, its mouse button's code). The arcade cabinet's serial
//!   boards are dead code here (their reader is a `ret`), but the frame's switch check
//!   (`0x40a872`) still takes the I/O board's flags: `0x49e880` / `0x49ea84` the start buttons,
//!   `0x49e870` the test switch (the setup menu at the next screen change; in it, the next
//!   item), `0x49732c` the service switch in the setup menu (the item's next value, or its
//!   action). The coins (`0x460c1c`) go up by one per coin, by the price (`0x455090`,
//!   `BUSTER.CFG`'s first value, 0 free play) per service credit, with the coin / credit sounds
//!   of the board's reader.
//! - The payload writes them from the virtual sticks in the game's thread, before each frame
//!   (the main loop's `Sleep(0)`, an import hook): each gun's place in the picture (the 640x480
//!   mode is scaled to the screen by Wine, 4:3 kept, centered: the screen's size is read before
//!   the game's mode change), its trigger (released off the picture), a reload press (or a
//!   trigger off the picture) fills the tank in the game and the shop (modes 5 / 9), the flags
//!   at the buttons' presses. A start without a credit is dropped: the title's first phase
//!   kept the flag until a coin, which then started a game at once. The title screen has no
//!   attract cycle of its own in free play (it waits for a start): the test switch fades it out
//!   as a start does (`0x4409cd`), its exit opens the setup menu.
//!   `payload-common`'s `input_filter` keeps the real mouse and keyboard away (the game's window
//!   messages: the mouse player's moves and clicks, `1` / `2` its start keys, `Enter` a start
//!   with a free coin, `Esc` its exit); its DirectInput is the fake one, without devices
//!   (`WAL_DINPUT_DISABLE`).
//! - **Free play** (`WAL_FREEPLAY`, default on): the price read from `BUSTER.CFG` (`0x411faa`)
//!   and from the setup menu at its exit (`0x43e9b4`) is 0, the game's own free play ("FREE
//!   PLAY", starts without coins); the setup still shows and saves the operator's price.
//! - **Crosshair.** The game draws each player's own pointer (`Image\Setup\pointer.dds`); the
//!   common crosshair option off (`WAL_CROSSHAIR=0`) hides it in the game (`0x4263c1`, as the
//!   game does for a joystick player), the menus keep it.
//! - `WAL_BUGBUSTERS_TRACE=1`: the mode, coins, players' records and tanks logged each second.
//!
//! Natives: `trigger reload offscreen start coin` (each player), `test service`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, input, log};
use wal_protocol::Axis;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, GetWindowThreadProcessId, SM_CXSCREEN, SM_CYSCREEN};

const PLAYERS: usize = 2;

/// Relative to the executable (image base 0x400000).
const WINDOW: usize = 0x60BF0;
const MODE: usize = 0x60BF8;
/// Player 1's record (x, y, trigger), player 2's 0x1c further.
const RECORD: usize = 0x5FBA8;
const RECORD_SIZE: usize = 0x1C;
const TANK: usize = 0x5509C;
const COINS: usize = 0x60C1C;
const PRICE: usize = 0x55090;
const COIN_SOUND: usize = 0x60C60;
const CREDIT_SOUND: usize = 0x60C64;
/// `play(sound, 0)` (cdecl).
const PLAY: usize = 0x4501D;
/// The I/O board's flags: start buttons, test switch, the setup's select button.
const START_FLAGS: [usize; PLAYERS] = [0x9E880, 0x9EA84];
const TEST_FLAG: usize = 0x9E870;
const SELECT_FLAG: usize = 0x9732C;
/// The title screen's state (1: shown), its fade out started, the fade (`fade(1)` starts it).
const TITLE_STATE: usize = 0xC8EB0;
const TITLE_FADING: usize = 0xC8F00;
const FADE: usize = 0x12C12;
/// Game modes: the title, in the game, the shop between stages, the setup menu.
const MODE_TITLE: u32 = 1;
const MODE_GAME: u32 = 5;
const MODE_SHOP: u32 = 9;
const MODE_SETUP: u32 = 12;

/// The price's reads (`mov edx, [0x464a78]`, `mov eax, [0x456f0c]`), made 0 for free play.
const PRICE_READS: [(usize, &[u8], &[u8]); 2] = [
    (0x11FAA, &[0x8B, 0x15, 0x78, 0x4A, 0x46, 0x00], &[0x31, 0xD2, 0x90, 0x90, 0x90, 0x90]),
    (0x3E9B4, &[0xA1, 0x0C, 0x6F, 0x45, 0x00], &[0x31, 0xC0, 0x90, 0x90, 0x90]),
];
/// The in-game pointer's skip for joystick players (`je`): never skipped by it.
const POINTER_CHECK: (usize, &[u8], &[u8]) = (0x263C1, &[0x74, 0x0B], &[0x90, 0x90]);

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Reload,
    Offscreen,
    Start,
    Coin,
    Test,
    Service,
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("reload", Native::Reload),
    ("offscreen", Native::Offscreen),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("test", Native::Test),
    ("service", Native::Service),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("b2", "reload"),
    ("b6", "offscreen"),
    ("start", "start"),
    ("coin", "coin"),
    ("test", "test"),
    ("service", "service"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

static EXE: AtomicUsize = AtomicUsize::new(0);
/// The real screen's size, before the game's mode change (width << 16 | height).
static SCREEN: AtomicU32 = AtomicU32::new(0);
static SLEEP: AtomicUsize = AtomicUsize::new(0);
/// The game window's thread (0: not known yet).
static MAIN_THREAD: AtomicU32 = AtomicU32::new(0);

fn at<T>(offset: usize) -> *mut T {
    (EXE.load(Ordering::Relaxed) + offset) as *mut T
}

/// The players' buttons held at the last frame.
struct Held {
    reload: [bool; PLAYERS],
    start: [bool; PLAYERS],
    coin: [bool; PLAYERS],
    test: bool,
    service: bool,
}

static mut HELD: Held = Held { reload: [false; PLAYERS], start: [false; PLAYERS], coin: [false; PLAYERS], test: false, service: false };

/// The 4:3 picture's place on a `sw` x `sh` screen (fractions: x, y, width, height): the
/// largest centered rectangle, as Wine scales the game's 640x480 mode to the screen.
fn picture(sw: f32, sh: f32) -> [f32; 4] {
    if sw <= 0.0 || sh <= 0.0 {
        return [0.0, 0.0, 1.0, 1.0];
    }
    let (w, h) = if sw * 3.0 > sh * 4.0 { (sh * 4.0 / 3.0 / sw, 1.0) } else { (1.0, sw * 3.0 / 4.0 / sh) };
    [(1.0 - w) / 2.0, (1.0 - h) / 2.0, w, h]
}

/// `player`'s gun: its place in the picture (fractions), `None` off it.
fn gun(player: usize) -> Option<(f32, f32)> {
    let stick = input(player);
    let f = |v: i16| (v as f32 + 32768.0) / 65535.0;
    let screen = SCREEN.load(Ordering::Relaxed);
    let [px, py, w, h] = picture((screen >> 16) as f32, (screen & 0xFFFF) as f32);
    let (x, y) = ((f(stick.axis(Axis::LeftX)) - px) / w, (f(stick.axis(Axis::LeftY)) - py) / h);
    ((0.0..1.0).contains(&x) && (0.0..1.0).contains(&y)).then_some((x, y))
}

/// The board's coin or service credit: the coins, the sound of a coin or of a full credit.
fn add_coins(n: u32) {
    unsafe {
        let coins = at::<u32>(COINS);
        *coins = (*coins).wrapping_add(n);
        log!("bugbusters: {n} coin(s), {} in the game", *coins);
        let price = *at::<u32>(PRICE);
        let sound = if price == 0 || *coins % price == 0 { CREDIT_SOUND } else { COIN_SOUND };
        let sound = *at::<usize>(sound);
        if sound != 0 {
            let play: extern "C" fn(usize, i32) = std::mem::transmute(EXE.load(Ordering::Relaxed) + PLAY);
            play(sound, 0);
        }
    }
}

/// The frame's inputs, in the game's thread.
fn frame() {
    let mode = unsafe { *at::<u32>(MODE) };
    let held = unsafe { &mut *std::ptr::addr_of_mut!(HELD) };
    let (mut test, mut service) = (false, false);
    for player in 0..PLAYERS {
        let (mut trigger, mut reload, mut offscreen, mut start, mut coin) = (false, false, false, false, false);
        map().for_each_pressed(&input(player), |n| match n {
            Native::Trigger => trigger = true,
            Native::Reload => reload = true,
            Native::Offscreen => offscreen = true,
            Native::Start => start = true,
            Native::Coin => coin = true,
            Native::Test => test = true,
            Native::Service => service = true,
        });
        let aim = gun(player).filter(|_| !offscreen);
        let record = at::<i32>(RECORD + RECORD_SIZE * player);
        unsafe {
            if let Some((x, y)) = aim {
                *record = ((x * 640.0) as i32).clamp(0, 639);
                *record.add(1) = ((y * 480.0) as i32).clamp(0, 479);
            }
            *record.add(2) = i32::from(trigger && aim.is_some());
        }
        // a reload: the button, or the trigger off the picture
        let reloading = reload || (trigger && aim.is_none());
        if reloading && !held.reload[player] && matches!(mode, MODE_GAME | MODE_SHOP) {
            let tank = at::<f32>(TANK + 4 * player);
            log!("bugbusters: player {} reloads (tank {:.2})", player + 1, unsafe { *tank });
            unsafe { *tank = 1.0 };
        }
        held.reload[player] = reloading;
        if start && !held.start[player] {
            // without a credit, dropped: the title's first phase keeps the flag until a coin,
            // which then started the game at once
            let (price, coins) = unsafe { (*at::<u32>(PRICE), *at::<u32>(COINS)) };
            if mode == MODE_SETUP || coins >= price {
                log!("bugbusters: player {} start", player + 1);
                unsafe { *at::<u32>(START_FLAGS[player]) = 1 };
            }
        }
        held.start[player] = start;
        if coin && !held.coin[player] {
            add_coins(1);
        }
        held.coin[player] = coin;
    }
    if test && !held.test {
        log!("bugbusters: test (mode {mode})");
        unsafe { *at::<u32>(TEST_FLAG) = 1 };
        leave_title(mode);
    }
    held.test = test;
    if service && !held.service {
        if mode == MODE_SETUP {
            unsafe { *at::<u32>(SELECT_FLAG) = 1 };
        } else {
            add_coins(unsafe { *at::<u32>(PRICE) }.max(1));
        }
    }
    held.service = service;
    trace(mode);
}

/// `WAL_BUGBUSTERS_TRACE=1`: the mode, the players' records and tanks, once a second.
fn trace(mode: u32) {
    static ON: OnceLock<bool> = OnceLock::new();
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    if !*ON.get_or_init(|| std::env::var("WAL_BUGBUSTERS_TRACE").is_ok_and(|v| v.trim() == "1")) {
        return;
    }
    let mut last = LAST.lock().unwrap();
    if last.is_some_and(|t| t.elapsed().as_secs() < 1) {
        return;
    }
    *last = Some(std::time::Instant::now());
    let r = |p: usize, i: usize| unsafe { *at::<i32>(RECORD + RECORD_SIZE * p + 4 * i) };
    let t = |p: usize| unsafe { *at::<f32>(TANK + 4 * p) };
    let playing = |p: usize| unsafe { *at::<u32>(0x60C20 + 4 * p) };
    log!(
        "bugbusters: mode {mode}, coins {}, P1 {} ({},{}) trigger {} +c {} tank {:.2}, P2 {} ({},{}) trigger {} +c {} tank {:.2}",
        unsafe { *at::<u32>(COINS) },
        playing(0), r(0, 0), r(0, 1), r(0, 2), r(0, 3), t(0),
        playing(1), r(1, 0), r(1, 1), r(1, 2), r(1, 3), t(1)
    );
}

/// The title screen waits for a start (no attract cycle in free play): the test switch fades it
/// out as a start does (`0x4409cd`), its exit then opens the setup menu (`0x440ac9`).
fn leave_title(mode: u32) {
    unsafe {
        if mode == MODE_TITLE && *at::<u32>(TITLE_STATE) == 1 && *at::<u32>(TITLE_FADING) == 0 {
            let fade: extern "C" fn(u32) -> u32 = std::mem::transmute(EXE.load(Ordering::Relaxed) + FADE);
            fade(1);
            *at::<u32>(TITLE_FADING) = 1;
        }
    }
}

/// Whether this is the game window's thread.
fn main_thread() -> bool {
    let mut id = MAIN_THREAD.load(Ordering::Relaxed);
    if id == 0 {
        let hwnd = unsafe { *at::<usize>(WINDOW) };
        if hwnd == 0 {
            return false;
        }
        id = unsafe { GetWindowThreadProcessId(hwnd as _, std::ptr::null_mut()) };
        MAIN_THREAD.store(id, Ordering::Relaxed);
        log!("bugbusters: the game's window, thread {id}");
    }
    id == unsafe { GetCurrentThreadId() }
}

/// The main loop's `Sleep(0)` before each frame.
unsafe extern "system" fn sleep(ms: u32) {
    if ms == 0 && main_thread() {
        frame();
    }
    let f: unsafe extern "system" fn(u32) = unsafe { std::mem::transmute(SLEEP.load(Ordering::Relaxed)) };
    unsafe { f(ms) };
}

/// Writes `code` at `offset` when the executable has `expected` there.
fn patch(offset: usize, expected: &[u8], code: &[u8]) -> bool {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    let p = at::<u8>(offset);
    if unsafe { std::slice::from_raw_parts(p, expected.len()) } != expected {
        log!("bugbusters: unexpected code at {offset:#x}, not patched");
        return false;
    }
    unsafe {
        let mut old = 0;
        VirtualProtect(p as *const _, code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), p, code.len());
        VirtualProtect(p as *const _, code.len(), old, &mut old);
    }
    true
}

pub fn init() {
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    EXE.store(exe, Ordering::Relaxed);
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    SCREEN.store((sw as u32) << 16 | (sh as u32 & 0xFFFF), Ordering::Relaxed);
    let (offset, expected, _) = POINTER_CHECK;
    if unsafe { std::slice::from_raw_parts((exe + offset) as *const u8, expected.len()) } != expected {
        log!("bugbusters: unknown BBPC.exe build, no inputs");
        return;
    }
    if wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") != Some(false) {
        let done = PRICE_READS.iter().all(|&(offset, expected, code)| patch(offset, expected, code));
        log!("bugbusters: free play {}", if done { "on" } else { "not set" });
    }
    if !wal_payload_common::crosshairs(true) {
        let (offset, expected, code) = POINTER_CHECK;
        patch(offset, expected, code);
        log!("bugbusters: no pointer in the game");
    }
    match unsafe { iat::hook("KERNEL32.dll", "Sleep", sleep as *const () as usize) } {
        Some(o) => SLEEP.store(o, Ordering::Relaxed),
        None => {
            log!("bugbusters: no Sleep import, no inputs");
            return;
        }
    }
    log!("bugbusters: the guns and the I/O board's switches");
}

#[cfg(test)]
mod tests {
    use super::picture;

    #[test]
    fn picture_rect() {
        assert_eq!(picture(1920.0, 1080.0), [0.125, 0.0, 0.75, 1.0]);
        assert_eq!(picture(640.0, 480.0), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(picture(1280.0, 1024.0), [0.0, 0.03125, 1.0, 0.9375]);
    }
}
