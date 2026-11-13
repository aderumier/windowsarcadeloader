//! Wonderland Wars (Sega, Nu 1.1, game id SDAP; `WAL_ALLS_BOARD=wlw`): a 32-bit Direct3D 9
//! game played with a pen on a touch screen and a stick, amLib linked in (no amdaemon): the
//! system's devices (`daemon.rs`: keychip, AMEX board, ALL.Net) in the game's own process, and
//! the cabinet's boards as segatools' carolhook (public domain) emulates them:
//!
//! - COM1: the touch screen's 3M MicroTouch controller: text commands (`\x01` command `\r`,
//!   answered `\x010\r`), then a stream of reports (10 touches, 6 bytes each: status 0x80 and
//!   0x40 while touching, X and Y in 7-bit halves over 0..16383, Y from the bottom, the touch's
//!   id). The pen: the launcher's touch surface (a touchscreen, the touchpad), else
//!   player 1's gun (the mouse, a lightgun) while `b1` is held; a red dot where it touches
//!   (the common crosshair option, `WAL_CROSSHAIR_AXES=touch`).
//! - Free play: amLib's credit record (as CHUNITHM's, `chuni.rs`: `WAL_ALLS_FREEPLAY`, else the
//!   common `WAL_FREEPLAY`), set in the backup at start and in the game's writes.
//! - COM11: the 837-15070 LED board (`led.rs`).
//! - The game's servers (its server list `servers.cgi`, database, matching: WinINet requests to
//!   ALL.Net's uri, the virtual router's port 80): no server, each request answered at once
//!   with an empty `200 OK`, instead of a connection timeout to an address absent from the
//!   network (and the list asked again every 9 s).
//! - COM12: the 837-15312 pen controller board: its LPC111x boot loader's text lines first,
//!   then `E0` frames (destination, source, length, command, data, a sum from the destination);
//!   its polling answer: the pen's button (`b2`), the dodge button (`b3`) and the stick (player
//!   1's directions, signed bytes). The game checks the status bytes of the previous answer it
//!   kept, none at first: the board's first initialization always fails, the second one plays. The game first runs the board's firmware updater
//!   (`15312firm\firmupdate_1113.exe -COM:12 -FirmUpdate ...`) and opens the board after its
//!   exit code 0 (up to date): `cmd /c exit 0` is started instead.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use wal_payload_common::{gun, iat, log, serial};
use wal_protocol::button;

pub(crate) fn init() {
    free_play();
    serial::install_streaming("COM1", touch_command, touch_report);
    crate::led::init_15070("COM11");
    serial::install("COM12", control_board);
    wal_payload_common::crosshair::init();
    std::thread::spawn(empty_server);
    unsafe {
        if let Some(orig) = iat::hook("kernel32.dll", "CreateProcessA", create_process_a as *const () as usize) {
            CREATE_PROCESS_A.store(orig, Ordering::Relaxed);
        }
        if let Some(orig) = iat::hook("kernel32.dll", "CreateProcessW", create_process_w as *const () as usize) {
            CREATE_PROCESS_W.store(orig, Ordering::Relaxed);
        }
        if let Some(orig) = iat::hook("wininet.dll", "InternetConnectA", internet_connect_a as *const () as usize) {
            INTERNET_CONNECT_A.store(orig, Ordering::Relaxed);
        }
        if let Some(orig) = iat::hook("wininet.dll", "InternetOpenUrlA", internet_open_url_a as *const () as usize) {
            INTERNET_OPEN_URL_A.store(orig, Ordering::Relaxed);
        }
    }
    log!("alls: wlw: touch controller on COM1, LED board on COM11, pen controller board on COM12");
}

// ── free play ───────────────────────────────────────────────────────────────────────────

fn free_play() {
    use crate::chuni::{CREDIT, CREDIT_SIZE, set_free_play};
    const PATH: &str = "Y:\\amex\\EEPROM.bin";
    if let Ok(mut eeprom) = std::fs::read(PATH) {
        let mut changed = false;
        for at in CREDIT.map(|a| a as usize) {
            if let Some(record) = eeprom.get_mut(at..at + CREDIT_SIZE) {
                changed |= set_free_play(record);
            }
        }
        if changed && std::fs::write(PATH, &eeprom).is_ok() {
            log!("alls: wlw: free play {} in the credit record", crate::chuni::free_play());
        }
    }
    wal_payload_common::amex::set_eeprom_write_filter(|offset, data| {
        for at in CREDIT {
            let Some(start) = at.checked_sub(offset).map(|s| s as usize) else { continue };
            if let Some(record) = data.get_mut(start..start + CREDIT_SIZE) {
                set_free_play(record);
            }
        }
    });
}

// ── COM1: the touch controller ──────────────────────────────────────────────────────────

/// A command (`\x01` name `\r`): acknowledged; the name, identity and unit type asked.
fn touch_command(packet: &[u8]) -> Vec<u8> {
    let cmd = packet.strip_prefix(&[1]).unwrap_or(packet);
    let cmd = cmd.split(|&b| b == b'\r').next().unwrap_or(&[]);
    match cmd {
        b"NM" | b"OI" => b"\x01AD1000\r".to_vec(),
        b"UT" => b"\x01AD****00\r".to_vec(),
        _ => b"\x010\r".to_vec(),
    }
}

/// The last touch's position (window pixels), kept for its release report.
static LAST_TOUCH: Mutex<Option<(u16, u16)>> = Mutex::new(None);

/// One report of the stream (10 touches, the pen in the first), when the game reads the port.
fn touch_report() -> Vec<u8> {
    let pen = pen_position();
    let mut report = [0u8; 60];
    for touch in report.chunks_mut(6) {
        touch[0] = 0x80;
    }
    let mut last = LAST_TOUCH.lock().unwrap();
    let (status, at) = match pen {
        Some(at) => (0xC0, Some(at)),
        None => (0x80, last.take()),
    };
    if let Some((x, y)) = at {
        report[..6].copy_from_slice(&[status, (x & 0x7F) as u8, (x >> 7 & 0x7F) as u8, (y & 0x7F) as u8, (y >> 7 & 0x7F) as u8, 1]);
    }
    if pen.is_some() {
        *last = pen;
    }
    report.to_vec()
}

/// The controller's coordinates: 14 bits, Y from the bottom (pixels, 0..1919 and 0..1079,
/// touched only the screen's bottom left corner).
const RANGE: f32 = 16383.0;

/// Where the pen touches, in the controller's coordinates (the touch surface's or the gun's
/// place in the game's picture, whatever the window's size).
fn pen_position() -> Option<(u16, u16)> {
    let (u, v) = match wal_payload_common::touches().first() {
        Some(&(x, y)) => (x as f32 / 65535.0, y as f32 / 65535.0),
        None if wal_payload_common::input(0).pressed(button::B1) => gun::gun_in_picture()?,
        None => return None,
    };
    Some(((u.clamp(0.0, 1.0) * RANGE) as u16, ((1.0 - v.clamp(0.0, 1.0)) * RANGE) as u16))
}

// ── COM12: the pen controller board ─────────────────────────────────────────────────────

/// The board's command numbers (segatools' carolhook `controlbd.h`).
const RESET: u8 = 0x10;
const POLLING: u8 = 0x81;
const BOARD_INFO: u8 = 0xF0;
const FIRMWARE_SUM: u8 = 0xF2;

/// The pen's button and the dodge button in the polling answer.
const PEN_BUTTON: u8 = 1;
const DODGE_BUTTON: u8 = 2;

/// A boot loader line or an `E0` frame.
fn control_board(packet: &[u8]) -> Vec<u8> {
    if packet.first() != Some(&0xE0) {
        return boot_loader(packet);
    }
    if packet.len() < 6 {
        return Vec::new();
    }
    let cmd = packet[4];
    match cmd {
        RESET => frame(cmd, &[]),
        // number, chip, firmware revision 0x90, buffer size 1
        BOARD_INFO => frame(cmd, &[1, b'1', b'5', b'3', b'1', b'2', b' ', b' ', b' ', 0x0A, b'6', b'6', b'9', b'9', b' ', 0xFF, 0x90, 1, 0]),
        FIRMWARE_SUM => frame(cmd, &[1, 0x36, 0x1B]),
        POLLING => frame(cmd, &polling()),
        _ => {
            if UNKNOWN_LOGGED.fetch_add(1, Ordering::Relaxed) < 16 {
                log!("alls: wlw: control board command {cmd:#04x}");
            }
            frame(cmd, &[1])
        }
    }
}

static UNKNOWN_LOGGED: AtomicU8 = AtomicU8::new(0);


/// The polling answer: ack, three fixed bytes, the buttons, 7 unknown bytes, the stick.
fn polling() -> [u8; 14] {
    let p1 = wal_payload_common::input(0);
    let mut buttons = 0;
    if p1.pressed(button::B2) {
        buttons |= PEN_BUTTON;
    }
    if p1.pressed(button::B3) {
        buttons |= DODGE_BUTTON;
    }
    let axis = |neg: u32, pos: u32| -> i8 {
        if p1.pressed(neg) {
            -127
        } else if p1.pressed(pos) {
            127
        } else {
            0
        }
    };
    // the lever: X positive right, Y positive up
    let (x, y) = (axis(button::LEFT, button::RIGHT), axis(button::DOWN, button::UP));
    [1, 3, 1, 1, buttons, 0, 0, 0, 0, 0, 0, 0, x as u8, y as u8]
}

/// An answer frame: `E0`, destination 1, source 2, length, report 1, the command, the data and
/// the sum of the bytes after `E0`.
fn frame(cmd: u8, data: &[u8]) -> Vec<u8> {
    let mut v = vec![0xE0, 0x01, 0x02, (data.len() + 2) as u8, 0x01, cmd];
    v.extend_from_slice(data);
    let sum = v[1..].iter().fold(0u8, |s, b| s.wrapping_add(*b));
    v.push(sum);
    v
}

/// The LPC111x boot loader every boot goes through: synchronized, no firmware update ("NG"),
/// the other commands echoed with a `0` result.
fn boot_loader(packet: &[u8]) -> Vec<u8> {
    match packet {
        b"?" => Vec::new(),
        b"Synchronized\r\n" => b"Synchronized\r\nNG\r\n".to_vec(),
        b"12000\r\n" => b"12000\r\nOK\r\n".to_vec(),
        _ => {
            let mut v = packet.to_vec();
            v.extend_from_slice(b"0\r\n");
            v
        }
    }
}

// ── the board's firmware updater ────────────────────────────────────────────────────────

static CREATE_PROCESS_A: AtomicUsize = AtomicUsize::new(0);
static CREATE_PROCESS_W: AtomicUsize = AtomicUsize::new(0);

type CreateProcessA = unsafe extern "system" fn(*const u8, *mut u8, *const c_void, *const c_void, i32, u32, *const c_void, *const u8, *const c_void, *mut c_void) -> i32;
type CreateProcessW = unsafe extern "system" fn(*const u16, *mut u16, *const c_void, *const c_void, i32, u32, *const c_void, *const u16, *const c_void, *mut c_void) -> i32;

const UP_TO_DATE: &str = "cmd.exe /c exit 0";

fn is_updater(command: &str) -> bool {
    command.to_ascii_lowercase().contains("firmupdate_1113")
}

unsafe extern "system" fn create_process_a(app: *const u8, cmd: *mut u8, pa: *const c_void, ta: *const c_void, inherit: i32, flags: u32, env: *const c_void, dir: *const u8, si: *const c_void, pi: *mut c_void) -> i32 {
    let real: CreateProcessA = unsafe { std::mem::transmute(CREATE_PROCESS_A.load(Ordering::Relaxed)) };
    let command = if cmd.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(cmd.cast()) }.to_string_lossy().into_owned() };
    if is_updater(&command) {
        log!("alls: wlw: {command} -> {UP_TO_DATE} (firmware up to date)");
        let mut line = format!("{UP_TO_DATE}\0").into_bytes();
        return unsafe { real(std::ptr::null(), line.as_mut_ptr(), pa, ta, inherit, flags, env, std::ptr::null(), si, pi) };
    }
    unsafe { real(app, cmd, pa, ta, inherit, flags, env, dir, si, pi) }
}

unsafe extern "system" fn create_process_w(app: *const u16, cmd: *mut u16, pa: *const c_void, ta: *const c_void, inherit: i32, flags: u32, env: *const c_void, dir: *const u16, si: *const c_void, pi: *mut c_void) -> i32 {
    let real: CreateProcessW = unsafe { std::mem::transmute(CREATE_PROCESS_W.load(Ordering::Relaxed)) };
    let command = if cmd.is_null() {
        String::new()
    } else {
        let len = (0..).take_while(|&i| unsafe { *cmd.add(i) } != 0).count();
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(cmd, len) })
    };
    if is_updater(&command) {
        log!("alls: wlw: {command} -> {UP_TO_DATE} (firmware up to date)");
        let mut line: Vec<u16> = UP_TO_DATE.encode_utf16().chain([0]).collect();
        return unsafe { real(std::ptr::null(), line.as_mut_ptr(), pa, ta, inherit, flags, env, std::ptr::null(), si, pi) };
    }
    unsafe { real(app, cmd, pa, ta, inherit, flags, env, dir, si, pi) }
}

// ── the game's servers ──────────────────────────────────────────────────────────────────

static INTERNET_CONNECT_A: AtomicUsize = AtomicUsize::new(0);

type InternetConnectA = unsafe extern "system" fn(usize, *const u8, u16, *const u8, *const u8, u32, u32, usize) -> usize;

/// The game's servers' stand-in: every request answered at once with an empty `200 OK` (the
/// server list without servers), on 127.0.0.1:`WAL_PORT`+110.
fn empty_port() -> u16 {
    std::env::var("WAL_PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(wal_protocol::DEFAULT_PORT) + 110
}

fn empty_server() {
    use std::io::{BufRead, BufReader, Write};
    let listener = match std::net::TcpListener::bind(("127.0.0.1", empty_port())) {
        Ok(l) => l,
        Err(e) => return log!("alls: wlw: game servers' stand-in: {e}"),
    };
    for stream in listener.incoming().flatten() {
        std::thread::spawn(move || {
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let _ = (&stream).write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        });
    }
}

static INTERNET_OPEN_URL_A: AtomicUsize = AtomicUsize::new(0);

type InternetOpenUrlA = unsafe extern "system" fn(usize, *const u8, *const u8, u32, u32, usize) -> usize;

fn router_host() -> String {
    let r = crate::netenv::router();
    format!("{}.{}.{}.{}", r[0], r[1], r[2], r[3])
}

fn refused(what: &str) {
    static LOGGED: AtomicU8 = AtomicU8::new(0);
    if LOGGED.fetch_add(1, Ordering::Relaxed) < 8 {
        log!("alls: wlw: game server {what}: none, an empty answer");
    }
}

/// A URL on the virtual router (`http://<router>/...`) sent to the stand-in.
unsafe extern "system" fn internet_open_url_a(session: usize, url: *const u8, headers: *const u8, length: u32, flags: u32, context: usize) -> usize {
    let real: InternetOpenUrlA = unsafe { std::mem::transmute(INTERNET_OPEN_URL_A.load(Ordering::Relaxed)) };
    if !url.is_null() {
        let text = unsafe { std::ffi::CStr::from_ptr(url.cast()) }.to_string_lossy().into_owned();
        let host = router_host();
        let after = text.split_once("://").map_or(text.as_str(), |(_, rest)| rest);
        let on_router = after.strip_prefix(host.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']));
        if on_router {
            refused(&text);
            let path = after[host.len()..].trim_start_matches(|c: char| c == ':' || c.is_ascii_digit());
            let moved = format!("http://127.0.0.1:{}{path}\0", empty_port());
            return unsafe { real(session, moved.as_ptr(), headers, length, flags, context) };
        }
    }
    unsafe { real(session, url, headers, length, flags, context) }
}

unsafe extern "system" fn internet_connect_a(session: usize, server: *const u8, port: u16, user: *const u8, password: *const u8, service: u32, flags: u32, context: usize) -> usize {
    let real: InternetConnectA = unsafe { std::mem::transmute(INTERNET_CONNECT_A.load(Ordering::Relaxed)) };
    let host = if server.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(server.cast()) }.to_string_lossy().into_owned() };
    if host == router_host() {
        refused(&format!("{host}:{port}"));
        return unsafe { real(session, c"127.0.0.1".as_ptr().cast(), empty_port(), user, password, service, flags, context) };
    }
    unsafe { real(session, server, port, user, password, service, flags, context) }
}
