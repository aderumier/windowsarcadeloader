//! JoJo's Bizarre Adventure: Last Survivor (2019, Unreal Engine 4, 64-bit),
//! `AJB\Binaries\Win64\AJB-Win64-Shipping.exe` (build `JJL117-1-NA-MPR0-R02`, 17.02). The
//! Bandai Namco BNA1 cabinet: no launcher, no dongle library; the payload is the game's
//! `xinput1_3.dll` (ordinals 2 / 3 imported, see [`crate::game_pads_hidden`]).
//!
//! - I/O board: the USIO on USB through the game's `winusio.dll` (`CWinUsio`, WinUSB: `usio.rs`).
//! - dongle: `nbamUsbFinder_GetInformation(0, 0, 0x0b9a, 0x0c20, info)` (the Namco USB key
//!   library), 1 and a 12-digit serial at `info + 0x428` ([`SERIAL`]).
//! - touch panel: the check is `GetSystemMetrics(SM_TABLETPC)`.
//! - card reader: Sega's Aime reader on COM1 and its display (VFD) on COM2, for `libamw.dll`
//!   (`sgreader.rs`, `vfd.rs`; the card reader check wants both); libamw's NVRAM
//!   is `D:\sysfile.dat` (the dump's `D` folder, profile `drives`).
//! - network: the AMCUS auth daemon (`IAuth`) in-process (`amcus.rs`), ALL.Net identity from
//!   the dump's `AMCUS\AMConfig.ini` (game id SDEX, game code JJL1 17.02).

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use wal_payload_common::mapping::ButtonMap;
use wal_protocol::{Axis, StickState};

use crate::{GAME_BASE, module, sgreader, usio};

pub(crate) const GAME: &str = "AJB-Win64-Shipping.exe";

/// The cabinet's dongle serial, `MMMM R T CC NNNN` as Bandai Namco's: the game's check wants
/// region 1, type 3, 4 or 5, CC 03, and the network id `ABLN` + the serial's last 7 digits.
const SERIAL: &str = "282713030001";
const NET_ID: &str = "ABLN3030001";
const GAME_ID: &str = "SDEX";
const GAME_CD: &str = "JJL1";

/// `nbamUsbFinder_GetInformation(.., .., vid, pid, info)`: the USB key, its serial (UTF-16) at
/// `info + 0x428` of 0x628 bytes.
unsafe extern "system" fn usb_finder_information(_a: usize, _b: usize, _vid: usize, pid: usize, info: *mut u8) -> i32 {
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| log!("jojo: usb key {pid:#06x}, serial {SERIAL}"));
    if std::env::var("WAL_NAMCOES3_JOJO_PROBE").is_ok_and(|v| v == "1") {
        unsafe extern "system" {
            fn RtlCaptureStackBackTrace(skip: u32, count: u32, frames: *mut usize, hash: *mut u32) -> u16;
        }
        let mut frames = [0usize; 8];
        let n = unsafe { RtlCaptureStackBackTrace(0, 8, frames.as_mut_ptr(), std::ptr::null_mut()) } as usize;
        let base = GAME_BASE.load(Ordering::Relaxed);
        let rvas: Vec<String> = frames[..n].iter().map(|f| format!("{:#x}", f.wrapping_sub(base))).collect();
        log!("probe: usb key stack {}", rvas.join(" "));
    }
    if !info.is_null() {
        let mut serial = [0u8; 12];
        serial.copy_from_slice(SERIAL.as_bytes());
        unsafe { crate::patches::usb_key(info, pid as u16, &serial) };
    }
    1
}

/// The AMActivator library (v28: the USB key's BitLocker unlock, the cabinet signature and its
/// one-time key), answered as bananatools' `platform/amactivator.c`: done, valid, never busy.
mod activator {
    use std::ffi::c_void;

    pub(super) static HANDLE: [u8; 0x3E8] = [0; 0x3E8];
    pub(super) extern "system" fn create() -> *const c_void {
        HANDLE.as_ptr().cast()
    }
    pub(super) extern "system" fn nothing(_h: usize) {}
    pub(super) extern "system" fn set(_h: usize, _value: usize, _len: usize) -> u64 {
        0
    }
    pub(super) extern "system" fn yes(_h: usize) -> bool {
        true
    }
    pub(super) extern "system" fn no(_h: usize) -> bool {
        false
    }
    pub(super) extern "system" fn generation(_h: usize) -> i32 {
        1
    }
    /// `GetSignatureLastStatus`, `GetOneTimeKeyLastStatus`: 3 (done)
    pub(super) extern "system" fn status(_h: usize) -> i32 {
        3
    }
    pub(super) extern "system" fn one_time_key(_h: usize) -> i32 {
        99_999_999
    }
    pub(super) extern "system" fn expiration(_h: usize) -> u32 {
        u32::MAX
    }
}

fn hook_activator(base: usize) {
    let hooks: &[(&str, usize)] = &[
        ("AMActivator_Create", activator::create as *const () as usize),
        ("AMActivator_Destroy", activator::nothing as *const () as usize),
        ("AMActivator_Update", activator::nothing as *const () as usize),
        ("AMActivator_SetDevelopmentMode", activator::nothing as *const () as usize),
        ("AMActivator_IsBusy", activator::no as *const () as usize),
        ("AMActivator_SetUSBSerialID", activator::set as *const () as usize),
        ("AMActivator_SetUSBProductID", activator::set as *const () as usize),
        ("AMActivator_SetUSBVendorID", activator::set as *const () as usize),
        ("AMActivator_SetUSBBitLockerPassword", activator::set as *const () as usize),
        ("AMActivator_BitLockerLock", activator::yes as *const () as usize),
        ("AMActivator_BitLockerUnlock", activator::yes as *const () as usize),
        ("AMActivator_RequestSignature", activator::yes as *const () as usize),
        ("AMActivator_RequestOneTimeKey", activator::yes as *const () as usize),
        ("AMActivator_Restore", activator::yes as *const () as usize),
        ("AMActivator_GetSignatureGeneration", activator::generation as *const () as usize),
        ("AMActivator_GetSignatureLastStatus", activator::status as *const () as usize),
        ("AMActivator_GetOneTimeKeyLastStatus", activator::status as *const () as usize),
        ("AMActivator_GetOneTimeKey", activator::one_time_key as *const () as usize),
        ("AMActivator_GetOneTimeKeyExpiration", activator::expiration as *const () as usize),
    ];
    let mut hooked = 0;
    for &(name, f) in hooks {
        if unsafe { iat::hook_module(base, "amactivator.dll", name, f) }.is_some() {
            hooked += 1;
        }
    }
    if hooked > 0 {
        log!("jojo: AMActivator answered ({hooked} functions)");
    }
}

static GET_VOLUME_INFORMATION_A: AtomicUsize = AtomicUsize::new(0);

/// libamw looks for its log volume, labelled `SEGA_AM_LOG`, from `\\.\Z:\` down to A:: the
/// dump's `L` folder (drive L:, profile `drives`).
unsafe extern "system" fn get_volume_information_a(root: *const u8, name: *mut u8, size: u32, serial: *mut u32, max: *mut u32, flags: *mut u32, fs: *mut u8, fs_size: u32) -> i32 {
    const LABEL: &[u8] = b"SEGA_AM_LOG\0";
    let path = if root.is_null() { &[][..] } else { unsafe { std::ffi::CStr::from_ptr(root.cast()) }.to_bytes() };
    if path.eq_ignore_ascii_case(b"\\\\.\\L:\\") || path.eq_ignore_ascii_case(b"L:\\") {
        unsafe {
            if !name.is_null() && size as usize >= LABEL.len() {
                std::ptr::copy_nonoverlapping(LABEL.as_ptr(), name, LABEL.len());
            }
            if !serial.is_null() {
                *serial = 0x5741_4c00;
            }
            if !max.is_null() {
                *max = 255;
            }
            if !flags.is_null() {
                *flags = 0;
            }
            if !fs.is_null() && fs_size >= 5 {
                std::ptr::copy_nonoverlapping(b"NTFS\0".as_ptr(), fs, 5);
            }
        }
        return 1;
    }
    type F = unsafe extern "system" fn(*const u8, *mut u8, u32, *mut u32, *mut u32, *mut u32, *mut u8, u32) -> i32;
    let real: F = unsafe { std::mem::transmute(GET_VOLUME_INFORMATION_A.load(Ordering::Relaxed)) };
    unsafe { real(root, name, size, serial, max, flags, fs, fs_size) }
}

static GET_VOLUME_INFORMATION_W: AtomicUsize = AtomicUsize::new(0);

/// The same for the wide calls (`L:\`).
unsafe extern "system" fn get_volume_information_w(root: *const u16, name: *mut u16, size: u32, serial: *mut u32, max: *mut u32, flags: *mut u32, fs: *mut u16, fs_size: u32) -> i32 {
    let len = if root.is_null() { 0 } else { (0..).take_while(|&i| unsafe { *root.add(i) } != 0).count() };
    let path = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(root, len) });
    if path.eq_ignore_ascii_case("L:\\") || path.eq_ignore_ascii_case("\\\\.\\L:\\") {
        let label: Vec<u16> = "SEGA_AM_LOG\0".encode_utf16().collect();
        let ntfs: Vec<u16> = "NTFS\0".encode_utf16().collect();
        unsafe {
            if !name.is_null() && size as usize >= label.len() {
                std::ptr::copy_nonoverlapping(label.as_ptr(), name, label.len());
            }
            if !serial.is_null() {
                *serial = 0x5741_4c00;
            }
            if !max.is_null() {
                *max = 255;
            }
            if !flags.is_null() {
                *flags = 0;
            }
            if !fs.is_null() && fs_size as usize >= ntfs.len() {
                std::ptr::copy_nonoverlapping(ntfs.as_ptr(), fs, ntfs.len());
            }
        }
        return 1;
    }
    type F = unsafe extern "system" fn(*const u16, *mut u16, u32, *mut u32, *mut u32, *mut u32, *mut u16, u32) -> i32;
    let real: F = unsafe { std::mem::transmute(GET_VOLUME_INFORMATION_W.load(Ordering::Relaxed)) };
    unsafe { real(root, name, size, serial, max, flags, fs, fs_size) }
}

static GET_SYSTEM_METRICS: AtomicUsize = AtomicUsize::new(0);

/// The touch panel check is `GetSystemMetrics(SM_TABLETPC)`: a tablet PC.
unsafe extern "system" fn get_system_metrics(index: i32) -> i32 {
    const SM_TABLETPC: i32 = 86;
    if index == SM_TABLETPC {
        return 1;
    }
    let real: unsafe extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(GET_SYSTEM_METRICS.load(Ordering::Relaxed)) };
    unsafe { real(index) }
}

unsafe extern "system" {
    fn SetLastError(code: u32);
}

/// The cabinet's address in the store network (the game's `GetMUCHAServerLocalIP`), set by the game
/// with netsh.
const CABINET_IP: [u8; 4] = [192, 168, 171, 2];
static STATIC_ADDRESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// `IcmpSendEcho`: the store network check pings the MUCHA server and the content router
/// gateway (an answer within 1 ms).
unsafe extern "system" fn icmp_send_echo(_handle: usize, dest: u32, _data: *const u8, _size: u16, _options: usize, reply: *mut u8, reply_size: u32, _timeout: u32) -> u32 {
    static LOGGED: std::sync::Mutex<Vec<u32>> = std::sync::Mutex::new(Vec::new());
    {
        let mut logged = LOGGED.lock().unwrap();
        if !logged.contains(&dest) {
            logged.push(dest);
            log!("jojo: ping {}", std::net::Ipv4Addr::from(u32::from_be(dest)));
        }
    }
    // the cabinet's own address: free before the game sets it (else another cabinet is the
    // store's server: the game becomes its DHCP client), then it answers
    if dest == u32::from_be_bytes(CABINET_IP).to_be() && !STATIC_ADDRESS.load(Ordering::Relaxed) {
        unsafe { SetLastError(11010) }; // IP_REQ_TIMED_OUT
        return 0;
    }
    // ICMP_ECHO_REPLY: address, status (0 success), round trip, data size, data, options (TTL)
    if reply.is_null() || reply_size < 40 {
        return 0;
    }
    unsafe {
        std::ptr::write_bytes(reply, 0, 40);
        *(reply as *mut u32) = dest;
        *(reply.add(8) as *mut u32) = 1;
        *reply.add(24) = 64;
    }
    1
}

static GET_ADAPTERS_INFO: AtomicUsize = AtomicUsize::new(0);
static GET_ADAPTERS_ADDRESSES: AtomicUsize = AtomicUsize::new(0);

/// `GetAdaptersInfo`: once the game set the cabinet's address, the first adapter with an
/// address has it (IP_ADAPTER_INFO x64: address 0x1c0, mask 0x1d0, gateway 0x1f0, text).
unsafe extern "system" fn get_adapters_info(adapters: *mut u8, size: *mut u32) -> u32 {
    type F = unsafe extern "system" fn(*mut u8, *mut u32) -> u32;
    let real: F = unsafe { std::mem::transmute(GET_ADAPTERS_INFO.load(Ordering::Relaxed)) };
    let status = unsafe { real(adapters, size) };
    if status != 0 || adapters.is_null() || !STATIC_ADDRESS.load(Ordering::Relaxed) {
        return status;
    }
    let put = |at: *mut u8, text: &str| unsafe {
        std::ptr::write_bytes(at, 0, 16);
        std::ptr::copy_nonoverlapping(text.as_ptr(), at, text.len());
    };
    let ip = std::net::Ipv4Addr::from(CABINET_IP).to_string();
    let gateway = std::net::Ipv4Addr::from([CABINET_IP[0], CABINET_IP[1], CABINET_IP[2], 1]).to_string();
    unsafe {
        let mut node = adapters;
        while !node.is_null() {
            let current = std::ffi::CStr::from_ptr(node.add(0x1c0).cast()).to_bytes();
            if !current.is_empty() && current != b"0.0.0.0" && !current.starts_with(b"127.") {
                put(node.add(0x1c0), &ip);
                put(node.add(0x1d0), "255.255.255.0");
                put(node.add(0x1f0), &gateway);
                break;
            }
            node = *(node as *const *mut u8);
        }
    }
    status
}

/// `GetAdaptersAddresses`: the same, the first non-loopback adapter's first IPv4 address.
unsafe extern "system" fn get_adapters_addresses(family: u32, flags: u32, reserved: usize, adapters: *mut u8, size: *mut u32) -> u32 {
    type F = unsafe extern "system" fn(u32, u32, usize, *mut u8, *mut u32) -> u32;
    let real: F = unsafe { std::mem::transmute(GET_ADAPTERS_ADDRESSES.load(Ordering::Relaxed)) };
    let status = unsafe { real(family, flags, reserved, adapters, size) };
    if status != 0 || adapters.is_null() || !STATIC_ADDRESS.load(Ordering::Relaxed) {
        return status;
    }
    unsafe {
        // IP_ADAPTER_ADDRESSES: Next +8, FirstUnicastAddress +24; the unicast's SOCKET_ADDRESS
        // at +16 (sockaddr pointer)
        let mut node = adapters;
        while !node.is_null() {
            let unicast = *(node.add(24) as *const *mut u8);
            if !unicast.is_null() {
                let sa = *(unicast.add(16) as *const *mut u8);
                if !sa.is_null() && *(sa as *const u16) == 2 && *sa.add(4) != 127 {
                    std::ptr::copy_nonoverlapping(CABINET_IP.as_ptr(), sa.add(4), 4);
                    break;
                }
            }
            node = *(node.add(8) as *const *mut u8);
        }
    }
    status
}

static CREATE_PROCESS_W: AtomicUsize = AtomicUsize::new(0);

/// `CreateProcessW`: the store network check sets the cabinet's address with `netsh interface ip
/// set address "<adapter>" static <ip> 255.255.255.0 <gateway>` (and the DNS) and wants exit
/// code 0; Wine's netsh does not do it. The cabinet's address is answered elsewhere: a command
/// that ends with 0 runs instead.
unsafe extern "system" fn create_process_w(app: *const u16, cmd: *mut u16, pa: usize, ta: usize, inherit: i32, flags: u32, env: usize, dir: *const u16, si: usize, pi: usize) -> i32 {
    type F = unsafe extern "system" fn(*const u16, *mut u16, usize, usize, i32, u32, usize, *const u16, usize, usize) -> i32;
    let real: F = unsafe { std::mem::transmute(CREATE_PROCESS_W.load(Ordering::Relaxed)) };
    let text = |p: *const u16| -> String {
        if p.is_null() {
            return String::new();
        }
        let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
    };
    let (app_s, cmd_s) = (text(app), text(cmd));
    if app_s.to_ascii_lowercase().contains("netsh") || cmd_s.to_ascii_lowercase().trim_start_matches('"').starts_with("netsh") {
        log!("jojo: {app_s} {cmd_s}: done");
        if cmd_s.contains("set address") {
            STATIC_ADDRESS.store(cmd_s.contains(" static "), Ordering::Relaxed);
        }
        let mut replacement: Vec<u16> = "cmd.exe /c exit 0".encode_utf16().chain(Some(0)).collect();
        return unsafe { real(std::ptr::null(), replacement.as_mut_ptr(), pa, ta, inherit, flags, env, dir, si, pi) };
    }
    unsafe { real(app, cmd, pa, ta, inherit, flags, env, dir, si, pi) }
}

static CO_CREATE_INSTANCE: AtomicUsize = AtomicUsize::new(0);

/// The AMCUS auth daemon (`IAuth`), in-process (`amcus.rs`).
unsafe extern "system" fn co_create_instance(clsid: *const u8, outer: *mut c_void, context: u32, iid: *const u8, out: *mut *mut c_void) -> i32 {
    if !clsid.is_null() && unsafe { std::slice::from_raw_parts(clsid, 16) } == crate::amcus::CLSID {
        if !out.is_null() {
            let object = crate::amcus::daemon(&crate::amcus::OBJECT_NEW);
            unsafe { *out = (object as *const crate::amcus::Object).cast_mut().cast() };
        }
        log!("jojo: in-process AMCUS auth (IAuth)");
        return 0;
    }
    type F = unsafe extern "system" fn(*const u8, *mut c_void, u32, *const u8, *mut *mut c_void) -> i32;
    let real: F = unsafe { std::mem::transmute(CO_CREATE_INSTANCE.load(Ordering::Relaxed)) };
    unsafe { real(clsid, outer, context, iid, out) }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Native {
    Test,
    Service,
    Coin,
    Touch,
    Switch(u32),
}

/// The Usio plugin's keys and their switch bits (its `USIO_*` key index -> bit table).
const SW_TEST: u32 = 0x80;
const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("touch", Native::Touch),
    ("start", Native::Switch(0x8000)),
    ("up", Native::Switch(0x2000)),
    ("down", Native::Switch(0x1000)),
    ("left", Native::Switch(0x800)),
    ("right", Native::Switch(0x400)),
    ("push1", Native::Switch(0x200)),
    ("push2", Native::Switch(0x100)),
    ("push3", Native::Switch(0x80_0000)),
    ("push4", Native::Switch(0x40_0000)),
    ("push5", Native::Switch(0x20_0000)),
    ("push6", Native::Switch(0x10_0000)),
    ("push7", Native::Switch(0x8_0000)),
    ("push8", Native::Switch(0x4_0000)),
    ("push9", Native::Switch(0x2_0000)),
    ("push10", Native::Switch(0x1_0000)),
    ("push11", Native::Switch(0x8000_0000)),
];

/// Virtual stick -> cabinet; profiles override it with `native_map`. The game's actions on the
/// USIO keys (its `AddActionMapping` calls): push9 jump / decide, push8 crouch / cancel,
/// push11 interact, push10 map, push2 / push3 / push4 L1 / L2 / L3, push5 / push6 R1 / R2,
/// push7 emote; the menus move with the left stick (analog 0 / 1).
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("b1", "push9"),
    ("b2", "push8"),
    ("b3", "push11"),
    ("b4", "push10"),
    ("b5", "push2"),
    ("b6", "push5"),
    ("b7", "push3"),
    ("b8", "push6"),
    ("kp1", "push4"),
    ("kp2", "push7"),
    ("kp0", "touch"),
];

/// The service switch bit (also counted as service credits).
const SW_SERVICE: u32 = 0x4000;

struct State {
    map: ButtonMap<Native>,
    coins: u16,
    coin_held: bool,
    test_on: bool,
    test_held: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// The touch native held (the USIO thread polls the sticks).
static TOUCH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn usio_poll() -> usio::Inputs {
    let mut guard = STATE.lock().unwrap();
    let s = guard.get_or_insert_with(|| State { map: ButtonMap::new(NATIVES, DEFAULT_MAP), coins: 0, coin_held: false, test_on: false, test_held: false });
    let stick: StickState = wal_payload_common::input(0);
    let mut sw = 0u32;
    let (mut coin, mut test, mut touch) = (false, false, false);
    s.map.for_each_pressed(&stick, |n| match n {
        Native::Test => test = true,
        Native::Coin => coin = true,
        Native::Service => sw |= SW_SERVICE,
        Native::Touch => touch = true,
        Native::Switch(bit) => sw |= bit,
    });
    TOUCH.store(touch, Ordering::Relaxed);
    // the cabinet's test switch latches
    if test && !s.test_held {
        s.test_on = !s.test_on;
        log!("jojo: test switch {}", if s.test_on { "on" } else { "off" });
    }
    s.test_held = test;
    if s.test_on {
        sw |= SW_TEST;
    }
    if s.coin_held && !coin {
        s.coins = s.coins.wrapping_add(1) & 0x3FFF;
    }
    s.coin_held = coin;
    // the two sticks: analog 0 / 1 and 2 / 3, up high; the directions push the left one
    let mut analog = [0x8000u16; 8];
    analog[0] = stick.axis_u16(Axis::LeftX);
    analog[1] = !stick.axis_u16(Axis::LeftY);
    analog[2] = stick.axis_u16(Axis::RightX);
    analog[3] = !stick.axis_u16(Axis::RightY);
    for (bit, index, value) in [(0x2000, 1, 0xFFFF), (0x1000, 1, 0), (0x800, 0, 0), (0x400, 0, 0xFFFF)] {
        if sw & bit != 0 {
            analog[index] = value;
        }
    }
    usio::Inputs { switches: sw, coins: [s.coins, 0], analog }
}

/// The touch panel: Unreal's left mouse button (the game's `UI_Touch`). Where: the real
/// cursor (the desktop's mouse or a lightgun moving it) on the game's window, posted as mouse
/// messages (`input_filter`: the real mouse never reaches the game); when: the `touch` native.
mod touch {
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    use wal_payload_common::input_filter::{own_mouse, post_mouse};
    use wal_payload_common::{gun, log, pointer_d3d11};
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, GetCursorPos, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};

    /// `wParam`'s left button flag.
    const MK_LBUTTON: usize = 0x0001;

    /// The real cursor in the game window's client area (pixels), on it.
    fn cursor() -> Option<(i32, i32, i32, i32)> {
        let hwnd = gun::find_window();
        let (mut p, mut r) = (POINT { x: 0, y: 0 }, RECT { left: 0, top: 0, right: 0, bottom: 0 });
        if hwnd.is_null() || unsafe { GetCursorPos(&mut p) } == 0 || unsafe { ScreenToClient(hwnd, &mut p) } == 0 || unsafe { GetClientRect(hwnd, &mut r) } == 0 {
            return None;
        }
        (p.x >= 0 && p.y >= 0 && p.x < r.right && p.y < r.bottom).then_some((p.x, p.y, r.right, r.bottom))
    }

    /// The touch position on the picture (fractions), for the pointer dot.
    fn in_picture() -> Option<(f32, f32)> {
        cursor().map(|(x, y, w, h)| (x as f32 / w as f32, y as f32 / h as f32))
    }

    fn thread() {
        let (mut touched, mut last) = (false, (i32::MIN, i32::MIN));
        loop {
            std::thread::sleep(Duration::from_millis(8));
            let hwnd = gun::find_window();
            if hwnd.is_null() {
                continue;
            }
            let at = cursor().map_or(last, |(x, y, _, _)| (x, y));
            let touch = super::TOUCH.load(Ordering::Relaxed);
            let buttons = if touch { MK_LBUTTON } else { 0 };
            if at != last && at.0 != i32::MIN {
                post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, buttons);
                last = at;
            }
            if touch != touched && last.0 != i32::MIN {
                post_mouse(hwnd, if touch { WM_LBUTTONDOWN } else { WM_LBUTTONUP }, last.0, last.1, buttons);
                touched = touch;
            }
        }
    }

    pub(super) fn init(base: usize) {
        own_mouse(base);
        // the common pointer option: a dot where it touches
        if wal_payload_common::crosshairs(false) {
            let size = std::env::var("WAL_CROSSHAIR_SIZE").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.5);
            pointer_d3d11::init(pointer_d3d11::Dot { color: [1.0, 0.125, 0.125], size: (size / 100.0).clamp(0.005, 0.2), position: in_picture, crosshair: false });
        }
        std::thread::spawn(thread);
        log!("jojo: touch panel from the cursor");
    }
}

/// The game server (unfinished, `WAL_JOJO_ONLINE=1`): the card login works and the game goes
/// online, but the sync server's protocol is not known yet (the game leaves every ~26 s,
/// "disconnected from the game server", even in training); off, the game plays offline as a
/// guest.
fn online() -> bool {
    std::env::var("WAL_JOJO_ONLINE").is_ok_and(|v| v == "1")
}

/// The player's Aime card (`WAL_NAMCOES3_ACCESS_CODE`, else one made once and kept in
/// `F:\\aime.txt`), touched like a player does: on the reader for 20 seconds after a coin (the
/// game reads cards at the title and its card prompt), 3 seconds after the card button, until
/// the game reads it. `WAL_JOJO_NO_CARD=1`: none (guests only).
fn card() {
    if std::env::var("WAL_JOJO_NO_CARD").is_ok_and(|v| v == "1") {
        return log!("jojo: no card on the reader");
    }
    let code = crate::bpreader::player_card_in("F:\\aime.txt").access_code.clone();
    log!("jojo: card {code}");
    std::thread::spawn(move || {
        let (mut coin_held, mut card_held) = (false, false);
        loop {
            let input = wal_payload_common::input(0);
            let (coin, card) = (input.pressed(wal_protocol::button::COIN), input.pressed(wal_protocol::button::CARD));
            if coin && !coin_held {
                sgreader::insert(&code, std::time::Duration::from_secs(20));
            }
            if card && !card_held {
                sgreader::insert(&code, std::time::Duration::from_secs(3));
            }
            (coin_held, card_held) = (coin, card);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
}

/// The game's libcurl checks the server's certificate against its CA bundle (the console
/// variable `n.VerifyPeer`, on by default): the game server's is self-signed. The user's saved
/// engine config, read after the game's own (and only once the payload is loaded), sets the
/// variable off (`[SystemSettings]`, where the engine takes console variables from).
fn no_peer_verification() {
    let Ok(local) = std::env::var("LOCALAPPDATA") else {
        return log!("jojo: no LOCALAPPDATA, certificate check left on");
    };
    let dir = std::path::Path::new(&local).join("AJB\\Saved\\Config\\WindowsNoEditor");
    let path = dir.join("Engine.ini");
    let ini = std::fs::read_to_string(&path).unwrap_or_default();
    if ini.contains("n.VerifyPeer=0") {
        return;
    }
    let ini = format!("{}\r\n[SystemSettings]\r\nn.VerifyPeer=0\r\n", ini.trim_end());
    match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, ini)) {
        Ok(()) => log!("jojo: server certificate check off ({})", path.display()),
        Err(e) => log!("jojo: {}: {e}", path.display()),
    }
}

pub(crate) fn attach(base: usize) {
    GAME_BASE.store(base, Ordering::Relaxed);
    unsafe {
        if iat::hook_module(base, "nbamUsbFinder.dll", "nbamUsbFinder_GetInformation", usb_finder_information as *const () as usize).is_none() {
            log!("jojo: nbamUsbFinder_GetInformation not hooked");
        }
        match iat::hook_module(base, "user32.dll", "GetSystemMetrics", get_system_metrics as *const () as usize) {
            Some(o) => GET_SYSTEM_METRICS.store(o, Ordering::Relaxed),
            None => log!("jojo: GetSystemMetrics not hooked"),
        }
        match iat::hook_module(base, "kernel32.dll", "CreateProcessW", create_process_w as *const () as usize) {
            Some(o) => CREATE_PROCESS_W.store(o, Ordering::Relaxed),
            None => log!("jojo: CreateProcessW not hooked"),
        }
        match iat::hook_module(base, "iphlpapi.dll", "GetAdaptersInfo", get_adapters_info as *const () as usize) {
            Some(o) => GET_ADAPTERS_INFO.store(o, Ordering::Relaxed),
            None => log!("jojo: GetAdaptersInfo not hooked"),
        }
        match iat::hook_module(base, "iphlpapi.dll", "GetAdaptersAddresses", get_adapters_addresses as *const () as usize) {
            Some(o) => GET_ADAPTERS_ADDRESSES.store(o, Ordering::Relaxed),
            None => log!("jojo: GetAdaptersAddresses not hooked"),
        }
        if iat::hook_module(base, "iphlpapi.dll", "IcmpSendEcho", icmp_send_echo as *const () as usize).is_none() {
            log!("jojo: IcmpSendEcho not hooked");
        }
        match iat::hook_module(base, "ole32.dll", "CoCreateInstance", co_create_instance as *const () as usize) {
            Some(o) => CO_CREATE_INSTANCE.store(o, Ordering::Relaxed),
            None => log!("jojo: CoCreateInstance not hooked"),
        }
    }
    hook_activator(base);
    if std::env::var("WAL_NAMCOES3_JOJO_PROBE").is_ok_and(|v| v == "1") {
        probe::init(base);
    }
    let amw = module("libamw.dll");
    match unsafe { iat::hook_module(amw, "kernel32.dll", "GetVolumeInformationA", get_volume_information_a as *const () as usize) } {
        Some(o) => GET_VOLUME_INFORMATION_A.store(o, Ordering::Relaxed),
        None => log!("jojo: libamw GetVolumeInformationA not hooked"),
    }
    match unsafe { iat::hook_module(amw, "kernel32.dll", "GetVolumeInformationW", get_volume_information_w as *const () as usize) } {
        Some(o) => GET_VOLUME_INFORMATION_W.store(o, Ordering::Relaxed),
        None => log!("jojo: libamw GetVolumeInformationW not hooked"),
    }
    sgreader::init("COM1", &[amw]);
    if online() {
        card();
    }
    crate::vfd::init("COM2", &[amw]);
    if std::env::var("WAL_NAMCOES3_AMW_TRACE").is_ok_and(|v| v == "1") {
        amw_trace::init(amw);
        amw_trace::game(base);
    }
    crate::amcus::settle_idle();
    crate::amcus::set_identity(crate::amcus::Identity { game_id: GAME_ID, game_cd: GAME_CD, game_ver: (28, 2), serial: SERIAL, scheme: "https://", path: "/", net_id: NET_ID });
    // the store network the game sets up (netsh): its gateway is the content router, which
    // the game wants apart from the shop's router (else "No Content Router", 5-1)
    crate::amcus::set_cabinet_network(["192.168.0.1", "192.168.171.1", "192.168.171.2", "255.255.255.0", "192.168.171.1", "192.168.171.1"]);
    if online() {
        no_peer_verification();
        let verbose = std::env::var("WAL_NAMCOES3_SERVER_LOG").is_ok_and(|v| v == "1");
        wal_namcoes3srv::jojo::start(&crate::amcus::game_server(), "F:\\jojosrv", |line| log!("{line}"), verbose);
        wal_namcoes3srv::jojo::start_sync("127.0.0.1:9003");
    }
    let winusio = module("winusio.dll");
    if winusio == 0 {
        log!("jojo: winusio.dll not loaded, no I/O board");
    } else {
        usio::init(winusio, usio_poll);
        // the boot check wants firmware 120..=126
        usio::set_memory(0x12, &126u16.to_le_bytes());
        usio::set_memory(0x1800, b"NBGI.REAL_USIO\0");
    }
    crate::game_pads_hidden(base);
    wal_payload_common::input_filter::init(base, |_| false);
    touch::init(base);
}

/// `WAL_NAMCOES3_AMW_TRACE=1`: libamw's files, volumes and registry keys to the log.
mod amw_trace {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use wal_payload_common::{iat, log};

    static CREATE_FILE_W: AtomicUsize = AtomicUsize::new(0);
    static CREATE_FILE_A: AtomicUsize = AtomicUsize::new(0);
    static VOLUME_A: AtomicUsize = AtomicUsize::new(0);
    static VOLUME_W: AtomicUsize = AtomicUsize::new(0);
    static REG_OPEN: AtomicUsize = AtomicUsize::new(0);
    static REG_CREATE: AtomicUsize = AtomicUsize::new(0);
    static ATTRIBUTES: AtomicUsize = AtomicUsize::new(0);

    fn wide(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
    }
    fn ansi(p: *const u8) -> String {
        if p.is_null() {
            return String::new();
        }
        unsafe { std::ffi::CStr::from_ptr(p.cast()) }.to_string_lossy().into_owned()
    }

    unsafe extern "system" fn create_file_w(name: *const u16, a: u32, s: u32, sa: usize, d: u32, f: u32, t: usize) -> usize {
        let real: unsafe extern "system" fn(*const u16, u32, u32, usize, u32, u32, usize) -> usize = unsafe { std::mem::transmute(CREATE_FILE_W.load(Ordering::Relaxed)) };
        let h = unsafe { real(name, a, s, sa, d, f, t) };
        log!("amw: CreateFileW {} -> {h:#x}", wide(name));
        h
    }
    unsafe extern "system" fn create_file_a(name: *const u8, a: u32, s: u32, sa: usize, d: u32, f: u32, t: usize) -> usize {
        let real: unsafe extern "system" fn(*const u8, u32, u32, usize, u32, u32, usize) -> usize = unsafe { std::mem::transmute(CREATE_FILE_A.load(Ordering::Relaxed)) };
        let h = unsafe { real(name, a, s, sa, d, f, t) };
        log!("amw: CreateFileA {} -> {h:#x}", ansi(name));
        h
    }
    unsafe extern "system" fn volume_a(root: *const u8, name: *mut u8, n: u32, serial: *mut u32, max: *mut u32, flags: *mut u32, fs: *mut u8, fsn: u32) -> i32 {
        let real: unsafe extern "system" fn(*const u8, *mut u8, u32, *mut u32, *mut u32, *mut u32, *mut u8, u32) -> i32 = unsafe { std::mem::transmute(VOLUME_A.load(Ordering::Relaxed)) };
        let r = unsafe { real(root, name, n, serial, max, flags, fs, fsn) };
        log!("amw: GetVolumeInformationA {} -> {r} {}", ansi(root), if r != 0 { ansi(name) } else { String::new() });
        r
    }
    unsafe extern "system" fn volume_w(root: *const u16, name: *mut u16, n: u32, serial: *mut u32, max: *mut u32, flags: *mut u32, fs: *mut u16, fsn: u32) -> i32 {
        let real: unsafe extern "system" fn(*const u16, *mut u16, u32, *mut u32, *mut u32, *mut u32, *mut u16, u32) -> i32 = unsafe { std::mem::transmute(VOLUME_W.load(Ordering::Relaxed)) };
        let r = unsafe { real(root, name, n, serial, max, flags, fs, fsn) };
        log!("amw: GetVolumeInformationW {} -> {r} {}", wide(root), if r != 0 { wide(name) } else { String::new() });
        r
    }
    unsafe extern "system" fn reg_open(key: usize, sub: *const u16, o: u32, sam: u32, out: *mut usize) -> i32 {
        let real: unsafe extern "system" fn(usize, *const u16, u32, u32, *mut usize) -> i32 = unsafe { std::mem::transmute(REG_OPEN.load(Ordering::Relaxed)) };
        let r = unsafe { real(key, sub, o, sam, out) };
        log!("amw: RegOpenKeyExW {key:#x} {} -> {r}", wide(sub));
        r
    }
    unsafe extern "system" fn reg_create(key: usize, sub: *const u16, r0: u32, class: usize, o: u32, sam: u32, sa: usize, out: *mut usize, disp: *mut u32) -> i32 {
        let real: unsafe extern "system" fn(usize, *const u16, u32, usize, u32, u32, usize, *mut usize, *mut u32) -> i32 = unsafe { std::mem::transmute(REG_CREATE.load(Ordering::Relaxed)) };
        let r = unsafe { real(key, sub, r0, class, o, sam, sa, out, disp) };
        log!("amw: RegCreateKeyExW {key:#x} {} -> {r}", wide(sub));
        r
    }
    unsafe extern "system" fn attributes(name: *const u16) -> u32 {
        let real: unsafe extern "system" fn(*const u16) -> u32 = unsafe { std::mem::transmute(ATTRIBUTES.load(Ordering::Relaxed)) };
        let r = unsafe { real(name) };
        log!("amw: GetFileAttributesW {} -> {r:#x}", wide(name));
        r
    }

    static CARD_READER_PROPERTY: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn card_reader_property(index: usize, out: *mut u8) -> u8 {
        let real: unsafe extern "C" fn(usize, *mut u8) -> u8 = unsafe { std::mem::transmute(CARD_READER_PROPERTY.load(Ordering::Relaxed)) };
        let r = unsafe { real(index, out) };
        // logged when it changes
        static LAST: std::sync::Mutex<Vec<u8>> = std::sync::Mutex::new(Vec::new());
        let bytes = if out.is_null() { Vec::new() } else { unsafe { std::slice::from_raw_parts(out, 16) }.to_vec() };
        let mut last = LAST.lock().unwrap();
        if *last != bytes {
            let text = |off: usize| -> String {
                let p = unsafe { out.add(off) } as *const u16;
                let len = (0..64).take_while(|&i| unsafe { *p.add(i) } != 0).count();
                String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
            };
            let strings = if out.is_null() { String::new() } else { format!("[{}] [{}] [{}] [{}]", text(4), text(0x84), text(0x104), text(0x184)) };
            log!("amw: amwAimeGetCardReaderProperty({index}) -> {r} {bytes:02X?} {strings}");
            *last = bytes;
        }
        r
    }

    pub(super) fn game(base: usize) {
        if let Some(o) = unsafe { iat::hook_module(base, "libamw.dll", "amwAimeGetCardReaderProperty", card_reader_property as *const () as usize) } {
            CARD_READER_PROPERTY.store(o, Ordering::Relaxed);
        }
    }

    pub(super) fn init(module: usize) {
        let hooks: &[(&str, &str, usize, &AtomicUsize)] = &[
            ("kernel32.dll", "CreateFileW", create_file_w as *const () as usize, &CREATE_FILE_W),
            ("kernel32.dll", "CreateFileA", create_file_a as *const () as usize, &CREATE_FILE_A),
            ("kernel32.dll", "GetVolumeInformationA", volume_a as *const () as usize, &VOLUME_A),
            ("kernel32.dll", "GetVolumeInformationW", volume_w as *const () as usize, &VOLUME_W),
            ("kernel32.dll", "GetFileAttributesW", attributes as *const () as usize, &ATTRIBUTES),
            ("advapi32.dll", "RegOpenKeyExW", reg_open as *const () as usize, &REG_OPEN),
            ("advapi32.dll", "RegCreateKeyExW", reg_create as *const () as usize, &REG_CREATE),
        ];
        for &(dll, name, f, slot) in hooks {
            if let Some(o) = unsafe { iat::hook_module(module, dll, name, f) } {
                slot.store(o, Ordering::Relaxed);
            }
        }
    }
}

/// Debug probes (`WAL_NAMCOES3_JOJO_PROBE=1`): the boot checks' results to the log, through
/// detours on the game's functions (v28.02 addresses).
mod probe {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use wal_payload_common::log;

    /// Jumps from `base + rva` to `hook`; the `len` bytes there (whole instructions, none
    /// relative) run first in the returned trampoline.
    pub(super) fn detour(base: usize, rva: usize, len: usize, hook: usize) -> usize {
        use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc};
        let at = base + rva;
        let tramp = unsafe { VirtualAlloc(std::ptr::null(), 64, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) } as *mut u8;
        unsafe {
            std::ptr::copy_nonoverlapping(at as *const u8, tramp, len);
            // jmp [rip]; dq at + len
            let jmp = tramp.add(len);
            std::ptr::copy_nonoverlapping([0xFFu8, 0x25, 0, 0, 0, 0].as_ptr(), jmp, 6);
            std::ptr::write_unaligned(jmp.add(6) as *mut u64, (at + len) as u64);
        }
        let mut code = vec![0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xE0];
        code[2..10].copy_from_slice(&(hook as u64).to_le_bytes());
        code.resize(len, 0x90);
        crate::patches::write(at, &code);
        tramp as usize
    }

    static SERIAL: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn serial_check(a: u8, b: usize, c: usize, d: usize, e: usize, f: usize) -> u8 {
        let real: unsafe extern "C" fn(u8, usize, usize, usize, usize, usize) -> u8 = unsafe { std::mem::transmute(SERIAL.load(Ordering::Relaxed)) };
        IN_SERIAL.with(|f| f.set(true));
        let r = unsafe { real(a, b, c, d, e, f) };
        IN_SERIAL.with(|f| f.set(false));
        let fstr = |p: usize| -> String {
            if p == 0 {
                return "null".into();
            }
            let (data, num) = unsafe { (*(p as *const *const u16), *((p + 8) as *const i32)) };
            if data.is_null() || num <= 1 {
                return String::new();
            }
            String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(data, num as usize - 1) })
        };
        let list = |p: usize| -> String {
            let (data, num) = unsafe { (*(p as *const usize), *((p + 8) as *const i32)) };
            (0..num.max(0) as usize).map(|i| fstr(data + 16 * i)).collect::<Vec<_>>().join(",")
        };
        let _ = (b, c, e);
        log!("probe: serial check ({a}) -> {r}: [{}] [{}]", list(d), fstr(f));
        r
    }

    thread_local! {
        static IN_SERIAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    static STRCMP: AtomicUsize = AtomicUsize::new(0);

    fn wide(p: *const u16) -> String {
        if p.is_null() {
            return "null".into();
        }
        let len = (0..256).take_while(|&i| unsafe { *p.add(i) } != 0).count();
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
    }

    /// The engine's wide `strcmp` (0x1e09c0), logged inside the serial check.
    unsafe extern "C" fn strcmp(a: *const u16, b: *const u16) -> i32 {
        let real: unsafe extern "C" fn(*const u16, *const u16) -> i32 = unsafe { std::mem::transmute(STRCMP.load(Ordering::Relaxed)) };
        let r = unsafe { real(a, b) };
        if IN_SERIAL.with(|f| f.get()) {
            log!("probe: strcmp [{}] [{}] -> {r}", wide(a), wide(b));
        }
        r
    }

    static INVOKE: AtomicUsize = AtomicUsize::new(0);
    static BASE: AtomicUsize = AtomicUsize::new(0);
    /// Per native (its FName), the last result byte logged.
    static SEEN: std::sync::Mutex<Option<std::collections::HashMap<u64, i32>>> = std::sync::Mutex::new(None);

    /// The UObject's name (`FName::ToString` at 0x692ed0, the name at +0x18).
    fn name(object: usize) -> String {
        #[repr(C)]
        struct FString {
            data: *const u16,
            num: i32,
            max: i32,
        }
        let mut out = FString { data: std::ptr::null(), num: 0, max: 0 };
        let to_string: unsafe extern "C" fn(usize, *mut FString) = unsafe { std::mem::transmute(BASE.load(Ordering::Relaxed) + 0x692ed0) };
        unsafe { to_string(object + 0x18, &mut out) };
        if out.data.is_null() || out.num <= 1 {
            return String::new();
        }
        String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(out.data, out.num as usize - 1) })
    }

    /// `UFunction::Invoke(function, object, stack, result)`: each native's first call and its
    /// result byte when it changes.
    unsafe extern "C" fn invoke(function: usize, object: usize, stack: usize, result: *mut u8) {
        let real: unsafe extern "C" fn(usize, usize, usize, *mut u8) = unsafe { std::mem::transmute(INVOKE.load(Ordering::Relaxed)) };
        unsafe { real(function, object, stack, result) };
        let key = unsafe { *((function + 0x18) as *const u64) };
        let value = if result.is_null() { -1 } else { (unsafe { *result }) as i32 };
        let mut seen = SEEN.lock().unwrap();
        let seen = seen.get_or_insert_with(Default::default);
        if seen.get(&key) != Some(&value) {
            seen.insert(key, value);
            drop(seen);
            log!("probe: {}.{} -> {value}", name(object), name(function));
        }
    }

    static ADD_ACTION_MAPPING: AtomicUsize = AtomicUsize::new(0);

    /// `UPlayerInput::AddActionMapping(input, mapping)` (0x1793c60): the action (FName at +0)
    /// and its key (FKey's name at +0x10).
    unsafe extern "C" fn add_action_mapping(input: usize, mapping: usize) {
        let real: unsafe extern "C" fn(usize, usize) = unsafe { std::mem::transmute(ADD_ACTION_MAPPING.load(Ordering::Relaxed)) };
        log!("probe: action {} = {}", fname(mapping), fname(mapping + 0x10));
        unsafe { real(input, mapping) };
    }

    /// The FName at `at` (`FName::ToString` at 0x692ed0).
    fn fname(at: usize) -> String {
        name(at - 0x18)
    }

    static OFFLINE: AtomicUsize = AtomicUsize::new(0);
    static OFFLINE_LAST: AtomicUsize = AtomicUsize::new(usize::MAX);

    /// `UAJBNetworkObserver::IsOfflineMode` (0x4ed5d0): the flags it reads, when they change
    /// (the game instance from 0x49bca0: its observer at +0x3c8, a state at +0xc0).
    unsafe extern "C" fn is_offline_mode(ctx: usize) -> u8 {
        let real: unsafe extern "C" fn(usize) -> u8 = unsafe { std::mem::transmute(OFFLINE.load(Ordering::Relaxed)) };
        let r = unsafe { real(ctx) };
        let get_gi: unsafe extern "C" fn(usize) -> usize = unsafe { std::mem::transmute(BASE.load(Ordering::Relaxed) + 0x49bca0) };
        let gi = unsafe { get_gi(ctx) };
        if gi != 0 {
            let b = |p: usize| unsafe { *(p as *const u8) } as usize;
            let (obs, st, b8) = unsafe { (*((gi + 0x3c8) as *const usize), *((gi + 0xc0) as *const usize), *((gi + 0xb8) as *const usize)) };
            let v = [b(b8 + 0x14d), b(b8 + 0x150), b(obs + 0x1e0), b(obs + 0x3b), b(obs + 0x3c), b(st + 0xfa), b(st + 0xfb), b(st + 0xd4)];
            let fstr = |p: usize| -> String {
                let (data, num) = unsafe { (*(p as *const *const u16), *((p + 8) as *const i32)) };
                if data.is_null() || num <= 1 { String::new() } else { String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(data, num as usize - 1) }) }
            };
            let (session, user) = (fstr(gi + 0xd0), fstr(gi + 0xe0));
            let (sync_host, sync_port) = (fstr(gi + 0xd0 + 0x110), unsafe { *((gi + 0xd0 + 0x120) as *const i32) });
            let key = v.iter().fold(r as usize, |a, x| a << 4 | (x & 15)) ^ (sync_port as usize) << 40 ^ session.len() << 56;
            if OFFLINE_LAST.swap(key, Ordering::Relaxed) != key {
                log!("probe: offline {r}: b8+14d {} b8+150 {} maint {} obs+3b {} obs+3c {} st+fa {} st+fb {} st+d4 {} session {session:?} user {user:?} sync {sync_host}:{sync_port}", v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]);
            }
        }
        r
    }

    static SOCKET_CONNECT: AtomicUsize = AtomicUsize::new(0);

    /// `UNetSocket::Connect` (0x228830): the host and port (FStrings) the game connects to.
    unsafe extern "C" fn socket_connect(this: usize, ctx: usize, host: usize, port: usize, a: usize, b: usize) -> usize {
        let real: unsafe extern "C" fn(usize, usize, usize, usize, usize, usize) -> usize = unsafe { std::mem::transmute(SOCKET_CONNECT.load(Ordering::Relaxed)) };
        let fstr = |p: usize| -> String {
            if p == 0 { return "null".into() }
            let (data, num) = unsafe { (*(p as *const *const u16), *((p + 8) as *const i32)) };
            if data.is_null() || num <= 1 { String::new() } else { String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(data, num as usize - 1) }) }
        };
        log!("probe: socket connect {}:{}", fstr(host), fstr(port));
        unsafe { real(this, ctx, host, port, a, b) }
    }

    static CONNECT: AtomicUsize = AtomicUsize::new(0);

    /// `ws2_32!connect`: the address (IPv4) each socket connects to.
    unsafe extern "system" fn connect(s: usize, name: *const u8, len: i32) -> i32 {
        let real: unsafe extern "system" fn(usize, *const u8, i32) -> i32 = unsafe { std::mem::transmute(CONNECT.load(Ordering::Relaxed)) };
        if !name.is_null() && len >= 8 {
            let b = unsafe { std::slice::from_raw_parts(name, 8) };
            if b[0] == 2 {
                log!("probe: connect {}.{}.{}.{}:{}", b[4], b[5], b[6], b[7], u16::from_be_bytes([b[2], b[3]]));
            }
        }
        unsafe { real(s, name, len) }
    }

    pub(super) fn init(base: usize) {
        BASE.store(base, Ordering::Relaxed);
        if let Some(o) = unsafe { wal_payload_common::iat::hook_ordinal(base, "WS2_32.dll", 4, connect as *const () as usize) } {
            CONNECT.store(o, Ordering::Relaxed);
        }
        OFFLINE.store(detour(base, 0x4ed5d0, 15, is_offline_mode as *const () as usize), Ordering::Relaxed);
        SOCKET_CONNECT.store(detour(base, 0x228830, 15, socket_connect as *const () as usize), Ordering::Relaxed);
        ADD_ACTION_MAPPING.store(detour(base, 0x1793c60, 15, add_action_mapping as *const () as usize), Ordering::Relaxed);
        INVOKE.store(detour(base, 0x70dbf0, 15, invoke as *const () as usize), Ordering::Relaxed);
        STRCMP.store(detour(base, 0x1e09c0, 13, strcmp as *const () as usize), Ordering::Relaxed);
        SERIAL.store(detour(base, 0x233cc0, 15, serial_check as *const () as usize), Ordering::Relaxed);
        std::thread::spawn(move || {
            for _ in 0..30 {
                std::thread::sleep(std::time::Duration::from_secs(3));
                let code = unsafe { *((base + 0x3006290) as *const u16) };
                let flag = unsafe { *((base + 0x2d5c20c) as *const u8) };
                log!("probe: amw error code {code:#x}, card reader error flag {flag}");
            }
        });
        log!("probe: installed");
    }
}
