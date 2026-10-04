//! NESiCA card reader emulation (port of WindowsLoader's RfidEmu).
//!
//! The Taito RFID board sits on a serial port (`COM2`) and speaks JVS framing. The game's
//! serial calls are intercepted (IAT hooks on the game executable): opening the port returns
//! a fake handle, written packets are answered into a reply queue the game reads back.
//!
//! The card is inserted/removed by the virtual stick `card` input of any player (toggle).
//!
//! Options: `WAL_NESICA_RFID=0` disables it, `WAL_NESICA_RFID_PORT` (default `COM2`),
//! `WAL_NESICA_CARD_ID` (16 digits, default the WindowsLoader card `7020392010281502`).

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use wal_payload_common::jvs::{self, Encoder, REPORT_OK};
use wal_payload_common::{iat, log};
use wal_protocol::button;

type P = *mut c_void;

/// Odd value: never a real handle.
const FAKE: usize = 0x1337;
const BOARD_ID: &[u8] = b"TAITO CORP.;RFID CTRL P.C.B.;Ver1.00;";
const CARD_HEADER: [u8; 8] = [0x04, 0xC2, 0x3D, 0xDA, 0x6F, 0x52, 0x80, 0x00];

static REPLIES: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
static ADDRESSED: AtomicBool = AtomicBool::new(false);
static INSERTED: AtomicBool = AtomicBool::new(false);
static PORT: OnceLock<String> = OnceLock::new();
static CARD: OnceLock<[u8; 0x18]> = OnceLock::new();

fn card_data() -> &'static [u8; 0x18] {
    CARD.get_or_init(|| {
        let id = std::env::var("WAL_NESICA_CARD_ID").unwrap_or_else(|_| "7020392010281502".into());
        let mut data = [b'0'; 0x18];
        data[..8].copy_from_slice(&CARD_HEADER);
        for (d, c) in data[8..].iter_mut().zip(id.bytes()) {
            *d = c;
        }
        data
    })
}

/// Answers one request packet of the game (WindowsLoader `process_stream`).
fn process(packet: &[u8]) -> Vec<u8> {
    let Some(req) = jvs::parse(packet) else { return Vec::new() };
    if !matches!(req.node, 0x00 | 0x01 | 0xFF) {
        return Vec::new();
    }
    let arg = |c: &[u8], i: usize| c.get(i).copied().unwrap_or(0);
    let mut r = Encoder::new();
    let mut cmds = req.commands;
    while !cmds.is_empty() {
        let step = match cmds[0] {
            0xF0 => 2, // bus reset
            0xF1 => {
                r.push(REPORT_OK);
                ADDRESSED.store(true, Ordering::Relaxed);
                2
            }
            0x01 | 0x03 => {
                r.extend(&[REPORT_OK, 1]);
                2
            }
            0x04 => {
                r.push(REPORT_OK);
                1
            }
            0x05 => {
                r.push(REPORT_OK);
                3
            }
            0x10 => {
                r.push(REPORT_OK);
                r.extend(BOARD_ID);
                r.push(0);
                1
            }
            0x11 => {
                r.extend(&[REPORT_OK, 0x13]); // command format revision
                1
            }
            0x12 => {
                r.extend(&[REPORT_OK, 0x30]); // JVS revision
                1
            }
            0x13 => {
                r.extend(&[REPORT_OK, 0x10]); // communication revision
                1
            }
            0x14 => {
                r.extend(&[REPORT_OK, 1, 7, 0, 8, 0, 0x12, 8, 0, 0, 0]); // features
                1
            }
            0x20 => {
                r.extend(&[REPORT_OK, 0, 0, 0, 0, 0]); // switches
                3
            }
            0x21 => {
                r.extend(&[REPORT_OK, 0, 0, 0, 0]); // coins
                2
            }
            0x26 => {
                // general purpose input: card presence
                let n = arg(cmds, 1) as usize;
                r.push(REPORT_OK);
                let v = if INSERTED.load(Ordering::Relaxed) { 0x19 } else { 0 };
                for _ in 0..n {
                    r.push(v);
                }
                2 + n
            }
            0x2F => 1, // retransmit
            0x30 | 0x31 => {
                r.push(REPORT_OK); // coin decrease / payout
                4
            }
            0x32 => {
                // general purpose output: card data
                let n = arg(cmds, 1) as usize;
                r.push(REPORT_OK);
                if INSERTED.load(Ordering::Relaxed) {
                    r.extend(card_data());
                } else {
                    for _ in 0..n * 0x18 {
                        r.push(0);
                    }
                }
                r.push(REPORT_OK);
                2 + n
            }
            other => {
                log!("rfid: unknown command {other:#04x}");
                r.push(REPORT_OK);
                1
            }
        };
        cmds = &cmds[step.min(cmds.len())..];
    }
    r.finish()
}

/// Logs the first packets exchanged with the game.
fn trace(request: &[u8], reply: &[u8]) {
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    if COUNT.fetch_add(1, Ordering::Relaxed) < 40 {
        let hex = |d: &[u8]| d.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ");
        log!("rfid: <- {}  -> {}", hex(request), hex(reply));
    }
}

fn is_port_a(name: *const u8) -> bool {
    !name.is_null() && {
        let s = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy();
        port_matches(&s)
    }
}

fn is_port_w(name: *const u16) -> bool {
    if name.is_null() {
        return false;
    }
    let mut len = 0;
    while unsafe { *name.add(len) } != 0 {
        len += 1;
    }
    port_matches(&String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) }))
}

fn port_matches(name: &str) -> bool {
    let port = PORT.get_or_init(|| std::env::var("WAL_NESICA_RFID_PORT").unwrap_or_else(|_| "COM2".into()));
    let name = name.strip_prefix("\\\\.\\").unwrap_or(name);
    name.eq_ignore_ascii_case(port)
}

// Original functions (previous IAT entries), by hook index.
const COUNT: usize = 15;
static ORIG: [AtomicUsize; COUNT] = [const { AtomicUsize::new(0) }; COUNT];

fn orig<F: Copy>(idx: usize) -> F {
    let p = ORIG[idx].load(Ordering::Relaxed);
    unsafe { std::mem::transmute_copy(&p) }
}

unsafe extern "system" fn create_file_a(name: *const u8, a: u32, s: u32, sa: P, d: u32, f: u32, t: P) -> P {
    if is_port_a(name) {
        log!("rfid: card reader port opened");
        return FAKE as P;
    }
    unsafe { orig::<unsafe extern "system" fn(*const u8, u32, u32, P, u32, u32, P) -> P>(0)(name, a, s, sa, d, f, t) }
}

unsafe extern "system" fn create_file_w(name: *const u16, a: u32, s: u32, sa: P, d: u32, f: u32, t: P) -> P {
    if is_port_w(name) {
        log!("rfid: card reader port opened");
        return FAKE as P;
    }
    unsafe { orig::<unsafe extern "system" fn(*const u16, u32, u32, P, u32, u32, P) -> P>(1)(name, a, s, sa, d, f, t) }
}

unsafe extern "system" fn write_file(h: P, buf: *const u8, n: u32, written: *mut u32, ov: P) -> i32 {
    if h as usize != FAKE {
        return unsafe { orig::<unsafe extern "system" fn(P, *const u8, u32, *mut u32, P) -> i32>(2)(h, buf, n, written, ov) };
    }
    let packet = unsafe { std::slice::from_raw_parts(buf, n as usize) };
    let reply = process(packet);
    trace(packet, &reply);
    REPLIES.lock().unwrap().extend(reply);
    if !written.is_null() {
        unsafe { *written = n };
    }
    1
}

unsafe extern "system" fn read_file(h: P, buf: *mut u8, n: u32, read: *mut u32, ov: P) -> i32 {
    if h as usize != FAKE {
        return unsafe { orig::<unsafe extern "system" fn(P, *mut u8, u32, *mut u32, P) -> i32>(3)(h, buf, n, read, ov) };
    }
    let mut q = REPLIES.lock().unwrap();
    let count = (n as usize).min(q.len());
    for i in 0..count {
        unsafe { *buf.add(i) = q.pop_front().unwrap() };
    }
    if !read.is_null() {
        unsafe { *read = count as u32 };
    }
    1
}

unsafe extern "system" fn close_handle(h: P) -> i32 {
    if h as usize == FAKE {
        return 1;
    }
    unsafe { orig::<unsafe extern "system" fn(P) -> i32>(4)(h) }
}

unsafe extern "system" fn get_comm_modem_status(h: P, stat: *mut u32) -> i32 {
    if h as usize != FAKE {
        return unsafe { orig::<unsafe extern "system" fn(P, *mut u32) -> i32>(5)(h, stat) };
    }
    if !stat.is_null() {
        unsafe { *stat = if ADDRESSED.load(Ordering::Relaxed) { 0x10 } else { 0 } };
    }
    1
}

unsafe extern "system" fn clear_comm_error(h: P, errors: *mut u32, stat: *mut u32) -> i32 {
    if h as usize != FAKE {
        return unsafe { orig::<unsafe extern "system" fn(P, *mut u32, *mut u32) -> i32>(6)(h, errors, stat) };
    }
    if !errors.is_null() {
        unsafe { *errors = 0 };
    }
    if !stat.is_null() {
        // COMSTAT { flags, cbInQue, cbOutQue }
        let queued = REPLIES.lock().unwrap().len() as u32;
        unsafe { std::ptr::write_bytes(stat, 0, 3) };
        unsafe { *stat.add(1) = queued };
    }
    1
}

/// Serial configuration calls: accepted as is on the fake port.
macro_rules! accept_on_port {
    ($($idx:literal $name:ident ($($a:ident: $t:ty),*);)*) => {$(
        unsafe extern "system" fn $name(h: P, $($a: $t),*) -> i32 {
            if h as usize == FAKE {
                return 1;
            }
            unsafe { orig::<unsafe extern "system" fn(P, $($t),*) -> i32>($idx)(h, $($a),*) }
        }
    )*};
}

accept_on_port! {
    7 escape_comm_function(f: u32);
    8 setup_comm(i: u32, o: u32);
    9 get_comm_state(dcb: P);
    10 set_comm_state(dcb: P);
    11 set_comm_mask(m: u32);
    12 get_comm_timeouts(t: P);
    13 set_comm_timeouts(t: P);
    14 purge_comm(f: u32);
}

pub(crate) fn init() {
    if std::env::var("WAL_NESICA_RFID").is_ok_and(|v| v == "0") {
        return;
    }
    let hooks: [(&str, usize); COUNT] = [
        ("CreateFileA", create_file_a as *const () as usize),
        ("CreateFileW", create_file_w as *const () as usize),
        ("WriteFile", write_file as *const () as usize),
        ("ReadFile", read_file as *const () as usize),
        ("CloseHandle", close_handle as *const () as usize),
        ("GetCommModemStatus", get_comm_modem_status as *const () as usize),
        ("ClearCommError", clear_comm_error as *const () as usize),
        ("EscapeCommFunction", escape_comm_function as *const () as usize),
        ("SetupComm", setup_comm as *const () as usize),
        ("GetCommState", get_comm_state as *const () as usize),
        ("SetCommState", set_comm_state as *const () as usize),
        ("SetCommMask", set_comm_mask as *const () as usize),
        ("GetCommTimeouts", get_comm_timeouts as *const () as usize),
        ("SetCommTimeouts", set_comm_timeouts as *const () as usize),
        ("PurgeComm", purge_comm as *const () as usize),
    ];
    for (idx, (name, f)) in hooks.into_iter().enumerate() {
        if let Some(o) = unsafe { iat::hook("kernel32.dll", name, f) } {
            ORIG[idx].store(o, Ordering::Relaxed);
        }
    }

    // card insert/remove: toggled by the `card` virtual input
    std::thread::spawn(|| {
        let mut held = false;
        loop {
            let pressed = (0..wal_protocol::MAX_PLAYERS).any(|p| wal_payload_common::input(p).pressed(button::CARD));
            if pressed && !held {
                let now = !INSERTED.fetch_xor(true, Ordering::Relaxed);
                log!("rfid: card {}", if now { "inserted" } else { "removed" });
            }
            held = pressed;
            std::thread::sleep(Duration::from_millis(30));
        }
    });
}
