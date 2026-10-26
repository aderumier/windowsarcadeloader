//! Happy Water Sprite II (SpyderZSoft, "SpriteT2", Unity 3.5, Mono, 32-bit, two water guns):
//! its "CommKing" I/O board, HASP HL dongle and registration (`WAL_ARCADEPC_BOARD=commking`).
//!
//! * I/O board: `CommKingDeviceManager` on COM1 (19200 bauds), the game's own overlapped
//!   kernel32 calls (`DllImport`). Frames both ways `AA len cmd data.. xor 55` (`len` = the data
//!   length + 2, `xor` of every byte from `AA` to the last data byte). The PC polls the board
//!   (cmd 1 online, 2 its ID), opens the guns' valves (8: player, on) and the pump (9), asks the
//!   water level (0x0B), pays tickets (4: player, on), drives lamps and the big guns: each
//!   answered as the board does (the receipts the game repeats its commands until). The board
//!   sends the coins (3: player, count), the keys (0x0E down, 0x0F up: 0 start P1 / its big
//!   gun, 1 start P2, 2 set (held: the system menu), 3 select, 4 reduce, 5 add, 6 right, 7
//!   down, 8 ok), the paid tickets (5: player, count) and the guns' aim (0x0A: x1 y1 x2 y2, big
//!   endian, 40..980): from the virtual sticks.
//! * Aim: the gun's position on the game's picture, linear (x 40 + 900 * x, y 40 + 900 * y from
//!   the top, as the game's default calibration points go): the game maps it (less 40) through
//!   its 13-point calibration (the system menu's PositionCorrection, saved in sys.dat), each
//!   point then 900 times its place on the screen: (0, 0), (450, 0), (900, 0), (0, 450)...,
//!   the centers (225, 225)... (the dump's sys.dat holds it). A water gun has no trigger: the game
//!   opens a playing player's valve itself.
//! * Dongle: `hasp_windows_96625.dll` (`hasp_login` feature 61027, `hasp_read` of the read/write
//!   file: three digits `n` then a text xored with `n`, `hasp_get_sessioninfo`'s `<hasp id=..>`).
//! * Registration: the game shows a machine code unless its PlayerPrefs `tmp1`..`tmp5` hold
//!   the MD5s (`MD5Hash`: MD5 with another first word, 0x67452201) of `xbly`, a GUID, the board's
//!   ID and the dongle's serial (with suffixes), obfuscated: written at start as an operator's
//!   registration would (`HKCU\Software\yeruigame\SpriteT2_EN`, Unity's `<key>_h<hash>`;
//!   Unity 3.5's strings are REG_SZ, a REG_BINARY one was never read).
//! * `ExitWindowsEx` (the system menu's reboot): ignored.
//!
//! Natives: `start coin bigstart` (each player), `set select reduce add right down ok` (the
//! operator's keys).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log, serial};

use crate::{demul, mono_imports};

const PORT: &str = "COM1";
/// The board's ID (cmd 2) and the dongle's serial: the registration is computed from them.
const BOARD_ID: [u8; 7] = [0x57, 0x41, 0x4C, 0x00, 0x00, 0x00, 0x01];
const HASP_SERIAL: &str = "1270530625";
/// The dongle's read/write file: the text the game checks, unxored (`000`).
const HASP_DATA: &str = "000xbly,xblygold,xblyhand,xblymachine,xblybest,xblyanimation,xblymd5hash,xblyoffzw";
/// The game's PlayerPrefs (company, product).
const PREFS_KEY: &str = "Software\\yeruigame\\SpriteT2_EN";

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Start,
    Coin,
    /// A board key (0..8).
    Key(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("set", Native::Key(2)),
    ("select", Native::Key(3)),
    ("reduce", Native::Key(4)),
    ("add", Native::Key(5)),
    ("right", Native::Key(6)),
    ("down", Native::Key(7)),
    ("ok", Native::Key(8)),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("start", "start"),
    ("b1", "start"),
    ("coin", "coin"),
    ("test", "set"),
    ("service", "select"),
    ("left", "reduce"),
    ("right", "add"),
    ("up", "right"),
    ("down", "down"),
    ("b2", "ok"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// A board frame: `AA len cmd data.. xor 55`.
fn frame(cmd: u8, data: &[u8]) -> Vec<u8> {
    let mut f = vec![0xAA, data.len() as u8 + 2, cmd];
    f.extend_from_slice(data);
    let xor = f.iter().fold(0u8, |x, b| x ^ b);
    f.extend([xor, 0x55]);
    f
}

/// Players paying out tickets (cmd 4 on).
static TICKETS: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];

/// The board's answers to the frames the game writes.
fn handler(packet: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut rest = packet;
    while let Some(start) = rest.iter().position(|b| *b == 0xAA) {
        rest = &rest[start..];
        let Some(&len) = rest.get(1) else { break };
        let total = len as usize + 3;
        if rest.len() < total {
            break;
        }
        let (f, next) = rest.split_at(total);
        rest = next;
        let (cmd, data) = (f[2], &f[3..total - 2]);
        match cmd {
            // online, the board's ID
            1 => out.extend(frame(1, &[0xF1])),
            2 => out.extend(frame(2, &[[0xF1].as_slice(), &BOARD_ID].concat())),
            // tickets: paid one by one while on
            4 => {
                if let [player, on, ..] = data[1..] {
                    if let Some(t) = TICKETS.get(player as usize) {
                        t.store(on != 0, Ordering::Relaxed);
                    }
                }
            }
            // the gun's valve (player), the pump: their receipts
            8 => out.extend(frame(8, &[data.get(1).copied().unwrap_or(0)])),
            9 => out.extend(frame(9, &[data.get(1).copied().unwrap_or(0)])),
            // the water level: normal
            0x0B => out.extend(frame(0x0B, &[0, 0])),
            _ => {}
        }
        static SEEN: Mutex<[bool; 256]> = Mutex::new([false; 256]);
        let mut seen = SEEN.lock().unwrap();
        if !seen[cmd as usize] {
            seen[cmd as usize] = true;
            log!("commking: first command {cmd:#04x} {f:02x?}");
        }
    }
    out
}

/// The board's inputs: aim frames, key and coin events, tickets paid.
fn board_thread() {
    let mut keys = [false; 9];
    let mut coins = [false; 2];
    let mut last_aim: Option<(Vec<u8>, Instant)> = None;
    let mut last_ticket = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let mut now_keys = [false; 9];
        let mut now_coins = [false; 2];
        let mut aim = Vec::with_capacity(8);
        for player in 0..2 {
            let stick = input(player);
            map().for_each_pressed(&stick, |n| match n {
                Native::Start => now_keys[player] = true,
                Native::Coin => now_coins[player] = true,
                // the operator's keys: player 1's
                Native::Key(k) if player == 0 => now_keys[k as usize] = true,
                Native::Key(_) => {}
            });
            let (x, y) = demul::gun_of_player_in_picture(player).unwrap_or((0.5, 0.5));
            let ax = (40.0 + 900.0 * x.clamp(0.0, 1.0)) as u16;
            let ay = (40.0 + 900.0 * y.clamp(0.0, 1.0)) as u16;
            aim.extend(ax.to_be_bytes());
            aim.extend(ay.to_be_bytes());
        }
        let mut out = Vec::new();
        for (player, (&now, was)) in now_coins.iter().zip(coins.iter_mut()).enumerate() {
            if now && !*was {
                out.extend(frame(3, &[player as u8, 1]));
            }
            *was = now;
        }
        for (key, (&now, was)) in now_keys.iter().zip(keys.iter_mut()).enumerate() {
            if now != *was {
                out.extend(frame(if now { 0x0E } else { 0x0F }, &[key as u8]));
                *was = now;
            }
        }
        if last_ticket.elapsed() >= Duration::from_millis(150) {
            last_ticket = Instant::now();
            for (player, paying) in TICKETS.iter().enumerate() {
                if paying.load(Ordering::Relaxed) {
                    out.extend(frame(5, &[player as u8, 1]));
                }
            }
        }
        let aim_due = last_aim.as_ref().is_none_or(|(a, t)| *a != aim || t.elapsed() >= Duration::from_millis(100));
        if aim_due {
            out.extend(frame(0x0A, &aim));
            last_aim = Some((aim, Instant::now()));
        }
        if !out.is_empty() {
            serial::push(PORT, &out);
        }
    }
}

// The dongle (HASP HL API, stdcall).

const HASP_STATUS_OK: u32 = 0;

extern "system" fn hasp_login(feature: u32, _vendor: *const u8, handle: *mut u32) -> u32 {
    log!("commking: hasp_login feature {feature}");
    if !handle.is_null() {
        unsafe { handle.write(1) };
    }
    HASP_STATUS_OK
}

extern "system" fn hasp_logout(_handle: u32) -> u32 {
    HASP_STATUS_OK
}

extern "system" fn hasp_read(_handle: u32, _file: u32, offset: u32, length: u32, buffer: *mut u8) -> u32 {
    let data = HASP_DATA.as_bytes();
    for i in 0..length as usize {
        let b = data.get(offset as usize + i).copied().unwrap_or(0);
        unsafe { buffer.add(i).write(b) };
    }
    HASP_STATUS_OK
}

/// The session's XML (its `<hasp id>`: the serial), kept alive.
fn session_info() -> &'static [u8] {
    static INFO: OnceLock<Vec<u8>> = OnceLock::new();
    INFO.get_or_init(|| {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n<hasp_info>\n  <hasp id=\"{HASP_SERIAL}\" type=\"HASP-HL\">\n    <feature id=\"61027\" />\n  </hasp>\n</hasp_info>\n\0"
        )
        .into_bytes()
    })
}

extern "system" fn hasp_get_sessioninfo(_handle: u32, _format: *const u8, info: *mut *const u8) -> u32 {
    if !info.is_null() {
        unsafe { info.write(session_info().as_ptr()) };
    }
    HASP_STATUS_OK
}

extern "system" fn hasp_get_info(_scope: *const u8, _format: *const u8, _vendor: *const u8, info: *mut *const u8) -> u32 {
    if !info.is_null() {
        unsafe { info.write(session_info().as_ptr()) };
    }
    HASP_STATUS_OK
}

extern "system" fn hasp_free(_info: *const u8) {}

extern "system" fn exit_windows_ex(flags: u32, _reason: u32) -> i32 {
    log!("commking: ExitWindowsEx({flags:#x}) ignored");
    1
}

/// The dongle's functions, `ExitWindowsEx`, and the serial functions of the emulated port.
fn resolve(name: &str, kernel32: bool) -> Option<usize> {
    let f = match name {
        "hasp_login" => hasp_login as *const () as usize,
        "hasp_logout" => hasp_logout as *const () as usize,
        "hasp_read" => hasp_read as *const () as usize,
        "hasp_get_sessioninfo" => hasp_get_sessioninfo as *const () as usize,
        "hasp_get_info" => hasp_get_info as *const () as usize,
        "hasp_free" => hasp_free as *const () as usize,
        "ExitWindowsEx" => exit_windows_ex as *const () as usize,
        _ if kernel32 => return serial::replacement(name),
        _ => return None,
    };
    Some(f)
}

// The registration: the game's MD5 and PlayerPrefs.

/// The game's `MD5Hash.CalcMD5Hash`: MD5 whose first word starts at 0x67452201, its four words
/// printed `XXXXXXXX-XXXXXXXX-XXXXXXXX-XXXXXXXX`.
fn game_md5(text: &str) -> String {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 4, 11,
        16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let mut data = text.as_bytes().to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend(bits.to_le_bytes());
    let mut h: [u32; 4] = [0x6745_2201, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476];
    for chunk in data.chunks(64) {
        let m: Vec<u32> = chunk.chunks(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let [mut a, mut b, mut c, mut d] = h;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let t = d;
            d = c;
            c = b;
            b = b.wrapping_add(a.wrapping_add(f).wrapping_add(k[i]).wrapping_add(m[g]).rotate_left(S[i]));
            a = t;
        }
        for (h, v) in h.iter_mut().zip([a, b, c, d]) {
            *h = h.wrapping_add(v);
        }
    }
    format!("{:08X}-{:08X}-{:08X}-{:08X}", h[0], h[1], h[2], h[3])
}

/// The game's `SetKey` obfuscation (two key bytes, then the text xored with them alternately),
/// hexadecimal; fixed key bytes.
fn obfuscate(text: &str) -> String {
    let key = [0x5A, 0xA5];
    let mut out = format!("{:02X}{:02X}", key[0], key[1]);
    for (i, b) in text.bytes().enumerate() {
        out.push_str(&format!("{:02X}", b ^ key[i % 2]));
    }
    out
}

/// Unity's PlayerPrefs value name of `key`: `<key>_h<djb2 xor hash>`.
fn prefs_name(key: &str) -> String {
    let hash = key.bytes().fold(5381u32, |h, c| h.wrapping_mul(33) ^ c as u32);
    format!("{key}_h{hash}")
}

/// Writes the registration (PlayerPrefs `tmp1`..`tmp5`) of our board and dongle.
fn register() {
    use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_WRITE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegSetValueExW};
    let id: String = BOARD_ID.iter().map(|b| format!("{b:02X}")).collect();
    let reg = format!("xblyC3ED648A-AA5F-4459-B024-F8D897A0C92D{id}{HASP_SERIAL}");
    let values = [("tmp1", "8462"), ("tmp2", "4455"), ("tmp3", "6798"), ("tmp4", ""), ("tmp5", "2846")];
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let mut key: HKEY = std::ptr::null_mut();
    let path = wide(PREFS_KEY);
    let r = unsafe {
        RegCreateKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, std::ptr::null(), 0, KEY_WRITE, std::ptr::null(), &mut key, std::ptr::null_mut())
    };
    if r != 0 {
        log!("commking: cannot open HKCU\\{PREFS_KEY} ({r})");
        return;
    }
    for (name, suffix) in values {
        // a string PlayerPref (Unity 3.5): REG_SZ
        let data = wide(&obfuscate(&game_md5(&format!("{reg}{suffix}"))));
        let value = wide(&prefs_name(name));
        unsafe { RegSetValueExW(key, value.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) };
    }
    unsafe { RegCloseKey(key) };
    log!("commking: registration written (machine code {})", game_md5(&reg));
}

pub fn init() {
    // the originals of the serial functions (no module's imports to hook)
    serial::install_in(PORT, handler, &[]);
    mono_imports::hook(resolve);
    register();
    std::thread::spawn(board_thread);
    log!("commking: CommKing board on {PORT}, HASP dongle");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefs_hash() {
        assert_eq!(prefs_name("mouseMode"), "mouseMode_h3016333319");
    }

    #[test]
    fn frames() {
        // the game's own online poll
        assert_eq!(frame(1, &[0xF1]), [0xAA, 3, 1, 0xF1, 0x59, 0x55]);
        assert_eq!(frame(2, &[0xF1]), [0xAA, 3, 2, 0xF1, 0x5A, 0x55]);
    }
}
