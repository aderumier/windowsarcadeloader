//! PGA TOUR Golf Championship Edition's trackball board: `UsbTrackerDll.dll`'s `CUSBIO` class,
//! imported by the game's engine. Its constructor, `CUSBIO_Init` and `CUSBIO_Close` have the
//! names of USBIOExtreme's (`usbio.rs` hands them over in this game); `CUSBIO_Update()` and the
//! board memory's `ReadSPIMemory` / `WriteSPIMemory` are this one's.
//!
//! The engine keeps a static object it reads after each update:
//!
//! | offset | content |
//! |---|---|
//! | 0x00 / 0x02 | u16 trackball X / Y counters (wrapping) |
//! | 0x18 | buttons (the first Championship Edition's library: 0x14) |
//! | 0x1c | `timeGetTime` of the update (the engine's swing speed: counts over milliseconds; none in
//! the first edition's, another variable follows its buttons) |
//!
//! The engine's swing is the ball rolled toward the screen (Y counter going up) after the
//! backswing. The ball rolls at the virtual stick's tilt (`WAL_PGA_TRACKBALL_SPEED` counts per
//! second at full tilt: the swing is the stick pulled back, then pushed forward) and by the mouse:
//! the profile maps it to the right stick axes with `mouse_wrap`, a trackball's counters
//! (`counter_motion`).
//!
//! The engine loads the payload's copy as `UsbTrackerDll.dll`, a module of its own (exports only,
//! no launcher connection): its calls go on to the main payload's (`GVRInputRaw.dll`) exports.
//!
//! Championship Edition's (1.2) library has no `CUSBIO_Update`: its board thread kept the object
//! up to date, its game (`TW2002.exe`) and shell control only read it. The object is filled every
//! 5 ms from its init on, whatever the version ([`updates`]).

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use wal_payload_common::{log, mapping::ButtonMap};
use wal_protocol::Axis;
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleHandleA, GetModuleHandleExW,
    GetProcAddress,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Native(u8);

const NATIVES: &[(&str, Native)] = &[
    ("button1", Native(0x1)),
    ("button2", Native(0x2)),
    ("button3", Native(0x4)),
    ("button4", Native(0x8)),
    ("button5", Native(0x10)),
    ("button6", Native(0x20)),
];

/// Virtual stick -> board buttons, overridden by the profile `native_map`.
const DEFAULT_MAP: &[(&str, &str)] =
    &[("b1", "button4"), ("start", "button4"), ("b2", "button3"), ("b3", "button1"), ("b4", "button2"), ("b5", "button5"), ("test", "button6")];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP))
}

/// The game is PGA TOUR's (this board, not USBIOExtreme's).
pub(crate) fn active() -> bool {
    static ACTIVE: OnceLock<bool> = OnceLock::new();
    *ACTIVE.get_or_init(|| {
        let exe = crate::frontend_fit::exe_name();
        exe == "pga_tour_golf.exe" || exe == "tw2002.exe" || own_module_name() == "usbtrackerdll.dll"
    })
}

/// This payload module's file name, lower case (the copy installed as `UsbTrackerDll.dll`).
fn own_module_name() -> String {
    let mut own = std::ptr::null_mut();
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            own_module_name as *const () as *const u16,
            &mut own,
        )
    };
    let mut buf = [0u16; 260];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(own, buf.as_mut_ptr(), 260) } as usize;
    let path = String::from_utf16_lossy(&buf[..n]);
    path.rsplit('\\').next().unwrap_or(&path).to_ascii_lowercase()
}

/// Counts per second at full tilt.
fn speed() -> f32 {
    static SPEED: OnceLock<f32> = OnceLock::new();
    *SPEED.get_or_init(|| std::env::var("WAL_PGA_TRACKBALL_SPEED").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(2000.0))
}

/// The ball's position (sub-count), the last update and the mouse counters then.
struct Ball {
    x: f32,
    y: f32,
    last: Option<Instant>,
    counters: Option<[i16; 2]>,
}

static BALL: Mutex<Ball> = Mutex::new(Ball { x: 0.0, y: 0.0, last: None, counters: None });

/// The mouse motion since `last`: the right stick axes are its wrapping counters
/// (`input.mouse_wrap`, the launcher's mouse position wrapping around the virtual screen).
pub(crate) fn counter_motion(stick: &wal_protocol::StickState, last: &mut Option<[i16; 2]>) -> (i32, i32) {
    let now = [stick.axis(Axis::RightX), stick.axis(Axis::RightY)];
    let motion = last.map_or((0, 0), |[x, y]| (now[0].wrapping_sub(x) as i32, now[1].wrapping_sub(y) as i32));
    *last = Some(now);
    motion
}
static LAST_BUTTONS: AtomicU32 = AtomicU32::new(u32::MAX);

/// The board's buttons pressed on the virtual stick (also the shell control's joystick buttons:
/// the shell's `Operator\Buttons` bits).
pub(crate) fn buttons(stick: &wal_protocol::StickState) -> u8 {
    let mut buttons = 0u8;
    map().for_each_pressed(stick, |Native(bit)| buttons |= bit);
    buttons
}

#[link(name = "winmm")]
unsafe extern "system" {
    fn timeGetTime() -> u32;
}

unsafe fn fill(this: *mut u8) {
    let stick = wal_payload_common::input(0);
    let now = Instant::now();
    let (x, y) = {
        let mut ball = BALL.lock().unwrap_or_else(|e| e.into_inner());
        let seconds = ball.last.map_or(0.0, |t| now.duration_since(t).as_secs_f32().min(0.1));
        ball.last = Some(now);
        let (mx, my) = counter_motion(&stick, &mut ball.counters);
        // the stick and the mouse forward (up) roll the ball toward the screen
        ball.x += stick.axis(Axis::LeftX) as f32 / 32767.0 * speed() * seconds + mx as f32;
        ball.y -= stick.axis(Axis::LeftY) as f32 / 32767.0 * speed() * seconds + my as f32;
        (ball.x as i64 as u16, ball.y as i64 as u16)
    };
    let buttons = buttons(&stick);
    unsafe {
        (this as *mut u16).write_unaligned(x);
        (this.add(2) as *mut u16).write_unaligned(y);
        if crate::pga::is_first_edition() {
            *this.add(0x14) = buttons;
        } else {
            *this.add(0x18) = buttons;
            (this.add(0x1c) as *mut u32).write_unaligned(timeGetTime());
        }
    }
    if LAST_BUTTONS.swap(buttons as u32, Ordering::Relaxed) != buttons as u32 {
        log!("pga: trackball buttons {buttons:#x}");
    }
}

/// The main payload's export `name` when this module is the `UsbTrackerDll.dll` copy.
fn main_export(name: &std::ffi::CStr) -> Option<usize> {
    let main = unsafe { GetModuleHandleA(c"GVRInputRaw.dll".as_ptr().cast()) };
    let mut own = std::ptr::null_mut();
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            main_export as *const () as *const u16,
            &mut own,
        )
    };
    if main.is_null() || main == own {
        return None;
    }
    unsafe { GetProcAddress(main, name.as_ptr().cast()) }.map(|f| f as usize)
}

/// The main payload's export `name`, looked up once.
macro_rules! forward {
    ($name:literal, $ty:ty) => {{
        static FORWARD: OnceLock<Option<usize>> = OnceLock::new();
        FORWARD.get_or_init(|| main_export($name)).map(|f| unsafe { std::mem::transmute::<usize, $ty>(f) })
    }};
}

/// `CUSBIO_Init()`: the board found.
pub(crate) unsafe fn init(this: *mut u8) -> i32 {
    if let Some(init) = forward!(c"pga_trackball_init", unsafe extern "thiscall" fn(*mut u8) -> i32) {
        return unsafe { init(this) };
    }
    unsafe { pga_trackball_init(this) }
}

/// The main payload's `CUSBIO_Init()` for the `UsbTrackerDll.dll` copy (whatever the executable):
/// the object filled now and kept up to date.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn pga_trackball_init(this: *mut u8) -> i32 {
    unsafe { fill(this) };
    let object = this as usize;
    std::thread::spawn(move || updates(object));
    log!("pga: trackball board ready");
    1
}

/// The object at `object` filled every 5 ms (the board thread of the 1.2 library).
fn updates(object: usize) {
    loop {
        std::thread::sleep(std::time::Duration::from_millis(5));
        unsafe { fill(object as *mut u8) };
    }
}

/// `void CUSBIO::CUSBIO_Update()`
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn pga_trackball_update(this: *mut u8) {
    if let Some(update) = forward!(c"?CUSBIO_Update@CUSBIO@@QAEXXZ", unsafe extern "thiscall" fn(*mut u8)) {
        return unsafe { update(this) };
    }
    if !this.is_null() {
        unsafe { fill(this) };
    }
}

/// The board's memory, 4 bytes at each address.
static SPI: Mutex<[[u8; 4]; 256]> = Mutex::new([[0; 4]; 256]);

/// `void CUSBIO::ReadSPIMemory(unsigned char address, unsigned char *four)`
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn pga_trackball_read_spi(this: usize, address: u8, out: *mut u8) {
    if let Some(read) = forward!(c"?ReadSPIMemory@CUSBIO@@QAEXEPAE@Z", unsafe extern "thiscall" fn(usize, u8, *mut u8)) {
        return unsafe { read(this, address, out) };
    }
    if !out.is_null() {
        let value = SPI.lock().unwrap_or_else(|e| e.into_inner())[address as usize];
        unsafe { std::ptr::copy_nonoverlapping(value.as_ptr(), out, 4) };
    }
}

/// `void CUSBIO::WriteSPIMemory(unsigned char address, unsigned char *four)`
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn pga_trackball_write_spi(this: usize, address: u8, data: *const u8) {
    if let Some(write) = forward!(c"?WriteSPIMemory@CUSBIO@@QAEXEPAE@Z", unsafe extern "thiscall" fn(usize, u8, *const u8)) {
        return unsafe { write(this, address, data) };
    }
    if !data.is_null() {
        let mut spi = SPI.lock().unwrap_or_else(|e| e.into_inner());
        unsafe { std::ptr::copy_nonoverlapping(data, spi[address as usize].as_mut_ptr(), 4) };
    }
}
