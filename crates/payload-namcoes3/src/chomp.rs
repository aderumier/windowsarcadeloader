//! Pac-Man Battle Royale Chompionship (Bandai Namco Amusements America, a Raspberry Pi 4
//! Linux game): the dump's `chomp.exe` 0.5.12 is a static recompilation of it to 64-bit Windows
//! (OpenGL through a GLUT shim, ALSA through `waveOut`, the guest's memory at 0x10_0000_0000,
//! started with `--no_hub`). It draws its 1280x720 picture on a borderless window the size of
//! the screen.
//!
//! **Controls.** The cabinet's I/O hub (SPI) reports eight player stations (stick, push button,
//! two coin switches) and the operator buttons. The recompilation's hub read (0x1400953b0)
//! builds the hub's answer from words its window procedure (0x140092de0) keeps from the keyboard
//! (`WM_KEYDOWN` / `WM_KEYUP`, players 1 and 2 only):
//! - one 16-bit word per player (0x1401cab60, 8 of them): bit 0 right, 1 left, 2 down, 3 up,
//!   4 push, 8 coin 1, 9 coin 2;
//! - the operator buttons (a byte, 0x1401cab54): bit 0 select (opens the operator menu, then
//!   changes the item under the cursor), 1 up, 2 down, 3 back;
//! - the TEST switch (a dword, 0x1401cab50, `F2`): the hub's diagnostic page while held.
//!
//! The payload writes these words from the launcher's four virtual sticks (players 1..4: the
//! bottom row's stations, yellow, pink, ...) every few milliseconds; the real keyboard never
//! reaches the game (`payload-common`'s `input_filter`), nor its F11 / Alt+Enter (full screen
//! toggle) and F12 (screenshot) keys.
//!
//! **Free play** (the common `WAL_FREEPLAY`, default on; unset: the operator's coinage kept): the adjustments are in the cabinet's
//! I2C EEPROM, the runtime's `eeprom.bin` beside the executable (16 KiB, read once at start, no
//! checksum). Its "credits to start" (offset 0xbc, a 32-bit word) 0 is free play. The payload
//! sets it before the game reads the file: 0 with free play, else back to the factory 1 when it
//! is 0 (another value the operator chose is kept). Without the file (a first start) the game
//! would write its factory image and keep its in-memory adjustments for that run: the payload
//! writes that factory image itself first ([`factory_image`]).
//!
//! Natives: `up`, `down`, `left`, `right`, `push`, `coin`, `coin2` (each player's), `select`,
//! `opup`, `opdown`, `back`, `test` (the operator's, from any player).

use std::sync::OnceLock;
use std::time::Duration;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log};

pub(crate) const GAME: &str = "chomp.exe";

/// The build the addresses below are for (its PE time stamp).
const TIMESTAMP: u32 = 0x6b05_db2f;

/// The players' words, the operator buttons, the TEST switch (RVAs).
const PLAYERS: usize = 0x1cab60;
const OPERATOR: usize = 0x1cab54;
const TEST: usize = 0x1cab50;

/// The EEPROM's size, its first two words (the image's marks), its "credits to start".
const EEPROM_SIZE: usize = 0x4000;
const EEPROM_MAGIC: [u32; 2] = [0x3bea_2f29, 0x1234_968b];
const CREDITS_TO_START: usize = 0xbc;

/// The factory adjustments (offset, value): what the game writes to a blank EEPROM.
const FACTORY_ADJUSTMENTS: &[(usize, u32)] = &[
    (0xa4, 10),
    (0xa8, 1),
    (0xac, 1),
    (0xb0, 4),
    (CREDITS_TO_START, 1),
    (0xc4, 1),
    // lives to start
    (0xc8, 3),
    (0xd0, 10),
    (0xd4, 4),
    (0xd8, 8),
    (0xdc, 10),
    (0xe0, 0x55),
    (0xe4, 4),
    (0xe8, 1),
    (0xf4, 1),
    (0x110, 5),
];

/// The factory high score table (at 0x200: a 12-byte name, the score, -1 per entry; -1 after).
const HIGH_SCORES: usize = 0x200;
const HIGH_SCORES_END: usize = 0x300;
const FACTORY_HIGH_SCORES: &[(&str, u32)] = &[
    ("BLINKY", 100_000),
    ("PINKY", 50_000),
    ("INKY", 45_000),
    ("CLYDE", 40_000),
    ("TAIZO", 35_000),
    ("POOKA", 30_000),
    ("FYGAR", 25_000),
    ("MAPPY", 20_000),
    ("MEOWKIE", 15_000),
    ("GORO", 10_000),
];

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// A bit of the player's word.
    Player(u16),
    /// A bit of the operator buttons.
    Operator(u8),
    Test,
}

const NATIVES: &[(&str, Native)] = &[
    ("right", Native::Player(1 << 0)),
    ("left", Native::Player(1 << 1)),
    ("down", Native::Player(1 << 2)),
    ("up", Native::Player(1 << 3)),
    ("push", Native::Player(1 << 4)),
    ("coin", Native::Player(1 << 8)),
    ("coin2", Native::Player(1 << 9)),
    ("select", Native::Operator(1 << 0)),
    ("opup", Native::Operator(1 << 1)),
    ("opdown", Native::Operator(1 << 2)),
    ("back", Native::Operator(1 << 3)),
    ("test", Native::Test),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "push"),
    ("b5", "push"),
    ("start", "push"),
    ("coin", "coin"),
    ("service", "select"),
    ("test", "test"),
    ("b3", "opup"),
    ("b4", "opdown"),
    ("b2", "back"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

fn input_thread(base: usize) {
    loop {
        std::thread::sleep(Duration::from_millis(4));
        let (mut operator, mut test) = (0u8, false);
        for player in 0..wal_protocol::MAX_PLAYERS {
            let mut word = 0u16;
            map().for_each_pressed(&input(player), |n| match n {
                Native::Player(bit) => word |= bit,
                Native::Operator(bit) => operator |= bit,
                Native::Test => test = true,
            });
            unsafe { std::ptr::write_volatile((base + PLAYERS + 2 * player) as *mut u16, word) };
        }
        unsafe {
            std::ptr::write_volatile((base + OPERATOR) as *mut u8, operator);
            std::ptr::write_volatile((base + TEST) as *mut u32, test as u32);
        }
    }
}

fn put(image: &mut [u8], offset: usize, value: u32) {
    image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn word(image: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(image[offset..offset + 4].try_into().unwrap())
}

/// The EEPROM image the game writes on a blank EEPROM (its marks, factory adjustments and high
/// scores; audits 0).
fn factory_image() -> Vec<u8> {
    let mut image = vec![0u8; EEPROM_SIZE];
    put(&mut image, 0, EEPROM_MAGIC[0]);
    put(&mut image, 4, EEPROM_MAGIC[1]);
    for &(offset, value) in FACTORY_ADJUSTMENTS {
        put(&mut image, offset, value);
    }
    image[HIGH_SCORES..HIGH_SCORES_END].fill(0xff);
    for (i, (name, score)) in FACTORY_HIGH_SCORES.iter().enumerate() {
        let entry = HIGH_SCORES + 20 * i;
        image[entry..entry + 12].fill(0);
        image[entry..entry + name.len()].copy_from_slice(name.as_bytes());
        put(&mut image, entry + 12, *score);
    }
    image
}

/// `eeprom.bin` beside the executable, its "credits to start" set for free play.
fn free_play(free: bool) {
    let Some(path) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("eeprom.bin"))) else {
        return;
    };
    let (mut image, created) = match std::fs::read(&path) {
        Ok(image) if image.len() >= EEPROM_SIZE && word(&image, 0) == EEPROM_MAGIC[0] => (image, false),
        _ => (factory_image(), true),
    };
    let now = word(&image, CREDITS_TO_START);
    let want = if free { 0 } else if now == 0 { 1 } else { now };
    if !created && now == want {
        log!("chomp: free play {free}, credits to start {now}");
        return;
    }
    put(&mut image, CREDITS_TO_START, want);
    match std::fs::write(&path, &image) {
        Ok(()) => log!("chomp: free play {free}, credits to start {now} -> {want}{}", if created { " (factory EEPROM)" } else { "" }),
        Err(e) => log!("chomp: {}: {e}", path.display()),
    }
}

pub(crate) fn attach(base: usize) {
    let stamp = crate::exe_timestamp(base);
    if stamp != TIMESTAMP {
        log!("chomp: unknown build (time stamp {stamp:#x}): no input");
        return;
    }
    match wal_payload_common::free_play("WAL_NAMCOES3_FREEPLAY") {
        Some(free) => free_play(free),
        None => log!("chomp: no free play option, the operator's coinage kept"),
    }
    // the real keyboard's messages dropped (the game's message loop is the executable's)
    wal_payload_common::input_filter::init(base, |_| false);
    std::thread::spawn(move || input_thread(base));
    log!("chomp: 4 players from the virtual sticks");
}
