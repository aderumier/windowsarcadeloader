//! Deathsmiles II (Cave, 2009; `cvgame.exe`, Direct3D 9, 32-bit): `WAL_ARCADEPC_BOARD=cave`.
//!
//! The cabinet's I/O board on COM3: 4-byte messages, ASCII commands.
//!
//! - **Start.** The game writes `@***` and reads 20 bytes (the board's version, not checked),
//!   then `&***` and reads 9: `&` and the board's 8 digit serial number (`0`-`9`), kept with the
//!   machine's network address in the backup data.
//! - **Board to PC**, read 4 bytes at a time until the port is empty, each sent when its inputs
//!   change (the game's state starts released): `Q b1 b2 0` player 1, `R b1 b2 0` player 2
//!   (`b1` bit 7 start, 6 service, 5 up, 4 down, 3 left, 2 right, 1 button 1, 0 button 2; `b2`
//!   bit 7 button 3; set: pressed), `P b1 0 0` the test switch (`ff` pressed, held 2.5 s at least: a shorter press does
//!   nothing), `S 0 0 0` a coin
//!   (one credit each), `!!!` the board alive (once, first).
//! - **PC to board**: `+**n` and 64 bytes plus a sum: the backup data copied to the board's
//!   memory; accepted, nothing kept.
//! - The first 3 s after the start sequence the board says nothing (as the game's own reads
//!   while it loads).
//!
//! The game's file loader (`CGMFileLoadMan`, a singleton made at its first use, 0x475fa0) leaves
//! its load queue's head, tail and count uninitialized until its queue is set up: a request
//! before then divided by the queue's size (0, 0x483801) when Wine's heap gave the object
//! leftover bytes (1 boot of 5, whatever the board did). Its allocation (`operator new`,
//! 0x475fc1) is ours: cleared memory.
//!
//! Free play (the common `WAL_FREEPLAY`): the SYSTEM menu's COIN MODE (`0x7654f0`: 0 FREE
//! PLAY, 1 one coin one credit, 2-9 more coins), loaded from the backup data (`RES\_gdata987.rpm`)
//! and saved with it: kept at 0; off, set back to 1 when it is 0 (the first 2 minutes).
//!
//! Natives: `up down left right b1 b2 b3 start` (each player), `coin`, `service` (player 1),
//! `test`.

use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log, serial};

const PORT: &str = "COM3";
/// The board's serial number (8 digits).
const SERIAL: &[u8; 8] = b"20091014";

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// A bit of the player's first byte.
    Player(u8),
    /// Button 3: bit 7 of the player's second byte.
    Button3,
    Coin,
    Test,
}

const NATIVES: &[(&str, Native)] = &[
    ("start", Native::Player(7)),
    ("service", Native::Player(6)),
    ("up", Native::Player(5)),
    ("down", Native::Player(4)),
    ("left", Native::Player(3)),
    ("right", Native::Player(2)),
    ("b1", Native::Player(1)),
    ("b2", Native::Player(0)),
    ("b3", Native::Button3),
    ("coin", Native::Coin),
    ("test", Native::Test),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "b1"),
    ("b2", "b2"),
    ("b3", "b3"),
    ("start", "start"),
    ("coin", "coin"),
    ("service", "service"),
    ("test", "test"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// Silence after the start sequence.
const QUIET: Duration = Duration::from_secs(3);
/// The test switch is seen held this long at least: the game takes a shorter press for
/// nothing (1 s pressed did not open the test menu, 3 s did).
const TEST_HOLD: Duration = Duration::from_millis(2500);
/// How often the board looks at its inputs.
const PERIOD: Duration = Duration::from_millis(8);

#[derive(Clone, Copy, Default, PartialEq)]
struct Inputs {
    players: [[u8; 2]; 2],
    test: bool,
    coins: [bool; 2],
}

struct Board {
    /// The end of the start sequence (`&***` answered).
    started: Option<Instant>,
    last: Option<Instant>,
    /// The inputs sent (the game's state).
    sent: Inputs,
    /// The last test switch press.
    test_pressed: Option<Instant>,
}

static BOARD: Mutex<Board> = Mutex::new(Board { started: None, last: None, sent: Inputs { players: [[0; 2]; 2], test: false, coins: [false; 2] }, test_pressed: None });

fn read_inputs() -> Inputs {
    let mut now = Inputs::default();
    for player in 0..2 {
        let b = &mut now.players[player];
        let mut coin = false;
        let mut test = false;
        map().for_each_pressed(&input(player), |n| match n {
            // player 2 has no service switch
            Native::Player(6) if player == 1 => {}
            Native::Player(bit) => b[0] |= 1 << bit,
            Native::Button3 => b[1] |= 0x80,
            Native::Coin => coin = true,
            Native::Test => test = player == 0,
        });
        now.coins[player] = coin;
        now.test |= test;
    }
    now
}

/// The messages of the inputs that changed, when the game reads the port.
fn idle() -> Vec<u8> {
    let mut board = BOARD.lock().unwrap();
    if !board.started.is_some_and(|t| t.elapsed() >= QUIET) || board.last.is_some_and(|t| t.elapsed() < PERIOD) {
        return Vec::new();
    }
    let mut out = Vec::new();
    if board.last.is_none() {
        log!("cave: board reports started");
        out.extend(*b"!!!\0");
    }
    board.last = Some(Instant::now());
    let mut now = read_inputs();
    let sent = board.sent;
    if now.test && !sent.test {
        board.test_pressed = Some(Instant::now());
    }
    now.test |= board.test_pressed.is_some_and(|t| t.elapsed() < TEST_HOLD);
    for (player, tag) in [(0, b'Q'), (1, b'R')] {
        if now.players[player] != sent.players[player] {
            out.extend([tag, now.players[player][0], now.players[player][1], 0]);
        }
        // a coin counted when it is pressed
        if now.coins[player] && !sent.coins[player] {
            out.extend(*b"S\0\0\0");
        }
    }
    if now.test != sent.test {
        out.extend([b'P', if now.test { 0xff } else { 0 }, 0, 0]);
    }
    board.sent = now;
    out
}

fn handler(packet: &[u8]) -> Vec<u8> {
    match packet.first() {
        Some(b'@') => {
            let mut reply = b"@CAVE I/O Ver1.00".to_vec();
            reply.resize(20, b'*');
            reply
        }
        Some(b'&') => {
            BOARD.lock().unwrap().started = Some(Instant::now());
            [&b"&"[..], SERIAL].concat()
        }
        _ => Vec::new(),
    }
}

/// The game's `operator new` (cdecl).
const OPERATOR_NEW: usize = 0x45eec2;
/// The file loader's allocation: `call operator new`.
const LOADER_NEW_CALL: usize = 0x475fc1;

/// The file loader's allocation, cleared.
extern "C" fn loader_new(size: usize) -> *mut u8 {
    let new: extern "C" fn(usize) -> *mut u8 = unsafe { std::mem::transmute(OPERATOR_NEW) };
    let p = new(size);
    if !p.is_null() {
        unsafe { std::ptr::write_bytes(p, 0, size) };
    }
    p
}

fn patch_loader_new() {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    let at = LOADER_NEW_CALL as *mut u8;
    // the expected `call operator new`
    let rel = (OPERATOR_NEW as isize - (LOADER_NEW_CALL as isize + 5)) as i32;
    let original = [&[0xe8u8][..], &rel.to_le_bytes()].concat();
    if unsafe { std::slice::from_raw_parts(at, 5) } != original.as_slice() {
        log!("cave: file loader allocation not found, not cleared");
        return;
    }
    let rel = (loader_new as usize as isize - (LOADER_NEW_CALL as isize + 5)) as i32;
    let mut old = 0;
    unsafe {
        VirtualProtect(at.cast(), 5, PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(rel.to_le_bytes().as_ptr(), at.add(1), 4);
        VirtualProtect(at.cast(), 5, old, &mut old);
    }
    log!("cave: file loader allocation cleared");
}

/// The SYSTEM menu's COIN MODE.
const COIN_MODE: usize = 0x7654f0;

fn free_play() {
    let Some(free) = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") else { return };
    std::thread::spawn(move || {
        let mode = COIN_MODE as *mut u32;
        // the backup data loaded (the board's reports start 3 s after the start sequence)
        loop {
            std::thread::sleep(Duration::from_millis(250));
            if BOARD.lock().unwrap().last.is_some() {
                break;
            }
        }
        let now = unsafe { mode.read_volatile() };
        log!("cave: coin mode {now} (free play {free})");
        if !free {
            // until the backup data is surely loaded (the attract mode)
            for _ in 0..480 {
                if unsafe { mode.read_volatile() } == 0 {
                    unsafe { mode.write_volatile(1) };
                    log!("cave: coin mode 1 coin 1 credit");
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            return;
        }
        loop {
            if unsafe { mode.read_volatile() } != 0 {
                unsafe { mode.write_volatile(0) };
                log!("cave: coin mode free play");
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
}

pub fn init() {
    patch_loader_new();
    free_play();
    serial::install_streaming(PORT, handler, idle);
    log!("cave: I/O board on {PORT}");
}
