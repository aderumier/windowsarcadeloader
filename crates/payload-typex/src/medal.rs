//! Medal I/O board of New Super Mario Bros. Wii Coin World (`WAL_TYPEX_MEDAL_PORT`, COM1).
//!
//! Text protocol at 115200 baud, one frame each way per poll, lines ending with CR LF. The game
//! sends `:<date> <time>` (first frame), `/S`, one line per unit of each satellite (station
//! 1-4) `<unit>:<station>,<command>,<value>,0` then `/E` and ETX (0x03). Units: `H` hopper,
//! `C` medal selector, `O` lamps (3 values), `L` and `M` (stations 1-2 only); commands `R`
//! run, `S` stop, `C` clear error. The board answers `/S`, a line per unit
//! `<unit>:<station>,<state>,<d1>,<d2>,<error>,<detail>` (state `R`/`S`, `E` error), the
//! switches of each satellite `S:<station>,S,0,0,3,<state>,<released>,<pressed>` (hex words,
//! state active low, the edges since the last frame), `/E`, ETX. Without replies the game
//! shows "I/O board communication timeout" (7201).
//!
//! Switch bits (the game's input test): 3/2/0/1 up/down/left/right, 4 START (right round
//! button), 5 BET (left round button), 6 payout (square button), 8 medal accepted, 7 fake
//! medal, 11 test/enter, 12 select, 13 cancel/error reset, 10 satellite key (active high:
//! "adjusting" 98xx), 9 maintenance door (closed when low, "door open" 99xx otherwise), 14
//! hopper count, 15 hopper over-current.
//!
//! Satellite N is played by player N: medals with `coin` (one per press), BET/START/payout
//! buttons, test/select/cancel for the satellite test menu (`native_map` names: up down left
//! right bet start payout medal test select cancel key door). A hopper request (`H:n,R,count`)
//! is paid out at once.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{log, serial};

const STATIONS: usize = 4;

/// Bit of each native switch in the satellite's switch word.
const NATIVES: &[(&str, u16)] = &[
    ("up", 1 << 3),
    ("down", 1 << 2),
    ("left", 1 << 0),
    ("right", 1 << 1),
    ("start", 1 << 4),
    ("bet", 1 << 5),
    ("payout", 1 << 6),
    ("medal", 1 << 8),
    ("test", 1 << 11),
    ("select", 1 << 12),
    ("cancel", 1 << 13),
    ("key", 1 << 10),
    ("door", 1 << 9),
];

/// Virtual stick -> switch; games override with `WAL_MAP` (profile `native_map`).
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("coin", "medal"),
    ("b1", "bet"),
    ("start", "start"),
    ("b2", "payout"),
    ("test", "test"),
    ("b3", "select"),
    ("service", "cancel"),
];

/// Active high switches (the satellite key); the others are active low.
const ACTIVE_HIGH: u16 = 1 << 10;
/// Door switch: closed (low) unless the door input opens it.
const DOOR: u16 = 1 << 9;
const MEDAL: u16 = 1 << 8;

struct Board {
    map: ButtonMap<u16>,
    /// Bytes of the frame being received.
    rx: Vec<u8>,
    /// Pressed switches of each satellite at the last frame.
    pressed: [u16; STATIONS],
    /// Medals paid by each hopper for its current request.
    paid: [u32; STATIONS],
    /// Medals inserted in each satellite, not reported yet.
    medals: [u32; STATIONS],
    /// Last line of each unit, logged when it changes.
    last: std::collections::HashMap<(u8, usize), String>,
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);
static TRACE: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn init() {
    let Ok(port) = std::env::var("WAL_TYPEX_MEDAL_PORT") else { return };
    *BOARD.lock().unwrap() =
        Some(Board { map: ButtonMap::new(NATIVES, DEFAULT_MAP), rx: Vec::new(), pressed: [0; STATIONS], paid: [0; STATIONS], medals: [0; STATIONS], last: Default::default() });
    serial::install(port.trim(), process);
    log!("medal: I/O board on {port}");
}

/// Pressed switches of satellite `station` (1-4).
fn pressed(map: &ButtonMap<u16>, station: usize) -> u16 {
    let stick = wal_payload_common::input(station - 1);
    let mut bits = 0;
    map.for_each_pressed(&stick, |b| bits |= b);
    // the door is closed unless its input is held
    bits ^ DOOR
}

/// Switch word as the board reports it: active low, except the satellite key.
fn wire(pressed: u16) -> u16 {
    !pressed ^ ACTIVE_HIGH
}

fn unit_line(board: &mut Board, unit: char, station: usize, command: &str, value: u32) -> String {
    match unit {
        // hopper: pays the requested medals, one per poll, then stops
        'H' if (1..=STATIONS).contains(&station) => {
            let paid = &mut board.paid[station - 1];
            if command != "R" || value == 0 {
                *paid = 0;
                return format!("H:{station},S,0,0,0,0");
            }
            if *paid == 0 {
                log!("medal: satellite {station} pays {value} medals");
            }
            *paid = (*paid + 1).min(value);
            format!("H:{station},{},{value},{paid},0,0", if *paid < value { 'R' } else { 'S' })
        }
        // selector: the medals accepted since the last poll, while it runs
        'C' if (1..=STATIONS).contains(&station) => {
            let medals = if command == "R" { std::mem::take(&mut board.medals[station - 1]) } else { 0 };
            format!("C:{station},{command},0,{medals},0,0")
        }
        _ => format!("{unit}:{station},{command},0,0,0,0"),
    }
}

fn process(data: &[u8]) -> Vec<u8> {
    let mut guard = BOARD.lock().unwrap();
    let Some(board) = guard.as_mut() else { return Vec::new() };
    board.rx.extend_from_slice(data);
    let Some(end) = board.rx.iter().position(|b| *b == 0x03) else { return Vec::new() };
    let frame: Vec<u8> = board.rx.drain(..=end).collect();
    let text = String::from_utf8_lossy(&frame[..end]).into_owned();
    let lines: Vec<&str> = text.split("\r\n").filter(|l| !l.is_empty()).collect();

    let mut out = String::from("/S\r\n");
    for line in &lines {
        let b = line.as_bytes();
        if b.len() < 3 || b[1] != b':' || b[0] == b'/' {
            continue;
        }
        let mut f = line[2..].split(',');
        let station = f.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let command = f.next().unwrap_or("S");
        let value = f.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        if board.last.insert((b[0], station), line.to_string()).as_deref() != Some(line) && TRACE.fetch_add(1, Ordering::Relaxed) < 500 {
            log!("medal: <- {line}");
        }
        out.push_str(&unit_line(board, b[0] as char, station, command, value));
        out.push_str("\r\n");
    }
    for station in 1..=STATIONS {
        let now = pressed(&board.map, station);
        let before = std::mem::replace(&mut board.pressed[station - 1], now);
        let (on, off) = (now & !before, before & !now);
        if on & MEDAL != 0 {
            board.medals[station - 1] += 1;
        }
        out.push_str(&format!("S:{station},S,0,0,3,{:04X},{off:04X},{on:04X}\r\n", wire(now)));
    }
    out.push_str("/E\r\n\u{3}");
    out.into_bytes()
}
