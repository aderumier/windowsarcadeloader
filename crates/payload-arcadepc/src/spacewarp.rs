//! Space Warp 66 (Touchmagix; Unity 2019.3, Mono, 64-bit): `WAL_ARCADEPC_BOARD=spacewarp`. A
//! ticket game played with one spinner, a Linux cabinet's game in a Windows build.
//!
//! - **The Linux calls.** `SerialConnection` lists its ports with `/usr/bin/find`, `GameManager`
//!   reads the disk's serial with `/bin/udevadm`: Mono's `CreateProcessW` import answers them
//!   ([`create_process`]). `find` gives the two ports; `udevadm` nothing, as `/dev/disk/by-id/` (an
//!   empty folder of `C:`) has no disk: the two empty serials agree, the licence's key is then
//!   always [`LOCAL_KEY`].
//! - **The paths.** `/touchmagix/config/` (the settings, the profile's `c_folders`) and
//!   `/dev/disk/by-id/` are on the current drive: `C:` (the launcher's run directory).
//! - **The settings** (`Config.xml`): made here with the game's own defaults, the licence's key
//!   and the common `WAL_FREEPLAY` written at each start. The dump's `StreamingAssets/Config.xml`
//!   is an old one the game does not read.
//! - **The boards** (Mono's `SerialPort`, lines at 115200 bauds, ended by its Windows `NewLine`,
//!   `\r\n`). `/dev/ttyUSB0`, the credits
//!   board: `G` answers the game's `I` (which tells the ports apart), `play` is a coin; the tickets
//!   it asks to pay are dropped. `/dev/ttyUSB1`, the spinner: each line is the degrees turned since
//!   the last one. In free play any line starts a game from the attract screen (the `start` button
//!   sends a null turn); with coins a coin does. A cabinet's Linux port throws on a read timeout;
//!   Mono's Windows `SerialPort` would give an empty line, a line all the same: [`read_file`].
//! - **The picture.** The game renders with OpenGL: its default framebuffer is the window's, a
//!   1080x1920 fake desktop would be clipped. `Screen.SetResolution(1080, 1920, true)` is fitted on
//!   the screen's height (`bullseye::replace_set_resolution`), in the profile's 9:16 window. That
//!   window is not managed (the profile's registry) and never gets the focus: it is told it has it
//!   (Unity ignores the keys posted otherwise).
//! - **The spinner.** The game takes one line per frame into a static string and its `FixedUpdate`
//!   empties it: a frame without a physics step would overwrite a turn. So the next line waits
//!   until the last one is consumed ([`line_consumed`]), the motion adding up. The mouse (`mouse_wrap` on
//!   `rx`) turns it `WAL_SPACEWARP_MOUSE` degrees per count (default 0.3), the left stick and
//!   `left` / `right` at up to `WAL_SPACEWARP_SPEED` degrees per second (default 270).
//!
//! Natives: `coin start left right up down menu ok`; the arrows and `menu` (`C`), `ok` (Enter) are
//! the keys of the game's operator menu, posted (`input_filter`: the real ones never reach it).

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use wal_payload_common::input_filter::post_key;
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::mono::mono;
use wal_payload_common::{gun, iat, input, log, mono_imports, serial};
use wal_protocol::{Axis, StickState};
use windows_sys::Win32::Foundation::{ERROR_IO_PENDING, HANDLE, HMODULE, SetLastError};
use windows_sys::Win32::Storage::FileSystem::WriteFile;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::{CreateEventW, GetCurrentProcessId, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW};
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

const CREDITS_PORT: &str = "/dev/ttyUSB0";
const GAME_PORT: &str = "/dev/ttyUSB1";
/// `find`'s answer: the credits board first.
const PORTS: &str = "/dev/ttyUSB0\n/dev/ttyUSB1\n";

const CONFIG_DIR: &str = r"C:\touchmagix\config";
const DISKS_DIR: &str = r"C:\dev\disk\by-id";
/// The licence's key without a disk serial: MD5(MD5(key1) + key2), `GameManager.checkforLicence`.
const LOCAL_KEY: &str = "66c43ccaa9bc28ef668cac779b4cea2b";
/// `GameManager.createConfigXML`'s settings.
const DEFAULT_CONFIG: &[(&str, &str)] = &[
    ("GamePort", "USB0"),
    ("CreditPort", "USB1"),
    ("AverageTicketsPayout", "30"),
    ("AverageScorePercent", "70"),
    ("MinimumTickets", "1"),
    ("PerTicketValue", "1"),
    ("CreditsPerGame", "1"),
    ("FreePlayMode", "NO"),
    ("LocalKey", LOCAL_KEY),
    ("AttractSound", "4"),
    ("RetryHelper", "YES"),
    ("Difficulty", "MEDIUM"),
    ("TicketDensity", "MEDIUM"),
    ("CustomizedDifficulty", "NO"),
    ("MinJackPotValue", "500"),
    ("MaxJackPotValue", "1000"),
    ("ProgressiveJackPotValue", "10"),
    ("CurrentJackPotValue", "500"),
    ("TargetScore", "66"),
    ("SpinDirection", "Straight"),
    ("MoveSpeedAtL1", "20"),
    ("MoveSpeedAtL2", "30"),
    ("MoveSpeedAtL3", "42"),
    ("MoveSpeedAtL4", "51"),
    ("MoveSpeedAtL5", "58"),
    ("MoveSpeedAtL6", "65"),
    ("MoveSpeedAtL7", "70"),
    ("RotateSpeedAtL1", "0"),
    ("RotateSpeedAtL2", "15"),
    ("RotateSpeedAtL3", "30"),
    ("RotateSpeedAtL4", "45"),
    ("RotateSpeedAtL5", "50"),
    ("RotateSpeedAtL6", "70"),
    ("RotateSpeedAtL7", "80"),
];

/// The operator menu's virtual keys: left, right, up, down, `C`, Enter. A held key is its bit.
const KEYS: [u16; 6] = [0x25, 0x27, 0x26, 0x28, 0x43, 0x0D];
const LEFT: u8 = 0;
const RIGHT: u8 = 1;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Coin,
    Start,
    /// A key of the operator menu: its index in [`KEYS`].
    Key(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("coin", Native::Coin),
    ("start", Native::Start),
    ("left", Native::Key(LEFT)),
    ("right", Native::Key(RIGHT)),
    ("up", Native::Key(2)),
    ("down", Native::Key(3)),
    ("menu", Native::Key(4)),
    ("ok", Native::Key(5)),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("coin", "coin"),
    ("start", "start"),
    ("left", "left"),
    ("right", "right"),
    ("up", "up"),
    ("down", "down"),
    ("test", "menu"),
    ("service", "ok"),
    ("b1", "ok"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

// The settings

/// `xml` with the tag's value, the tag added at the end if it is missing.
fn with_tag(xml: &str, tag: &str, value: &str) -> String {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    if let Some(start) = xml.find(&open).map(|i| i + open.len())
        && let Some(end) = xml[start..].find(&close).map(|i| i + start)
    {
        return format!("{}{value}{}", &xml[..start], &xml[end..]);
    }
    match xml.rfind("</ConfigXML>") {
        Some(end) => format!("{}  {open}{value}{close}\n{}", &xml[..end], &xml[end..]),
        None => xml.to_string(),
    }
}

fn default_config() -> String {
    let tags: String = DEFAULT_CONFIG.iter().map(|(tag, value)| format!("  <{tag}>{value}</{tag}>\n")).collect();
    format!("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<ConfigXML>\n{tags}</ConfigXML>\n")
}

/// The folders the game reads, its settings ready for its first read.
fn prepare_files() {
    for dir in [CONFIG_DIR, DISKS_DIR] {
        if let Err(e) = std::fs::create_dir_all(dir) {
            log!("spacewarp: {dir}: {e}");
        }
    }
    let path = format!(r"{CONFIG_DIR}\Config.xml");
    let mut xml = std::fs::read_to_string(&path).unwrap_or_else(|_| default_config());
    xml = with_tag(&xml, "LocalKey", LOCAL_KEY);
    if let Some(on) = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") {
        xml = with_tag(&xml, "FreePlayMode", if on { "YES" } else { "NO" });
    }
    for file in [path, format!(r"{CONFIG_DIR}\Config_Backup.xml")] {
        if let Err(e) = std::fs::write(&file, &xml) {
            log!("spacewarp: {file}: {e}");
        }
    }
}

// The Linux commands

/// Mono's `CreateProcessW`.
static ORIG_CREATE_PROCESS: AtomicUsize = AtomicUsize::new(0);

/// What a command of the game prints, `None` for any other program.
fn command_output(command: &str) -> Option<&'static str> {
    if command.contains("/usr/bin/find") {
        Some(PORTS)
    } else if command.contains("/bin/udevadm") {
        Some("")
    } else {
        None
    }
}

fn wide_string(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    while unsafe { *p.add(len) } != 0 {
        len += 1;
    }
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

/// The game's commands answered on the standard output Mono gave, their process already over
/// (signaled handles); the others started.
unsafe extern "system" fn create_process(
    app: *const u16,
    cmd: *mut u16,
    process_attr: *const c_void,
    thread_attr: *const c_void,
    inherit: i32,
    flags: u32,
    env: *const c_void,
    dir: *const u16,
    startup: *const STARTUPINFOW,
    info: *mut PROCESS_INFORMATION,
) -> i32 {
    let command = format!("{} {}", wide_string(app), wide_string(cmd));
    if let Some(output) = command_output(&command) {
        log!("spacewarp: {command}: answered");
        unsafe {
            if (*startup).dwFlags & STARTF_USESTDHANDLES != 0 {
                let mut written = 0;
                WriteFile((*startup).hStdOutput, output.as_ptr(), output.len() as u32, &mut written, std::ptr::null_mut());
            }
            let signaled = || -> HANDLE { CreateEventW(std::ptr::null(), 1, 1, std::ptr::null()) };
            (*info).hProcess = signaled();
            (*info).hThread = signaled();
            (*info).dwProcessId = GetCurrentProcessId();
            (*info).dwThreadId = 0;
        }
        return 1;
    }
    type CreateProcessW = unsafe extern "system" fn(*const u16, *mut u16, *const c_void, *const c_void, i32, u32, *const c_void, *const u16, *const STARTUPINFOW, *mut PROCESS_INFORMATION) -> i32;
    let orig: CreateProcessW = unsafe { std::mem::transmute(ORIG_CREATE_PROCESS.load(Ordering::Relaxed)) };
    unsafe { orig(app, cmd, process_attr, thread_attr, inherit, flags, env, dir, startup, info) }
}

/// Hooks Mono's `CreateProcessW` once Unity has loaded the runtime.
fn hook_commands() {
    loop {
        let module: HMODULE = unsafe { GetModuleHandleW(windows_sys::w!("mono-2.0-bdwgc.dll")) };
        if !module.is_null() {
            match unsafe { iat::hook_module(module as usize, "kernel32.dll", "CreateProcessW", create_process as *const () as usize) } {
                Some(orig) => ORIG_CREATE_PROCESS.store(orig, Ordering::Relaxed),
                None => log!("spacewarp: Mono does not import CreateProcessW"),
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

// The boards

/// The coins not sent yet.
static COINS: AtomicU32 = AtomicU32::new(0);
/// The `start` presses not sent yet.
static STARTS: AtomicU32 = AtomicU32::new(0);
/// The degrees turned and not sent yet.
static TURN: Mutex<f32> = Mutex::new(0.0);
/// The address of the game's `SerialConnection.gameDataReceived`.
static LINE_FIELD: AtomicUsize = AtomicUsize::new(0);

/// Whether the game's `FixedUpdate` took the last line (`gameDataReceived` empty). Called on the
/// game's thread, which reads the port.
fn line_consumed() -> bool {
    let mut field = LINE_FIELD.load(Ordering::Relaxed);
    if field == 0 {
        let Some(m) = mono() else { return true };
        let class = m.class(m.image("Assembly-CSharp"), "", "SerialConnection");
        field = m.static_addr(class, m.field(class, "gameDataReceived")) as usize;
        if field == 0 {
            return true;
        }
        LINE_FIELD.store(field, Ordering::Relaxed);
    }
    let string = unsafe { *(field as *const usize) };
    // a MonoString: object header, then its length
    string == 0 || unsafe { *((string + 2 * size_of::<usize>()) as *const i32) } == 0
}

/// A spinner line, `None` under what the game would show.
fn turn_line(degrees: f32) -> Option<String> {
    (degrees.abs() >= 0.01).then(|| format!("{degrees:.3}\r\n"))
}

/// The spinner board's next line, asked when the game polls the port.
fn game_idle() -> Vec<u8> {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    // On the attract screen the game does not empty the string: a line is sent all the same
    // after a while (free play: it starts a game).
    if !line_consumed() && last.is_some_and(|t| t.elapsed() < Duration::from_millis(100)) {
        return Vec::new();
    }
    let line = {
        let mut turn = TURN.lock().unwrap();
        let line = turn_line(*turn);
        if line.is_some() {
            *turn = 0.0;
        }
        line
    };
    let line = line.or_else(|| (STARTS.swap(0, Ordering::Relaxed) > 0).then(|| "0.000\r\n".to_string()));
    match line {
        Some(line) => {
            *last = Some(Instant::now());
            line.into_bytes()
        }
        None => Vec::new(),
    }
}

/// The game writes `ver` to the spinner board: nothing to answer.
fn game_handler(_packet: &[u8]) -> Vec<u8> {
    Vec::new()
}

/// The credits board's next line: a coin.
fn credits_idle() -> Vec<u8> {
    let coins = COINS.load(Ordering::Relaxed);
    if coins > 0 && COINS.compare_exchange(coins, coins - 1, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
        return b"play\r\n".to_vec();
    }
    Vec::new()
}

/// The credits board: the port's identification answered (tickets, resets, `ver`: dropped).
fn credits_handler(packet: &[u8]) -> Vec<u8> {
    if packet == b"I" { b"G\r\n".to_vec() } else { Vec::new() }
}

/// `ReadFile`: nothing to read is a pending read, already over with no byte (`GetOverlappedResult`):
/// Mono's `TimeoutException`, which the game's reading coroutines catch.
unsafe extern "system" fn read_file(h: *mut c_void, buf: *mut u8, n: u32, read: *mut u32, ov: *mut c_void) -> i32 {
    type ReadFile = unsafe extern "system" fn(*mut c_void, *mut u8, u32, *mut u32, *mut c_void) -> i32;
    let Some(f) = serial::replacement("ReadFile") else { return 0 };
    let f: ReadFile = unsafe { std::mem::transmute(f) };
    let done = unsafe { f(h, buf, n, read, ov) };
    if done != 0 && !ov.is_null() && !read.is_null() && unsafe { *read } == 0 && serial::port_of(h).is_some() {
        unsafe { SetLastError(ERROR_IO_PENDING) };
        return 0;
    }
    done
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    match (module, name) {
        ("kernel32.dll", "ReadFile") => Some(read_file as *const () as usize),
        ("kernel32.dll", _) => serial::replacement(name),
        _ => None,
    }
}

// The controls

/// The keys held ([`KEYS`]' bits), for the game's key polls.
static HELD: AtomicU32 = AtomicU32::new(0);

pub fn key_held(vk: i32) -> bool {
    KEYS.iter().position(|&k| k as i32 == vk).is_some_and(|i| HELD.load(Ordering::Relaxed) >> i & 1 != 0)
}

fn env_speed(name: &str, default: f32) -> f32 {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

/// The degrees of a counter's motion (a wrapping `i16`, the mouse's trackball).
fn counter_motion(now: i16, was: i16, degrees_per_count: f32) -> f32 {
    now.wrapping_sub(was) as f32 * degrees_per_count
}

/// The tilt the stick and the keys turn the spinner at, -1 (left) .. 1 (right); a dead zone around
/// the stick's center.
fn tilt(stick: &StickState, keys: u32) -> f32 {
    let digital = (keys >> RIGHT & 1) as f32 - (keys >> LEFT & 1) as f32;
    if digital != 0.0 {
        return digital;
    }
    let axis = stick.axis(Axis::LeftX) as f32 / 32767.0;
    const DEAD_ZONE: f32 = 0.15;
    if axis.abs() < DEAD_ZONE { 0.0 } else { (axis - DEAD_ZONE.copysign(axis)) / (1.0 - DEAD_ZONE) }
}

/// The game's window told it is active and has the focus.
fn activate(hwnd: *mut c_void) {
    const WM_ACTIVATE: u32 = 0x0006;
    const WM_SETFOCUS: u32 = 0x0007;
    const WM_ACTIVATEAPP: u32 = 0x001C;
    unsafe {
        PostMessageW(hwnd, WM_ACTIVATEAPP, 1, 0);
        PostMessageW(hwnd, WM_ACTIVATE, 1, 0);
        PostMessageW(hwnd, WM_SETFOCUS, 0, 0);
    }
}

fn input_thread() {
    let degrees_per_count = env_speed("WAL_SPACEWARP_MOUSE", 0.3);
    let degrees_per_second = env_speed("WAL_SPACEWARP_SPEED", 270.0);
    let (mut keys_was, mut coin_was, mut start_was) = (0u32, false, false);
    let mut mouse_was = input(0).axis(Axis::RightX);
    let mut last = Instant::now();
    let mut last_activation = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(4));
        let hwnd = gun::find_window();
        // repeated: Unity ignores it until its window procedure is ready, and does not run its
        // first frame without it
        if !hwnd.is_null() && last_activation.elapsed() >= Duration::from_millis(500) {
            activate(hwnd);
            last_activation = Instant::now();
        }
        let elapsed = last.elapsed().as_secs_f32();
        last = Instant::now();
        let stick = input(0);
        let (mut keys, mut coin, mut start) = (0u32, false, false);
        map().for_each_pressed(&stick, |n| match n {
            Native::Coin => coin = true,
            Native::Start => start = true,
            Native::Key(key) => keys |= 1 << key,
        });
        if coin && !coin_was {
            COINS.fetch_add(1, Ordering::Relaxed);
        }
        if start && !start_was {
            STARTS.fetch_add(1, Ordering::Relaxed);
        }
        (coin_was, start_was) = (coin, start);
        // the game's line turns the ball left when positive: to the right is negative
        let mouse = stick.axis(Axis::RightX);
        let right = counter_motion(mouse, mouse_was, degrees_per_count) + tilt(&stick, keys) * degrees_per_second * elapsed;
        mouse_was = mouse;
        *TURN.lock().unwrap() -= right;
        HELD.store(keys, Ordering::Relaxed);
        let changed = keys ^ keys_was;
        if changed != 0 {
            if !hwnd.is_null() {
                for (i, &vk) in KEYS.iter().enumerate().filter(|(i, _)| changed >> i & 1 != 0) {
                    post_key(hwnd, vk, keys >> i & 1 != 0);
                }
                keys_was = keys;
            }
        }
    }
}

pub fn init() {
    std::thread::spawn(crate::bullseye::replace_set_resolution);
    prepare_files();
    serial::install_streaming_in(CREDITS_PORT, credits_handler, credits_idle, &[]);
    serial::install_streaming_in(GAME_PORT, game_handler, game_idle, &[]);
    mono_imports::hook(resolve);
    std::thread::spawn(hook_commands);
    std::thread::spawn(input_thread);
    log!("spacewarp: credits board on {CREDITS_PORT}, spinner on {GAME_PORT}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags() {
        let xml = "<ConfigXML>\n  <LocalKey>InValid</LocalKey>\n</ConfigXML>\n";
        assert!(with_tag(xml, "LocalKey", "k").contains("<LocalKey>k</LocalKey>"));
        let added = with_tag(xml, "FreePlayMode", "YES");
        assert!(added.contains("  <FreePlayMode>YES</FreePlayMode>\n</ConfigXML>"));
        assert!(default_config().contains(&format!("<LocalKey>{LOCAL_KEY}</LocalKey>")));
    }

    #[test]
    fn commands() {
        assert_eq!(command_output("/usr/bin/find /dev -name ttyUSB*"), Some(PORTS));
        assert_eq!(command_output("/bin/udevadm info --query=all --name=/dev/sda"), Some(""));
        assert_eq!(command_output("C:\\game.exe"), None);
    }

    #[test]
    fn spinner() {
        assert_eq!(counter_motion(-32768, 32767, 1.0), 1.0);
        assert_eq!(counter_motion(32767, -32768, 1.0), -1.0);
        assert_eq!(turn_line(-1.5).as_deref(), Some("-1.500\r\n"));
        assert_eq!(turn_line(0.001), None);
    }

    #[test]
    fn credits() {
        assert_eq!(credits_handler(b"I"), b"G\r\n");
        assert!(credits_handler(b"30").is_empty());
    }
}
