//! The cabinet's LED board (837-15093-04) on COM1: the game drives the cabinet's lights
//! through it. Answered as a board without lights (segatools' `board/led15093.c`, public
//! domain). Initial D THE ARCADE's 837-15070-04 ([`init_15070`]): the same frames, its own
//! commands (segatools' `board/led15070.h` for their numbers).
//!
//! Frame: `E0 dest src len data... sum` (sum of the bytes after E0; E0 / D0 in the rest are
//! escaped as `D0 byte-1`). Request data: the command, its parameters; answer data: status
//! (1 ok), the command, report (1 ok), the command's values.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use wal_payload_common::{log, serial};

const SYNC: u8 = 0xE0;
const ESC: u8 = 0xD0;
/// The board's address when the host broadcasts (one board).
const BOARD: u8 = 1;

static RX: Mutex<Vec<u8>> = Mutex::new(Vec::new());
static RESPONSES: AtomicBool = AtomicBool::new(true);
/// An 837-15070 board (Initial D THE ARCADE), in its boot loader.
static BOARD_15070: AtomicBool = AtomicBool::new(false);
static BOOT_15070: AtomicBool = AtomicBool::new(false);

const EEPROM: &str = "Y:\\idac\\led15070.eeprom";

/// The 15070's EEPROM byte at `addr` (0-7), written first when `value` is given.
fn eeprom(addr: u8, value: Option<u8>) -> u8 {
    let mut data = std::fs::read(EEPROM).unwrap_or_default();
    data.resize(8, 0);
    let i = (addr & 7) as usize;
    if let Some(v) = value {
        data[i] = v;
        let _ = std::fs::create_dir_all("Y:\\idac");
        let _ = std::fs::write(EEPROM, &data);
    }
    data[i]
}

/// An answer from `src` to `host` (the request's source).
fn encode(cmd: u8, values: &[u8], src: u8, host: u8) -> Vec<u8> {
    let mut plain = vec![host, src, 3 + values.len() as u8, 1, cmd, 1];
    plain.extend_from_slice(values);
    let sum = plain.iter().fold(0u8, |s, b| s.wrapping_add(*b));
    plain.push(sum);
    let mut out = vec![SYNC];
    for b in plain {
        if b == SYNC || b == ESC {
            out.extend([ESC, b - 1]);
        } else {
            out.push(b);
        }
    }
    out
}

/// The next whole request in `rx` (unescaped, its sum checked), removed from it.
fn next_frame(rx: &mut Vec<u8>) -> Option<Vec<u8>> {
    loop {
        let start = rx.iter().position(|&b| b == SYNC)?;
        rx.drain(..start);
        let mut frame = vec![SYNC];
        let mut escape = false;
        let mut used = None;
        for (i, &b) in rx.iter().enumerate().skip(1) {
            if b == SYNC {
                // a new frame before this one ended: drop this one
                used = Some((i, false));
                break;
            }
            if b == ESC {
                escape = true;
                continue;
            }
            frame.push(if escape { b.wrapping_add(1) } else { b });
            escape = false;
            if frame.len() >= 4 && frame.len() == frame[3] as usize + 5 {
                used = Some((i + 1, true));
                break;
            }
        }
        let (n, whole) = used?;
        rx.drain(..n);
        if !whole {
            continue;
        }
        let sum = frame[1..frame.len() - 1].iter().fold(0u8, |s, b| s.wrapping_add(*b));
        if sum == frame[frame.len() - 1] {
            return Some(frame);
        }
    }
}

fn answer(frame: &[u8]) -> Vec<u8> {
    let dest = frame[1];
    let src = if dest == 0 { BOARD } else { dest };
    let host = frame[2];
    let encode = |cmd: u8, values: &[u8], src: u8| encode(cmd, values, src, host);
    let data = &frame[4..frame.len() - 1];
    let Some(&cmd) = data.first() else { return Vec::new() };
    let args = &data[1..];
    let responses = RESPONSES.load(Ordering::Relaxed);
    if BOARD_15070.load(Ordering::Relaxed) {
        return match cmd {
            0x10 => {
                RESPONSES.store(true, Ordering::Relaxed);
                encode(cmd, &[], src)
            }
            // ack on / off: the lights' commands answered or not
            0x7D | 0x7E => {
                RESPONSES.store(cmd == 0x7D, Ordering::Relaxed);
                encode(cmd, &[], src)
            }
            // EEPROM (8 bytes, kept in Y:\idac\led15070.eeprom): read (address) / write
            // (address, value)
            0x7C => encode(cmd, &[eeprom(args.first().copied().unwrap_or(0), None)], src),
            0x7B => {
                eeprom(args.first().copied().unwrap_or(0), args.get(1).copied());
                encode(cmd, &[], src)
            }
            // inputs, the lights (12/8 bit, flash, fade, palettes, dot correction, FETs...)
            0x28 | 0x30..=0x3F => {
                if responses {
                    encode(cmd, &[], src)
                } else {
                    Vec::new()
                }
            }
            // board info: number, end code, firmware 0x90
            0xF0 => {
                let mut v = b"15070-04".to_vec();
                v.extend([0xFF, 0x90]);
                encode(cmd, &v, src)
            }
            // status: timeouts, PWM, FET timeout
            0xF1 => encode(cmd, &[0, 0, 0, 0], src),
            0xF2 => encode(cmd, &[0, 0], src),
            // protocol: application mode 1.4, the boot loader 1.1 after "to boot mode"
            0xF3 => {
                if BOOT_15070.load(Ordering::Relaxed) {
                    encode(cmd, &[0, 1, 1], src)
                } else {
                    encode(cmd, &[1, 1, 4], src)
                }
            }
            0xFD => {
                BOOT_15070.store(true, Ordering::Relaxed);
                encode(cmd, &[], src)
            }
            _ => {
                log!("led: 15070 command {cmd:#04x}");
                encode(cmd, &[], src)
            }
        };
    }
    match cmd {
        // reset
        0x10 => {
            RESPONSES.store(true, Ordering::Relaxed);
            encode(cmd, &[], src)
        }
        // timeout: the count back
        0x11 => encode(cmd, &[args.get(1).copied().unwrap_or(0), args.first().copied().unwrap_or(0)], src),
        0x12 | 0x18 | 0x19 => encode(cmd, &[], src),
        // disable response
        0x14 => {
            if let Some(&sw) = args.first() {
                RESPONSES.store(sw == 0, Ordering::Relaxed);
            }
            encode(cmd, &[!RESPONSES.load(Ordering::Relaxed) as u8], src)
        }
        // the lights: answered only while responses are on
        0x3F | 0x80..=0x85 => {
            if responses {
                encode(cmd, &[], src)
            } else {
                Vec::new()
            }
        }
        0x86 => {
            if responses {
                encode(cmd, &[66, args.get(1).copied().unwrap_or(0)], src)
            } else {
                Vec::new()
            }
        }
        // board info: number, LF, chip, FF, firmware, receive buffer 204
        0xF0 => {
            let mut v = b"15093-04\n6704 ".to_vec();
            v.extend([0xFF, 0x90, 204, 0]);
            encode(cmd, &v, src)
        }
        // board status: board / UART / command flags, DIP switch 1 on
        0xF1 => encode(cmd, &[0, 0, 0, 1], src),
        0xF2 => encode(cmd, &[0xAE, 0xD9], src),
        // protocol: application mode 1.4
        0xF3 => encode(cmd, &[1, 1, 4], src),
        0xFD => encode(cmd, &[0], src),
        _ => {
            log!("led: command {cmd:#04x}");
            Vec::new()
        }
    }
}

fn packet(data: &[u8]) -> Vec<u8> {
    let mut rx = RX.lock().unwrap();
    rx.extend_from_slice(data);
    let mut out = Vec::new();
    while let Some(frame) = next_frame(&mut rx) {
        out.extend(answer(&frame));
    }
    out
}

/// The board on `port` (`COM1`; Sega World Drivers Championship's indicator LEDs: `COM2`).
pub(crate) fn init(port: &str) {
    serial::install(port, packet);
}

/// An 837-15070 board on `port` (Initial D THE ARCADE: `COM2`).
pub(crate) fn init_15070(port: &str) {
    BOARD_15070.store(true, Ordering::Relaxed);
    serial::install(port, packet);
}
