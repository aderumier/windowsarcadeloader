//! Wild West Shoot Out (`WAL_ARCADEPC_BOARD=wws`, x86 build, `CowBoy.exe`, Unity 5.6, Mono): the
//! cabinet's hardware without its community plugin (BepInEx), the payload's instead.
//!
//! * **The I/O board** (`LSerialPort`: COM1, 38400 bauds, 8-byte frames `FF <head> <6 bytes>`):
//!   emulated on the game's `SerialPort` (Mono's `DllImport`s of `kernel32`). The game's I/O
//!   check (`5A` and six random bytes) is echoed; the board sends `DD` with each gun's trigger
//!   (bit 0 of byte 1 + its number) and the operator buttons in byte 1 (16 test: the settings,
//!   128 select), each time they change, and `CC <player> <count>` for a coin (its 16 bits in
//!   four nibbles; the game sends it back).
//! * **The guns.** The cabinet's camera (`BaseCamera`, the MindVision SDK) sees each gun's light:
//!   its methods do nothing (`Play`, `InitCamera` and `GetFrameData` true, the SDK's statuses
//!   success), as the plugin's, and none calls the SDK (its DLL does not load under Wine). The
//!   gun's place is `BaseData.Point` (per gun, `BaseGun.FrameData`), turned into screen
//!   fractions by the calibration (`BaseCalibrate.GetFinalScreenPoint`): that is the identity,
//!   `BaseGun.Update` (which parsed the camera's frames) the payload's: each frame it writes
//!   the guns' places as fractions of Unity's picture (`y` from the bottom), the invalid point
//!   (-1, -1) off the picture or while reloading (the game reloads a gun pointed away). The
//!   game's own crosshairs, shots and reloads follow.
//! * **The dongle.** `BaseCom.CheckDog` does nothing (as the plugin's).
//!
//! Reloads are the game's: a shot with the gun pointed away (off the picture); the reload
//! button (b2, the right button) points it away, then shoots.
//!
//! Natives (each player): `trigger`, `offscreen` (`demul.rs`'s), `reload`, `coin`; `test` (the
//! settings), `select` (their item).

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::mono::{self, mono};
use wal_payload_common::{desktop, input, log, mono_imports, serial};

use crate::demul::{client_area, find_window, player_state, resolution};

const PORT: &str = "COM1";
const PLAYERS: usize = 2;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Reload,
    Coin,
    Test,
    Select,
}

/// The board's buttons and the reload button (the guns' aim and trigger are `demul.rs`'s).
const NATIVES: &[(&str, Native)] = &[("reload", Native::Reload), ("coin", Native::Coin), ("test", Native::Test), ("select", Native::Select)];
const DEFAULTS: &[(&str, &str)] = &[("b2", "reload"), ("coin", "coin"), ("test", "test"), ("service", "select")];

/// A reload button press: the gun pointed away this long (the game reloads on a shot after 6
/// physics frames without its point), then a shot this long.
const RELOAD_AWAY: Duration = Duration::from_millis(200);
const RELOAD_SHOT: Duration = Duration::from_millis(60);

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

fn pressed(player: usize, native: Native) -> bool {
    let mut on = false;
    map().for_each_pressed(&input(player), |n| on |= n == native);
    on
}

/// `BaseGun.FrameData`'s offset and `BaseData.Point`'s (0: not found).
static FRAME_DATA: AtomicUsize = AtomicUsize::new(0);
static POINT: AtomicUsize = AtomicUsize::new(0);
/// The guns' places for the game's next frame: `x`, `y` (`f32` bits) per gun.
static PLACES: [AtomicU32; 2 * PLAYERS] = [const { AtomicU32::new(0) }; 2 * PLAYERS];

/// The board: the game's packets (`FF` and 7 bytes) answered.
fn handler(packet: &[u8]) -> Vec<u8> {
    let mut replies = Vec::new();
    for frame in packet.chunks(8) {
        // the I/O check: its bytes back
        if frame.len() == 8 && frame[0] == 0xFF && frame[1] == 0x5A {
            log!("wws: I/O check answered");
            replies.extend_from_slice(frame);
        }
    }
    replies
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    if module == "kernel32.dll" { serial::replacement(name) } else { None }
}

pub fn init() {
    serial::install_in(PORT, handler, &[]);
    mono_imports::hook(resolve);
    std::thread::spawn(patch_game);
    std::thread::spawn(board_thread);
    log!("wws: I/O board on {PORT}, guns into the game's camera data");
}

/// The board's own packets: the buttons on change, coins; the guns' places for the game.
fn board_thread() {
    let mut sent: Option<[u8; 2]> = None;
    let mut coin_was = [false; PLAYERS];
    let (mut reload_was, mut reload_at) = ([false; PLAYERS], [None::<Instant>; PLAYERS]);
    loop {
        std::thread::sleep(Duration::from_millis(4));
        let area = client_area(find_window());
        let mut bytes = [0u8; 2];
        for p in 0..PLAYERS {
            let s = player_state(p, area);
            // the picture's size in the game's units: its desktop (`WAL_DESKTOP_SIZE`), its
            // resolution (`WAL_DEMUL_RESOLUTION`) or its window
            let (rw, rh) = desktop::size()
                .map(|(w, h)| (w as f32, h as f32))
                .or_else(resolution)
                .or_else(|| area.map(|(_, _, w, h)| (w, h)))
                .unwrap_or((1.0, 1.0));
            // the reload button: the gun away, then a shot
            let reload = pressed(p, Native::Reload);
            if reload && !reload_was[p] {
                reload_at[p] = Some(Instant::now());
            }
            reload_was[p] = reload;
            let reloading = reload_at[p].map(|t| t.elapsed()).filter(|e| *e < RELOAD_AWAY + RELOAD_SHOT);
            if reloading.is_none() {
                reload_at[p] = None;
            }
            // off the picture the invalid point (-1, -1): a shot there reloads
            let away = !s.on_picture || reloading.is_some();
            let (x, y) = if away { (-1.0, -1.0) } else { (s.x / rw, s.y / rh) };
            PLACES[2 * p].store(x.to_bits(), Ordering::Relaxed);
            PLACES[2 * p + 1].store(y.to_bits(), Ordering::Relaxed);
            bytes[p] = match reloading {
                Some(e) => (e >= RELOAD_AWAY) as u8,
                None => s.trigger as u8,
            };
            let coin = pressed(p, Native::Coin);
            if coin && !coin_was[p] {
                let n = 1u16;
                serial::push(PORT, &[0xFF, 0xCC, p as u8 + 1, (n >> 12) as u8 & 15, (n >> 8) as u8 & 15, (n >> 4) as u8 & 15, n as u8 & 15, 0]);
                log!("wws: coin, player {}", p + 1);
            }
            coin_was[p] = coin;
        }
        if pressed(0, Native::Test) {
            bytes[0] |= 16;
        }
        if pressed(0, Native::Select) {
            bytes[0] |= 128;
        }
        if sent != Some(bytes) {
            serial::push(PORT, &[0xFF, 0xDD, bytes[0], bytes[1], 0, 0, 0, 0]);
            sent = Some(bytes);
        }
    }
}

/// `BaseGun.Update`: the guns' places written into their camera data.
extern "C" fn gun_update(this: *mut u8) {
    let (frame_data, point) = (FRAME_DATA.load(Ordering::Relaxed), POINT.load(Ordering::Relaxed));
    if this.is_null() || frame_data == 0 || point == 0 {
        return;
    }
    let array = unsafe { mono::read::<usize>(this, frame_data) } as *mut u8;
    if array.is_null() {
        return;
    }
    // a managed array: its length, then its elements
    let len = unsafe { mono::read::<usize>(array, 3 * size_of::<usize>()) }.min(PLAYERS);
    for i in 0..len {
        let data = unsafe { mono::read::<usize>(array, 4 * size_of::<usize>() + i * size_of::<usize>()) } as *mut u8;
        if data.is_null() {
            continue;
        }
        let (x, y) = (f32::from_bits(PLACES[2 * i].load(Ordering::Relaxed)), f32::from_bits(PLACES[2 * i + 1].load(Ordering::Relaxed)));
        unsafe {
            mono::write(data, point, x);
            mono::write(data, point + 4, y);
        }
    }
}

/// `BaseData.get_ScreenPoint` (the crosshairs' places, `FinalPoint` times `Screen.width` and
/// `Screen.height`) in the crosshairs' canvas units: 1920 wide, its height at the picture's
/// aspect (the cabinet's 1920 pixels wide screen; at another resolution they were off the guns).
/// Its `call Screen.get_width; conv.r4` and `call Screen.get_height; conv.r4` made constants
/// (`ldc.r4`, `nop`).
fn crosshair_units(m: &mono::Mono, image: mono::P) {
    let (w, h) = resolution().unwrap_or((1920.0, 1080.0));
    let units = [1920.0f32, 1920.0 * h / w];
    let Some(il) = m.il_code(m.method(m.class(image, "MVSDK", "BaseData"), "get_ScreenPoint", 0)) else {
        log!("wws: BaseData.get_ScreenPoint not found");
        return;
    };
    // call <member reference> (0x28, token 0x0a......), conv.r4 (0x6b)
    let at: Vec<usize> = (0..il.len().saturating_sub(5)).filter(|&i| il[i] == 0x28 && il[i + 4] == 0x0A && il[i + 5] == 0x6B).collect();
    if at.len() != 2 {
        log!("wws: BaseData.get_ScreenPoint: {} screen size calls, unchanged", at.len());
        return;
    }
    for (&i, units) in at.iter().zip(units) {
        il[i] = 0x22;
        il[i + 1..i + 5].copy_from_slice(&units.to_le_bytes());
        il[i + 5] = 0x00;
    }
    log!("wws: the crosshairs in {}x{} canvas units", units[0], units[1]);
    // the camera's jitter filter: the crosshair moved only once 8 units away from its last place
    // (`MVConfig.Instance.Correct`), steps on a gun's smooth moves: its `call
    // MVConfig.get_Instance; ldfld Correct` made `ldc.r4 0` (and nops)
    let at: Vec<usize> = (0..il.len().saturating_sub(9)).filter(|&i| il[i] == 0x28 && il[i + 4] == 0x06 && il[i + 5] == 0x7B && il[i + 9] == 0x04).collect();
    match at[..] {
        [i] => {
            il[i] = 0x22;
            il[i + 1..i + 5].fill(0);
            il[i + 5..i + 10].fill(0);
            log!("wws: the crosshairs follow the guns (no jitter filter)");
        }
        _ => log!("wws: BaseData.get_ScreenPoint: {} jitter thresholds, unchanged", at.len()),
    }
}

/// The game's methods patched (their IL, before their first call) or replaced.
fn patch_game() {
    let start = Instant::now();
    let (m, image) = loop {
        if let Some(m) = mono() {
            let image = m.image("Assembly-CSharp");
            if !image.is_null() {
                break (m, image);
            }
        }
        if start.elapsed() > Duration::from_secs(60) {
            log!("wws: Assembly-CSharp not loaded: the game unpatched");
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    // (namespace, class, method, its arguments, the body's end: the rest made nops)
    const RET: &[u8] = &[0x2A];
    const TRUE: &[u8] = &[0x17, 0x2A];
    // ldc.i4.0 (CAMERA_STATUS_SUCCESS, 0); ret
    const ZERO: &[u8] = &[0x16, 0x2A];
    // ldc.r4 0.0; ret
    const ZERO_F: &[u8] = &[0x22, 0, 0, 0, 0, 0x2A];
    // ldarg.1 (the point); ret
    const SAME: &[u8] = &[0x03, 0x2A];
    let patches: &[(&str, &str, &str, i32, &[u8])] = &[
        ("MVSDK", "BaseCalibrate", "GetFinalScreenPoint", 1, SAME),
        ("", "BaseCom", "CheckDog", 0, RET),
        ("MVSDK", "BaseCamera", "Play", 0, TRUE),
        ("MVSDK", "BaseCamera", "InitCamera", 0, TRUE),
        ("MVSDK", "BaseCamera", "GetFrameData", 2, TRUE),
        ("MVSDK", "BaseCamera", "CaptureThreadProc", 0, RET),
        ("MVSDK", "BaseCamera", "Pause", 0, RET),
        ("MVSDK", "BaseCamera", "Close", 0, RET),
        ("MVSDK", "BaseCamera", "GetImageResolution", 4, RET),
        ("MVSDK", "BaseCamera", "LoadDefaultConfig", 0, RET),
        // the SDK's other calls (its DLL does not load under Wine: an exception stopped the gun
        // system's start)
        ("MVSDK", "BaseCamera", "UpdateExposure", 0, RET),
        ("MVSDK", "BaseCamera", "SetImageResolution", 4, ZERO),
        ("MVSDK", "BaseCamera", "SetMonochrome", 1, RET),
        ("MVSDK", "BaseCamera", "SetExposureState", 1, ZERO),
        ("MVSDK", "BaseCamera", "set_ExposureTime", 1, RET),
        ("MVSDK", "BaseCamera", "set_AnalogGain", 1, RET),
        ("MVSDK", "BaseCamera", "set_FrameSpeed", 1, RET),
        ("MVSDK", "BaseCamera", "get_RealExposureTime", 0, ZERO_F),
        ("MVSDK", "BaseCamera", "get_RealAnalogGain", 0, ZERO),
    ];
    for &(namespace, class, name, args, body) in patches {
        match m.il_code(m.method(m.class(image, namespace, class), name, args)) {
            Some(il) if il.len() >= body.len() => {
                il.fill(0x00);
                let n = il.len();
                il[n - body.len()..].copy_from_slice(body);
                log!("wws: {class}.{name} patched");
            }
            _ => log!("wws: {class}.{name} not found"),
        }
    }
    // compiling a method (its replacement) needs a thread of Mono's
    while m.domain().is_null() || !m.attach() {
        std::thread::sleep(Duration::from_millis(1));
    }
    crosshair_units(m, image);
    let gun = m.class(image, "MVSDK", "BaseGun");
    FRAME_DATA.store(m.offset(m.field(gun, "FrameData")), Ordering::Relaxed);
    POINT.store(m.offset(m.field(m.class(image, "MVSDK", "BaseData"), "Point")), Ordering::Relaxed);

    let replaced = m.replace(m.method(gun, "Update", 0), gun_update as *const () as usize);
    log!(
        "wws: BaseGun.Update {} (FrameData +{:#x}, Point +{:#x})",
        if replaced { "replaced" } else { "not replaced" },
        FRAME_DATA.load(Ordering::Relaxed),
        POINT.load(Ordering::Relaxed)
    );
}
