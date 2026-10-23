//! Chase Chase Jokers (`chaseproject.exe`, Unity, Mono, 64-bit): the game drives its cabinet
//! through Konami's AIO libraries, which it calls with `DllImport`s (`aio`, `aio-iob`,
//! `aio-iob2_video`): the BI2X I/O board over USB (the TBS board: stick, buttons, coin mech,
//! test and service switches, lamps, LED tapes, headphone amplifier). The payload is installed
//! as the three libraries beside the executable (`libaio.dll`, `libaio-iob.dll`,
//! `libaio-iob2_video.dll`: the game's folder comes first in the DLL search) and exports what
//! the game imports from them.
//!
//! The three copies are separate modules: the board's objects are opaque pointers the game only
//! passes back, the node manager and controller always ready (stateless answers); the
//! `libaio-iob2_video.dll` copy, which reports the board's status, runs the payload (launcher
//! connection, the trackball of `ccj_trackball.rs`).
//!
//! The board's status (`aioIob2Bi2xTBS_GetDeviceStatus`, the managed `DEVSTATUS`): its INPUT at
//! offset 4. The game swaps the stick's axes (its X is the field `StickY`, high = left; its Y
//! the field `StickX`, high = up); the default calibration is 0..65535 centered on 32768. The
//! coin mech's count is a counter the game follows (each coin adds one).
//!
//! Without the cabinet's `kamunity.dll` (AVS, e-Amusement), the game runs in its local mode
//! (`KamunityInstance.isActive` false): no network, no card login; the profile starts it with
//! `-no_sioman` (no serial devices on COM1: card reader). Layouts cross-checked with spice2x's
//! `games/ccj`.
//!
//! The boot's monitor check (one screen here, the cabinet's two) is skipped (`ccj_mono.rs`).
//!
//! Native inputs for `native_map`: `test service coin dash skill attack jump slide headphone
//! up down left right touch`.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use wal_payload_common::log;
use wal_payload_common::mapping::ButtonMap;
use wal_protocol::Axis;

type P = *mut c_void;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    /// The stick's trigger (`bTrigger1`).
    Dash,
    /// The special button, lit (`bButton0`).
    Skill,
    Attack,
    Jump,
    Slide,
    /// Headphones plugged in.
    Headphone,
    Up,
    Down,
    Left,
    Right,
    /// A touch of the main screen (the pointer's position, `ccj_pointer.rs`).
    Touch,
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("dash", Native::Dash),
    ("skill", Native::Skill),
    ("attack", Native::Attack),
    ("jump", Native::Jump),
    ("slide", Native::Slide),
    ("headphone", Native::Headphone),
    ("up", Native::Up),
    ("down", Native::Down),
    ("left", Native::Left),
    ("right", Native::Right),
    ("touch", Native::Touch),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "attack"),
    ("b2", "jump"),
    ("b3", "slide"),
    ("b4", "skill"),
    ("b5", "dash"),
    ("b7", "touch"),
    ("start", "attack"),
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP))
}

/// Opaque objects handed to the game (never read): one per kind, distinct addresses.
static OBJECTS: [[u64; 4]; 6] = [[0; 4]; 6];

fn object(kind: usize) -> P {
    OBJECTS[kind].as_ptr() as P
}

const SCI: usize = 0;
const NODE_MGR: usize = 1;
const NODE_CTL: usize = 2;
const WRITE_FIRM: usize = 3;

/// `DevIoCounter`: a new status at each read (the game's I/O watchdog).
static IO_COUNTER: AtomicU8 = AtomicU8::new(0);
/// The coin mech's count.
static COINS: AtomicU8 = AtomicU8::new(0);
static COIN_HELD: AtomicBool = AtomicBool::new(false);

/// This copy is `libaio-iob2_video.dll`: it runs the payload.
pub fn detected(module: crate::HINSTANCE) -> bool {
    crate::file_name(module).eq_ignore_ascii_case("libaio-iob2_video.dll")
}

pub fn init() {
    wal_payload_common::start("konami");
    crate::ccj_trackball::init();
    crate::ccj_pointer::init();
    log!("konami: BI2X TBS board and trackball emulated (Chase Chase Jokers)");
}

// ---- libaio.dll ----

#[unsafe(no_mangle)]
pub extern "C" fn aioSystem_Initialize() {}

#[unsafe(no_mangle)]
pub extern "C" fn aioSystem_Finalize() {}

#[unsafe(no_mangle)]
pub extern "C" fn aioNodeCtl_UpdateDevicesStatus() {}

#[unsafe(no_mangle)]
pub extern "C" fn aioNodeMgr_Destroy(_mgr: P) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioNodeMgr_GetState(_mgr: P) -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn aioNodeMgr_IsReady(_mgr: P, _state: i32) -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn aioNodeMgr_IsError(_mgr: P, _state: i32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn aioNodeCtl_Destroy(_ctl: P) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioNodeCtl_GetState(_ctl: P) -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn aioNodeCtl_IsReady(_ctl: P, _state: i32) -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn aioNodeCtl_IsError(_ctl: P, _state: i32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn aioSci_Destroy(_sci: P) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioSci_GetCommStatus(_sci: P, _status: P) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioSciComm_OpenB8PNS1(_port: u32, _baud: u32) -> P {
    object(SCI)
}

// ---- libaio-iob.dll ----

#[unsafe(no_mangle)]
pub extern "C" fn aioNMgrIob2_Create(_sci: P, _mode: u32) -> P {
    log!("ccj: I/O board node manager opened");
    object(NODE_MGR)
}

#[unsafe(no_mangle)]
pub extern "C" fn aioNMgrIob_Create(_sci: P, _mode: u32) -> P {
    object(NODE_MGR)
}

#[unsafe(no_mangle)]
pub extern "C" fn aioNMgrIob_BeginManage(_mgr: P) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioNMgrIob_GetCommStatus(_mgr: P, _status: P) {}

/// NODEINFO: NodeIdSetResult, NodeId, DevId, FIRMINFO (Type[4], Flag, version[3], PCode[4],
/// ...): the board's product code.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aioNCtlIob_GetNodeInfo(_ctl: P, info: *mut u8) {
    if info.is_null() {
        return;
    }
    unsafe {
        std::ptr::write_bytes(info, 0, 0xa3);
        *info.add(1) = 1;
        *info.add(2) = 1;
        std::ptr::copy_nonoverlapping(b"TBS ".as_ptr(), info.add(3 + 8), 4);
    }
}

// ---- libaio-iob2_video.dll ----

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2x_OpenSciUsbCdc(_serial: u32) -> P {
    object(SCI)
}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2x_SciOpenB8PNS1(_ctl: P, _port: u32, _baud: u32) -> P {
    object(SCI)
}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2x_CreateWriteFirmContext(_serial: u32, _iob: u32) -> P {
    object(WRITE_FIRM)
}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2x_DestroyWriteFirmContext(_ctx: P) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2x_WriteFirmGetState(_ctx: P) -> i32 {
    8
}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2x_WriteFirmIsCompleted(_state: i32) -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2x_WriteFirmIsError(_state: i32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2x_SetTapeLedDataGroup(_ctl: P, _group: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2x_SetTapeLedDataLimit(_ctl: P, _channel: u32, _scale: u8, _limit: u8) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xTBS_Create(_mgr: P, _dev: u32) -> P {
    log!("ccj: TBS board opened");
    crate::ccj_mono::skip_monitor_check();
    object(NODE_CTL)
}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xUJK_Create(_mgr: P, _dev: u32) -> P {
    object(NODE_CTL)
}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xAC1_Create(_mgr: P, _dev: u32, _setting: P) -> P {
    object(NODE_CTL)
}

/// The TBS board's status: INPUT at offset 4 of DEVSTATUS (InputCounter, OutputCounter,
/// IoResetCounter, TapeLedCounter first); the input and output data after it are left alone.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aioIob2Bi2xTBS_GetDeviceStatus(_ctl: P, status: *mut u8) {
    if status.is_null() {
        return;
    }
    let input = tbs_input();
    unsafe { std::ptr::copy_nonoverlapping(input.as_ptr(), status.add(4), input.len()) };
}

/// Player 1 touches the screen.
pub(crate) fn touching() -> bool {
    let mut touch = false;
    map().for_each_pressed(&wal_payload_common::input(0), |n| touch |= n == Native::Touch);
    touch
}

/// The TBS INPUT: DevIoCounter, bExIoAErr, bExIoBErr, bPcPowerOn, bPcPowerCheck, CoinCount,
/// bTest, bService, bCoinSw, bCoinJam, bHPDetect, (pad), StickX u16, StickY u16, bStickBtn,
/// bTrigger1, bTrigger2, bButton0..3, (pad).
fn tbs_input() -> [u8; 24] {
    let stick = wal_payload_common::input(0);
    let mut b = [0u8; 24];
    b[0] = IO_COUNTER.fetch_add(1, Ordering::Relaxed);
    b[3] = 1; // PC power on
    // the game's Y (high = up) and X (high = left)
    let mut y = 65535 - stick.axis_u16(Axis::LeftY);
    let mut x = 65535 - stick.axis_u16(Axis::LeftX);
    let mut coin = false;
    map().for_each_pressed(&stick, |n| match n {
        Native::Test => b[6] = 1,
        Native::Service => b[7] = 1,
        Native::Coin => coin = true,
        Native::Headphone => b[10] = 1,
        Native::Dash => b[17] = 1,
        Native::Skill => b[19] = 1,
        Native::Attack => b[20] = 1,
        Native::Jump => b[21] = 1,
        Native::Slide => b[22] = 1,
        Native::Up => y = 65535,
        Native::Down => y = 0,
        Native::Left => x = 65535,
        Native::Right => x = 0,
        Native::Touch => {}
    });
    if coin && !COIN_HELD.swap(true, Ordering::Relaxed) {
        COINS.fetch_add(1, Ordering::Relaxed);
    } else if !coin {
        COIN_HELD.store(false, Ordering::Relaxed);
    }
    b[5] = COINS.load(Ordering::Relaxed);
    b[8] = coin as u8;
    b[12..14].copy_from_slice(&y.to_le_bytes());
    b[14..16].copy_from_slice(&x.to_le_bytes());
    b
}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xAC1_GetDeviceStatus(_ctl: P, _status: P) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xUJK_GetDeviceStatus(_ctl: P, _status: P) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xTBS_IoReset(_ctl: P, _reset: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xAC1_IoReset(_ctl: P, _reset: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xUJK_IoReset(_ctl: P, _reset: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xTBS_SetWatchDogTimer(_ctl: P, _count: u8) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xAC1_SetWatchDogTimer(_ctl: P, _count: u8) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xUJK_SetWatchDogTimer(_ctl: P, _count: u8) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xTBS_ControlCoinBlocker(_ctl: P, _slot: u32, _open: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xUJK_ControlCoinBlocker(_ctl: P, _slot: u32, _open: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xTBS_AddCounter(_ctl: P, _counter: u32, _count: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xAC1_AddCounter(_ctl: P, _counter: u32, _count: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xUJK_AddCounter(_ctl: P, _counter: u32, _count: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xTBS_SetAmpVolume(_ctl: P, _amp: u32, _volume: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xAC1_SetAmpVolume(_ctl: P, _amp: u32, _volume: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xUJK_SetAmpVolume(_ctl: P, _amp: u32, _volume: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xTBS_EnableUsbCharger(_ctl: P, _enable: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xTBS_SetIrLed(_ctl: P, _on: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xTBS_SetButton0Lamp(_ctl: P, _on: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xUJK_SetSkillButtonLamp(_ctl: P, _on: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xTBS_SetIccrLed(_ctl: P, _rgb: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xUJK_SetIccrLed(_ctl: P, _rgb: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xTBS_SetStickLed(_ctl: P, _rgb: u32) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xTBS_SetTapeLedData(_ctl: P, _tape: u32, _data: P) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xUJK_SetTapeLedData(_ctl: P, _tape: u32, _data: P) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xAC1_SetOutputData(_ctl: P, _pin: u32, _data: u8) {}

#[unsafe(no_mangle)]
pub extern "C" fn aioIob2Bi2xAC1_SetTapeLedDataPart(_ctl: P, _ch: u32, _offset: u32, _data: P, _count: u32, _reverse: i32) {}
