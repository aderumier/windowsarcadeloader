//! UNIS's After Dark (1.1 and 2.3: Virtools 3.5, 32-bit, `GamePlayer.exe`):
//! `WAL_UNIS_BOARD=afterdark`.
//!
//! The cabinet: two guns and the coins on a serial board (COM1, 19200 bauds), and a USB HID
//! board (`HID\Vid_0471&Pid_485e`: the dongle, the guns' vibration), both read by the
//! executable's own building blocks that the scripts (`main.vmo`) use:
//!
//! - **Serial board.** 26-byte frames `40 80`, gun A's and gun B's buttons (bytes 2, 3: 0x01
//!   the start, 0x02 the trigger, 0x04 the motion seat's button, 0x08 the operator menu,
//!   0x10 / 0x20 its + / -, 0x80 a coin, counted once per 100 ms held), their X and Y (bytes
//!   7..14, big endian, 12 bits: 0..4095 across the screen from its bottom left, which the
//!   game maps through its calibration, `coin.ini`'s `Gun1xMin` ... `Gun2yMax`, set in the operator menu's gun
//!   settings), `0D` and the XOR of the 25 bytes before. The PC's frames (vibration)
//!   are ignored.
//! - **HID board.** Its driver (in the executable) speaks an encrypted protocol to the board;
//!   the payload replaces the driver's device object instead: the init that enumerates the
//!   boards (`0x415CA0`) puts ours in the board table. The dongle block writes 8 bytes (command
//!   0x42) and reads them back (command 0x30, sub-command 1); the I/O block's polls (sub-command
//!   2: the vibration) get 16 zero bytes.
//!
//! `WAL_UNIS_FREEPLAY` (default on; `0`: coins): the game has no free play, a start press is
//! a coin and then the start.
//!
//! `WAL_UNIS_AFTERDARK_STAGE4` (default on; `0`: off): the fourth stage open from the start
//! (`afterdark_stage4.rs`).
//!
//! Natives: `start trigger platform test plus minus coin` (each gun).

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log, serial};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

const PORT: &str = "COM1";
const GUNS: usize = 2;
const RAW_MAX: f32 = 4095.0;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// A bit of the gun's buttons byte.
    Button(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("start", Native::Button(START)),
    ("trigger", Native::Button(0x02)),
    ("platform", Native::Button(0x04)),
    ("test", Native::Button(0x08)),
    ("plus", Native::Button(0x10)),
    ("minus", Native::Button(0x20)),
    ("coin", Native::Button(COIN)),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("start", "start"),
    ("coin", "coin"),
    ("test", "test"),
    ("service", "plus"),
    ("up", "plus"),
    ("down", "minus"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The last frame's time.
static LAST: Mutex<Option<Instant>> = Mutex::new(None);
static FRAMES: AtomicU64 = AtomicU64::new(0);

const START: u8 = 0x01;
const COIN: u8 = 0x80;

/// Free play (`WAL_UNIS_FREEPLAY`, default on): each gun's start key held, and when it was
/// pressed.
struct FreePlay {
    held: [bool; GUNS],
    pressed: [Option<Instant>; GUNS],
}

static FREE_PLAY: Mutex<FreePlay> = Mutex::new(FreePlay { held: [false; GUNS], pressed: [None; GUNS] });

fn free_play_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| wal_payload_common::free_play("WAL_UNIS_FREEPLAY").unwrap_or(true))
}

/// Free play: a start press is a coin (its first 120 ms: the board counts one per 100 ms
/// held), then the start (from 250 ms to 400 ms, held longer while the key is), the game
/// having no free play of its own.
fn free_play(g: usize, buttons: u8) -> u8 {
    let mut fp = FREE_PLAY.lock().unwrap();
    let held = buttons & START != 0;
    if held && !fp.held[g] {
        fp.pressed[g] = Some(Instant::now());
    }
    fp.held[g] = held;
    let mut out = buttons & !START;
    if let Some(t) = fp.pressed[g] {
        let ms = t.elapsed().as_millis();
        match ms {
            0..120 => out |= COIN,
            250..400 => out |= START,
            400.. if held => out |= START,
            400.. => fp.pressed[g] = None,
            _ => {}
        }
    }
    out
}

/// A board frame: `40 80 data[22] 0D xor`.
fn frame(data: &[u8; 22]) -> Vec<u8> {
    let mut f = vec![0x40, 0x80];
    f.extend_from_slice(data);
    f.push(0x0D);
    f.push(f.iter().fold(0, |a, b| a ^ b));
    f
}

/// The board's frame when the game reads the port (every 16 ms at most).
fn idle() -> Vec<u8> {
    {
        let mut last = LAST.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < Duration::from_millis(16)) {
            return Vec::new();
        }
        *last = Some(Instant::now());
    }
    let mut data = [0u8; 22];
    for g in 0..GUNS {
        map().for_each_pressed(&input(g), |Native::Button(bit)| data[g] |= bit);
        if free_play_on() {
            data[g] = free_play(g, data[g]);
        }
        // the game full screen: the gun's place on the screen
        let screen = gun::screen_size();
        let (sx, sy) = gun::on_screen(&input(g), screen);
        let (fx, fy) = ((sx / screen.0).clamp(0.0, 1.0), (sy / screen.1).clamp(0.0, 1.0));
        let raw = |f: f32| (f * RAW_MAX).round() as u16;
        // y from the bottom (the calibration's yMin the bottom edge)
        let (x, y) = (raw(fx), raw(1.0 - fy));
        data[5 + 4 * g..9 + 4 * g].copy_from_slice(&[(x >> 8) as u8, x as u8, (y >> 8) as u8, y as u8]);
    }
    if FRAMES.fetch_add(1, Ordering::Relaxed) == 0 {
        log!("afterdark: first frame to the game");
    }
    frame(&data)
}

/// The PC's frames: the vibration, nothing to answer.
fn handler(_packet: &[u8]) -> Vec<u8> {
    Vec::new()
}

// The HID board's device object (COM-like: the methods the game calls, stdcall)

type P = *mut c_void;

const S_OK: i32 = 0;
const E_NOINTERFACE: i32 = 0x80004002u32 as i32;

/// The bytes the dongle block wrote (command 0x42).
static WRITTEN: Mutex<[u8; 8]> = Mutex::new([0; 8]);

extern "system" fn dev_query(_this: P, _iid: P, out: *mut P) -> i32 {
    if !out.is_null() {
        unsafe { *out = std::ptr::null_mut() };
    }
    E_NOINTERFACE
}

extern "system" fn dev_ref(_this: P) -> u32 {
    1
}

/// +0x10: opens the board.
extern "system" fn dev_open(_this: P) -> i32 {
    S_OK
}

/// +0x30: the board's 16-byte id.
extern "system" fn dev_id(_this: P, id: *mut u8) -> i32 {
    if !id.is_null() {
        unsafe { std::ptr::write_bytes(id, 0, 16) };
    }
    S_OK
}

/// +0x50: a command (`input` its `len` bytes), its answer in `out`.
extern "system" fn dev_command(_this: P, out: *mut u8, input: *const u8, len: u32, command: u32) -> i32 {
    let input = if input.is_null() || len == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(input, len.min(0x64) as usize) } };
    match command {
        0x42 => {
            let mut w = WRITTEN.lock().unwrap();
            let n = input.len().min(8);
            w[..n].copy_from_slice(&input[..n]);
        }
        0x30 if !out.is_null() => {
            let answer: &[u8] = match input.first() {
                // the dongle: the bytes written, back
                Some(1) => &input[1..input.len().min(9)],
                _ => &[0; 16],
            };
            unsafe { std::ptr::copy_nonoverlapping(answer.as_ptr(), out, answer.len()) };
        }
        _ => {}
    }
    S_OK
}

/// +0x58: the board's game name check ("3DGame").
extern "system" fn dev_name(_this: P, _name: *const u8, _len: u32) -> i32 {
    S_OK
}

/// Any other method: never called by the game (logged).
extern "system" fn dev_other(_this: P) -> i32 {
    log!("afterdark: unexpected HID board method");
    S_OK
}

#[repr(C)]
struct Device {
    vtable: &'static [usize; 28],
}

fn device() -> usize {
    static VTABLE: OnceLock<[usize; 28]> = OnceLock::new();
    static DEVICE: OnceLock<Device> = OnceLock::new();
    let vtable = VTABLE.get_or_init(|| {
        let mut v = [dev_other as *const () as usize; 28];
        v[0] = dev_query as *const () as usize;
        v[1] = dev_ref as *const () as usize;
        v[2] = dev_ref as *const () as usize;
        v[0x10 / 4] = dev_open as *const () as usize;
        v[0x30 / 4] = dev_id as *const () as usize;
        v[0x50 / 4] = dev_command as *const () as usize;
        v[0x58 / 4] = dev_name as *const () as usize;
        v
    });
    DEVICE.get_or_init(|| Device { vtable }) as *const Device as usize
}

/// The board init (`0x415CA0`) and its first bytes; the board table (`0x8983B0`).
const INIT_RVA: usize = 0x15CA0;
const INIT_HEAD: [u8; 8] = [0x83, 0xEC, 0x24, 0xA1, 0x10, 0x4E, 0x87, 0x00];
const BOARDS_RVA: u32 = 0x4983B0;

/// The board init puts our device in the table and succeeds:
/// `mov dword [boards], device; mov al, 1; ret`.
fn replace_board_init() -> bool {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let init = (base + INIT_RVA) as *mut u8;
    if unsafe { std::slice::from_raw_parts(init, INIT_HEAD.len()) } != INIT_HEAD {
        return false;
    }
    let mut code = vec![0xC7, 0x05];
    code.extend_from_slice(&(base as u32 + BOARDS_RVA).to_le_bytes());
    code.extend_from_slice(&(device() as u32).to_le_bytes());
    code.extend_from_slice(&[0xB0, 0x01, 0xC3]);
    let mut old = 0;
    unsafe {
        VirtualProtect(init as *const c_void, code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), init, code.len());
        VirtualProtect(init as *const c_void, code.len(), old, &mut old);
    }
    true
}

// The stage select's composition: a copy with the fourth stage open (afterdark_stage4.rs)

static CREATE_FILE_A: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
/// The copy's path (NUL terminated), once made; None: not made (the original opened).
static STAGE4_COPY: OnceLock<Option<Vec<u8>>> = OnceLock::new();

/// `Module\UI.nmo` with the fourth stage open, written beside the executable (the run
/// directory's `Module` is the dump's).
fn stage4_copy(original: &str) -> Option<Vec<u8>> {
    let file = std::fs::read(original).ok()?;
    let patched = crate::afterdark_stage4::open_stage4(&file)?;
    let path = std::env::current_exe().ok()?.with_file_name("wal-UI.nmo");
    std::fs::write(&path, patched).ok()?;
    let mut bytes = path.to_str()?.as_bytes().to_vec();
    bytes.push(0);
    Some(bytes)
}

unsafe extern "system" fn create_file_a(name: *const u8, access: u32, share: u32, security: P, disposition: u32, flags: u32, template: P) -> P {
    let orig: unsafe extern "system" fn(*const u8, u32, u32, P, u32, u32, P) -> P = unsafe { std::mem::transmute(CREATE_FILE_A.load(Ordering::Relaxed)) };
    if !name.is_null() {
        let path = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy();
        if path.to_ascii_lowercase().replace('/', "\\").ends_with("\\module\\ui.nmo")
            && let Some(copy) = STAGE4_COPY.get_or_init(|| stage4_copy(&path)).as_ref()
        {
            return unsafe { orig(copy.as_ptr(), access, share, security, disposition, flags, template) };
        }
    }
    unsafe { orig(name, access, share, security, disposition, flags, template) }
}

pub fn init() {
    // the MFC message loop's keyboard and mouse filtered too: the PC keyboard's F1 was MFC's
    // help (WinHelp, the game hung)
    wal_payload_common::input_filter::messages(unsafe { GetModuleHandleW(windows_sys::w!("mfc90.dll")) } as usize);
    serial::install_streaming(PORT, handler, idle);
    // after the serial port's: its hook chained
    if std::env::var("WAL_UNIS_AFTERDARK_STAGE4").map_or(true, |v| v.trim() != "0")
        && let Some(o) = unsafe { wal_payload_common::iat::hook("kernel32.dll", "CreateFileA", create_file_a as *const () as usize) }
    {
        CREATE_FILE_A.store(o, Ordering::Relaxed);
    }
    let hid = replace_board_init();
    log!("afterdark: board on {PORT}, HID board {}", if hid { "emulated" } else { "not found (unknown executable)" });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames() {
        let f = frame(&[0; 22]);
        assert_eq!((f.len(), f[0], f[1], f[24]), (26, 0x40, 0x80, 0x0D));
        assert_eq!(f[..25].iter().fold(0, |a, b| a ^ b), f[25]);
    }
}

