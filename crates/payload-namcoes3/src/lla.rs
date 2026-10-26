//! Lost Land Adventure (2014, Unreal Engine 3, 64-bit): `RSLauncher.exe` (cabinet launcher,
//! test menu) starts `LLA\Binaries\Win64\DomeShooterGame-Win64-Shipping.exe`.
//!
//! Launcher: the I/O board (JVS, `wajvio.dll` on COM3, `jvs.rs`), the Banapass reader (COM4,
//! `bpreader.rs`; the international ROM, `Data\romVersion.txt` "LLA100-1", wants one), the
//! language (`WAL_NAMCOES3_LANGUAGE`), the NEC projector (COM1, `projector.rs`), the status
//! window (hidden while small).
//!
//! Game: the dongle thread (0xa30950) checks every 3 seconds either a USB key (the
//! enumeration at 0x20b0, id 0x0c10) or the HASP (0xa2ac80: 0x40 bytes at 0xd00, byte 0x3f the
//! complement of 0x3e, then 13 bytes against the serial template 0x16b3248, '*' matching any
//! character). Both answer [`SERIAL`].
//!
//! Guns: the cabinet's gun board is a camera on COM2 (115200 bauds) streaming 12-byte packets
//! `5F player x y buttons F5` (x, y: f32 camera coordinates, 0..1 across its view; buttons:
//! bit 0 player 1's trigger, bit 2 player 2's, as `GunPeripheral1PTriggerMask` /
//! `GunPeripheral2PTriggerMask`, bit 6 camera error). The game (0xa2eec0) maps them through the
//! dome geometry to a point on the dome (0xa2e570) and a projector position (0xa2e820, 0..1);
//! the crosshair it draws is elsewhere (a yaw / pitch from the eye, [`crosshair`]). The
//! payload inverts the camera -> crosshair mapping (Newton steps through the game's
//! 0xa2e570), so each gun aims where it points on the picture. The game writes 2 bytes back
//! per poll (gun outputs).

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::log;
use wal_payload_common::mapping::ButtonMap;
use wal_protocol::{Axis, StickState};

use crate::{GAME_BASE, module, patches};

/// This library's name in this game's dump.
const HASP: &str = "hasp_windows_x64_106993.dll";
const LAUNCHER: &str = "RSLauncher.exe";
const GAME: &str = "DomeShooterGame-Win64-Shipping.exe";

/// The cabinet serial (HASP and USB key). The launcher's gun controller check (0x68ad0, error
/// 06-01) wants "2728", then characters 8 and 9 '0', 10 '0'..'4', 11 not '0'; the game its
/// template ("27282*15****").
pub(crate) const SERIAL: &[u8; 12] = b"272820150001";

/// The game's gun parameters (dome geometry, window size), set when it opens the port.
const GUN_PARAMS: usize = 0x181ff70;

pub(crate) fn is_lla() -> bool {
    module(HASP) != 0
}

unsafe extern "C" fn usb_key_enum(_a: i32, _b: i32, _c: u16, _d: u16, out: *mut u8) -> i32 {
    unsafe { patches::usb_key(out, 0x0c10, SERIAL) };
    1
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Native {
    Trigger,
}

const NATIVES: &[(&str, Native)] = &[("trigger", Native::Trigger)];
const DEFAULT_MAP: &[(&str, &str)] = &[("b1", "trigger")];

struct Guns {
    maps: [ButtonMap<Native>; 2],
    /// Last camera coordinates of each gun (the next solve's start).
    camera: [(f32, f32); 2],
}

static GUNS: Mutex<Option<Guns>> = Mutex::new(None);

/// The game's direction (a point on the dome) for gun camera coordinates `(x, y)`. `None`
/// before the game set its parameters.
fn direction(x: f32, y: f32) -> Option<[f32; 3]> {
    let base = crate::game_base();
    let params = base + GUN_PARAMS;
    // the projector image size, set with the rest
    let size = unsafe { *((params + 0x50) as *const [i32; 2]) };
    if size[0] <= 0 || size[1] <= 0 {
        return None;
    }
    type ToDirection = unsafe extern "C" fn(usize, *mut [f32; 3], u64) -> usize;
    let to_direction: ToDirection = unsafe { std::mem::transmute(base + 0xa2e570) };
    let mut direction = [0f32; 3];
    let xy = (x.to_bits() as u64) | (y.to_bits() as u64) << 32;
    unsafe { to_direction(params, &mut direction, xy) };
    Some(direction)
}

/// Where the game draws the crosshair (screen, 0..1 from the top left corner) for a dome
/// point: a yaw / pitch seen from the eye, measured on the game's crosshair through the
/// patched dome texture (`dome.rs`; fitted on a 5 x 5 grid, residuals under 0.5% of the
/// screen), then placed on the payload's flat view.
fn crosshair(d: [f32; 3]) -> (f32, f32) {
    let (x, y) = crosshair_patched_view(d);
    crate::dome::from_patched_view(x, y)
}

fn crosshair_patched_view(d: [f32; 3]) -> (f32, f32) {
    const EYE_Y: f32 = 0.3751;
    const EYE_Z: f32 = -0.1338;
    const YAW_SCALE: f32 = 1.0985;
    const PITCH_SCALE: f32 = 1.5159;
    const PITCH: f32 = 0.4233;
    const WIDTH: f32 = 0.2812;
    const HEIGHT: f32 = 0.3921;
    let (x, y, z) = (d[0], d[1] - EYE_Y, d[2] - EYE_Z);
    let yaw = x.atan2(z) * YAW_SCALE;
    let pitch = y.atan2(x.hypot(z)) * PITCH_SCALE + PITCH;
    (0.5 + WIDTH * yaw.tan(), 0.5 - HEIGHT * pitch.tan() / yaw.cos())
}

fn forward(x: f32, y: f32) -> Option<(f32, f32)> {
    direction(x, y).map(crosshair)
}

/// Camera coordinates whose crosshair is at `target`, from `start` (Newton steps).
fn solve(target: (f32, f32), start: (f32, f32)) -> (f32, f32) {
    const STEP: f32 = 1e-3;
    let mut p = start;
    for _ in 0..8 {
        let (Some(f), Some(fx), Some(fy)) = (forward(p.0, p.1), forward(p.0 + STEP, p.1), forward(p.0, p.1 + STEP)) else {
            return start;
        };
        let (ex, ey) = (f.0 - target.0, f.1 - target.1);
        if ex.abs() < 1e-4 && ey.abs() < 1e-4 {
            break;
        }
        // Jacobian columns, then J d = -e
        let (a, c) = ((fx.0 - f.0) / STEP, (fx.1 - f.1) / STEP);
        let (b, d) = ((fy.0 - f.0) / STEP, (fy.1 - f.1) / STEP);
        let det = a * d - b * c;
        if !det.is_normal() {
            break;
        }
        let dx = (-ex * d + ey * b) / det;
        let dy = (ex * c - ey * a) / det;
        p = ((p.0 + dx).clamp(-1.0, 2.0), (p.1 + dy).clamp(-1.0, 2.0));
    }
    p
}

/// The data the board streams, asked when the game polls the port: one packet per gun.
fn gun_packets() -> Vec<u8> {
    let mut guard = GUNS.lock().unwrap();
    let guns = guard.get_or_insert_with(|| Guns {
        maps: [ButtonMap::new(NATIVES, DEFAULT_MAP), ButtonMap::with_defaults(NATIVES, DEFAULT_MAP)],
        camera: [(0.5, 0.5); 2],
    });
    let mut buttons = 0u8;
    let mut aims = [(0f32, 0f32); 2];
    for player in 0..2 {
        let stick: StickState = wal_payload_common::input(player);
        guns.maps[player].for_each_pressed(&stick, |n| match n {
            Native::Trigger => buttons |= [0x01, 0x04][player],
        });
        // the gun's position on the screen, 0..1 from the top left corner
        let unit = |a: Axis| (stick.axis(a) as f32 + 32768.0) / 65535.0;
        let target = (unit(Axis::LeftX), unit(Axis::LeftY));
        guns.camera[player] = solve(target, guns.camera[player]);
        aims[player] = guns.camera[player];
    }
    let mut out = Vec::with_capacity(24);
    for (player, (x, y)) in aims.into_iter().enumerate() {
        out.extend_from_slice(&[0x5F, player as u8]);
        out.extend_from_slice(&x.to_le_bytes());
        out.extend_from_slice(&y.to_le_bytes());
        out.extend_from_slice(&[buttons, 0xF5]);
    }
    out
}

static CREATE_WINDOW: AtomicUsize = AtomicUsize::new(0);
static SHOW_WINDOW: AtomicUsize = AtomicUsize::new(0);
static SET_WINDOW_POS: AtomicUsize = AtomicUsize::new(0);
/// The launcher's status window.
static STATUS_WINDOW: AtomicUsize = AtomicUsize::new(0);

/// The launcher's top-level window: a 256x60 status overlay (network icon) at the top left,
/// topmost, full screen in test mode (0x62ee0: the test menu, the game window shrunk). The
/// overlay took the focus from the game, and GNOME pinged it ("not responding"): it is
/// created as a borderless popup and kept hidden while it is small.
unsafe extern "system" fn create_window(
    ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: usize, menu: usize, inst: usize, param: usize,
) -> usize {
    const WS_POPUP: u32 = 0x8000_0000;
    const WS_CHILD: u32 = 0x4000_0000;
    let top = parent == 0 && style & WS_CHILD == 0;
    let style = if top { WS_POPUP } else { style };
    type CreateWindowExW = unsafe extern "system" fn(u32, *const u16, *const u16, u32, i32, i32, i32, i32, usize, usize, usize, usize) -> usize;
    let orig: CreateWindowExW = unsafe { std::mem::transmute(CREATE_WINDOW.load(Ordering::Relaxed)) };
    let hwnd = unsafe { orig(ex, class, title, style, x, y, w, h, parent, menu, inst, param) };
    if top {
        STATUS_WINDOW.store(hwnd, Ordering::Relaxed);
    }
    hwnd
}

unsafe extern "system" fn show_window(hwnd: usize, cmd: i32) -> i32 {
    let orig: unsafe extern "system" fn(usize, i32) -> i32 = unsafe { std::mem::transmute(SHOW_WINDOW.load(Ordering::Relaxed)) };
    // shown by SetWindowPos when it goes full screen
    let cmd = if hwnd != 0 && hwnd == STATUS_WINDOW.load(Ordering::Relaxed) { 0 } else { cmd };
    unsafe { orig(hwnd, cmd) }
}

unsafe extern "system" fn set_window_pos(hwnd: usize, after: isize, x: i32, y: i32, w: i32, h: i32, flags: u32) -> i32 {
    const SWP_NOACTIVATE: u32 = 0x10;
    const SWP_SHOWWINDOW: u32 = 0x40;
    const SWP_HIDEWINDOW: u32 = 0x80;
    let orig: unsafe extern "system" fn(usize, isize, i32, i32, i32, i32, u32) -> i32 = unsafe { std::mem::transmute(SET_WINDOW_POS.load(Ordering::Relaxed)) };
    let flags = if hwnd != 0 && hwnd == STATUS_WINDOW.load(Ordering::Relaxed) {
        if h >= 800 { (flags | SWP_SHOWWINDOW) & !SWP_HIDEWINDOW } else { (flags | SWP_HIDEWINDOW | SWP_NOACTIVATE) & !SWP_SHOWWINDOW }
    } else {
        flags
    };
    unsafe { orig(hwnd, after, x, y, w, h, flags) }
}

pub(crate) fn attach(exe: &str, base: usize) {
    if exe.eq_ignore_ascii_case(LAUNCHER) {
        let libs: Vec<usize> = [base, module("wajvio.dll")].into_iter().filter(|m| *m != 0).collect();
        crate::jvs::init(&libs);
        crate::bpreader::init("COM4", &libs);
        for (name, f, slot) in [
            ("CreateWindowExW", create_window as *const () as usize, &CREATE_WINDOW),
            ("ShowWindow", show_window as *const () as usize, &SHOW_WINDOW),
            ("SetWindowPos", set_window_pos as *const () as usize, &SET_WINDOW_POS),
        ] {
            match unsafe { wal_payload_common::iat::hook_module(base, "user32.dll", name, f) } {
                Some(o) => slot.store(o, Ordering::Relaxed),
                None => log!("lla: {name} not hooked"),
            }
        }
        crate::projector::init("COM1", &libs);
        // [0x341084] = operator language 1..8 -> "-Language=JPN..THA", else English
        patches::language(base, 0x2c28e, b"\x8b\x05\xf0\x4d\x31\x00");
    } else if exe.eq_ignore_ascii_case(GAME) {
        GAME_BASE.store(base, std::sync::atomic::Ordering::Relaxed);
        patches::hook_usb_key(base, usb_key_enum);
        wal_payload_common::serial::install_streaming_in("COM2", |_| Vec::new(), gun_packets, &[base]);
        crate::dome::init(base);
    } else {
        log!("lla: loaded in {exe}, nothing to do");
    }
}
