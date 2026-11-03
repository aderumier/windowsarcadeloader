//! Time Crisis 5 (2015, Unreal Engine 3, 64-bit): `RSLauncher.exe` (cabinet launcher) starts
//! `TC5\Binaries\Win64\TimeCrisisGame-Win64-Shipping.exe`. Both import
//! `hasp_windows_x64_105293.dll`. One player per PC (a cabinet side).
//!
//! Game:
//! - dongle: the dongle type (`[0x1861958]`) selects the HASP (0x40 bytes at 0xd00, byte 0x3f the
//!   complement of 0x3e, then the serial against the template 0x16f4360 `2737212499**`) or the
//!   USB key (the enumeration at 0x20b0, id 0x0c10, template 0x16f4350 `27371*2400**` or the
//!   script's 0x16f4330). [`HASP_SERIAL`] / [`usb_key_enum`] answer both.
//! - I/O board: the USIO on USB (`usio.rs`): switches, coins, outputs.
//! - gun board: a serial device the game looks for on COM1, COM2, COM15..COM3 (38400 bauds),
//!   one command byte, then a fixed-size answer whose last byte is the complement of the sum
//!   of the others (table 0x16f2f80): `08` 10 bytes (x, y, two more 16-bit values big endian,
//!   flags: 0x20 trigger, 0x80 out of screen, 0x10 unknown), `1C` 10 bytes (10-bit x / y), `14`, `18`,
//!   `0C`, `04` 1 byte; `10` asks a 31-byte version string starting with `R`.
//! - launcher: Banapass reader on COM4, NEC projector on COM1, the status window as Lost Land
//!   Adventure's; the game it starts gets a borderless window the size of the screen.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

use wal_payload_common::log;
use wal_payload_common::mapping::ButtonMap;
use wal_protocol::{Axis, StickState};

use crate::{GAME_BASE, module, patches, usio};

/// This library's name in this game's dump.
const HASP: &str = "hasp_windows_x64_105293.dll";
const LAUNCHER: &str = "RSLauncher.exe";
const GAME: &str = "TimeCrisisGame-Win64-Shipping.exe";

/// The cabinet serial for the HASP check (template `2737212499**`).
pub(crate) const HASP_SERIAL: &[u8; 12] = b"273721249900";
const HASP_SERIAL_STR: &str = "273721249900";

pub(crate) fn is_tc5() -> bool {
    module(HASP) != 0
}

unsafe extern "C" fn usb_key_enum(_a: i32, _b: i32, _c: u16, _d: u16, out: *mut u8) -> i32 {
    // the script's template, else the built-in one; the HASP serial when it matches (the
    // launcher compares the serial the game found with the cabinet's: 05-56 otherwise)
    let base = crate::game_base();
    let script = unsafe { *((base + 0x16f4330) as *const u8) } != b'*';
    let template = if script { 0x16f4330 } else { 0x16f4350 };
    let matches = (0..12).all(|i| {
        let t = unsafe { *((base + template + i) as *const u8) };
        t == b'*' || t == HASP_SERIAL[i]
    });
    let serial = if matches { *HASP_SERIAL } else { patches::serial(template) };
    unsafe { patches::usb_key(out, 0x0c10, &serial) };
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| log!("tc5: usb key serial {}", String::from_utf8_lossy(&serial)));
    1
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Native {
    Test,
    Service,
    Coin,
    Enter,
    Up,
    Down,
    Trigger,
    Weapon,
    LeftPedal,
    RightPedal,
    Offscreen,
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("enter", Native::Enter),
    ("up", Native::Up),
    ("down", Native::Down),
    ("trigger", Native::Trigger),
    ("weapon", Native::Weapon),
    ("left_pedal", Native::LeftPedal),
    ("right_pedal", Native::RightPedal),
    ("offscreen", Native::Offscreen),
];

/// Virtual stick -> cabinet; profiles override it with `native_map`. The cabinet has no start
/// button: a pedal starts the game. The keyboard's C / V (b7 / b8) are the pedals.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "left_pedal"),
    ("b1", "trigger"),
    ("b2", "weapon"),
    ("b3", "left_pedal"),
    ("b4", "right_pedal"),
    ("b5", "enter"),
    ("b6", "offscreen"),
    ("b7", "left_pedal"),
    ("b8", "right_pedal"),
    ("up", "up"),
    ("down", "down"),
];

/// USIO switch bits (names from the operator menu's switch test; the trigger and the sensor
/// are the gun board's).
const SW_TEST: u32 = 0x80;
const SW_ENTER: u32 = 0x200;
const SW_RIGHT_PEDAL: u32 = 0x400;
const SW_LEFT_PEDAL: u32 = 0x800;
const SW_DOWN: u32 = 0x1000;
const SW_UP: u32 = 0x2000;
const SW_SERVICE: u32 = 0x4000;
const SW_GUN_BUTTON: u32 = 0x80_0000;

struct State {
    map: ButtonMap<Native>,
    coins: u16,
    coin_held: bool,
    test_on: bool,
    test_held: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// The test switch outlives the game process: a game that ends with it on (a crash entering the
/// operator menu) is started again by the launcher in the operator menu, as on the cabinet
/// where the switch stays on. Kept in a file, cleared when the launcher starts.
fn test_switch_file() -> std::path::PathBuf {
    std::env::temp_dir().join("wal-tc5-test-switch")
}

fn with_state<T>(f: impl FnOnce(&mut State, &StickState) -> T) -> T {
    let mut guard = STATE.lock().unwrap();
    let state = guard.get_or_insert_with(|| State {
        map: ButtonMap::new(NATIVES, DEFAULT_MAP),
        coins: 0,
        coin_held: false,
        test_on: test_switch_file().exists(),
        test_held: false,
    });
    let stick: StickState = wal_payload_common::input(0);
    f(state, &stick)
}

fn usio_poll() -> usio::Inputs {
    with_state(|s, stick| {
        let mut sw = 0u32;
        let (mut coin, mut test) = (false, false);
        s.map.for_each_pressed(stick, |n| match n {
            Native::Test => test = true,
            Native::Coin => coin = true,
            Native::Service => sw |= SW_SERVICE,
            Native::Enter => sw |= SW_ENTER,
            Native::Up => sw |= SW_UP,
            Native::Down => sw |= SW_DOWN,
            Native::Weapon => sw |= SW_GUN_BUTTON,
            Native::LeftPedal => sw |= SW_LEFT_PEDAL,
            Native::RightPedal => sw |= SW_RIGHT_PEDAL,
            Native::Trigger | Native::Offscreen => {}
        });
        // the cabinet's test switch latches
        if test && !s.test_held {
            s.test_on = !s.test_on;
            log!("tc5: test switch {}", if s.test_on { "on" } else { "off" });
            let _ = if s.test_on { std::fs::write(test_switch_file(), b"1") } else { std::fs::remove_file(test_switch_file()) };
        }
        s.test_held = test;
        if s.test_on {
            sw |= SW_TEST;
        }
        if s.coin_held && !coin {
            s.coins = s.coins.wrapping_add(1) & 0x3FFF;
        }
        s.coin_held = coin;
        usio::Inputs { switches: sw, coins: [s.coins, 0] }
    })
}

fn checksum(data: &[u8]) -> u8 {
    !data.iter().fold(0u8, |s, b| s.wrapping_add(*b))
}

/// Gun board coordinates (0..1, y up) the game puts at screen position `(x, y)` (0..1 from
/// the top left corner), with the operator menu's default calibration: measured in its gun
/// test, a linear fit (residuals under 2% of the screen).
fn camera(x: f32, y: f32) -> (f32, f32) {
    (((x + 0.080) / 1.148).clamp(0.0, 1.0), ((1.163 - y) / 1.268).clamp(0.0, 1.0))
}

/// The gun board's answer to one command byte.
fn gun_reply(cmd: u8) -> Vec<u8> {
    let mut out = match cmd {
        0x08 | 0x1C => {
            let (x, y, trigger, offscreen) = with_state(|s, stick| {
                let (mut trigger, mut offscreen) = (false, false);
                s.map.for_each_pressed(stick, |n| match n {
                    Native::Trigger => trigger = true,
                    Native::Offscreen => offscreen = true,
                    _ => {}
                });
                let unit = |a: Axis| (stick.axis(a) as f32 + 32768.0) / 65535.0;
                (unit(Axis::LeftX), unit(Axis::LeftY), trigger, offscreen)
            });
            let (x, y) = camera(x, y);
            let (x, y) = ((x * 1023.0).round() as u16, (y * 1023.0).round() as u16);
            let mut flags = 0u8;
            if trigger {
                flags |= 0x20;
            }
            if offscreen {
                flags |= 0x80;
            }
            if cmd == 0x08 {
                let mut d = Vec::with_capacity(11);
                for v in [x, y, x, y] {
                    d.extend_from_slice(&v.to_be_bytes());
                }
                d.extend_from_slice(&[flags, 0]);
                d
            } else {
                let mut d = vec![(x >> 2) as u8, (x << 6) as u8, (y >> 2) as u8, (y << 6) as u8];
                d.extend_from_slice(&[0; 5]);
                d.push(flags);
                d
            }
        }
        0x10 => {
            let mut v = b"RTC5 GUN BOARD Ver.1.00".to_vec();
            v.resize(31, b' ');
            return v;
        }
        0x14 | 0x18 | 0x0C | 0x04 => vec![0],
        c => {
            log!("tc5: gun board command {c:02x}");
            return Vec::new();
        }
    };
    out.push(checksum(&out));
    out
}

fn gun_board(data: &[u8]) -> Vec<u8> {
    data.iter().flat_map(|&c| gun_reply(c)).collect()
}

static CO_CREATE_INSTANCE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The launcher's AMCUS auth daemon (`IAuth`), in-process (`amcus.rs`).
unsafe extern "system" fn co_create_instance(clsid: *const u8, outer: *mut c_void, context: u32, iid: *const u8, out: *mut *mut c_void) -> i32 {
    if !clsid.is_null() && unsafe { std::slice::from_raw_parts(clsid, 16) } == crate::amcus::CLSID {
        if !out.is_null() {
            let object = crate::amcus::daemon(&crate::amcus::OBJECT);
            unsafe { *out = (object as *const crate::amcus::Object).cast_mut().cast() };
        }
        log!("tc5: in-process AMCUS auth (IAuth)");
        return 0;
    }
    type F = unsafe extern "system" fn(*const u8, *mut c_void, u32, *const u8, *mut *mut c_void) -> i32;
    let real: F = unsafe { std::mem::transmute(CO_CREATE_INSTANCE.load(Ordering::Relaxed)) };
    unsafe { real(clsid, outer, context, iid, out) }
}

/// UE3 keeps the window's last size as `ResX` / `ResY` in the user's
/// `My Games\UnrealEngine3\TimeCrisisGame\Config\TimeCrisisSystemSettings.ini` and starts at it
/// (`-NOINI` does not apply): a window once minimized (160x31) made a 160x120 game. Set to the
/// screen size at each start, before the engine reads it.
fn reset_saved_resolution() {
    let Ok(profile) = std::env::var("USERPROFILE") else { return };
    let path = std::path::Path::new(&profile).join("Documents\\My Games\\UnrealEngine3\\TimeCrisisGame\\Config\\TimeCrisisSystemSettings.ini");
    let Ok(text) = std::fs::read_to_string(&path) else { return };
    unsafe extern "system" {
        fn GetSystemMetrics(index: i32) -> i32;
    }
    let (w, h) = unsafe { (GetSystemMetrics(0), GetSystemMetrics(1)) };
    let fixed: String = text
        .lines()
        .map(|l| {
            if l.starts_with("ResX=") {
                format!("ResX={w}")
            } else if l.starts_with("ResY=") {
                format!("ResY={h}")
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n");
    if fixed.trim_end() != text.trim_end() {
        match std::fs::write(&path, fixed + "\r\n") {
            Ok(()) => log!("tc5: saved resolution set to {w}x{h}"),
            Err(e) => log!("tc5: {}: {e}", path.display()),
        }
    }
}

/// The game's window is a borderless popup (`WAL_WINDOW_POPUP`), but the engine sizes it for a
/// captioned frame (AdjustWindowRect) then fits that frame to the screen: a 1863x1048 game under
/// the top left corner of a 1920x1080 screen (the command line's `FULLSCREEN` is ignored, the
/// cabinet build stays windowed). No frame: the client area is the screen.
fn frameless(base: usize) {
    for (name, f) in [("AdjustWindowRect", adjust_window_rect as *const () as usize), ("AdjustWindowRectEx", adjust_window_rect_ex as *const () as usize)] {
        if unsafe { wal_payload_common::iat::hook_module(base, "user32.dll", name, f) }.is_none() {
            log!("tc5: {name} not hooked");
        }
    }
}

unsafe extern "system" fn adjust_window_rect(rect: *mut c_void, _style: u32, _menu: i32) -> i32 {
    i32::from(!rect.is_null())
}

unsafe extern "system" fn adjust_window_rect_ex(rect: *mut c_void, _style: u32, _menu: i32, _ex_style: u32) -> i32 {
    i32::from(!rect.is_null())
}

/// The Bink movie player's frame copy (0x345d20) locks its plane textures (`[movie+0x180]`,
/// `0x188`, `0x190`) without checking them (only the alpha plane at 0x198 is): entering the
/// operator menu frees them while the movie thread still copies, a null texture read at
/// +0x9e22c4, the game ended and started again by the launcher, in a loop with the test switch
/// on. A null check in the padding before the copy function (0x345bf0), its three unchecked
/// calls sent through it.
fn bink_null_texture(base: usize) {
    // cmp qword [rcx], 0; jne copy; ret
    if !patches::patch("bink null texture check", base, 0x345be5, &[0xCC; 7], &[0x48, 0x83, 0x39, 0x00, 0x75, 0x05, 0xC3]) {
        return;
    }
    for (rva, old, new) in [
        (0x345d87, [0xE8, 0x64, 0xFE, 0xFF, 0xFF], [0xE8, 0x59, 0xFE, 0xFF, 0xFF]),
        (0x345dc5, [0xE8, 0x26, 0xFE, 0xFF, 0xFF], [0xE8, 0x1B, 0xFE, 0xFF, 0xFF]),
        (0x345e03, [0xE8, 0xE8, 0xFD, 0xFF, 0xFF], [0xE8, 0xDD, 0xFD, 0xFF, 0xFF]),
    ] {
        patches::patch("bink plane copy", base, rva, &old, &new);
    }
}

/// Entering the operator menu releases the Direct3D device (`[GDynamicRHI+0x34]`, the RHI at
/// `[0x1841260]`) and makes a new one, while the movie thread renders its frame with it (its
/// render to texture 0x348df0, its clears, its end of drawing): null device reads at +0x9d9261, +0x9d7fa6,
/// the game ended and started again by the launcher. Guards in the unused end of `.text`'s last
/// page (0x1075f8c..0x1076000): that render skipped without a device, and once a first device
/// exists the movie's whole frame (0x19c100, called at 0x19df5a) skipped without one (before
/// it, at boot, the movie must go on: skipping it stalled the boot).
fn bink_no_device(base: usize) {
    // mov rax, [GDynamicRHI]; test rax, rax; je done; cmp qword [rax+0x34], 0; je done;
    // push rdi; sub rsp, 0x30 (its first instructions); jmp 0x348df6; done: ret
    const RENDER_GUARD: [u8; 31] = [0x48, 0x8B, 0x05, 0xC9, 0xB2, 0x7C, 0x00, 0x48, 0x85, 0xC0, 0x74, 0x12, 0x48, 0x83, 0x78, 0x34, 0x00, 0x74, 0x0B, 0x40, 0x57, 0x48, 0x83, 0xEC, 0x30, 0xE9, 0x48, 0x2E, 0x2D, 0xFF, 0xC3];
    if patches::patch("bink render guard", base, 0x1075f90, &[0; 31], &RENDER_GUARD) {
        patches::patch("bink render entry", base, 0x348df0, &[0x40, 0x57, 0x48, 0x83, 0xEC, 0x30], &[0xE9, 0x9B, 0xD1, 0xD2, 0x00, 0x90]);
    }
    // the movie's clears through the RHI (its stop 0x192542, its frame 0x198817): rcx the RHI,
    // rax its vtable. cmp qword [rcx+0x34], 0; je done; jmp [rax+0x570]; done: ret
    const CLEAR_GUARD: [u8; 14] = [0x48, 0x83, 0x79, 0x34, 0x00, 0x74, 0x06, 0xFF, 0xA0, 0x70, 0x05, 0x00, 0x00, 0xC3];
    if patches::patch("bink clear guard", base, 0x1075fd0, &[0; 14], &CLEAR_GUARD) {
        for site in [0x192542, 0x198817] {
            patches::patch("bink clear call", base, site, &[0xFF, 0x90, 0x70, 0x05, 0x00, 0x00], &match site {
                0x192542 => [0xE8, 0x89, 0x3A, 0xEE, 0x00, 0x90],
                _ => [0xE8, 0xB4, 0xD7, 0xED, 0x00, 0x90],
            });
        }
    }
    // the movie target's end of drawing (0x33bc00, after 0x348df0 in its vtable), same as its render
    const END_GUARD: [u8; 31] = [0x48, 0x8B, 0x05, 0x79, 0xB2, 0x7C, 0x00, 0x48, 0x85, 0xC0, 0x74, 0x12, 0x48, 0x83, 0x78, 0x34, 0x00, 0x74, 0x0B, 0x40, 0x57, 0x48, 0x83, 0xEC, 0x30, 0xE9, 0x08, 0x5C, 0x2C, 0xFF, 0xC3];
    if patches::patch("bink end guard", base, 0x1075fe0, &[0; 31], &END_GUARD) {
        patches::patch("bink end entry", base, 0x33bc00, &[0x40, 0x57, 0x48, 0x83, 0xEC, 0x30], &[0xE9, 0xDB, 0xA3, 0xD3, 0x00, 0x90]);
    }
    // mov rax, [GDynamicRHI]; test rax, rax; je done; cmp qword [rax+0x34], 0; je done;
    // jmp frame; done: ret
    const FRAME_GUARD: [u8; 25] = [0x48, 0x8B, 0x05, 0xA9, 0xB2, 0x7C, 0x00, 0x48, 0x85, 0xC0, 0x74, 0x0C, 0x48, 0x83, 0x78, 0x34, 0x00, 0x74, 0x05, 0xE9, 0x38, 0x61, 0x12, 0xFF, 0xC3];
    if !patches::patch("bink frame guard", base, 0x1075fb0, &[0; 25], &FRAME_GUARD) {
        return;
    }
    std::thread::spawn(move || {
        let device = || {
            let rhi = unsafe { *((base + 0x1841260) as *const usize) };
            rhi != 0 && unsafe { std::ptr::read_unaligned((rhi + 0x34) as *const usize) } != 0
        };
        while !device() {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        patches::patch("bink frame call", base, 0x19df5a, &[0xE8, 0xA1, 0xE1, 0xFF, 0xFF], &[0xE8, 0x51, 0x80, 0xED, 0x00]);
    });
}

static GET_DRIVE_TYPE_A: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The launcher keeps its settings, credits and bookkeeping on the cabinet drives F: and G:
/// only when they are removable or fixed disks (0x54450): a dump on a network share (the
/// Batocera roms folder over NFS) made them remote drives, nothing was saved (the operator
/// menu's changes lost). They are fixed disks.
unsafe extern "system" fn get_drive_type_a(root: *const u8) -> u32 {
    const DRIVE_FIXED: u32 = 3;
    if !root.is_null() {
        let letter = unsafe { *root }.to_ascii_uppercase();
        if matches!(letter, b'F' | b'G') && unsafe { *root.add(1) } == b':' {
            return DRIVE_FIXED;
        }
    }
    type F = unsafe extern "system" fn(*const u8) -> u32;
    let real: F = unsafe { std::mem::transmute(GET_DRIVE_TYPE_A.load(Ordering::Relaxed)) };
    unsafe { real(root) }
}

static FOPEN_S: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The operator menu's FREE PLAY is only shown by an offline build: the launcher reads its
/// build from `Data/romVersion.txt` (`TC5<n>-<region>-<RS|NA>-...`), the dump's is the online
/// one. Offline is region 5, region 2 with `NA`, or region 1 with an `X` at offset 12 (the
/// dump's JPN "offline" variant has it at 17: still online). Offline (no Banapass network), the
/// launcher opens the dump's international offline variant instead
/// (`Data/romVersions/romVersion_INT_Offline.txt`, `TC5100-2-NA-...`).
unsafe extern "C" fn fopen_s(file: *mut *mut c_void, name: *const u8, mode: *const u8) -> i32 {
    type F = unsafe extern "C" fn(*mut *mut c_void, *const u8, *const u8) -> i32;
    let real: F = unsafe { std::mem::transmute(FOPEN_S.load(Ordering::Relaxed)) };
    let path = if name.is_null() { String::new() } else { unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy().into_owned() };
    const OFFLINE: &str = "Data/romVersions/romVersion_INT_Offline.txt";
    if path.replace('\\', "/").eq_ignore_ascii_case("Data/romVersion.txt") && std::path::Path::new(OFFLINE).exists() {
        log!("tc5: {path} -> {OFFLINE}");
        return unsafe { real(file, c"Data/romVersions/romVersion_INT_Offline.txt".as_ptr().cast(), mode) };
    }
    unsafe { real(file, name, mode) }
}

/// `WAL_NAMCOES3_BANAPASS=1`: the network the Banapass cards need. The launcher's AMCUS auth
/// daemon (`IAuth`) in-process, the game server (`spp.rs`, players in `F:\tc5srv`) at its
/// ALL.Net uri, the player's card on the reader with the `card` button. The launcher is online
/// and reads the card, but the game does not load its player yet (the touch shows an empty
/// message). Off: offline, network errors 05-20 / 05-34 on the test menu.
fn network(base: usize) {
    let card = crate::bpreader::player_card();
    crate::bpreader::card_button(card);
    crate::amcus::set_identity(crate::amcus::Identity { game_id: "SBVE", game_cd: "TC51", game_ver: (1, 0), serial: HASP_SERIAL_STR, scheme: "" });
    match unsafe { wal_payload_common::iat::hook_module(base, "ole32.dll", "CoCreateInstance", co_create_instance as *const () as usize) } {
        Some(o) => CO_CREATE_INSTANCE.store(o, Ordering::Relaxed),
        None => log!("tc5: CoCreateInstance not hooked"),
    }
    let verbose = std::env::var("WAL_NAMCOES3_SERVER_LOG").is_ok_and(|v| v == "1");
    wal_wm5srv::spp::start(&crate::amcus::game_server(), "F:\\tc5srv", &card.access_code, |line| log!("{line}"), verbose);
}

pub(crate) fn attach(exe: &str, base: usize) {
    if exe.eq_ignore_ascii_case(LAUNCHER) {
        let _ = std::fs::remove_file(test_switch_file());
        crate::lla::status_window(base);
        match unsafe { wal_payload_common::iat::hook_module(base, "kernel32.dll", "GetDriveTypeA", get_drive_type_a as *const () as usize) } {
            Some(o) => GET_DRIVE_TYPE_A.store(o, Ordering::Relaxed),
            None => log!("tc5: GetDriveTypeA not hooked"),
        }
        // the operator menu is this process' window (the game hides its own and hands it its
        // switches): the PC keyboard reached it too, two moves per arrow press. Its message loops
        // (mfc100u.dll's and its own) get no keyboard or mouse messages.
        wal_payload_common::input_filter::init(module("mfc100u.dll"), |_| false);
        wal_payload_common::input_filter::messages(base);
        crate::bpreader::init("COM4", &[base]);
        crate::projector::init("COM1", &[base]);
        if std::env::var("WAL_NAMCOES3_BANAPASS").is_ok_and(|v| v == "1") {
            network(base);
        } else {
            match unsafe { wal_payload_common::iat::hook_module(base, "msvcr100.dll", "fopen_s", fopen_s as *const () as usize) } {
                Some(o) => FOPEN_S.store(o, Ordering::Relaxed),
                None => log!("tc5: fopen_s not hooked, online build"),
            }
        }
        // the game (started by this process, its environment) in a borderless window the
        // size of the screen; this process' own windows keep their sizes
        unsafe {
            std::env::set_var("WAL_WINDOW_POPUP", "1");
            std::env::set_var("WAL_WINDOW_SIZE", "screen");
        }
    } else if exe.eq_ignore_ascii_case(GAME) {
        GAME_BASE.store(base, Ordering::Relaxed);
        reset_saved_resolution();
        frameless(base);
        bink_null_texture(base);
        bink_no_device(base);
        patches::hook_usb_key(base, usb_key_enum);
        usio::init(base, usio_poll);
        wal_payload_common::serial::install_in("COM1", gun_board, &[base]);
        log!("tc5: gun board on COM1");
        // the game's keyboard and mouse (the PC build's controls): from the launcher only
        crate::game_pads_hidden(base);
        wal_payload_common::input_filter::init(base, |_| false);
    } else {
        log!("tc5: loaded in {exe}, nothing to do");
    }
}

