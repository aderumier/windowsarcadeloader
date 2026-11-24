//! Powerboat GT (Injoy Motion, Aquadelic GT engine, `Run-ENG-V1.7.exe`, Delphi, OpenGL,
//! 32-bit): `WAL_ARCADEPC_BOARD=powerboat`, x86 build.
//!
//! - **Board.** On the cabinet a separate program (`PBZ360-GAMEINTERFACE`) read the wheel,
//!   throttle, buttons and coins, and the Z360 motion seat program drove the seat. Both talked
//!   to the game through `MemAPI32.dll`'s shared memory ("IOIMAGE2", `IoImage.tab`). The game
//!   loads `Memapi32.dll` by name and only uses `GDEGetBit` / `GDESetBit` with register 0x30
//!   (one dword per index). The payload is that library (`payload_install`): its own dword
//!   table, and the two other programs' parts answered from the virtual stick.
//! - **Inputs.** 0x1c0 the wheel (0..65535, 0x8000 centred, higher right; the game's dead zone
//!   of ±10000 is skipped so a pad steers at once), 0x1c1 the throttle (0x8000 at rest, lower
//!   forward), 0x1c3 a second axis (centred), 0x1c4 start / OK, 0x1c5 a second button, 0x1c6
//!   nitro, 0x1c7 the camera (an index the game follows, stepped by the view button).
//! - **Interface.** Credits from the coins (`..\OpMenu\GameInfo.db`: `CoinPlay`, `CoinCtu`)
//!   or free play: start with credits sets 0x25c = 3 and 0x24 = 3 (the game starts), the
//!   game's 0x24 = 5 (game over) is shown until it clears 0x25c (3 s), then 0x24 = 0 (attract), its continue screen (0x1c8 = 1)
//!   continues on start with credits (0x1c8 = 2). 0x1cf holds credits + 1, 0x1d0 the coins.
//! - **Motion seat.** Ready at once (0x24 and 0x25c 0: attract; any other pair shows the
//!   game's waiting picture), home at a game's start (0x1ae = 2: the race intro's timer only
//!   counts then, at 0 the boat stays held at the start line); the game's race start request (0x1ae = 1) is answered at once (2: the
//!   seat is home); the seatbelt (0x1c9) and sensor (0x1e0) warnings stay off.
//!
//! - **Videos.** The boat selection AVIs are MS-MPEG4 v2 (`MP42`, written by ffdshow), played
//!   through Video for Windows (`DirectorAVI.dll`): the runner's `mpg4c32.dll` (GE-Proton patch
//!   `0136-mpg4c32-add-a-microsoft-mpeg-4-video-for-windows-decoder`) decodes them, registered by
//!   the profile's `WAL_VFW_CODECS` for prefixes made before it. Without it the game crashes at
//!   the boat selection (a NULL frame in `DirectorAVI.dll`).
//!
//! Natives: `start nitro button2 view coin accel brake left right`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log};
use wal_protocol::Axis;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Start,
    Nitro,
    Button2,
    View,
    Coin,
    Accel,
    Brake,
    Left,
    Right,
}

const NATIVES: &[(&str, Native)] = &[
    ("start", Native::Start),
    ("nitro", Native::Nitro),
    ("button2", Native::Button2),
    ("view", Native::View),
    ("coin", Native::Coin),
    ("accel", Native::Accel),
    ("brake", Native::Brake),
    ("left", Native::Left),
    ("right", Native::Right),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("start", "start"),
    ("b3", "nitro"),
    ("b4", "button2"),
    ("b5", "view"),
    ("coin", "coin"),
    ("b1", "accel"),
    ("b2", "brake"),
    ("left", "left"),
    ("right", "right"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

fn held() -> [bool; NATIVES.len()] {
    let mut held = [false; NATIVES.len()];
    map().for_each_pressed(&input(0), |n| held[n as usize] = true);
    held
}

// register 0x30 indexes
const STATE: usize = 0x24;
const START_DONE: usize = 0x18e;
const RACE_START: usize = 0x1ae;
const WHEEL: usize = 0x1c0;
const THROTTLE: usize = 0x1c1;
const AXIS2: usize = 0x1c3;
const BTN_START: usize = 0x1c4;
const BTN_2: usize = 0x1c5;
const BTN_NITRO: usize = 0x1c6;
const CAMERA: usize = 0x1c7;
const CONTINUE: usize = 0x1c8;
const SEATBELT: usize = 0x1c9;
const ABORT: usize = 0x1ca;
const CREDITS: usize = 0x1cf;
const COINS: usize = 0x1d0;
const SENSOR: usize = 0x1e0;
const EVENT: usize = 0x191;
const SEAT: usize = 0x25c;

const CELLS: usize = 0x1000;
static MEM: [AtomicU32; CELLS] = [const { AtomicU32::new(0) }; CELLS];

fn get(idx: usize) -> u32 {
    MEM[idx].load(Ordering::Relaxed)
}

fn set(idx: usize, v: u32) {
    MEM[idx].store(v, Ordering::Relaxed);
}

struct Settings {
    free_play: bool,
    coin_play: u32,
    coin_continue: u32,
    /// The game's seat value for a started game (3, or 2 when System.ini's GameStatus bit 1).
    seat: u32,
}

static SETTINGS: OnceLock<Settings> = OnceLock::new();

fn settings() -> &'static Settings {
    SETTINGS.get_or_init(|| Settings { free_play: true, coin_play: 1, coin_continue: 1, seat: 3 })
}

/// `key= value` of a GameInfo.db / System.ini text.
fn value(text: &str, key: &str) -> Option<u32> {
    text.lines().find_map(|l| {
        let (k, v) = l.split_once('=')?;
        (k.trim().eq_ignore_ascii_case(key)).then(|| v.split_whitespace().next()?.parse().ok())?
    })
}

/// The interface programs' state, kept between polls.
struct Interface {
    credits: u32,
    coins: u32,
    coin_held: bool,
    start_held: bool,
    view_held: bool,
    camera: u32,
}

static IFACE: std::sync::Mutex<Interface> =
    std::sync::Mutex::new(Interface { credits: 0, coins: 0, coin_held: false, start_held: false, view_held: false, camera: 0 });

/// The wheel: 0x8000 + the stick past the game's ±10000 dead zone (or full lock).
fn wheel(on: &[bool]) -> u32 {
    let lx = input(0).axis(Axis::LeftX) as i32;
    let lx = if on[Native::Left as usize] {
        -32767
    } else if on[Native::Right as usize] {
        32767
    } else {
        lx
    };
    const DEAD: i32 = 1500;
    let v = if lx.abs() < DEAD { 0 } else { lx.signum() * (10000 + (lx.abs() - DEAD) * 22767 / (32767 - DEAD)) };
    (0x8000 + v).clamp(0, 0xffff) as u32
}

/// The throttle lever: 0x8000 at rest, lower forward, higher back (the interface's
/// 1000..65535; the game's throttle is (0x8000 - raw) / 32768, positive forward).
fn throttle(on: &[bool]) -> u32 {
    let stick = input(0);
    let accel = if on[Native::Accel as usize] { 32767 } else { stick.axis(Axis::Accel).max(0) as i32 };
    let brake = if on[Native::Brake as usize] { 32767 } else { stick.axis(Axis::Brake).max(0) as i32 };
    (0x8000 - accel + brake).clamp(1000, 0xffff) as u32
}

/// One poll of the interface programs, before each read of the game.
fn update() {
    let on = held();
    let s = settings();
    let mut i = IFACE.lock().unwrap();
    let coin = on[Native::Coin as usize];
    if coin && !i.coin_held {
        i.coins += 1;
        i.credits += 1;
        log!("powerboat: coin, credits {}", i.credits);
    }
    i.coin_held = coin;
    let start = on[Native::Start as usize];
    let pressed = start && !i.start_held;
    i.start_held = start;
    let view = on[Native::View as usize];
    if view && !i.view_held {
        i.camera = (i.camera + 1) % 4;
    }
    i.view_held = view;

    // a game over: its picture until the game clears the seat (3 s), then attract
    if get(STATE) == 5 && get(SEAT) == 0 {
        set(STATE, 0);
        log!("powerboat: game over");
    }
    // start in attract
    if pressed && get(STATE) == 0 && get(SEAT) == 0 {
        if s.free_play || i.credits >= s.coin_play {
            if !s.free_play {
                i.credits -= s.coin_play;
            }
            set(SEAT, s.seat);
            set(STATE, 3);
            // the seat is home: the first race's intro runs (its timer only counts while 0x1ae
            // is 2; at 0 the boat is held at the start)
            set(RACE_START, 2);
            log!("powerboat: start, credits {}", i.credits);
        }
    }
    // the continue screen
    if pressed && get(CONTINUE) == 1 && (s.free_play || i.credits >= s.coin_continue) {
        if !s.free_play {
            i.credits -= s.coin_continue;
        }
        set(CONTINUE, 2);
        log!("powerboat: continue, credits {}", i.credits);
    }
    // the motion seat: home at once
    if get(RACE_START) == 1 {
        set(RACE_START, 2);
    }
    set(WHEEL, wheel(&on));
    set(THROTTLE, throttle(&on));
    set(AXIS2, 0x8000);
    set(BTN_START, start as u32);
    set(BTN_2, on[Native::Button2 as usize] as u32);
    set(BTN_NITRO, on[Native::Nitro as usize] as u32);
    set(CAMERA, i.camera);
    set(SEATBELT, 0);
    set(SENSOR, 0);
    set(ABORT, 0);
    set(START_DONE, 0);
    set(CREDITS, if s.free_play { 1 } else { i.credits + 1 });
    set(COINS, i.coins);
    // the interface took the game's event (nitro, hit)
    set(EVENT, 0);
}

static INIT: AtomicBool = AtomicBool::new(false);

#[unsafe(no_mangle)]
pub extern "C" fn pb_initialize_vram() -> i32 {
    if !INIT.swap(true, Ordering::Relaxed) {
        log!("powerboat: InitializeVRAM");
        for idx in [WHEEL, THROTTLE, AXIS2] {
            set(idx, 0x8000);
        }
    }
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_close_vram() -> i32 {
    1
}

/// `GDEGetBit(reg, idx, out)`: register 0x30 (dwords); the other registers read 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pb_get_bit(reg: u8, idx: u32, out: *mut u32) -> u32 {
    if out.is_null() {
        return 0;
    }
    let idx = idx as usize;
    if reg != 0x30 || idx >= CELLS {
        unsafe { *out = 0 };
        return (reg == 0x30) as u32 ^ 1;
    }
    if idx == WHEEL {
        update();
    }
    unsafe { *out = get(idx) };
    1
}

/// `GDESetBit(reg, idx, value)`.
#[unsafe(no_mangle)]
pub extern "C" fn pb_set_bit(reg: u8, idx: u32, v: u32) -> u32 {
    let idx = idx as usize;
    if reg != 0x30 || idx >= CELLS {
        return 0;
    }
    match idx {
        // live inputs: the game's recentring is the stick's
        WHEEL | THROTTLE | AXIS2 => {}
        STATE | RACE_START | CONTINUE | SEAT => {
            if get(idx) != v {
                log!("powerboat: game sets {idx:#x} = {v}");
            }
            set(idx, v);
        }
        _ => set(idx, v),
    }
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_get_bit_ex(_reg: i8, _idx: u32, out: *mut u32) -> u32 {
    if !out.is_null() {
        unsafe { *out = 0 };
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_set_bit_ex(_reg: i8, _idx: u32, _v: u32) -> u32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_get_config(_reg: u8, _what: u8, out: *mut u32) -> u32 {
    if !out.is_null() {
        unsafe { *out = 0 };
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn pb_get_pointer(_reg: u8) -> *mut u8 {
    std::ptr::null_mut()
}

/// The CPU check of one of the game's unit initializations (at 0x5f5791, run before the
/// others) brackets `cpuid` with 16-bit `pushaw` / `popaw`: the high words of the registers
/// keep `cpuid`'s values. A CPU whose extended leaf 0x80000001 returns EBX bits above 15 (a
/// Ryzen's package type) leaves the unit loop's counter (EBX) past the unit count, the loop
/// ends there and the later units never initialize (their objects nil: the boot's access
/// violation at 0x5d2c3e). 32-bit `pusha` / `popa` instead (the payload is loaded at the
/// executable's entry point, before the loop).
unsafe fn fix_cpu_check() {
    unsafe {
        patch(0x5f5791, &[0x66, 0x60], &[0x60, 0x90]);
        patch(0x5f57b7, &[0x66, 0x61], &[0x61, 0x90]);
    }
    log!("powerboat: CPU check pushaw / popaw -> pusha / popa");
    // Engine.ini PauseNonActive=1: the game pauses when its window loses the focus (its
    // deactivation handler, 0x5ef61c), the loading screen stays still behind another window on
    // a desktop; a cabinet never loses it: never paused
    if unsafe { patch(0x5ef626, &[0x74, 0x5f], &[0xeb, 0x5f]) } {
        log!("powerboat: no pause when the window loses the focus");
    }
}

/// Writes `bytes` at `at` when it holds `old` (this build); whether it did.
unsafe fn patch(at: usize, old: &[u8], bytes: &[u8]) -> bool {
    let p = at as *mut u8;
    let current = unsafe { std::slice::from_raw_parts(p, old.len()) };
    if current != old {
        log!("powerboat: {at:#x} holds {current:02x?}, not {old:02x?}: not patched");
        return false;
    }
    let mut prot = 0;
    unsafe {
        VirtualProtect(p.cast(), bytes.len(), PAGE_EXECUTE_READWRITE, &mut prot);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        VirtualProtect(p.cast(), bytes.len(), prot, &mut prot);
    }
    true
}

/// `Profiles.xml` cut after its root element; `None`: nothing after it.
fn profiles_fixed(xml: &[u8]) -> Option<Vec<u8>> {
    const END: &[u8] = b"</Profiles>";
    let at = xml.windows(END.len()).rposition(|w| w == END)? + END.len();
    let eol = xml[at..].iter().take_while(|b| b.is_ascii_whitespace()).count();
    (at + eol < xml.len()).then(|| xml[..at + eol].to_vec())
}

/// The game's save, `Game\Profiles.xml`: the cabinet's pilots (the attract's pilot choice; a
/// game without one asks for a new profile with the keyboard). It is signed (its `Integrity`
/// attribute); one that fails the check shows a message box ("Game state file was corrupted")
/// and the game falls back to no pilot. The dump's was edited by hand (its signature does not
/// match) and ends with a second XML declaration that MSXML refuses (EDOMParseError at boot):
/// the declaration is removed from the file and the signature check passes (`jne` -> `jmp`).
unsafe fn fix_profiles(save: &std::path::Path) {
    if unsafe { patch(0x536293, &[0x75, 0x26], &[0xeb, 0x26]) } {
        log!("powerboat: Profiles.xml signature not checked");
    }
    if let Some(fixed) = std::fs::read(save).ok().and_then(|x| profiles_fixed(&x)) {
        match std::fs::write(save, fixed) {
            Ok(()) => log!("powerboat: Profiles.xml: text after </Profiles> removed"),
            Err(e) => log!("powerboat: Profiles.xml not written: {e}"),
        }
    }
}

pub fn init() {
    let dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf()));
    let info = dir.as_ref().and_then(|d| std::fs::read_to_string(d.join("..").join("OpMenu").join("GameInfo.db")).ok()).unwrap_or_default();
    let system = dir.as_ref().and_then(|d| std::fs::read_to_string(d.join("System.ini")).ok()).unwrap_or_default();
    let free_play = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY").unwrap_or(value(&info, "FreePlay") == Some(1));
    let status = value(&system, "GameStatus").unwrap_or(5);
    let s = Settings {
        free_play,
        coin_play: value(&info, "CoinPlay").unwrap_or(1).max(1),
        coin_continue: value(&info, "CoinCtu").unwrap_or(1).max(1),
        seat: if status & 2 != 0 { 2 } else { 3 },
    };
    log!("powerboat: Memapi32.dll, free play {}, coins per game {}, per continue {}, seat {}", s.free_play, s.coin_play, s.coin_continue, s.seat);
    let _ = SETTINGS.set(s);
    unsafe { fix_cpu_check() };
    // the boat videos' MP42 codec (the runner's mpg4c32.dll, WAL_VFW_CODECS)
    wal_payload_common::vfw::init();
    if let Some(dir) = dir {
        unsafe { fix_profiles(&dir.join("Game").join("Profiles.xml")) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values() {
        let info = "Language= 0\r\nCoinPlay= 4\r\nCoinCtu= 2\r\nCoinPulse= 1 1\r\nFreePlay= 0\r\n";
        assert_eq!(value(info, "CoinPlay"), Some(4));
        assert_eq!(value(info, "coinctu"), Some(2));
        assert_eq!(value(info, "CoinPulse"), Some(1));
        assert_eq!(value(info, "Missing"), None);
    }

    #[test]
    fn profiles() {
        let bad = b"<?xml?>\r\n<Profiles>\r\n</Profiles>\r\n<?xml?>\r\n";
        assert_eq!(profiles_fixed(bad).unwrap(), b"<?xml?>\r\n<Profiles>\r\n</Profiles>\r\n");
        assert_eq!(profiles_fixed(b"<Profiles>\r\n</Profiles>\r\n"), None);
        assert_eq!(profiles_fixed(b"<Profiles/>"), None);
    }
}
