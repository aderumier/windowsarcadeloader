//! Sega World Drivers Championship's dashboard board (the speed / gear meters, the button
//! lamps): a text protocol on the first of COM20-COM29 that answers (the game's `BP_DashBoard`
//! Blueprint, Unreal's SerialPort plugin). Lines end with CRLF: `pr` asks the board's status,
//! answered `ps` and one 0/1 flag per button; `:sp...`, `:gr...`, `:le...`, `:rp...` set the
//! meters and lamps (not answered).

use std::sync::Mutex;

use wal_payload_common::{input, log, serial};
use wal_protocol::button as b;

static RX: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// The buttons of the status answer, in its order.
const BUTTONS: [u32; 10] = [b::B4, b::B2, b::B3, b::B5, b::START, b::B1, b::UP, b::DOWN, b::LEFT, b::RIGHT];

fn line(l: &str) -> Vec<u8> {
    let key: String = l.chars().take(3).collect();
    {
        let mut seen = SEEN.lock().unwrap();
        if !seen.contains(&key) {
            seen.push(key);
            log!("dashboard: {l:?}");
        }
    }
    if l == "pr" {
        let held = input(0).buttons;
        let mut out = b"ps".to_vec();
        out.extend(BUTTONS.iter().map(|&button| if held & button != 0 { b'1' } else { b'0' }));
        out.extend(b"\r\n");
        return out;
    }
    Vec::new()
}

fn packet(data: &[u8]) -> Vec<u8> {
    let mut rx = RX.lock().unwrap();
    rx.extend_from_slice(data);
    let mut out = Vec::new();
    while let Some(end) = rx.windows(2).position(|w| w == b"\r\n") {
        let l = String::from_utf8_lossy(&rx[..end]).into_owned();
        rx.drain(..end + 2);
        out.extend(line(&l));
    }
    out
}

pub(crate) fn init() {
    serial::install("COM20", packet);
}
