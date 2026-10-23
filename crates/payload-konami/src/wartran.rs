//! Wartran Troopers (`game.exe`): the payload is its `panicdialog.dll` (the game's fatal error
//! box, logged). Four players with guns, two per monitor (`two_screens.rs`).
//!
//! The cabinet's I/O is a chain of boards behind the "pcio" hub on COM2 (115200 8N1). Every
//! packet starts with `AA`; the game's checksum is the sum of a packet's bytes after the first.
//!
//! * Boot (the hub's own packets, `AA AA ...`): `AA AA AA 55` and `AA AA 00 00` are echoed;
//!   `AA AA 00 01 01 ..` asks the number of boards (`AA AA 00 01 01 n sum`), `AA AA n 02 00`
//!   board n's id and firmware version (28 bytes), `AA AA n 03 00` its status (13 bytes).
//! * Boards: 1 and 2 id 0x201 (firmware `D00.bin`, players 1-2 and 3-4: setup commands only),
//!   3 id 0x601 (`C22D_1-0-0.bin`: the guns' OPTION / SHOULDER / SENSE switches), 4 the hub
//!   (0x10000, `pcio-20030929-v108.bin`: START, TRIGGER, TEST, SERVICE, coins, the security
//!   plugs). The versions are the dump's firmware files' (else the game uploads them).
//! * Commands `AA 00 node cmd data.. sum`: the line echoes them, then the board answers
//!   `AA 01 node cmd reply.. sum`. The reply's length is the request queue's (the game's I/O
//!   context at `[0x5843b8]`: entries of 0x80c bytes at +0x60c, the one sent at `[+0x608] & 15`,
//!   its reply length at +0x808).
//!
//! The guns' aim comes from the gun boards on COM1 (`gun_board`); the game's gun read is
//! replaced by the pointer (`gun_read`).
//!
//! Native inputs for `native_map`: `start trigger option shoulder sense offscreen` (players
//! 1-4), `test service coin`.

use std::collections::HashMap;
use std::sync::Mutex;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log, serial};
use wal_protocol::Axis;

/// The I/O context pointer.
const IO_CONTEXT: usize = 0x5843b8;

/// Board ids, by node (1-based), and their firmware file and version offset.
const BOARDS: [(u32, &str, u64); 4] = [
    (0x201, "D00.bin", 0x404),
    (0x201, "D00.bin", 0x404),
    (0x601, "C22D_1-0-0.bin", 0x404),
    (0x10000, "pcio-20030929-v108.bin", 0x1018),
];

static SEEN: Mutex<Option<HashMap<(u8, u8), u32>>> = Mutex::new(None);

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Start,
    Trigger,
    Option,
    Shoulder,
    Sense,
    Offscreen,
    Test,
    Service,
    Coin,
}

const NATIVES: &[(&str, Native)] = &[
    ("start", Native::Start),
    ("trigger", Native::Trigger),
    ("option", Native::Option),
    ("shoulder", Native::Shoulder),
    ("sense", Native::Sense),
    ("offscreen", Native::Offscreen),
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("start", "start"),
    ("b1", "trigger"),
    ("b2", "option"),
    ("b3", "shoulder"),
    ("b6", "offscreen"),
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
];

/// The players' controls and the coin counter.
struct Panel {
    maps: [ButtonMap<Native>; 4],
    coins: u16,
    coin_held: [bool; 4],
}

static PANEL: Mutex<Option<Panel>> = Mutex::new(None);

/// What each player presses.
fn controls() -> [Vec<Native>; 4] {
    let mut guard = PANEL.lock().unwrap();
    let panel = guard.get_or_insert_with(|| Panel {
        maps: [
            ButtonMap::new(NATIVES, DEFAULT_MAP),
            ButtonMap::with_defaults(NATIVES, DEFAULT_MAP),
            ButtonMap::with_defaults(NATIVES, DEFAULT_MAP),
            ButtonMap::with_defaults(NATIVES, DEFAULT_MAP),
        ],
        coins: 0,
        coin_held: [false; 4],
    });
    std::array::from_fn(|player| {
        let stick = wal_payload_common::input(player);
        let mut pressed = Vec::new();
        panel.maps[player].for_each_pressed(&stick, |n| pressed.push(n));
        let coin = pressed.contains(&Native::Coin);
        if coin && !panel.coin_held[player] {
            panel.coins = panel.coins.wrapping_add(1);
            log!("wartran: coin (player {})", player + 1);
        }
        panel.coin_held[player] = coin;
        pressed
    })
}

fn coins() -> u16 {
    PANEL.lock().unwrap().as_ref().map_or(0, |p| p.coins)
}

/// The hub's switches (command 0x28, 66 bytes: 2 bytes, then the switch bytes): byte 0 the
/// players' START (P1 0x80, P2 0x20, P3 0x10, P4 0x08) and SERVICE 0x40, byte 2 their
/// TRIGGER (same bits), byte 1 TEST 0x02 and the coin mech 0x01; bytes 4-5 the coin counter
/// (big endian, the game counts its steps).
fn hub_switches(reply: &mut [u8]) {
    const PLAYER_BITS: [u8; 4] = [0x80, 0x20, 0x10, 0x08];
    let mut d = [0u8; 8];
    let mut coin = false;
    for (player, pressed) in controls().iter().enumerate() {
        for n in pressed {
            match n {
                Native::Start => d[0] |= PLAYER_BITS[player],
                Native::Trigger => d[2] |= PLAYER_BITS[player],
                Native::Test => d[1] |= 0x02,
                Native::Service => d[0] |= 0x40,
                Native::Coin => coin = true,
                _ => {}
            }
        }
    }
    if coin {
        d[1] |= 0x01;
    }
    d[4..6].copy_from_slice(&coins().to_be_bytes());
    for (i, b) in d.into_iter().enumerate() {
        if let Some(r) = reply.get_mut(2 + i) {
            *r = b;
        }
    }
}

/// The 0x601 board's poll (command 0x49, 17 bytes: a byte, then the data): data bytes 0-3 /
/// 4-7 / 8-11 counters of players 1-4 (the test menu's opt / shuld / gundata num, left 0),
/// bytes 12-14 the guns' OPTION, SHOULDER, SENSE switches (bit 3 - player, active low).
fn gun_switches(reply: &mut [u8]) {
    let mut d = [0u8; 16];
    d[12..15].fill(0x0f);
    for (player, pressed) in controls().iter().enumerate() {
        let bit = 0x08 >> player;
        for n in pressed {
            match n {
                Native::Option => d[12] &= !bit,
                Native::Shoulder => d[13] &= !bit,
                Native::Sense => d[14] &= !bit,
                _ => {}
            }
        }
    }
    for (i, b) in d.into_iter().enumerate() {
        if let Some(r) = reply.get_mut(1 + i) {
            *r = b;
        }
    }
}

/// The guns' read (`int gun_read(int gun, float *x, float *y, int *offscreen, int *raw_x,
/// int *raw_y)`): the gun board's raw bytes through the cabinet calibration of the settings
/// file, smoothed, to 0..1 over both screens (the game multiplies x by their width, y by 480;
/// off screen parks the aim). Replaced by the pointer itself: exact aim without a calibration;
/// off screen when outside the picture or with the `offscreen` control (a reload).
const GUN_READ: usize = 0x4b2c00;
const GUN_READ_BYTES: [u8; 6] = [0x51, 0x53, 0x8b, 0x5c, 0x24, 0x0c];

extern "C" fn gun_read(gun: i32, x: *mut f32, y: *mut f32, offscreen: *mut i32, raw_x: *mut i32, raw_y: *mut i32) -> i32 {
    let player = gun.clamp(0, 3) as usize;
    let stick = wal_payload_common::input(player);
    // the pointer over the window, to the picture's area (both screens, its aspect kept)
    let (left, top, width, height) = crate::two_screens::picture_area();
    let fx = ((stick.axis(Axis::LeftX) as f32 + 32768.0) / 65535.0 - left) / width;
    let fy = ((stick.axis(Axis::LeftY) as f32 + 32768.0) / 65535.0 - top) / height;
    let mut off = !(0.0..=1.0).contains(&fx) || !(0.0..=1.0).contains(&fy);
    if let Some(panel) = PANEL.lock().unwrap().as_ref() {
        panel.maps[player].for_each_pressed(&stick, |n| off |= n == Native::Offscreen);
    }
    unsafe {
        *x = fx.clamp(0.0, 1.0);
        *y = fy.clamp(0.0, 1.0);
        *offscreen = off as i32;
        *raw_x = (fx * 255.0) as i32;
        *raw_y = (fy * 255.0) as i32;
    }
    1
}

fn hook_gun_read() {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    let at = GUN_READ as *mut u8;
    if unsafe { std::slice::from_raw_parts(at, GUN_READ_BYTES.len()) } != GUN_READ_BYTES {
        log!("wartran: unexpected gun read code, guns aim through the board's calibration");
        return;
    }
    let rel = (gun_read as *const () as usize).wrapping_sub(GUN_READ + 5) as u32;
    let mut old = 0;
    unsafe {
        VirtualProtect(at.cast(), 5, PAGE_EXECUTE_READWRITE, &mut old);
        *at = 0xe9;
        at.add(1).cast::<u32>().write_unaligned(rel);
        VirtualProtect(at.cast(), 5, old, &mut old);
    }
}

/// The bezel offsets: during 3D scenes the game slides the left screen's picture 8 pixels
/// left and the right one's 8 right (`push -8` / `push 8` at rva 0x71f5 / 0x71fe, the
/// cabinet's gap between its monitors): a black band in the middle of one window. Made 0.
const SLIDE_PATCHES: [(usize, [u8; 2]); 2] = [(0x4071f5, [0x6a, 0xf8]), (0x4071fe, [0x6a, 0x08])];

fn no_bezel_slide() {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    for (at, bytes) in SLIDE_PATCHES {
        let p = at as *mut u8;
        if unsafe { std::slice::from_raw_parts(p, 2) } != bytes {
            log!("wartran: unexpected screen slide code at {at:#x}, left as is");
            continue;
        }
        let mut old = 0;
        unsafe {
            VirtualProtect(p.cast(), 2, PAGE_EXECUTE_READWRITE, &mut old);
            *p.add(1) = 0;
            VirtualProtect(p.cast(), 2, old, &mut old);
        }
    }
}

/// The security plug the hub reads next (`0x40`: the system plug, `0x41`: the game plug).
static PLUG: Mutex<u8> = Mutex::new(0);

/// The game plug's id (`WAL_KONAMI_PLUG`, read at start: not from inside the serial hooks).
static PLUG_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();

pub fn detected(module: windows_sys::Win32::Foundation::HINSTANCE) -> bool {
    crate::file_name(module).eq_ignore_ascii_case("panicdialog.dll")
}

pub fn init() {
    let id = std::env::var("WAL_KONAMI_PLUG").unwrap_or_else(|_| "GKD00UA".into());
    log!("wartran: game plug {id}");
    let _ = PLUG_ID.set(id);
    serial::install("COM2", hub);
    unsafe { iat::hook("kernel32.dll", "GetLogicalDriveStringsA", get_logical_drive_strings as *const () as usize) };
    seed_settings();
    hook_gun_read();
    no_bezel_slide();
    crate::two_screens::init("wartran");
    serial::install("COM1", gun_board);
    for (name, f) in [
        ("GetAsyncKeyState", get_async_key_state as *const () as usize),
        ("GetKeyboardState", get_keyboard_state as *const () as usize),
        ("GetCursorPos", get_cursor_pos as *const () as usize),
    ] {
        unsafe { iat::hook("user32.dll", name, f) };
    }
    log!("wartran: I/O boards on COM2, gun boards on COM1, two screens");
}

fn sum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b))
}

/// Appends the checksum of `packet` (the sum of its bytes after the first).
fn sealed(mut packet: Vec<u8>) -> Vec<u8> {
    packet.push(sum(&packet[1..]));
    packet
}

/// The 3 version bytes of a board's firmware file.
fn version(file: &str, offset: u64) -> [u8; 3] {
    use std::io::{Read, Seek, SeekFrom};
    let mut v = [0u8; 3];
    match std::fs::File::open(file) {
        Ok(mut f) => {
            if f.seek(SeekFrom::Start(offset)).and_then(|_| f.read_exact(&mut v)).is_err() {
                log!("wartran: {file}: version unreadable");
            }
        }
        Err(e) => log!("wartran: {file}: {e}"),
    }
    v
}

fn hub(p: &[u8]) -> Vec<u8> {
    match p {
        [0xaa, 0xaa, 0xaa, 0x55] | [0xaa, 0xaa, 0x00, 0x00] => p.to_vec(),
        [0xaa, 0xaa, 0x00, 0x01, 0x01, ..] => sealed(vec![0xaa, 0xaa, 0x00, 0x01, 0x01, BOARDS.len() as u8]),
        [0xaa, 0xaa, n, 0x02, ..] if (1..=BOARDS.len() as u8).contains(n) => {
            let (id, file, offset) = BOARDS[*n as usize - 1];
            let v = version(file, offset);
            log!("wartran: board {n}: id {id:#x}, version {v:02x?}");
            let mut info = vec![0xaa, 0xaa, *n, 0x02, 0x00, 0x00, 0xaa];
            info.extend([0x00, *n, 0x02, 0x00]);
            info.extend(id.to_be_bytes());
            info.push(0);
            info.extend(v);
            info.extend([0; 8]);
            info.push(sum(&info[7..]));
            info
        }
        [0xaa, 0xaa, n, 0x03, ..] => {
            let mut status = vec![0xaa, 0xaa, *n, 0x03, 0x00, 0x00, 0xaa, 0x00, *n, 0x03, 0x00, 0x00];
            status.push(sum(&status[7..]));
            status
        }
        [0xaa, 0x00, node, cmd, ..] => command(p, *node, *cmd),
        _ => {
            log!("wartran: COM2 unknown {p:02x?}");
            Vec::new()
        }
    }
}

/// The reply length the game expects for the request being sent.
fn reply_len() -> usize {
    unsafe {
        let ctx = *(IO_CONTEXT as *const usize);
        if ctx == 0 {
            return 0;
        }
        let index = (*((ctx + 0x608) as *const i32) & 15) as usize;
        *((ctx + 0x60c + index * 0x80c + 0x808) as *const i32) as usize
    }
}

fn command(p: &[u8], node: u8, cmd: u8) -> Vec<u8> {
    let len = reply_len();
    let data = &p[4..p.len() - 1];
    {
        let mut seen = SEEN.lock().unwrap();
        let count = seen.get_or_insert_with(HashMap::new).entry((node, cmd)).or_insert(0);
        if *count < 3 || count.is_power_of_two() {
            log!("wartran: node {node} cmd {cmd:#04x} data {data:02x?} reply {len} (#{count})");
        }
        *count += 1;
    }
    let mut reply = vec![0u8; len];
    match cmd {
        // reset: byte 0 1 (done), byte 1 0 (no error)
        0x00 if len >= 1 => reply[0] = 1,
        // the hub's security plugs: select one, read its 64 bytes
        0x40 | 0x41 if node == 4 => *PLUG.lock().unwrap() = cmd,
        0x28 if node == 4 => hub_switches(&mut reply),
        0x49 if node == 3 => gun_switches(&mut reply),
        0x48 if node == 4 && *PLUG.lock().unwrap() == 0x41 => {
            // the game plug: its id (region) at bytes 3.., byte 1 0
            let id = PLUG_ID.get().map_or("GKD00UA", String::as_str);
            for (i, b) in id.bytes().take(32).enumerate() {
                if let Some(r) = reply.get_mut(3 + i) {
                    *r = b;
                }
            }
        }
        _ => {}
    }
    let mut out = p.to_vec();
    out.extend(sealed([vec![0xaa, 0x01, node, cmd], reply].concat()));
    out
}

/// The gun boards' link (`0x4b2xxx`): the request being sent (`0x1a2f20c` its length) and
/// the reply's length (`0x1a2f210`).
const GUN_REQUEST_LEN: usize = 0x1a2f20c;
const GUN_REPLY_LEN: usize = 0x1a2f210;

/// The request bytes written so far (the game writes them one by one).
static GUN_PENDING: Mutex<Vec<u8>> = Mutex::new(Vec::new());

/// The gun boards on COM1 (19200 8N1, two units of two guns: silent, GUN BOARD INITIALIZE
/// ERROR 2-1003-0006). A request's first byte is the unit (0, 1) | the command << 2; a reply
/// ends with the complement of the sum of its other bytes. The version (command 1, 2 bytes)
/// is 1; the poll (command 2, 7 bytes) has both guns' raw x, y, their off-screen bits and 0x75
/// at byte 5 (else a data error): answered centered, the aim is `gun_read`'s.
fn gun_board(p: &[u8]) -> Vec<u8> {
    let (request_len, reply_len) = unsafe { (*(GUN_REQUEST_LEN as *const i32) as usize, *(GUN_REPLY_LEN as *const i32) as usize) };
    let mut pending = GUN_PENDING.lock().unwrap();
    pending.extend_from_slice(p);
    if pending.len() < request_len.max(1) {
        return Vec::new();
    }
    let request = std::mem::take(&mut *pending);
    drop(pending);
    if reply_len == 0 {
        return Vec::new();
    }
    let mut reply = vec![0u8; reply_len];
    reply[0] = 1;
    if reply_len == 7 {
        reply[..4].fill(0x80);
        reply[5] = 0x75;
    }
    reply[reply_len - 1] = !sum(&reply[..reply_len - 1]);
    let mut seen = SEEN.lock().unwrap();
    let count = seen.get_or_insert_with(HashMap::new).entry((0xff, request[0])).or_insert(0);
    if *count < 3 || count.is_power_of_two() {
        log!("wartran: gun board <- {request:02x?} reply {reply:02x?} (#{count})");
    }
    *count += 1;
    reply
}

/// `GetLogicalDriveStringsA`: C: and E:, the cabinet's fixed drives. The settings files are
/// on the second one (`E:\D00\GxD000.CFG`, `GxD001.CFG`); Wine's Z: (the host's `/`) would be
/// taken instead.
unsafe extern "system" fn get_logical_drive_strings(size: u32, out: *mut u8) -> u32 {
    const DRIVES: &[u8] = b"C:\\\0E:\\\0\0";
    if out.is_null() || (size as usize) < DRIVES.len() {
        return DRIVES.len() as u32;
    }
    unsafe { std::ptr::copy_nonoverlapping(DRIVES.as_ptr(), out, DRIVES.len()) };
    DRIVES.len() as u32 - 1
}

/// The settings files on E: from the dump's copies (`D00\game\GxD00n.CFG`) when E: has none:
/// else the boot stops on CONFIGRATION FILE IS NOT FOUND (SERVICE creates new ones).
fn seed_settings() {
    for name in ["GxD000.CFG", "GxD001.CFG"] {
        let target = format!("E:\\D00\\{name}");
        if std::path::Path::new(&target).exists() || !std::path::Path::new(name).exists() {
            continue;
        }
        let _ = std::fs::create_dir_all("E:\\D00");
        match std::fs::copy(name, &target) {
            Ok(_) => log!("wartran: {target} from the dump's {name}"),
            Err(e) => log!("wartran: {target}: {e}"),
        }
    }
}

/// `GetAsyncKeyState`: no PC key down (the game's debug keys); the players' input is the
/// launcher's.
unsafe extern "system" fn get_async_key_state(_key: i32) -> i16 {
    0
}

/// `GetKeyboardState`: no PC key down (the test menu's keyboard shortcuts).
unsafe extern "system" fn get_keyboard_state(keys: *mut u8) -> i32 {
    if !keys.is_null() {
        unsafe { std::ptr::write_bytes(keys, 0, 256) };
    }
    1
}

/// `GetCursorPos`: the PC mouse never moves (the screen's corner).
unsafe extern "system" fn get_cursor_pos(point: *mut i32) -> i32 {
    if !point.is_null() {
        unsafe { point.write(0); point.add(1).write(0) };
    }
    1
}

/// `void PanicDialog(char *message)`: the game's fatal error box.
#[unsafe(no_mangle)]
pub extern "C" fn PanicDialog(message: *const std::ffi::c_char) {
    let text = if message.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(message) }.to_string_lossy().into_owned() };
    log!("wartran: panic: {text}");
}
