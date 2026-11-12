//! Photo Finish Racing (Competitive Edge Games' horse racing derby, `run.exe` v1.23, Direct3D 8,
//! 32-bit, three players): `WAL_ARCADEPC_BOARD=photofinish`.
//!
//! - **Keys.** Each player's ball drop (a ball rolled into a hole: the horse runs) and credit
//!   switches are keys the game polls with `GetAsyncKeyState` every 10 ms (a `SetTimer`
//!   callback, 0x40b8a0 and 0x40b9c0), their virtual keys `PlayerNDropID` / `PlayerNCreditID`
//!   of its settings (default `B`, `C`, `D`... and `K`, `L`, `M`...). A hole is a number of drop
//!   pulses: the game counts them for half a second after the first one (0x40b7e0) and the
//!   horse runs as many steps: the yellow hole (trot) one, the blue (gallop) two, the red
//!   (sprint) three. Each press of a hole's button gives its pulses (30 ms down, 30 ms up). The operator keys are
//!   `WM_KEYDOWN`s of its window (0x40a1f1): `F1` the settings screen, `F2` the totals report
//!   (`F9` resets them there), Escape quits (never posted). In free play (`FreePlayMode`) Space
//!   opens the starting gate from the attract (0x40bf57), each player's credit key puts their
//!   horse in it (no credit taken) and Space again starts the race (0x40f8a7); in free play a
//!   player's start presses their credit key too: start joins, start again races. Players 1 to 3 are the first three
//!   virtual sticks; `payload-common`'s `input_filter`: the real keyboard and mouse never reach it.
//! - **Settings.** `<D: or C:>\CE Games\Photo Finish\PhotoFinish.ini` (`key= value` lines; the
//!   profile maps `D:` into the dump, so they persist). The common `WAL_FREEPLAY` is its
//!   `FreePlayMode`, written at start (the defaults written when the file is missing, as the game
//!   would).
//! - **Relay board** (`RelayCommPort`, COM1 at 9600 bauds): the finish and winner signals, 4-byte
//!   writes `90 02` + the relays' bits, nothing read. Without the port the game stops at start
//!   ("Failed to initialize communications port!", `RelayErrExit`): a device of the payload's,
//!   whose relays are logged.
//!
//! Natives: `yellow`, `blue`, `red`, `credit` (each player's), `start` (Space), `settings`
//! (`F1`), `report` (`F2`), `reset` (`F9`).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use wal_payload_common::input_filter::post_key;
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log, serial};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowA;

const PLAYERS: usize = 3;
const DIRS: [&str; 2] = [r"D:\CE Games\Photo Finish", r"C:\CE Games\Photo Finish"];

const VK_SPACE: u16 = 0x20;
const VK_F1: u16 = 0x70;
const VK_F2: u16 = 0x71;
const VK_F9: u16 = 0x78;
/// The operator keys posted to the window.
const OPERATOR: [u16; 4] = [VK_SPACE, VK_F1, VK_F2, VK_F9];

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// A hole: its drop pulses.
    Drop(u8),
    Credit,
    /// An operator key, posted to the window ([`OPERATOR`]'s index).
    Key(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("yellow", Native::Drop(1)),
    ("blue", Native::Drop(2)),
    ("red", Native::Drop(3)),
    ("credit", Native::Credit),
    ("start", Native::Key(0)),
    ("settings", Native::Key(1)),
    ("report", Native::Key(2)),
    ("reset", Native::Key(3)),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "red"),
    ("b2", "blue"),
    ("b3", "yellow"),
    ("coin", "credit"),
    ("start", "start"),
    ("service", "settings"),
    ("test", "report"),
    ("b5", "reset"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The players' drop and credit keys (the settings' `PlayerNDropID` / `PlayerNCreditID`).
static KEYS: OnceLock<[(i32, i32); PLAYERS]> = OnceLock::new();
/// Free play: a player's start also presses their credit key (a horse in the race).
static FREE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The keys held (bit n: virtual key n), for the game's `GetAsyncKeyState` polls.
static HELD: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

pub fn key_held(vk: i32) -> bool {
    (0..256).contains(&vk) && HELD[vk as usize / 64].load(Ordering::Relaxed) & 1 << (vk % 64) != 0
}

/// The settings file's defaults (`PhotoFinish.ini` as the game writes it, v1.23.0).
const DEFAULT_INI: &str = "// Photo Finish System Configuration             v1.23.0\r\n\r\n\
// System Settings\r\nWindowedMode= 0\r\nRelayErrExit= 1\r\nRelayCommPort= 1\r\nMaxSysPlayers= 3\r\n\
PayoutLevel= 0\r\nSignalDelay= 100\r\nFreePlayMode= 0\r\nShowAttractVideo= 1\r\nDebuggingMode= 0\r\n\
DisplayVersion= 128\r\n\r\n// Game Settings\r\nRaceBeginTime= 10\r\nResultsEndTime= 15\r\nTotalRaceTime= 30\r\n\
UseCompHorses= 1\r\nMaxCompHorses= 7\r\nComputerSkill= 3\r\nUseTransparency= 1\r\nAttractSfxVol= 100\r\n\
AttractMusicVol= 100\r\nAttractVoiceVol= 100\r\nUseVoiceInstruct= 0\r\n\r\n// Player's ball drop key values\r\n\
Player1DropID= 66\r\nPlayer2DropID= 67\r\nPlayer3DropID= 68\r\nPlayer4DropID= 69\r\nPlayer5DropID= 70\r\n\
Player6DropID= 71\r\nPlayer7DropID= 72\r\nPlayer8DropID= 73\r\n\r\n// Player's credit key values\r\n\
Player1CreditID= 75\r\nPlayer2CreditID= 76\r\nPlayer3CreditID= 77\r\nPlayer4CreditID= 78\r\n\
Player5CreditID= 79\r\nPlayer6CreditID= 80\r\nPlayer7CreditID= 81\r\nPlayer8CreditID= 82\r\n\r\n*END\r\n";

/// `key`'s value in the settings' text.
fn value(text: &str, key: &str) -> Option<i32> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim().eq_ignore_ascii_case(key)).then(|| v.trim().parse().ok()).flatten()
    })
}

/// The settings' text with `key` set to `value` (its line kept in place, the others untouched).
fn set_value(text: &str, key: &str, value: i32) -> String {
    let mut found = false;
    let mut out: String = text
        .split_inclusive('\n')
        .map(|l| match l.split_once('=') {
            Some((k, _)) if k.trim().eq_ignore_ascii_case(key) => {
                found = true;
                let end = if l.ends_with("\r\n") { "\r\n" } else if l.ends_with('\n') { "\n" } else { "" };
                format!("{}= {value}{end}", k.trim())
            }
            _ => l.to_string(),
        })
        .collect();
    if !found {
        out = match out.rfind("*END") {
            Some(i) => format!("{}{key}= {value}\r\n{}", &out[..i], &out[i..]),
            None => format!("{out}{key}= {value}\r\n"),
        };
    }
    out
}

/// The settings (the game's folder on `D:` when that drive exists, as the game chooses), free
/// play set; the text read.
fn settings() -> String {
    let dir = if std::path::Path::new("D:\\").exists() { DIRS[0] } else { DIRS[1] };
    let path = format!(r"{dir}\PhotoFinish.ini");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_INI.to_string());
    let Some(free) = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") else { return text };
    let text = set_value(&text, "FreePlayMode", free as i32);
    let _ = std::fs::create_dir_all(dir);
    match std::fs::write(&path, &text) {
        Ok(()) => log!("photofinish: {path}: FreePlayMode {}", free as i32),
        Err(e) => log!("photofinish: {path}: {e}"),
    }
    text
}

/// The relay board: the relays' state logged, nothing answered.
fn relays(packet: &[u8]) -> Vec<u8> {
    if let [0x90, 0x02, lo, hi] = packet {
        log!("photofinish: relays {:#06x}", u16::from_le_bytes([*lo, *hi]));
    } else {
        log!("photofinish: relay board {packet:02x?}");
    }
    Vec::new()
}

fn window() -> HWND {
    unsafe { FindWindowA(c"UGLAppWindow".as_ptr().cast(), std::ptr::null()) }
}

/// A drop pulse's half period.
const PULSE: Duration = Duration::from_millis(30);

fn input_thread() {
    let keys = KEYS.get().unwrap();
    let mut was = 0u32;
    // each player's holes held, pulses to give, and when the drop key next changes
    let mut holes = [0u8; PLAYERS];
    let mut pulses = [0u32; PLAYERS];
    let mut next = [std::time::Instant::now(); PLAYERS];
    let mut down = [false; PLAYERS];
    loop {
        std::thread::sleep(Duration::from_millis(5));
        let now = std::time::Instant::now();
        let mut held = [0u64; 4];
        let mut set = |vk: i32| {
            if (0..256).contains(&vk) {
                held[vk as usize / 64] |= 1 << (vk % 64);
            }
        };
        let mut operator = 0u32;
        for (player, &(drop, credit)) in keys.iter().enumerate() {
            let mut pressed = 0u8;
            map().for_each_pressed(&input(player), |n| match n {
                Native::Drop(n) => pressed |= 1 << (n - 1),
                Native::Credit => set(credit),
                Native::Key(i) => {
                    operator |= 1 << i;
                    if i == 0 && FREE.load(Ordering::Relaxed) {
                        set(credit);
                    }
                }
            });
            for n in 1..=3u32 {
                if pressed & !holes[player] & 1 << (n - 1) != 0 {
                    pulses[player] += n;
                }
            }
            holes[player] = pressed;
            if now >= next[player] && (down[player] || pulses[player] > 0) {
                if !down[player] {
                    pulses[player] -= 1;
                }
                down[player] = !down[player];
                next[player] = now + PULSE;
            }
            if down[player] {
                set(drop);
            }
        }
        for (h, k) in HELD.iter().zip(held) {
            h.store(k, Ordering::Relaxed);
        }
        let changed = operator ^ was;
        if changed != 0 {
            let hwnd = window();
            if !hwnd.is_null() {
                for (bit, &vk) in OPERATOR.iter().enumerate() {
                    if changed & 1 << bit != 0 {
                        post_key(hwnd, vk, operator & 1 << bit != 0);
                    }
                }
                was = operator;
            }
        }
    }
}

pub fn init() {
    let text = settings();
    let id = |key: String, default: i32| value(&text, &key).unwrap_or(default);
    let keys: [(i32, i32); PLAYERS] =
        std::array::from_fn(|i| (id(format!("Player{}DropID", i + 1), 0x42 + i as i32), id(format!("Player{}CreditID", i + 1), 0x4B + i as i32)));
    let _ = KEYS.set(keys);
    FREE.store(value(&text, "FreePlayMode") == Some(1), Ordering::Relaxed);
    let port = format!("COM{}", value(&text, "RelayCommPort").unwrap_or(1));
    serial::install(&port, relays);
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    wal_payload_common::input_filter::init(exe, key_held);
    std::thread::spawn(input_thread);
    log!("photofinish: players' drop / credit keys {keys:?}, relay board on {port}");
}
