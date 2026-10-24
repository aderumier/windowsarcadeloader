//! Rainbow Bomber Girl's I/O, through Mono's embedding API (`mono.rs`), from a thread of the
//! payload attached to Mono:
//!
//! * The system library: once the game has created its system interface (`SystemImport`, with
//!   its stand-ins), its System, Io and Consume modules are replaced by the game's own bindings
//!   to the library (`SystemImport_System`, ...), initialized with the payload's module handle:
//!   they find `bbg_system.rs`'s exports by name.
//! * The I/O board (UsbIO's `IoManagerMain`): the board would answer on COM6, kept absent (Mono
//!   resolves the serial `DllImport`s through its `GetProcAddress`: COM ports fail to open), so
//!   the board's boot fails at once. Every 10 ms the payload builds the board's status (16 bytes,
//!   the layout of UsbIO's `IoState`, bit fields from the most significant bit) and hands it to
//!   `IoManagerMain.SetBtnState`, as the board's packet thread does; the boot then reads as done
//!   (`m_BootStep` BOOT_SUCCESS_END).
//!   * analog channel 1 (bits 0-11), bits 12-14: test, service, coin; analog channel 2 (byte 2,
//!     12 bits); the stick (center 2048) read by the game as X = channel 1 (high = right),
//!     Y = channel 2 (high = up), or with its default stick type (TYPE_2, the stick mounted a
//!     quarter turned) X = 4096 - channel 2, Y = channel 1;
//!   * byte 9 from bit 7: the buttons of rows 1, 2 and 3 (left, center, right each: bomb, first
//!     skill, second skill), byte 10: (row 3 right), 10, rec jack, headphone jack (0: plugged),
//!     13-15, push button 1 (bit 0), byte 11: push buttons 2, 3, the stick's button (bit 5).
//! * The touch screen (`TouchPanelManager`): its touches come from a cabinet library the game
//!   no longer has. The payload writes touch 0 (`arrPointerInfo[0]`: state DOWN on the press, UP
//!   on the release; the position) in the touch screen's own coordinates: x 0..1280 from the
//!   left, y 0..-720 from the top (`bbg_screen.rs` places the touch screen in the picture).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::{iat, log};
use wal_protocol::Axis;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

use crate::bbg::Native;
use crate::bbg_system::{COIN, SERVICE, TEST};
use crate::mono::{self, ARRAY_DATA, Mono, P, mono};

/// IoManagerBootStep
const BOOT_SUCCESS_END: i32 = 7;
const BOOT_ERROR_END: i32 = 8;
/// POINTERSTATE
const TOUCH_DOWN: i32 = 1;
const TOUCH_UP: i32 = 3;

static GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static CREATE_FILE_W: AtomicUsize = AtomicUsize::new(0);
static CREATE_FILE_A: AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    std::thread::spawn(run);
}

fn run() {
    let m = loop {
        if let Some(m) = mono() {
            break m;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    hide_serial_ports();
    // the game's assemblies: loaded by Unity once Mono runs the first scene
    let (game, usbio) = loop {
        let (game, usbio) = (m.image("Assembly-CSharp"), m.image("UsbIO"));
        if !game.is_null() && !usbio.is_null() && m.attach() {
            break (game, usbio);
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    bind_system(m, game);
    let Some(board) = Board::new(m, usbio) else { return };
    let touch = Touch::new(m, game);
    log!("bbg: I/O board fed");
    let mut coin_held = false;
    loop {
        std::thread::sleep(Duration::from_millis(10));
        let pressed = crate::bbg::pressed();
        let coin = pressed.contains(&Native::Coin);
        if coin && !coin_held {
            crate::bbg_system::coin_inserted();
        }
        coin_held = coin;
        TEST.store(pressed.contains(&Native::Test), Ordering::Relaxed);
        SERVICE.store(pressed.contains(&Native::Service), Ordering::Relaxed);
        COIN.store(coin, Ordering::Relaxed);
        board.update(m, &pressed);
        if let Some(t) = &touch {
            t.update(pressed.contains(&Native::Touch));
        }
    }
}

// ---- the system library ----

/// Replaces the stand-ins of `SystemImport`'s System, Io and Consume modules by the game's
/// bindings to the payload's exports.
fn bind_system(m: &Mono, game: P) {
    let class = m.class(game, "", "SystemImport");
    let instance_field = m.field(class, "instance");
    let base = m.class(game, "", "IFSystemImportBase");
    let initialize = m.method(base, "Initialize", 1);
    let instance_addr = m.static_addr(class, instance_field);
    if instance_addr.is_null() || initialize.is_null() {
        log!("bbg: SystemImport not found");
        return;
    }
    // the game creates it at its first scene
    let instance = loop {
        let p: P = unsafe { mono::read(instance_addr, 0) };
        if !p.is_null() {
            break p;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let mut module = crate::bbg_system::module();
    for name in ["System", "Io", "Consume"] {
        let field = m.field(class, &format!("<{name}>k__BackingField"));
        let binding = m.new_object(m.class(game, "", &format!("SystemImport_{name}")));
        if field.is_null() || binding.is_null() {
            log!("bbg: SystemImport_{name} not found");
            continue;
        }
        // Initialize(IntPtr module): the delegates of the module's exports (throws if one is
        // missing)
        let mut args = [&mut module as *mut usize as P];
        if m.invoke(initialize, binding, &mut args).is_none() {
            log!("bbg: SystemImport_{name}: exports missing");
            continue;
        }
        m.set(instance, field, binding);
        log!("bbg: system module {name} bound");
    }
}

// ---- the serial ports ----

/// Mono's `GetProcAddress`: the serial port's `CreateFile` (System.IO.Ports' `DllImport`, ANSI:
/// `CreateFileA`) opens no COM port.
fn hide_serial_ports() {
    let module = unsafe { GetModuleHandleW(windows_sys::w!("mono-2.0-bdwgc.dll")) } as usize;
    match unsafe { iat::hook_module(module, "kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) } {
        Some(o) => GET_PROC_ADDRESS.store(o, Ordering::Relaxed),
        None => log!("bbg: Mono does not import GetProcAddress: COM6 not hidden"),
    }
}

type GetProcAddressFn = unsafe extern "system" fn(P, *const u8) -> P;
type CreateFileWFn = unsafe extern "system" fn(*const u16, u32, u32, P, u32, u32, P) -> P;
type CreateFileAFn = unsafe extern "system" fn(*const u8, u32, u32, P, u32, u32, P) -> P;

unsafe extern "system" fn get_proc_address(module: P, name: *const u8) -> P {
    let orig: GetProcAddressFn = unsafe { std::mem::transmute(GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    let f = unsafe { orig(module, name) };
    // by name (not an ordinal)
    if f.is_null() || (name as usize) >> 16 == 0 {
        return f;
    }
    let n = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes();
    if n == b"CreateFileW" {
        CREATE_FILE_W.store(f as usize, Ordering::Relaxed);
        return create_file_w as *const () as P;
    }
    if n == b"CreateFileA" {
        CREATE_FILE_A.store(f as usize, Ordering::Relaxed);
        return create_file_a as *const () as P;
    }
    f
}

/// A COM port's name (`COM6`, `\\.\COM6`): absent.
fn absent_port(path: &str) -> bool {
    let port = path.strip_prefix("\\\\.\\").unwrap_or(path);
    if port.len() > 3 && port[..3].eq_ignore_ascii_case("COM") && port[3..].bytes().all(|b| b.is_ascii_digit()) {
        log!("bbg: {port} absent (the I/O board is fed by the payload)");
        unsafe { windows_sys::Win32::Foundation::SetLastError(2) };
        return true;
    }
    false
}

unsafe extern "system" fn create_file_a(name: *const u8, access: u32, share: u32, security: P, disposition: u32, flags: u32, template: P) -> P {
    if !name.is_null() && absent_port(&unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy()) {
        return usize::MAX as P;
    }
    let orig: CreateFileAFn = unsafe { std::mem::transmute(CREATE_FILE_A.load(Ordering::Relaxed)) };
    unsafe { orig(name, access, share, security, disposition, flags, template) }
}

unsafe extern "system" fn create_file_w(name: *const u16, access: u32, share: u32, security: P, disposition: u32, flags: u32, template: P) -> P {
    if !name.is_null() {
        let mut len = 0;
        while unsafe { *name.add(len) } != 0 {
            len += 1;
        }
        if absent_port(&String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) })) {
            return usize::MAX as P;
        }
    }
    let orig: CreateFileWFn = unsafe { std::mem::transmute(CREATE_FILE_W.load(Ordering::Relaxed)) };
    unsafe { orig(name, access, share, security, disposition, flags, template) }
}

// ---- the I/O board ----

struct Board {
    /// IoManagerMain.instance's address.
    instance: *mut u8,
    boot_step: usize,
    /// m_StickType: TYPE_2 (the default) reads the stick turned a quarter.
    stick_type: usize,
    set_btn_state: P,
    set_data_array: P,
    /// The status object handed to the game and its bytes (pinned).
    state: P,
    data: P,
}

impl Board {
    fn new(m: &Mono, usbio: P) -> Option<Board> {
        let class = m.class(usbio, "UsbIO", "IoManagerMain");
        let state_class = m.class(usbio, "UsbIO.Command.IoCommands.IoStateGetCommandData", "IoState");
        let instance = m.static_addr(class, m.field(class, "instance"));
        let boot_step = m.field(class, "m_BootStep");
        let stick_type = m.field(class, "m_StickType");
        let set_btn_state = m.method(class, "SetBtnState", 1);
        let set_data_array = m.method(state_class, "SetDataArray", 1);
        if instance.is_null() || boot_step.is_null() || stick_type.is_null() || set_btn_state.is_null() || set_data_array.is_null() {
            log!("bbg: UsbIO's IoManagerMain not found: no I/O board");
            return None;
        }
        let state = m.pin(m.new_object(state_class));
        let data = m.pin(m.byte_array(16));
        if state.is_null() || data.is_null() {
            return None;
        }
        Some(Board { instance, boot_step: m.offset(boot_step), stick_type: m.offset(stick_type), set_btn_state, set_data_array, state, data })
    }

    fn update(&self, m: &Mono, pressed: &[Native]) {
        let manager: P = unsafe { mono::read(self.instance, 0) };
        if manager.is_null() {
            return;
        }
        let turned = unsafe { mono::read::<i32>(manager as *const u8, self.stick_type) } == 1;
        let status = status(pressed, turned);
        unsafe { std::ptr::copy_nonoverlapping(status.as_ptr(), (self.data as *mut u8).add(ARRAY_DATA), status.len()) };
        if m.invoke(self.set_data_array, self.state, &mut [self.data]).is_none() || m.invoke(self.set_btn_state, manager, &mut [self.state]).is_none() {
            return;
        }
        // the board's boot failed on the absent port: done, with the payload's status
        let step = manager as *mut u8;
        if unsafe { mono::read::<i32>(step, self.boot_step) } == BOOT_ERROR_END {
            unsafe { mono::write(step, self.boot_step, BOOT_SUCCESS_END) };
            log!("bbg: I/O board ready");
        }
    }
}

/// Sets the `bits` bits at bit `at` (from the most significant bit of byte 0) to `value`.
fn put(b: &mut [u8], at: usize, bits: usize, value: u32) {
    for i in 0..bits {
        let bit = at + i;
        if value >> (bits - 1 - i) & 1 != 0 {
            b[bit / 8] |= 0x80 >> (bit % 8);
        }
    }
}

/// The board's 16 status bytes (UsbIO's IoState). `turned`: the game's stick type TYPE_2, the
/// cabinet's stick mounted a quarter turned (the game's X is 4096 - channel 2, its Y channel 1).
fn status(pressed: &[Native], turned: bool) -> [u8; 16] {
    let stick = wal_payload_common::input(0);
    // 12 bits, 2048 the center; the game's Y high = up
    let axis = |v: i32| (2048 + v * 2047 / 32767).clamp(0, 4095) as u32;
    let mut x = axis(stick.axis(Axis::LeftX) as i32);
    let mut y = axis(-(stick.axis(Axis::LeftY) as i32));
    let mut b = [0u8; 16];
    let mut headphone = false;
    for &n in pressed {
        match n {
            Native::Test => put(&mut b, 12, 1, 1),
            Native::Service => put(&mut b, 13, 1, 1),
            Native::Coin => put(&mut b, 14, 1, 1),
            // byte 9: rows 1-3, left / center / right; the center button of each row
            Native::Bomb => put(&mut b, 9 * 8 + 1, 1, 1),
            Native::Skill1 => put(&mut b, 9 * 8 + 4, 1, 1),
            Native::Skill2 => put(&mut b, 9 * 8 + 7, 1, 1),
            Native::Detonate => put(&mut b, 9 * 8 + 15, 1, 1),
            Native::Back => put(&mut b, 9 * 8 + 18, 1, 1),
            Native::Headphone => headphone = true,
            Native::Up => y = 4095,
            Native::Down => y = 0,
            Native::Left => x = 0,
            Native::Right => x = 4095,
            Native::Touch => {}
        }
    }
    let (ch1, ch2) = if turned { (y, 4096 - x.max(1)) } else { (x, y) };
    put(&mut b, 0, 12, ch1);
    put(&mut b, 16, 12, ch2);
    // the headphone jack's switch: 1 when nothing is plugged
    if !headphone {
        put(&mut b, 9 * 8 + 11, 1, 1);
    }
    b
}

// ---- the touch screen ----

struct Touch {
    /// TouchPanelManager.instance's address.
    instance: *mut u8,
    pointers: usize,
    state: AtomicUsize,
    position: AtomicUsize,
    calibrated: AtomicUsize,
    /// Touching (the press was on the touch screen).
    down: std::sync::atomic::AtomicBool,
}

impl Touch {
    fn new(m: &Mono, game: P) -> Option<Touch> {
        let class = m.class(game, "", "TouchPanelManager");
        let instance = m.static_addr(class, m.field(class, "instance"));
        let pointers = m.field(class, "arrPointerInfo");
        if instance.is_null() || pointers.is_null() {
            log!("bbg: TouchPanelManager not found: no touch screen");
            return None;
        }
        Some(Touch {
            instance,
            pointers: m.offset(pointers),
            state: AtomicUsize::new(0),
            position: AtomicUsize::new(0),
            calibrated: AtomicUsize::new(0),
            down: false.into(),
        })
    }

    /// Touch 0 (`MyPointerInfo`), its field offsets looked up once.
    fn pointer(&self) -> Option<*mut u8> {
        let manager: *mut u8 = unsafe { mono::read(self.instance, 0) };
        if manager.is_null() {
            return None;
        }
        let array: *mut u8 = unsafe { mono::read(manager, self.pointers) };
        if array.is_null() {
            return None;
        }
        let info: *mut u8 = unsafe { mono::read(array, ARRAY_DATA) };
        if info.is_null() {
            return None;
        }
        if self.state.load(Ordering::Relaxed) == 0 {
            let m = mono()?;
            let class = m.class_of(info as P);
            let (s, p, c) = (m.field(class, "state"), m.field(class, "position"), m.field(class, "calibratedPosition"));
            if s.is_null() || p.is_null() || c.is_null() {
                return None;
            }
            self.position.store(m.offset(p), Ordering::Relaxed);
            self.calibrated.store(m.offset(c), Ordering::Relaxed);
            self.state.store(m.offset(s), Ordering::Relaxed);
        }
        Some(info)
    }

    fn update(&self, touching: bool) {
        let at = crate::bbg_screen::pointer_on_touch_screen();
        let was = self.down.load(Ordering::Relaxed);
        // a touch starts on the touch screen, goes on while the button is held
        let down = touching && (was || at.is_some());
        if !down && !was {
            return;
        }
        let Some(info) = self.pointer() else { return };
        if let Some((u, v)) = at {
            let pos = [u * 1280.0, -v * 720.0];
            unsafe {
                mono::write(info, self.position.load(Ordering::Relaxed), pos);
                mono::write(info, self.calibrated.load(Ordering::Relaxed), pos);
            }
        }
        if down != was {
            self.down.store(down, Ordering::Relaxed);
            unsafe { mono::write(info, self.state.load(Ordering::Relaxed), if down { TOUCH_DOWN } else { TOUCH_UP }) };
            log!("bbg: touch {} at {at:?}", if down { "down" } else { "up" });
        }
    }
}
