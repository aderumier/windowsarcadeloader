//! The "Type 4" USB I/O board (837-15257, `I/O CONTROL BD`): a HID device amdaemon finds
//! with SetupDi (vendor 0CA3) and reads 64-byte input reports from (segatools' `board/io4.c`,
//! public domain):
//!
//! report 01: adc[8], rotary[4], coin chutes[2] (count << 8), switches[2] (players 1 and 2:
//! bit 9 test, 6 service; the rest per game), system status, USB status.
//!
//! Output report 10: command byte (01 timeout, 02 sampling count, 03 clear status, 04 general
//! outputs: the lamps, 05 PWM, 41 unique outputs).
//!
//! The game's controls (`WAL_ALLS_BOARD`), from the virtual sticks:
//! - `tokyo` (Mario & Sonic at the Tokyo 2020 Olympic Games): player 1's b1 / left = PUSH LEFT
//!   (blue), b2 / up / start = PUSH CENTER (yellow), b3 / right = PUSH RIGHT (red); b4 / b5 the
//!   left / right foot on the floor panel (the game's run: step left and right in turn), b6 a
//!   jump (every jump sensor's beam broken, both feet off).
//! - `hotd` (House of the Dead: Scarlet Dawn): each player's start, b3 the gun's front button,
//!   b4 its side button (the triggers and the aim: the gun sensor board, `gunsensor.rs`).
//! - `swdc` (Sega World Drivers Championship): the wheel on adc 0, gas 1, brake 2 (amdaemon's
//!   `config.json`: 8 bits); player 1's up/down/left/right/start, b1 the view change; player 2's
//!   word the wheel's buttons: b2 blue, b3 green, b4 red, b5 yellow, b6 / b7 the paddles
//!   (left / right).
//! - `idac` (Initial D THE ARCADE): the wheel, gas and brake as `swdc`, player 1's
//!   up/down/left/right/start and b1 the view change as `swdc`; player 2's word the 6-speed H
//!   shifter's gate (up 5, down 4, left 3, right 2: 1st up + left, 2nd down + left, 3rd up, 4th
//!   down, 5th up + right, 6th down + right), its gear moved by b7 (up) / b6 (down), neutral 0,
//!   or held on kp1-kp6 (an H shifter's positions).
//! - `chusan` (CHUNITHM SUPERSTAR): the six air sensors (`chuni::hand`); the slider is the
//!   game's (COM1).

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use wal_payload_common::{iat, input, log};
use wal_protocol::Axis;
use wal_protocol::button as b;

use crate::daemon::{IO4, Overlapped, complete, pending};

/// The device's interface path (lower case: compared so).
pub(crate) const PATH: &str = "\\\\?\\hid#vid_0ca3&pid_0021#wal-io4#{4d1e55b2-f16f-11cf-88cb-001111000030}";
const HID_GUID: [u8; 16] = [0xB2, 0x55, 0x1E, 0x4D, 0x6F, 0xF1, 0xCF, 0x11, 0x88, 0xCB, 0x00, 0x11, 0x11, 0x00, 0x00, 0x30];
const DEVINFO: usize = 0x5A11_1000;

const MANUFACTURER: &str = "SEGA";
const PRODUCT: &str = "I/O CONTROL BD;15257;01;90;1831;6679A;00;GOUT=14_ADIN=8,E_ROTIN=4_COININ=2_SWIN=2,E_UQ1=41,6";

const TEST: u16 = 1 << 9;
const SERVICE: u16 = 1 << 6;

static SYSTEM_STATUS: AtomicU8 = AtomicU8::new(0x02);
static COINS: AtomicUsize = AtomicUsize::new(0);

/// The read amdaemon waits for (its buffer and OVERLAPPED), completed by the report thread.
struct Read {
    buf: usize,
    len: usize,
    ov: usize,
}

static PENDING: Mutex<VecDeque<Read>> = Mutex::new(VecDeque::new());
static WAKE: Condvar = Condvar::new();

/// The game's controls (`WAL_ALLS_BOARD`).
#[derive(Clone, Copy, PartialEq)]
enum Board {
    Tokyo,
    Hotd,
    Swdc,
    Idac,
    Chusan,
}

fn board() -> Board {
    static BOARD: std::sync::OnceLock<Board> = std::sync::OnceLock::new();
    *BOARD.get_or_init(|| match std::env::var("WAL_ALLS_BOARD").unwrap_or_default().trim() {
        "hotd" => Board::Hotd,
        "swdc" => Board::Swdc,
        "idac" => Board::Idac,
        "chusan" => Board::Chusan,
        _ => Board::Tokyo,
    })
}

/// Player 1 starting a game: Initial D THE ARCADE's gas pedal too (pressed past half: the game
/// starts on it as on start), Chrono Regalia's touch (`b1`: its attract waits for a touch, its
/// cabinet has no start button), for the automatic Aime card.
pub(crate) fn start_pressed(p1: &wal_protocol::StickState) -> bool {
    static HKB: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let hkb = *HKB.get_or_init(|| std::env::var("WAL_ALLS_BOARD").is_ok_and(|v| v.trim() == "hkb"));
    p1.pressed(b::START) || (board() == Board::Idac && p1.axis_u16(Axis::Accel) >= 0x8000) || (hkb && p1.pressed(b::B1))
}

/// The switches of players 1 and 2 from the virtual sticks.
fn switches() -> [u16; 2] {
    let p1 = input(0).buttons;
    let mut sw = [0u16; 2];
    if p1 & b::TEST != 0 {
        sw[0] |= TEST;
    }
    if p1 & b::SERVICE != 0 {
        sw[0] |= SERVICE;
    }
    if board() == Board::Swdc {
        for (button, bit) in [(b::UP, 5), (b::DOWN, 4), (b::LEFT, 3), (b::RIGHT, 2), (b::START, 7), (b::B1, 1)] {
            if p1 & button != 0 {
                sw[0] |= 1 << bit;
            }
        }
        for (button, bit) in [(b::B2, 15), (b::B3, 14), (b::B4, 13), (b::B5, 12), (b::B6, 1), (b::B7, 0)] {
            if p1 & button != 0 {
                sw[1] |= 1 << bit;
            }
        }
        return sw;
    }
    if board() == Board::Chusan {
        // CHUNITHM SUPERSTAR's air sensors (amdaemon's photo_sensor_1..6: players 1 and 2's
        // bits 13, 12, 11), on while their beam is clear, from the lowest (segatools'
        // chusanhook): beam 0 player 1's bit 13, beam 1 player 2's bit 13...
        let hand = crate::chuni::hand();
        for (i, bit) in [13, 13, 12, 12, 11, 11].into_iter().enumerate() {
            if hand as usize <= i {
                sw[i % 2] |= 1 << bit;
            }
        }
        return sw;
    }
    if board() == Board::Idac {
        for (button, bit) in [(b::UP, 5), (b::DOWN, 4), (b::LEFT, 3), (b::RIGHT, 2), (b::START, 7), (b::B1, 1)] {
            if p1 & button != 0 {
                sw[0] |= 1 << bit;
            }
        }
        sw[1] = wal_payload_common::hshifter::gate(wal_payload_common::hshifter::gear(p1)) as u16;
        return sw;
    }
    if board() == Board::Hotd {
        // House of the Dead: Scarlet Dawn: each player's start (bit 7) and the gun's two
        // buttons, front (gun_select, bit 4) and side (gun_special, bit 1); the triggers and the
        // aim come from the gun sensor board (`gunsensor.rs`)
        for (p, s) in sw.iter_mut().enumerate() {
            let buttons = input(p).buttons;
            for (button, bit) in [(b::START, 7), (b::B3, 4), (b::B4, 1)] {
                if buttons & button != 0 {
                    *s |= 1 << bit;
                }
            }
        }
        return sw;
    }
    // Mario & Sonic (tokyo): the three buttons, the floor panel's sensors
    if p1 & (b::B1 | b::LEFT) != 0 {
        sw[0] |= 1 << 1;
    }
    if p1 & (b::B2 | b::UP | b::START) != 0 {
        sw[0] |= 1 << 0;
    }
    if p1 & (b::B3 | b::RIGHT) != 0 {
        sw[0] |= 1 << 15;
    }
    let jump = p1 & b::B6 != 0;
    // the foot sensors read 1 while nothing stands on them
    let foot_l = p1 & b::B4 != 0 || jump;
    let foot_r = p1 & b::B5 != 0 || jump;
    if !foot_l {
        sw[0] |= 1 << 13;
    }
    if !foot_r {
        sw[1] |= 1 << 13;
    }
    if jump {
        // jump sensors 1-6: bits 12, 11, 10 of both players
        sw[0] |= (1 << 12) | (1 << 11) | (1 << 10);
        sw[1] |= (1 << 12) | (1 << 11) | (1 << 10);
    }
    sw
}

static COIN_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn report() -> [u8; 64] {
    let mut r = [0u8; 64];
    r[0] = 0x01;
    // coin chute 1 (offset 1 + 16 + 8 = 25): counts up on each press (either player's coin:
    // the games share the chutes)
    let coin = (input(0).buttons | input(1).buttons) & b::COIN != 0;
    if coin && !COIN_HELD.swap(true, Ordering::Relaxed) {
        COINS.fetch_add(1, Ordering::Relaxed);
    } else if !coin {
        COIN_HELD.store(false, Ordering::Relaxed);
    }
    let coins = (COINS.load(Ordering::Relaxed) as u16) << 8;
    r[25..27].copy_from_slice(&coins.to_le_bytes());
    if matches!(board(), Board::Swdc | Board::Idac) {
        // adc 0-2 (offset 1): the wheel (center 0x8000), gas, brake
        let p1 = input(0);
        let adc = [p1.axis_u16(Axis::LeftX), p1.axis_u16(Axis::Accel), p1.axis_u16(Axis::Brake)];
        for (i, v) in adc.iter().enumerate() {
            r[1 + i * 2..3 + i * 2].copy_from_slice(&v.to_le_bytes());
        }
    }
    let sw = switches();
    r[29..31].copy_from_slice(&sw[0].to_le_bytes());
    r[31..33].copy_from_slice(&sw[1].to_le_bytes());
    r[33] = SYSTEM_STATUS.load(Ordering::Relaxed);
    r
}

static TRACES: AtomicUsize = AtomicUsize::new(0);

/// The board's first calls in the log.
fn trace(args: std::fmt::Arguments) {
    if TRACES.fetch_add(1, Ordering::Relaxed) < 40 {
        log!("io4: {args}");
    }
}

unsafe fn fill(buf: *mut u8, len: usize) -> usize {
    trace(format_args!("read ({len} bytes)"));
    let r = report();
    let n = len.min(r.len());
    unsafe { std::ptr::copy_nonoverlapping(r.as_ptr(), buf, n) };
    n
}

/// `ReadFile` on the board: an input report, a little later (amdaemon reads again at once
/// until a read is pending).
pub(crate) unsafe fn read(buf: *mut u8, len: usize, read: *mut u32, ov: *mut Overlapped) -> i32 {
    if ov.is_null() {
        std::thread::sleep(Duration::from_millis(1));
        let n = unsafe { fill(buf, len) };
        unsafe { complete(ov, read, n) };
        return 1;
    }
    let (status, error) = pending();
    unsafe { (*ov).internal = status };
    PENDING.lock().unwrap().push_back(Read { buf: buf as usize, len, ov: ov as usize });
    WAKE.notify_one();
    unsafe { SetLastError(error) };
    0
}

/// The pending reads ended (`CancelIo`).
pub(crate) fn cancel() {
    let reads: Vec<Read> = PENDING.lock().unwrap().drain(..).collect();
    for r in reads {
        let ov = r.ov as *mut Overlapped;
        unsafe {
            (*ov).internal = 0xC000_0120; // STATUS_CANCELLED
            (*ov).internal_high = 0;
            if !(*ov).event.is_null() {
                SetEvent((*ov).event);
            }
        }
    }
}

fn report_thread() {
    loop {
        let r = {
            let mut q = PENDING.lock().unwrap();
            while q.is_empty() {
                q = WAKE.wait(q).unwrap();
            }
            q.pop_front().unwrap()
        };
        std::thread::sleep(Duration::from_millis(1));
        // still pending (not cancelled meanwhile)
        let ov = r.ov as *mut Overlapped;
        let n = unsafe { fill(r.buf as *mut u8, r.len) };
        unsafe { complete(ov, std::ptr::null_mut(), n) };
    }
}

/// An output report (`WriteFile`, `HidD_SetOutputReport`).
pub(crate) fn write(data: &[u8]) {
    trace(format_args!("write {:02x?}", &data[..data.len().min(8)]));
    if data.len() < 2 || data[0] != 0x10 {
        return;
    }
    match data[1] {
        0x01 | 0x02 => SYSTEM_STATUS.store(0x30, Ordering::Relaxed),
        0x03 => SYSTEM_STATUS.store(0x00, Ordering::Relaxed),
        0x04 | 0x05 | 0x41 => {}
        cmd => log!("io4: command {cmd:#04x}"),
    }
}

const IOCTL_HID_GET_MANUFACTURER_STRING: u32 = 0x000b_01ba;
const IOCTL_HID_GET_PRODUCT_STRING: u32 = 0x000b_01be;
const IOCTL_HID_GET_INPUT_REPORT: u32 = 0x000b_01a2;
const IOCTL_HID_SET_OUTPUT_REPORT: u32 = 0x000b_0195;

fn put_wide(out: &mut [u8], s: &str) -> Option<usize> {
    let w: Vec<u8> = s.encode_utf16().chain(Some(0)).flat_map(|c| c.to_le_bytes()).collect();
    let n = w.len().min(out.len());
    out[..n].copy_from_slice(&w[..n]);
    Some(n)
}

pub(crate) fn ioctl(code: u32, input: &[u8], out: &mut [u8]) -> Option<usize> {
    trace(format_args!("ioctl {code:#x} in {} out {}", input.len(), out.len()));
    match code {
        IOCTL_HID_GET_MANUFACTURER_STRING => put_wide(out, MANUFACTURER),
        IOCTL_HID_GET_PRODUCT_STRING => put_wide(out, PRODUCT),
        IOCTL_HID_GET_INPUT_REPORT => Some(unsafe { fill(out.as_mut_ptr(), out.len()) }),
        IOCTL_HID_SET_OUTPUT_REPORT => {
            write(input);
            Some(0)
        }
        _ => None,
    }
}

unsafe extern "system" {
    fn SetLastError(code: u32);
    fn SetEvent(event: *mut c_void) -> i32;
}

// HID.DLL: the strings and reports of the board

unsafe extern "system" fn hid_manufacturer(h: usize, buf: *mut u8, len: u32) -> u8 {
    hid_string(h, buf, len, MANUFACTURER, 0)
}

unsafe extern "system" fn hid_product(h: usize, buf: *mut u8, len: u32) -> u8 {
    hid_string(h, buf, len, PRODUCT, 1)
}

static HID_ORIG: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

fn hid_string(h: usize, buf: *mut u8, len: u32, s: &str, slot: usize) -> u8 {
    trace(format_args!("string {slot} ({len} bytes)"));
    if h != IO4 {
        let orig: unsafe extern "system" fn(usize, *mut u8, u32) -> u8 = unsafe { std::mem::transmute(HID_ORIG[slot].load(Ordering::Relaxed)) };
        return unsafe { orig(h, buf, len) };
    }
    let out = unsafe { std::slice::from_raw_parts_mut(buf, len as usize) };
    put_wide(out, s);
    1
}

unsafe extern "system" fn hid_set_output(h: usize, buf: *const u8, len: u32) -> u8 {
    if h != IO4 {
        let orig: unsafe extern "system" fn(usize, *const u8, u32) -> u8 = unsafe { std::mem::transmute(HID_ORIG[2].load(Ordering::Relaxed)) };
        return unsafe { orig(h, buf, len) };
    }
    write(unsafe { std::slice::from_raw_parts(buf, len as usize) });
    1
}

unsafe extern "system" fn hid_get_input(h: usize, buf: *mut u8, len: u32) -> u8 {
    trace(format_args!("get input report ({len} bytes)"));
    if h != IO4 {
        let orig: unsafe extern "system" fn(usize, *mut u8, u32) -> u8 = unsafe { std::mem::transmute(HID_ORIG[3].load(Ordering::Relaxed)) };
        return unsafe { orig(h, buf, len) };
    }
    unsafe { fill(buf, len as usize) };
    1
}

// SetupDi: the HID device list holds the board only

static CLASS_DEVS_W: AtomicUsize = AtomicUsize::new(0);
static CLASS_DEVS_A: AtomicUsize = AtomicUsize::new(0);
static CLASS_DEVS_EX_A: AtomicUsize = AtomicUsize::new(0);
static ENUM_INTERFACES: AtomicUsize = AtomicUsize::new(0);
static INTERFACE_DETAIL_W: AtomicUsize = AtomicUsize::new(0);
static DESTROY_LIST: AtomicUsize = AtomicUsize::new(0);

fn is_hid(guid: *const [u8; 16]) -> bool {
    !guid.is_null() && unsafe { *guid } == HID_GUID
}

unsafe extern "system" fn class_devs_w(guid: *const [u8; 16], enumerator: *const u16, hwnd: usize, flags: u32) -> usize {
    if is_hid(guid) {
        return DEVINFO;
    }
    let orig: unsafe extern "system" fn(*const [u8; 16], *const u16, usize, u32) -> usize = unsafe { std::mem::transmute(CLASS_DEVS_W.load(Ordering::Relaxed)) };
    unsafe { orig(guid, enumerator, hwnd, flags) }
}

unsafe extern "system" fn class_devs_a(guid: *const [u8; 16], enumerator: *const u8, hwnd: usize, flags: u32) -> usize {
    if is_hid(guid) {
        return DEVINFO;
    }
    let orig: unsafe extern "system" fn(*const [u8; 16], *const u8, usize, u32) -> usize = unsafe { std::mem::transmute(CLASS_DEVS_A.load(Ordering::Relaxed)) };
    unsafe { orig(guid, enumerator, hwnd, flags) }
}

unsafe extern "system" fn class_devs_ex_a(guid: *const [u8; 16], enumerator: *const u8, hwnd: usize, flags: u32, set: usize, machine: *const u8, reserved: usize) -> usize {
    if is_hid(guid) {
        return DEVINFO;
    }
    type F = unsafe extern "system" fn(*const [u8; 16], *const u8, usize, u32, usize, *const u8, usize) -> usize;
    let orig: F = unsafe { std::mem::transmute(CLASS_DEVS_EX_A.load(Ordering::Relaxed)) };
    unsafe { orig(guid, enumerator, hwnd, flags, set, machine, reserved) }
}

unsafe extern "system" fn enum_interfaces(set: usize, info: *mut c_void, guid: *const [u8; 16], index: u32, data: *mut u8) -> i32 {
    if set != DEVINFO {
        let orig: unsafe extern "system" fn(usize, *mut c_void, *const [u8; 16], u32, *mut u8) -> i32 = unsafe { std::mem::transmute(ENUM_INTERFACES.load(Ordering::Relaxed)) };
        return unsafe { orig(set, info, guid, index, data) };
    }
    if index != 0 || data.is_null() {
        unsafe { SetLastError(259) }; // ERROR_NO_MORE_ITEMS
        return 0;
    }
    // SP_DEVICE_INTERFACE_DATA: cbSize, class guid, flags (active), reserved
    unsafe {
        std::ptr::copy_nonoverlapping(HID_GUID.as_ptr(), data.add(4), 16);
        *(data.add(20) as *mut u32) = 1;
    }
    1
}

unsafe extern "system" fn interface_detail_w(set: usize, data: *mut u8, detail: *mut u8, size: u32, required: *mut u32, info: *mut u8) -> i32 {
    if set != DEVINFO {
        let orig: unsafe extern "system" fn(usize, *mut u8, *mut u8, u32, *mut u32, *mut u8) -> i32 = unsafe { std::mem::transmute(INTERFACE_DETAIL_W.load(Ordering::Relaxed)) };
        return unsafe { orig(set, data, detail, size, required, info) };
    }
    // SP_DEVICE_INTERFACE_DETAIL_DATA_W: cbSize, then the path
    let path: Vec<u16> = PATH.encode_utf16().chain(Some(0)).collect();
    let need = 4 + path.len() as u32 * 2;
    if !required.is_null() {
        unsafe { *required = need };
    }
    if detail.is_null() || size < need {
        unsafe { SetLastError(122) }; // ERROR_INSUFFICIENT_BUFFER
        return 0;
    }
    unsafe { std::ptr::copy_nonoverlapping(path.as_ptr(), detail.add(4) as *mut u16, path.len()) };
    // SP_DEVINFO_DATA: cbSize, class guid, devinst, reserved
    if !info.is_null() {
        unsafe {
            std::ptr::copy_nonoverlapping(HID_GUID.as_ptr(), info.add(4), 16);
            *(info.add(20) as *mut u32) = 1;
        }
    }
    1
}

unsafe extern "system" fn destroy_list(set: usize) -> i32 {
    if set == DEVINFO {
        return 1;
    }
    let orig: unsafe extern "system" fn(usize) -> i32 = unsafe { std::mem::transmute(DESTROY_LIST.load(Ordering::Relaxed)) };
    unsafe { orig(set) }
}

pub(crate) fn init() {
    unsafe {
        let hooks: [(&str, &str, usize, &AtomicUsize); 10] = [
            ("setupapi.dll", "SetupDiGetClassDevsW", class_devs_w as *const () as usize, &CLASS_DEVS_W),
            ("setupapi.dll", "SetupDiGetClassDevsA", class_devs_a as *const () as usize, &CLASS_DEVS_A),
            ("setupapi.dll", "SetupDiGetClassDevsExA", class_devs_ex_a as *const () as usize, &CLASS_DEVS_EX_A),
            ("setupapi.dll", "SetupDiEnumDeviceInterfaces", enum_interfaces as *const () as usize, &ENUM_INTERFACES),
            ("setupapi.dll", "SetupDiGetDeviceInterfaceDetailW", interface_detail_w as *const () as usize, &INTERFACE_DETAIL_W),
            ("setupapi.dll", "SetupDiDestroyDeviceInfoList", destroy_list as *const () as usize, &DESTROY_LIST),
            ("hid.dll", "HidD_GetManufacturerString", hid_manufacturer as *const () as usize, &HID_ORIG[0]),
            ("hid.dll", "HidD_GetProductString", hid_product as *const () as usize, &HID_ORIG[1]),
            ("hid.dll", "HidD_SetOutputReport", hid_set_output as *const () as usize, &HID_ORIG[2]),
            ("hid.dll", "HidD_GetInputReport", hid_get_input as *const () as usize, &HID_ORIG[3]),
        ];
        for (dll, name, f, slot) in hooks {
            match iat::hook(dll, name, f) {
                Some(orig) => slot.store(orig, Ordering::Relaxed),
                None => log!("io4: {name} not imported"),
            }
        }
    }
    std::thread::spawn(report_thread);
    let _ = IO4;
}
