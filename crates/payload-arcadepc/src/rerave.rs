//! ReRave (Step Revolution's touch screen rhythm game, 2012: `ReRaveArcade.exe`, Unity 3.5,
//! Mono, 32-bit): `WAL_ARCADEPC_BOARD=rerave`, x86 build.
//!
//! - **RaveSvc.** The cabinet's service (`RaveSvc.exe`) counts the coins, keeps the operator's
//!   coin and network settings, the bookkeeping and the machine's IDs; the game talks to it
//!   through `RaveSvcClient.dll` (`ArcadeMgr`'s `DllImport`s, `RaveSvc_*`). The payload is that
//!   service, its functions resolved for Mono (`mono_imports`): `RaveSvc_Listen` polled each
//!   frame answers 3 after a coin (the game reads `RaveSvc_GetCredits`), 4 for the operator menu
//!   (the test button); the service button adds a service credit. Settings are kept for the run,
//!   the network disabled, no Wi-Fi.
//! - **Dongle.** `Sense4` (Assembly-CSharp-firstpass) checks the cabinet's SenseLock dongle
//!   (`sense4.dll`: code run in the dongle, challenge and answer) before nearly everything the
//!   game does (`ArcadeMgr.Awake` loops forever without it), and compares the credits there
//!   (`S4Compare`). The checks the game calls are replaced (`Mono::replace`) by functions
//!   returning true, `S4Compare` by the comparison itself, as soon as Mono has loaded the
//!   assembly (before the first scene's `Awake`).
//! - **Touch screen.** `GameInput` reads Unity's mouse (`Input.GetMouseButton(0)`,
//!   `Input.mousePosition`: one touch; the Windows build has no multi-touch): a touch screen's
//!   first finger (the profile's `input.touch`), else the gun (or mouse) is the cursor on the
//!   game's picture, the trigger its left button, as messages and raw
//!   reports (`payload-common`'s `input_filter`: the real keyboard and mouse never reach the
//!   game, Unity's `GetCursorPos` answered with the payload's cursor).
//! - **Free play** (the common `WAL_FREEPLAY`): the game's Event Mode (`ReRavePrefs.xml`'s
//!   `EventMode`, "EVENT MODE" on the screens), written at start; off: back to 0. (Its other free
//!   play, zero credits a play, divides by zero in `CreditsTextSelector`.)
//!
//! Natives: `touch`, `coin`, `service` (a service credit), `test` (the operator menu).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_mouse, post_raw_buttons};
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{desktop, gun, input, log, mono, mono_imports};
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryA};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetClientRect, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE};

/// `wParam`'s left button flag.
const MK_LBUTTON: usize = 0x0001;
/// Raw mouse button transitions.
const RI_MOUSE_LEFT_BUTTON_DOWN: u16 = 0x0001;
const RI_MOUSE_LEFT_BUTTON_UP: u16 = 0x0002;

/// `RaveSvc_Listen`'s events (`ArcadeMgr.Update`): the credits changed, the operator menu.
const CREDIT_INSERTED: i32 = 3;
const SERVICE_MENU: i32 = 4;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Touch,
    Coin,
    Service,
    Test,
}

const NATIVES: &[(&str, Native)] =
    &[("touch", Native::Touch), ("coin", Native::Coin), ("service", Native::Service), ("test", Native::Test)];
const DEFAULTS: &[(&str, &str)] =
    &[("b1", "touch"), ("start", "touch"), ("coin", "coin"), ("service", "service"), ("test", "test")];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The service's state: credits, events not listened to yet, settings, bookkeeping.
struct Service {
    credits: u32,
    events: Vec<i32>,
    credits_per_play: u32,
    credits_per_pulse: u32,
    credit_type: u32,
    menu_time: u32,
    gamma: u32,
    volume: u32,
    attract_volume: u32,
    language: u32,
    country: u32,
    connection: u32,
    network_disabled: bool,
    allow_coming_soon: bool,
    allow_mature: bool,
    event_mode: bool,
    use_dhcp: bool,
    total_credits: u32,
    service_credits: u32,
    songs: u32,
}

static SERVICE: Mutex<Service> = Mutex::new(Service {
    credits: 0,
    events: Vec::new(),
    // the dump's Preferences.xml (the service's settings file)
    credits_per_play: 1,
    credits_per_pulse: 1,
    credit_type: 1,
    menu_time: 1,
    gamma: 128,
    volume: 100,
    attract_volume: 50,
    language: 0,
    country: 0,
    connection: 0,
    network_disabled: true,
    allow_coming_soon: true,
    allow_mature: false,
    event_mode: false,
    use_dhcp: true,
    total_credits: 0,
    service_credits: 0,
    songs: 0,
});

fn service() -> std::sync::MutexGuard<'static, Service> {
    SERVICE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A credit inserted (a coin, or the service button's).
fn credit(service_credit: bool) {
    let mut s = service();
    s.credits += 1;
    s.total_credits += 1;
    if service_credit {
        s.service_credits += 1;
    }
    s.events.push(CREDIT_INSERTED);
}

// --- RaveSvcClient.dll (cdecl, Mono's DllImports) ---

extern "C" fn yes() -> i32 {
    1
}
extern "C" fn no() -> i32 {
    0
}
extern "C" fn nothing() {}
extern "C" fn listen() -> i32 {
    let mut s = service();
    if s.events.is_empty() { 0 } else { s.events.remove(0) }
}
extern "C" fn get_credits() -> i32 {
    service().credits as i32
}
extern "C" fn subtract_credits(count: u32) {
    let mut s = service();
    s.credits = s.credits.saturating_sub(count);
}
extern "C" fn credit_inserted_code() -> i32 {
    CREDIT_INSERTED
}
extern "C" fn service_menu_code() -> i32 {
    SERVICE_MENU
}

macro_rules! setting {
    ($get:ident, $set:ident, $field:ident) => {
        extern "C" fn $get() -> i32 {
            service().$field as i32
        }
        extern "C" fn $set(value: u32) {
            service().$field = value as _;
        }
    };
}
setting!(get_credits_per_play, set_credits_per_play, credits_per_play);
setting!(get_credits_per_pulse, set_credits_per_pulse, credits_per_pulse);
setting!(get_credit_type, set_credit_type, credit_type);
setting!(get_menu_time, set_menu_time, menu_time);
setting!(get_gamma, set_gamma, gamma);
setting!(get_volume, set_volume, volume);
setting!(get_attract_volume, set_attract_volume, attract_volume);
setting!(get_language, set_language, language);
setting!(get_country, set_country, country);
setting!(get_connection, set_connection, connection);

macro_rules! flag {
    ($get:ident, $set:ident, $field:ident) => {
        extern "C" fn $get() -> i32 {
            service().$field as i32
        }
        extern "C" fn $set(value: i32) -> i32 {
            service().$field = value & 0xFF != 0;
            1
        }
    };
}
flag!(get_network_disabled, set_network_disabled, network_disabled);
flag!(get_allow_coming_soon, set_allow_coming_soon, allow_coming_soon);
flag!(get_allow_mature, set_allow_mature, allow_mature);
flag!(get_event_mode, set_event_mode, event_mode);
flag!(get_use_dhcp, set_use_dhcp, use_dhcp);

extern "C" fn total_credits() -> i32 {
    service().total_credits as i32
}
extern "C" fn service_credits() -> i32 {
    service().service_credits as i32
}
extern "C" fn songs_played() -> i32 {
    service().songs as i32
}
extern "C" fn increment_songs() {
    service().songs += 1;
}
extern "C" fn reset_collection_stats() {
    let mut s = service();
    s.total_credits = 0;
    s.service_credits = 0;
    s.songs = 0;
}
extern "C" fn reset_coin_settings() {
    let mut s = service();
    s.credits_per_play = 1;
    s.credits_per_pulse = 1;
    s.credit_type = 1;
}

/// A string for Mono's marshalling, which frees it (`CoTaskMemFree`).
fn marshalled(text: &str) -> *mut u8 {
    type Alloc = unsafe extern "system" fn(usize) -> *mut u8;
    static ALLOC: OnceLock<usize> = OnceLock::new();
    let alloc = *ALLOC.get_or_init(|| unsafe {
        let ole32 = LoadLibraryA(c"ole32.dll".as_ptr().cast());
        GetProcAddress(ole32, c"CoTaskMemAlloc".as_ptr().cast()).map_or(0, |f| f as usize)
    });
    if alloc == 0 {
        return std::ptr::null_mut();
    }
    let alloc: Alloc = unsafe { std::mem::transmute(alloc) };
    let p = unsafe { alloc(text.len() + 1) };
    if !p.is_null() {
        unsafe {
            std::ptr::copy_nonoverlapping(text.as_ptr(), p, text.len());
            *p.add(text.len()) = 0;
        }
    }
    p
}

extern "C" fn serial_number() -> *mut u8 {
    marshalled("WAL00001")
}
extern "C" fn machine_id() -> *mut u8 {
    marshalled("WAL")
}
extern "C" fn ip_address() -> *mut u8 {
    marshalled("127.0.0.1")
}
extern "C" fn netmask() -> *mut u8 {
    marshalled("255.0.0.0")
}
extern "C" fn empty_string() -> *mut u8 {
    marshalled("")
}

// --- Sense4 (Assembly-CSharp-firstpass), replaced ---

/// `Sense4.S4Compare(a, b, type)`: `CompareType` EQ, NE, GT, LT, GE, LE.
extern "C" fn s4_compare(a: i32, b: i32, kind: i32) -> i32 {
    (match kind {
        0 => a == b,
        1 => a != b,
        2 => a > b,
        3 => a < b,
        4 => a >= b,
        _ => a <= b,
    }) as i32
}

/// The dongle checks the game calls (Assembly-CSharp's call sites), replaced by `true`.
const CHECKS: &[&str] = &[
    "Injection1",
    "Injection4",
    "Injection5",
    "Injection12",
    "Injection13",
    "Injection18",
    "Injection19",
    "Injection33",
    "NestedInjection0",
    "NestedInjection2",
    "NestedInjection6",
    "NestedInjection7",
    "NestedInjection11",
    "NestedInjection14",
    "NestedInjection16",
    "NestedInjection20",
    "NestedInjection37",
    "Exists",
];

/// Set once the dongle's checks are replaced.
static DONGLE_DONE: AtomicBool = AtomicBool::new(false);

/// The dongle's checks replaced once Mono has loaded Assembly-CSharp-firstpass.
fn dongle() {
    let m = loop {
        if let Some(m) = mono::mono()
            && !m.domain().is_null()
            && !m.image("Assembly-CSharp-firstpass").is_null()
        {
            break m;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    m.attach();
    let class = m.class(m.image("Assembly-CSharp-firstpass"), "", "Sense4");
    if class.is_null() {
        log!("rerave: no Sense4 class: the dongle's checks left");
        return;
    }
    let mut missing = Vec::new();
    for name in CHECKS {
        if !m.replace(m.method(class, name, -1), yes as *const () as usize) {
            missing.push(*name);
        }
    }
    if !m.replace(m.method(class, "S4Compare", 3), s4_compare as *const () as usize) {
        missing.push("S4Compare");
    }
    DONGLE_DONE.store(true, Ordering::Relaxed);
    log!("rerave: the dongle's checks replaced (not found: {missing:?})");
}

/// `sense4.dll`: reached only when a check ran before its replacement (too late).
extern "C" fn s4_error() -> u32 {
    let replaced = DONGLE_DONE.load(Ordering::Relaxed);
    log!("rerave: sense4.dll called (a dongle check not replaced{})", if replaced { "" } else { " in time" });
    // SAR_FAILED
    0x01
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    if module == "sense4.dll" && name.starts_with("S4") {
        return Some(s4_error as *const () as usize);
    }
    if module != "ravesvcclient.dll" {
        return None;
    }
    let f = match name {
        "RaveSvc_Connect" | "RaveSvc_IsConnected" | "RaveSvc_NetworkIsConnected" | "RaveSvc_ForceReboot" => {
            yes as *const () as usize
        }
        "RaveSvc_Listen" => listen as *const () as usize,
        "RaveSvc_GetCredits" => get_credits as *const () as usize,
        "RaveSvc_SubtractCredits" => subtract_credits as *const () as usize,
        "RaveSvc_GetCreditInsertedCode" => credit_inserted_code as *const () as usize,
        "RaveSvc_GetServiceMenuCode" => service_menu_code as *const () as usize,
        "RaveSvc_GetCreditsPerPlay" => get_credits_per_play as *const () as usize,
        "RaveSvc_SetCreditsPerPlay" => set_credits_per_play as *const () as usize,
        "RaveSvc_GetCreditsPerPulse" => get_credits_per_pulse as *const () as usize,
        "RaveSvc_SetCreditsPerPulse" => set_credits_per_pulse as *const () as usize,
        "RaveSvc_GetCreditType" => get_credit_type as *const () as usize,
        "RaveSvc_SetCreditType" => set_credit_type as *const () as usize,
        "RaveSvc_GetMenuTimeAllowed" => get_menu_time as *const () as usize,
        "RaveSvc_SetMenuTimeAllowed" => set_menu_time as *const () as usize,
        "RaveSvc_GetGamma" => get_gamma as *const () as usize,
        "RaveSvc_SetGamma" => set_gamma as *const () as usize,
        "RaveSvc_GetVolume" => get_volume as *const () as usize,
        "RaveSvc_SetVolume" => set_volume as *const () as usize,
        "RaveSvc_GetAttractVolume" => get_attract_volume as *const () as usize,
        "RaveSvc_SetAttractVolume" => set_attract_volume as *const () as usize,
        "RaveSvc_GetLanguage" => get_language as *const () as usize,
        "RaveSvc_SetLanguage" => set_language as *const () as usize,
        "RaveSvc_GetCountryCode" => get_country as *const () as usize,
        "RaveSvc_SetCountryCode" => set_country as *const () as usize,
        "RaveSvc_GetConnectionType" => get_connection as *const () as usize,
        "RaveSvc_SetConnectionType" => set_connection as *const () as usize,
        "RaveSvc_GetNetworkDisabled" => get_network_disabled as *const () as usize,
        "RaveSvc_SetNetworkDisabled" => set_network_disabled as *const () as usize,
        "RaveSvc_GetAllowComingSoon" => get_allow_coming_soon as *const () as usize,
        "RaveSvc_SetAllowComingSoon" => set_allow_coming_soon as *const () as usize,
        "RaveSvc_GetAllowMatureContent" => get_allow_mature as *const () as usize,
        "RaveSvc_SetAllowMatureContent" => set_allow_mature as *const () as usize,
        "RaveSvc_GetEventMode" => get_event_mode as *const () as usize,
        "RaveSvc_SetEventMode" => set_event_mode as *const () as usize,
        "RaveSvc_GetUseDHCP" => get_use_dhcp as *const () as usize,
        "RaveSvc_SetUseDHCP" => set_use_dhcp as *const () as usize,
        "RaveSvc_GetTotalCreditsInserted" | "RaveSvc_GetCreditsToday" | "RaveSvc_GetCreditsThisWeek"
        | "RaveSvc_GetCreditsThisMonth" | "RaveSvc_GetCreditsThisYear" | "RaveSvc_GetCreditsAllTime" => {
            total_credits as *const () as usize
        }
        "RaveSvc_GetServiceCreditsInserted" => service_credits as *const () as usize,
        "RaveSvc_GetSongsPlayedToday" | "RaveSvc_GetSongsPlayedThisWeek" | "RaveSvc_GetSongsPlayedThisMonth"
        | "RaveSvc_GetSongsPlayedThisYear" | "RaveSvc_GetSongsPlayedAllTime" => songs_played as *const () as usize,
        "RaveSvc_IncrementSongsPlayed" => increment_songs as *const () as usize,
        "RaveSvc_ResetCollectionStats" => reset_collection_stats as *const () as usize,
        "RaveSvc_ResetCoinSettings" => reset_coin_settings as *const () as usize,
        "RaveSvc_GetSerialNumber" => serial_number as *const () as usize,
        "RaveSvc_GetDriveID" | "RaveSvc_GetProductID" | "RaveSvc_GetBIOSID" | "RaveSvc_GetMotherboardID" => {
            machine_id as *const () as usize
        }
        "RaveSvc_GetIpAddress" | "RaveSvc_GetGateWay" | "RaveSvc_GetDnsServer" => ip_address as *const () as usize,
        "RaveSvc_GetNetmask" => netmask as *const () as usize,
        "RaveSvc_GetAuthToken" | "RaveSvc_WifiSSID" | "RaveSvc_SetIpAddress" | "RaveSvc_SetGateWay"
        | "RaveSvc_SetDnsServer" => empty_string as *const () as usize,
        // no Wi-Fi networks, none connected
        "RaveSvc_WifiListSize" | "RaveSvc_WifiQuality" | "RaveSvc_WifiCipher" | "RaveSvc_WifiAuth"
        | "RaveSvc_WifiIsConnected" | "RaveSvc_WifiConnect_1" | "RaveSvc_WifiConnectAdvanced"
        | "RaveSvc_GetWifiScanCompleteCode" => no as *const () as usize,
        // the rest (disconnect, reboot notice, crash, resets, clock, network settings): nothing
        _ if name.starts_with("RaveSvc_") => nothing as *const () as usize,
        _ => return None,
    };
    Some(f)
}

/// The left button pressed or released at client point `at`, as a message and a raw report.
fn button(hwnd: HWND, down: bool, at: (i32, i32)) {
    if down {
        post_mouse(hwnd, WM_LBUTTONDOWN, at.0, at.1, MK_LBUTTON);
        post_raw_buttons(hwnd, RI_MOUSE_LEFT_BUTTON_DOWN);
    } else {
        post_mouse(hwnd, WM_LBUTTONUP, at.0, at.1, 0);
        post_raw_buttons(hwnd, RI_MOUSE_LEFT_BUTTON_UP);
    }
}

fn input_thread() {
    let (mut touch_was, mut coin_was, mut service_was, mut test_was) = (false, false, false, false);
    let mut last = (i32::MIN, i32::MIN);
    let mut tick = 0u32;
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let (mut touch, mut coin, mut service_button, mut test) = (false, false, false, false);
        map().for_each_pressed(&input(0), |n| match n {
            Native::Touch => touch = true,
            Native::Coin => coin = true,
            Native::Service => service_button = true,
            Native::Test => test = true,
        });
        if coin && !coin_was {
            credit(false);
            log!("rerave: coin");
        }
        if service_button && !service_was {
            credit(true);
            log!("rerave: service credit");
        }
        if test && !test_was {
            service().events.push(SERVICE_MENU);
            log!("rerave: operator menu");
        }
        (coin_was, service_was, test_was) = (coin, service_button, test);
        let hwnd = gun::find_window();
        if hwnd.is_null() {
            continue;
        }
        // the touch: a touch screen's first finger (on the whole screen: the game's window), else
        // the gun's place on the window's picture (kept at its last place off it)
        // (in the game's pixels: its 1920x1080 desktop's, whatever the stretched window's size)
        let (w, h) = desktop::size().unwrap_or_else(|| {
            let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            unsafe { GetClientRect(hwnd, &mut r) };
            (r.right, r.bottom)
        });
        let finger = wal_payload_common::touches().first().map(|&(x, y)| (x as f32 / 65535.0, y as f32 / 65535.0));
        touch |= finger.is_some();
        let at = match finger.or_else(|| gun::gun_of_player_in_picture(0)) {
            Some((x, y)) => ((x * w as f32) as i32, (y * h as f32) as i32),
            None => last,
        };
        let buttons = if touch { MK_LBUTTON } else { 0 };
        // again every half second: a cursor that did not move still reaches a new screen
        tick = (tick + 1) % 64;
        if (at != last || tick == 0) && at.0 != i32::MIN {
            post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, buttons);
            last = at;
        }
        if touch != touch_was && last.0 != i32::MIN {
            button(hwnd, touch, last);
        }
        touch_was = touch;
    }
}

/// `ReRavePrefs.xml`'s `EventMode` (the game's free play) set from `WAL_FREEPLAY`.
fn free_play() {
    let Some(free) = wal_payload_common::free_play("WAL_RERAVE_FREEPLAY") else { return };
    let path = "ReRavePrefs.xml";
    let Ok(text) = std::fs::read_to_string(path) else {
        log!("rerave: no {path}: free play not set");
        return;
    };
    let key = "<key>EventMode</key>";
    let value = if free { "1" } else { "0" };
    let new = match text.find(key) {
        Some(at) => {
            let Some(start) = text[at..].find("<value>").map(|v| at + v + "<value>".len()) else { return };
            let Some(end) = text[start..].find("</value>").map(|v| start + v) else { return };
            format!("{}{value}{}", &text[..start], &text[end..])
        }
        None => match text.rfind("</data>") {
            Some(at) => format!("{}{key}\n<type>System.Int32</type>\n<value>{value}</value>\n\n{}", &text[..at], &text[at..]),
            None => return,
        },
    };
    // a file of the run directory's own, the link to the dump's removed first: the game saves
    // its preferences by renaming files, which moved the dump's one out of it through the link
    let _ = std::fs::remove_file(path);
    match std::fs::write(path, &new) {
        Ok(()) => log!("rerave: event mode (free play) {value}"),
        Err(e) => log!("rerave: {path} not written: {e}"),
    }
}

pub fn init() {
    free_play();
    mono_imports::hook(resolve);
    std::thread::spawn(dongle);
    // Unity 3.5: the player is the executable
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    wal_payload_common::input_filter::init(exe, |_| false);
    own_mouse(exe);
    std::thread::spawn(input_thread);
    // a red dot where the gun (or mouse) touches (the common crosshair option, `WAL_CROSSHAIR`)
    wal_payload_common::crosshair::init();
    log!("rerave: RaveSvc emulated, touches from the gun");
}
