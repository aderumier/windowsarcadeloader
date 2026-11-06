//! Sega ALLS APM3 payload ("ALL.Net P-ras MULTI Version 3": Under Night In-Birth Exe:Late[cl-r]).
//!
//! APM3 games import `apm_x86.dll` (`apm.dll` for 64-bit games), the client library of the
//! cabinet's APM3 system: it talks to `amdaemon` (keychip, ALL.Net authentication and
//! accounting, the Type 4 I/O board, the Aime reader, credits, backup), which the game dumps
//! do not include. This DLL is installed in its place and implements the API itself (all 106
//! exports, `apm_x86.def`): no daemon, no keychip, no I/O board.
//!
//! * inputs: `Input_*` take an APM3 switch id (the names in the original library): 0 test,
//!   1 service, 2 up, 3 down, 4 left, 5 right, 6 start, 7-14 push01-push08 (the 8-button Vewlix
//!   layout: push01-04 top row, push05-08 bottom row, as segatools' apm3io), 15 home (back to the
//!   APM3 system menu: never pressed by default). One player per cabinet: player 1's stick.
//!   `Input_isOnNow` / `isOffNow` / `isFlipNow` are the changes between the last two
//!   `Core_execute` calls (one per game frame).
//! * credits: the coins of the `coin` input (the game's coin hook called), 1 credit per game,
//!   free play by default (`WAL_APM3_FREEPLAY=0`: coins, 1 credit per game).
//! * network: `WAL_APM3_ONLINE=1`: ALL.Net and the Aime server reported up, the game's patches
//!   `WAL_APM3_ONLINE_PATCHES` (as `WAL_PATCHES`), Aime cards (`aime.rs`) with the players' data
//!   on the payload's title server (`apms.rs`, `cts.rs`). Otherwise offline (ALL.Net
//!   authentication failed: single play, no card). Cabinet versus needs the ABAAS matching
//!   server `WAL_APM3_ABAAS_LINK` (none emulated yet); without it the game's
//!   `WAL_APM3_NO_LINK_PATCHES` apply (Under Night's boot no longer waits 30 s for its link).
//! * test menu: the game handles the test switch itself and ends (`Core_exitGame`): the payload
//!   then asks the launcher to start the cabinet's test program (`WAL_APM3_TEST`, default
//!   `TestModeApp.exe`, the dump's `gametest.bat`), whose exit starts the game again.
//!
//! Options (profile `env`): `WAL_APM3_FREEPLAY`, `WAL_APM3_ONLINE`, `WAL_APM3_TEST` (and
//! `WAL_APM3_TEST_ARGS`: its arguments, a test mode of the game's exe),
//! `WAL_APM3_ACCESS_CODE`, `WAL_APM3_AIME_ID` (the card), `WAL_APM3_APMS_TRACE=1` (the title
//! server's AES key and plain bodies in the log),
//! `WAL_APM3_KEYCHIP` (keychip id), `WAL_APM3_BOARD` (main board id), `WAL_APM3_GAME_ID`,
//! `WAL_APM3_LOCATION` (shop name), `WAL_APM3_RESOLUTION` (`<w>x<h>`, default 1920x1080),
//! `WAL_PATCHES`, `WAL_MAP` (natives `test service up down left right start push01`..`push08`
//! `home coin`).

#![allow(non_snake_case)]

mod aime;
mod apms;
mod backup;
mod cts;
mod debug;
mod emoney;
mod gs;
mod link;
mod tl;
mod touch;

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use wal_payload_common::{log, mapping::ButtonMap, patches};
use windows_sys::Win32::Foundation::{HINSTANCE, TRUE};
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

/// Native inputs: the APM3 switch ids, plus the coin slot.
const COIN: u32 = 16;
const TOUCH: u32 = 17;

const NATIVES: &[(&str, u32)] = &[
    ("test", 0),
    ("service", 1),
    ("up", 2),
    ("down", 3),
    ("left", 4),
    ("right", 5),
    ("start", 6),
    ("push01", 7),
    ("push02", 8),
    ("push03", 9),
    ("push04", 10),
    ("push05", 11),
    ("push06", 12),
    ("push07", 13),
    ("push08", 14),
    ("home", 15),
    ("coin", COIN),
    // a touch screen's touch (touch.rs), not an APM3 switch
    ("touch", TOUCH),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("up", "up"),
    ("down", "down"),
    ("left", "left"),
    ("right", "right"),
    ("start", "start"),
    ("coin", "coin"),
    ("b1", "push01"),
    ("b2", "push02"),
    ("b3", "push03"),
    ("b4", "push04"),
    ("b5", "push05"),
    ("b6", "push06"),
    ("b7", "push07"),
    ("b8", "push08"),
];

static MAP: OnceLock<ButtonMap<u32>> = OnceLock::new();
/// Switch states (bit = APM3 id) at the last two `Core_execute` calls.
static CURRENT: AtomicU32 = AtomicU32::new(0);
static PREVIOUS: AtomicU32 = AtomicU32::new(0);
static CREDITS: AtomicU32 = AtomicU32::new(0);
static FREE_PLAY: AtomicBool = AtomicBool::new(false);
pub(crate) static ONLINE: AtomicBool = AtomicBool::new(false);
static COIN_HOOK: AtomicUsize = AtomicUsize::new(0);
static EXITING: AtomicBool = AtomicBool::new(false);
/// The test switch was pressed: the game's next exit goes to the test menu.
static TEST_REQUESTED: AtomicBool = AtomicBool::new(false);
static IS_TEST_PROGRAM: AtomicBool = AtomicBool::new(false);

pub(crate) fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

fn env_on(name: &str) -> bool {
    env(name).is_some_and(|v| v == "1")
}

/// Logs the first call of each function (what the game uses).
pub(crate) fn first_call(name: &'static str) {
    static SEEN: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap();
    if !seen.contains(&name) {
        seen.push(name);
        log!("apm3: {name} called");
    }
}

/// A NUL terminated UTF-16 copy of `s`, kept for the process (at least `min` characters: the
/// game copies fixed size id buffers).
pub(crate) fn wide(s: &str, min: usize) -> *const u16 {
    static CACHE: Mutex<Option<HashMap<String, &'static [u16]>>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap();
    let cache = cache.get_or_insert_with(HashMap::new);
    cache
        .entry(s.to_string())
        .or_insert_with(|| {
            let mut v: Vec<u16> = s.encode_utf16().collect();
            v.resize(v.len().max(min) + 1, 0);
            Box::leak(v.into_boxed_slice())
        })
        .as_ptr()
}

/// A zeroed buffer kept for the process (structures the game reads and we do not fill).
fn zeroed(name: &'static str) -> *mut c_void {
    static BUFFERS: Mutex<Vec<(&'static str, usize)>> = Mutex::new(Vec::new());
    let mut buffers = BUFFERS.lock().unwrap();
    if let Some((_, p)) = buffers.iter().find(|(n, _)| *n == name) {
        return *p as *mut c_void;
    }
    let p = Box::leak(vec![0u8; 0x1000].into_boxed_slice()).as_mut_ptr() as usize;
    buffers.push((name, p));
    p as *mut c_void
}

/// Player 1's `touch` input held (touch.rs).
pub(crate) fn touch_held() -> bool {
    read_switches() & bit(TOUCH) != 0
}

/// Player 1's switches now (bit = APM3 id).
fn read_switches() -> u32 {
    let stick = wal_payload_common::input(0);
    let mut bits = 0u32;
    MAP.get().unwrap().for_each_pressed(&stick, |id| bits |= 1 << id);
    bits
}

fn bit(id: u32) -> u32 {
    if id < 32 { 1 << id } else { 0 }
}

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(_module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> i32 {
    if reason == DLL_PROCESS_ATTACH {
        wal_payload_common::start("apm3");
        patches::apply();
        MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULT_MAP));
        // free play unless `WAL_APM3_FREEPLAY=0` / `WAL_FREEPLAY=0` (coins, 1 credit per game)
        FREE_PLAY.store(wal_payload_common::free_play("WAL_APM3_FREEPLAY").unwrap_or(true), Ordering::Relaxed);
        ONLINE.store(env_on("WAL_APM3_ONLINE"), Ordering::Relaxed);
        let test = env("WAL_APM3_TEST").unwrap_or_else(|| "TestModeApp.exe".into());
        // a test mode of the game's own program (`WAL_APM3_TEST_ARGS`, Senxin Aleste's `-t`):
        // the test program is that exe started with these arguments
        let test_args = env("WAL_APM3_TEST_ARGS").unwrap_or_default();
        let args: Vec<String> = std::env::args().skip(1).collect();
        // its file name (Goonya Fighter's is in a folder beside the game's: ..\gametest\...)
        let test_name = test.rsplit(['\\', '/']).next().unwrap_or(&test);
        let is_test = patches::exe_name().eq_ignore_ascii_case(test_name)
            && test_args.split_whitespace().all(|a| args.iter().any(|b| b.eq_ignore_ascii_case(a)));
        IS_TEST_PROGRAM.store(is_test, Ordering::Relaxed);
        // the game's patches, not the test program's
        if !IS_TEST_PROGRAM.load(Ordering::Relaxed) {
            emoney::start();
            link::start();
            if touch::enabled() {
                touch::start();
            }
            if env_on("WAL_APM3_TRACE_PINVOKE") {
                debug::trace_pinvoke();
            }
            debug::peek();
            let online = ONLINE.load(Ordering::Relaxed);
            if online {
                if let Some(spec) = env("WAL_APM3_ONLINE_PATCHES") {
                    patches::apply_list(&spec);
                }
                apms::init();
            }
            // no matching server: the game must not wait for it
            if !online || env("WAL_APM3_ABAAS_LINK").is_none() {
                if let Some(spec) = env("WAL_APM3_NO_LINK_PATCHES") {
                    patches::apply_list(&spec);
                }
            }
        }
        log!(
            "apm3: initialized ({}, free play {}, {})",
            if IS_TEST_PROGRAM.load(Ordering::Relaxed) { "test program" } else { "game" },
            FREE_PLAY.load(Ordering::Relaxed),
            if ONLINE.load(Ordering::Relaxed) { "online" } else { "offline" }
        );
    }
    TRUE
}

// ---- Core ----

/// Once per frame: the switches' snapshot, the coins.
#[unsafe(no_mangle)]
pub extern "system" fn Core_execute() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Core_execute");
        backup::tick();
        let now = read_switches();
        let before = CURRENT.swap(now, Ordering::Relaxed);
        PREVIOUS.store(before, Ordering::Relaxed);
        if now & !before & bit(COIN) != 0 {
            let credits = CREDITS.fetch_add(1, Ordering::Relaxed) + 1;
            log!("apm3: coin, {credits} credit(s)");
            let hook = COIN_HOOK.load(Ordering::Relaxed);
            if hook != 0 {
                let hook: extern "system" fn(u32) -> bool = unsafe { std::mem::transmute(hook) };
                hook(1);
            }
        }
        if now & !before & bit(0) != 0 {
            log!("apm3: test switch");
            TEST_REQUESTED.store(true, Ordering::Relaxed);
        }
        true
    })() as u32
}

/// The game ends (to the test menu, or the APM3 system menu). The cabinet's system then runs
/// the next program: the launcher does (relaunch).
#[unsafe(no_mangle)]
pub extern "system" fn Core_exitGame(flag: u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        log!("apm3: Core_exitGame({flag})");
        if EXITING.swap(true, Ordering::Relaxed) {
            return true;
        }
        backup::flush();
        if IS_TEST_PROGRAM.load(Ordering::Relaxed) {
            log!("apm3: test menu ends: the game again");
            wal_payload_common::request_relaunch("");
        } else if TEST_REQUESTED.load(Ordering::Relaxed) {
            let test = env("WAL_APM3_TEST").unwrap_or_else(|| "TestModeApp.exe".into());
            let args = env("WAL_APM3_TEST_ARGS").unwrap_or_default();
            log!("apm3: the test menu: {test} {args}");
            wal_payload_common::request_relaunch(format!("exe={test} {args}").trim_end());
        }
        // the game waits for the system to close it
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(3));
            log!("apm3: game still running after Core_exitGame: ending it");
            std::process::exit(0);
        });
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Core_isExitNeeded() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Core_isExitNeeded");
        EXITING.load(Ordering::Relaxed)
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Core_hookException() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Core_hookException");
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Core_resetCount() {
    first_call("Core_resetCount");
}

#[unsafe(no_mangle)]
pub extern "system" fn Core_resetException() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Core_resetException");
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Error_isOccurred() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Error_isOccurred");
        false
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Error_setDeviceLost() {
    log!("apm3: Error_setDeviceLost");
}

// ---- Input ----

#[unsafe(no_mangle)]
pub extern "system" fn Input_isOn(id: u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Input_isOn");
        static SEEN: AtomicU32 = AtomicU32::new(0);
        if id < 32 && SEEN.fetch_or(1 << id, Ordering::Relaxed) & (1 << id) == 0 {
            log!("apm3: Input_isOn({id}) queried");
        }
        CURRENT.load(Ordering::Relaxed) & bit(id) != 0
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Input_isOnNow(id: u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Input_isOnNow");
        CURRENT.load(Ordering::Relaxed) & !PREVIOUS.load(Ordering::Relaxed) & bit(id) != 0
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Input_isOffNow(id: u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Input_isOffNow");
        !CURRENT.load(Ordering::Relaxed) & PREVIOUS.load(Ordering::Relaxed) & bit(id) != 0
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Input_isFlipNow(id: u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Input_isFlipNow");
        (CURRENT.load(Ordering::Relaxed) ^ PREVIOUS.load(Ordering::Relaxed)) & bit(id) != 0
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Input_getGamepadInfo() -> *mut c_void {
    first_call("Input_getGamepadInfo");
    zeroed("gamepad info")
}

#[unsafe(no_mangle)]
pub extern "system" fn Input_isGamepadDetect() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Input_isGamepadDetect");
        false
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Input_isOpenPewviewWindow() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Input_isOpenPewviewWindow");
        false
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Input_setGamepadConfig(_config: *const c_void) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Input_setGamepadConfig");
        true
    })() as u32
}

// The cabinet's gamepad support (a pad in place of the I/O board): unused, the inputs are
// the `Input_*` switches.

#[unsafe(no_mangle)]
pub extern "system" fn apmGamepadConfig(_a: u32, _b: u32, _c: u32, _d: u32, _e: u32) -> u32 {
    first_call("apmGamepadConfig");
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn apmGamepadGetApmInput(_out: *mut c_void) -> u32 {
    first_call("apmGamepadGetApmInput");
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn apmGamepadGetConfig() -> *mut c_void {
    first_call("apmGamepadGetConfig");
    zeroed("gamepad config")
}

#[unsafe(no_mangle)]
pub extern "system" fn apmGamepadGetDeviceCaps(_out: *mut c_void) -> u32 {
    first_call("apmGamepadGetDeviceCaps");
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn apmGamepadGetInfo(_out: *mut c_void) -> u32 {
    first_call("apmGamepadGetInfo");
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn apmGamepadGetRawInput(_out: *mut c_void) -> u32 {
    first_call("apmGamepadGetRawInput");
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn apmGamepadInitConfig() -> u32 {
    first_call("apmGamepadInitConfig");
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn apmGamepadIsDetected() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("apmGamepadIsDetected");
        false
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn apmGamepadIsXInputDevice() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("apmGamepadIsXInputDevice");
        false
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn apmGamepadUpdate() -> u32 {
    first_call("apmGamepadUpdate");
    0
}

// ---- Credit ----

fn credits() -> u32 {
    CREDITS.load(Ordering::Relaxed)
}

fn free_play() -> bool {
    FREE_PLAY.load(Ordering::Relaxed)
}

/// Takes `cost` credits (nothing in free play); false when there are not enough.
fn pay(cost: u32) -> bool {
    if free_play() {
        return true;
    }
    let paid = CREDITS.try_update(Ordering::Relaxed, Ordering::Relaxed, |c| c.checked_sub(cost)).is_ok();
    log!("apm3: pay {cost} credit(s): {}, {} left", if paid { "ok" } else { "not enough" }, credits());
    paid
}

#[unsafe(no_mangle)]
pub extern "system" fn Credit_getAddableCoin() -> u32 {
    first_call("Credit_getAddableCoin");
    99
}

#[unsafe(no_mangle)]
pub extern "system" fn Credit_getCoinMultipliers() -> *mut c_void {
    first_call("Credit_getCoinMultipliers");
    zeroed("coin multipliers")
}

#[unsafe(no_mangle)]
pub extern "system" fn Credit_getCoinToCredit() -> u32 {
    first_call("Credit_getCoinToCredit");
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn Credit_getCredit() -> u32 {
    first_call("Credit_getCredit");
    credits()
}

/// Coins inserted toward the next credit.
#[unsafe(no_mangle)]
pub extern "system" fn Credit_getRemain() -> u32 {
    first_call("Credit_getRemain");
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn Credit_isFreePlay() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Credit_isFreePlay");
        free_play()
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Credit_isGameCostEnough(cost: u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Credit_isGameCostEnough");
        free_play() || credits() >= cost
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Credit_isZero() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Credit_isZero");
        !free_play() && credits() == 0
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Credit_payGameCost(cost: u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Credit_payGameCost");
        pay(cost)
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Credit_setCoinInHook(hook: usize) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        log!("apm3: coin hook {hook:#x}");
        COIN_HOOK.store(hook, Ordering::Relaxed);
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Credit_resetCoinInHook() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Credit_resetCoinInHook");
        COIN_HOOK.store(0, Ordering::Relaxed);
        true
    })() as u32
}

/// The credits, as text: the game shows `CREDIT(S) <text>` (`FREE PLAY` itself).
#[unsafe(no_mangle)]
pub extern "system" fn Credit_toString() -> *const u16 {
    first_call("Credit_toString");
    wide(&credits().to_string(), 0)
}

// ---- Sequence: the plays (accounting, credits) ----

#[unsafe(no_mangle)]
pub extern "system" fn Sequence_beginPlay(a: u32, b: u32, c: u32, d: u32, e: u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        log!("apm3: Sequence_beginPlay({a:#x}, {b:#x}, {c:#x}, {d:#x}, {e:#x})");
        pay(1);
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Sequence_continuePlay(a: u32, b: u32, c: u32, d: u32, e: u32, f: u32, g: u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        log!("apm3: Sequence_continuePlay({a:#x}, {b:#x}, {c:#x}, {d:#x}, {e:#x}, {f:#x}, {g:#x})");
        pay(1);
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Sequence_endPlay(a: u32, b: u32, c: u32, d: u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        log!("apm3: Sequence_endPlay({a:#x}, {b:#x}, {c:#x}, {d:#x})");
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Sequence_clearBackup() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        log!("apm3: Sequence_clearBackup");
        true
    })() as u32
}

/// The bookkeeping counters of the test menu: zero.
#[unsafe(no_mangle)]
pub extern "system" fn Sequence_getBookkeeping() -> *mut c_void {
    first_call("Sequence_getBookkeeping");
    zeroed("bookkeeping")
}

#[unsafe(no_mangle)]
pub extern "system" fn Sequence_getPlayingAimeId() -> *mut c_void {
    first_call("Sequence_getPlayingAimeId");
    zeroed("playing aime id")
}

#[unsafe(no_mangle)]
pub extern "system" fn Sequence_isAccountingPlaying() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Sequence_isAccountingPlaying");
        false
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Sequence_isPlaying() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Sequence_isPlaying");
        false
    })() as u32
}

/// The cabinet is in its test mode (the system's, not the game's own test switch handling).
#[unsafe(no_mangle)]
pub extern "system" fn Sequence_isTest() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Sequence_isTest");
        // the game started as its own test mode (`WAL_APM3_TEST_ARGS`: Puyo Puyo e-Sports,
        // Koihime Enbu run the same program for both)
        IS_TEST_PROGRAM.load(Ordering::Relaxed) && env("WAL_APM3_TEST_ARGS").is_some()
    })() as u32
}

// ---- System ----

/// The game's root folder (`W:\` on the cabinet): its executable's folder.
#[unsafe(no_mangle)]
pub extern "system" fn System_getAppRootPath() -> *const u16 {
    first_call("System_getAppRootPath");
    static ROOT: OnceLock<String> = OnceLock::new();
    let root = ROOT.get_or_init(|| {
        let mut buf = [0u16; 520];
        let n = unsafe {
            windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32)
        } as usize;
        let path = String::from_utf16_lossy(&buf[..n]);
        let dir = path.rsplit_once('\\').map_or(path.as_str(), |(d, _)| d);
        log!("apm3: app root {dir}\\");
        format!("{dir}\\")
    });
    wide(root, 0)
}

/// An id structure: two UTF-16 strings, the first at 0 (17 characters), the second at byte 0x22
/// (the test menus print `[%s] [%s]`; Blade Strangers' ABAAS client takes the second, an
/// 11-character id).
fn two_ids(first: &str, second: &str) -> *const u16 {
    static CACHE: Mutex<Option<HashMap<String, &'static [u16]>>> = Mutex::new(None);
    let key = format!("{first}|{second}");
    let mut cache = CACHE.lock().unwrap();
    cache
        .get_or_insert_with(HashMap::new)
        .entry(key)
        .or_insert_with(|| {
            let mut v = vec![0u16; 17 + 24];
            for (i, c) in first.encode_utf16().take(16).enumerate() {
                v[i] = c;
            }
            for (i, c) in second.encode_utf16().take(23).enumerate() {
                v[17 + i] = c;
            }
            Box::leak(v.into_boxed_slice())
        })
        .as_ptr()
}

/// The main board's ids (`WAL_APM3_BOARD`).
#[unsafe(no_mangle)]
pub extern "system" fn System_getBoardId() -> *const u16 {
    first_call("System_getBoardId");
    let board = env("WAL_APM3_BOARD").unwrap_or_else(|| "ACAE01A99999999".into());
    two_ids(&board, &board.replace('-', "").chars().take(11).collect::<String>())
}

/// The keychip's ids (`WAL_APM3_KEYCHIP`, 16 characters `XXXX-XXXXXXXXXXX`): the id, then its
/// 11-character form (the dash removed, the first 11: `A69E01A8888`).
#[unsafe(no_mangle)]
pub extern "system" fn System_getKeychipId() -> *const u16 {
    first_call("System_getKeychipId");
    let keychip = env("WAL_APM3_KEYCHIP").unwrap_or_else(|| "A69E-01A88888888".into());
    two_ids(&keychip, &keychip.replace('-', "").chars().take(11).collect::<String>())
}

#[unsafe(no_mangle)]
pub extern "system" fn System_getGameId() -> *const u16 {
    first_call("System_getGameId");
    wide(&env("WAL_APM3_GAME_ID").unwrap_or_else(|| "SDEM".into()), 16)
}

/// `{major, minor}`.
#[unsafe(no_mangle)]
pub extern "system" fn System_getGameVersion() -> *const u32 {
    first_call("System_getGameVersion");
    static VERSION: [u32; 4] = [1, 0, 0, 0];
    VERSION.as_ptr()
}

#[unsafe(no_mangle)]
pub extern "system" fn System_getRegionCode() -> u32 {
    first_call("System_getRegionCode");
    1
}

/// `{width, height}`.
#[unsafe(no_mangle)]
pub extern "system" fn System_getResolution() -> *const u32 {
    first_call("System_getResolution");
    static RESOLUTION: OnceLock<[u32; 4]> = OnceLock::new();
    RESOLUTION
        .get_or_init(|| {
            let (w, h) = env("WAL_APM3_RESOLUTION")
                .and_then(|v| v.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))))
                .unwrap_or((1920, 1080));
            [w, h, 0, 0]
        })
        .as_ptr()
}

#[unsafe(no_mangle)]
pub extern "system" fn System_isDevelop() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("System_isDevelop");
        false
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn NetworkProperty_getAddressString() -> *const u16 {
    first_call("NetworkProperty_getAddressString");
    wide("192.168.168.11", 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn NetworkProperty_getAddressValue() -> u32 {
    first_call("NetworkProperty_getAddressValue");
    u32::from_be_bytes([192, 168, 168, 11])
}

// ---- ALL.Net ----

#[unsafe(no_mangle)]
pub extern "system" fn AllnetAuth_isGood() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("AllnetAuth_isGood");
        ONLINE.load(Ordering::Relaxed)
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn AllnetAuth_isDevelop() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("AllnetAuth_isDevelop");
        false
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn AllnetAuth_getAbaasGsServerName() -> *const u16 {
    first_call("AllnetAuth_getAbaasGsServerName");
    // online: the payload's game server (gs.rs): a host name (the client adds port 80, moved to
    // the title server's port by apms.rs' connect hook)
    if ONLINE.load(Ordering::Relaxed) {
        wide(&env("WAL_APM3_ABAAS_GS").unwrap_or_else(|| "127.0.0.1".into()), 0)
    } else {
        wide("", 0)
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn AllnetAuth_getAbaasLinkServerName() -> *const u16 {
    first_call("AllnetAuth_getAbaasLinkServerName");
    wide(&env("WAL_APM3_ABAAS_LINK").unwrap_or_default(), 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn AllnetAuth_getCountryCode() -> *const u16 {
    first_call("AllnetAuth_getCountryCode");
    wide("JPN", 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn AllnetAuth_getLocationId() -> u32 {
    first_call("AllnetAuth_getLocationId");
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn AllnetAuth_getLocationName() -> *const u16 {
    first_call("AllnetAuth_getLocationName");
    wide(&env("WAL_APM3_LOCATION").unwrap_or_else(|| "WINDOWS ARCADE LOADER".into()), 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn AllnetAuth_getLocationNickname(_index: u32) -> *const u16 {
    first_call("AllnetAuth_getLocationNickname");
    wide("", 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn AllnetAuth_getLocationNicknamePartCount() -> u32 {
    first_call("AllnetAuth_getLocationNicknamePartCount");
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn AllnetAuth_getRegionCode() -> u32 {
    first_call("AllnetAuth_getRegionCode");
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn AllnetAuth_getRegionName(_index: u32) -> *const u16 {
    first_call("AllnetAuth_getRegionName");
    wide("", 0)
}

#[unsafe(no_mangle)]
pub extern "system" fn AllnetAuth_getRegionNamePartCount() -> u32 {
    first_call("AllnetAuth_getRegionNamePartCount");
    1
}

#[unsafe(no_mangle)]
pub extern "system" fn AllnetAccounting_beginPlay(a: u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        log!("apm3: AllnetAccounting_beginPlay({a:#x})");
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn AllnetAccounting_endPlay(a: u32, b: u32, c: u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        log!("apm3: AllnetAccounting_endPlay({a:#x}, {b:#x}, {c:#x})");
        true
    })() as u32
}

// ---- system settings (the APM3 system's test menu) ----

#[unsafe(no_mangle)]
pub extern "system" fn ApmSystemSetting_getAdvertizeSound(out: *mut bool) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("ApmSystemSetting_getAdvertizeSound");
        if !out.is_null() {
            unsafe { *out = true };
        }
        true
    })() as u32
}

/// Store closing times: none (32 bytes the game keeps).
#[unsafe(no_mangle)]
pub extern "system" fn ApmSystemSetting_getClosingTimes(out: *mut u8) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("ApmSystemSetting_getClosingTimes");
        if !out.is_null() {
            unsafe { std::ptr::write_bytes(out, 0, 0x20) };
        }
        false
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn ApmSystemSetting_getEmoneySetting(_out: *mut c_void) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("ApmSystemSetting_getEmoneySetting");
        false
    })() as u32
}

/// The game returns to its title screen only (no home switch back to the system menu): off.
#[unsafe(no_mangle)]
pub extern "system" fn ApmSystemSetting_getFixedTitle(out: *mut bool) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("ApmSystemSetting_getFixedTitle");
        if !out.is_null() {
            unsafe { *out = false };
        }
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn ApmSystemSetting_getGamePadSetting(_out: *mut c_void) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("ApmSystemSetting_getGamePadSetting");
        false
    })() as u32
}

/// The store's matching group: a letter (the test menu's `GROUP: A`; Blade Strangers indexes
/// a picture with it: 0 crashes its title), `WAL_APM3_MATCHING_GROUP` (default `A`).
#[unsafe(no_mangle)]
pub extern "system" fn ApmSystemSetting_getMatchingGroup(out: *mut u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("ApmSystemSetting_getMatchingGroup");
        let group = env("WAL_APM3_MATCHING_GROUP").and_then(|g| g.bytes().next()).filter(u8::is_ascii_uppercase).unwrap_or(b'A');
        if !out.is_null() {
            unsafe { *out = group as u32 };
        }
        true
    })() as u32
}

/// Seconds to the store's closing time: -1, none.
#[unsafe(no_mangle)]
pub extern "system" fn ApmSystemSetting_getTimeToClosingTime(out: *mut i32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("ApmSystemSetting_getTimeToClosingTime");
        if !out.is_null() {
            unsafe { *out = -1 };
        }
        false
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn ApmSystemSetting_getUiSetting(_out: *mut c_void) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("ApmSystemSetting_getUiSetting");
        false
    })() as u32
}

// ---- Backup: backup.rs ----

// ---- Aime card reader: aime.rs ----

// ---- e-money: none ----

#[unsafe(no_mangle)]
pub extern "system" fn Emoney_close() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Emoney_close");
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Emoney_isOpenMainWindow() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Emoney_isOpenMainWindow");
        false
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Emoney_launch() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Emoney_launch");
        false
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Emoney_setSoundHook(_hook: usize) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Emoney_setSoundHook");
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Emoney_update(_a: u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Emoney_update");
        true
    })() as u32
}
