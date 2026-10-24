//! Rainbow Bomber Girl's BM-System library (`bm_system.dll`): the cabinet's launcher runs the
//! game with its module handle (`-hp`) and the game binds its system interface (`SystemImport`)
//! to the library's exports by name. Without it the game takes stand-ins (`DefaultSystemImport_*`):
//! no credits (free play), no test or service switch, a watchdog that quits the game after a
//! minute. The payload exports three of the library's modules and binds them (`bbg_io.rs`):
//!
//! * System: every boot check passed, the watchdog locked (its feed function is called once,
//!   its lock function at once, its unlock never: the game never times out), the region
//!   (`IsAsia`: `WAL_KONAMI_REGION=asia`, the export version's texts and pictures).
//! * Io: test, service and the coin switch (`IoGetBtnPress`).
//! * Consume: the credits. A coin adds one, a game or a gacha takes its price
//!   (`WAL_KONAMI_PRICE` credits per game, default 1; a gacha 1), PASELI is not offered;
//!   `WAL_KONAMI_FREEPLAY=1`: free play.
//!
//! The other modules (card entry, e-Amusement coin, network, logs, test mode settings) keep the
//! game's stand-ins: guest play.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use wal_payload_common::log;

type P = *mut c_void;

/// The payload's module handle: the game finds the exports there.
static MODULE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn set_module(module: usize) {
    MODULE.store(module, Ordering::Relaxed);
}

pub fn module() -> usize {
    MODULE.load(Ordering::Relaxed)
}

static CREDITS: AtomicI32 = AtomicI32::new(0);
static GAME_OVER_RESERVATION: AtomicBool = AtomicBool::new(false);
/// The switches IoGetBtnPress reports (bbg_io.rs): test, service, coin.
pub static TEST: AtomicBool = AtomicBool::new(false);
pub static SERVICE: AtomicBool = AtomicBool::new(false);
pub static COIN: AtomicBool = AtomicBool::new(false);

/// CONSUME_MODE
const START_CREDIT: i32 = 1;
const START_PASELI: i32 = 2;
const GACHA_CREDIT: i32 = 3;
const GACHA_PASELI: i32 = 4;
const START_COMPENSATION: i32 = 5;
/// PRODUCT_TYPE
const GAME_START: i32 = 1;
const ADD_CONSUME: i32 = 2;
const GAME_START_COMPENSATION: i32 = 3;
/// CONSUME_TYPE
const CREDIT: i32 = 1;
const PASELI: i32 = 2;
const COMPENSATION: i32 = 3;

struct Settings {
    free_play: bool,
    price: i32,
    asia: bool,
}

fn settings() -> &'static Settings {
    static S: OnceLock<Settings> = OnceLock::new();
    S.get_or_init(|| {
        let env = |name: &str| std::env::var(name).ok().map(|v| v.trim().to_ascii_lowercase());
        let s = Settings {
            free_play: env("WAL_KONAMI_FREEPLAY").is_some_and(|v| v == "1"),
            price: env("WAL_KONAMI_PRICE").and_then(|v| v.parse().ok()).filter(|p| (1..=9).contains(p)).unwrap_or(1),
            asia: env("WAL_KONAMI_REGION").is_some_and(|v| v == "asia"),
        };
        log!("bbg: {}, {} region", if s.free_play { "free play".to_string() } else { format!("{} credit(s) a game", s.price) }, if s.asia { "Asia" } else { "Japan" });
        s
    })
}

/// A coin inserted: one credit.
pub fn coin_inserted() {
    let n = CREDITS.fetch_add(1, Ordering::Relaxed) + 1;
    log!("bbg: coin, {n} credit(s)");
}

fn price(mode: i32) -> i32 {
    match mode {
        _ if settings().free_play => 0,
        START_CREDIT => settings().price,
        GACHA_CREDIT => 1,
        _ => 0,
    }
}

fn is_credit(mode: i32) -> bool {
    mode == START_CREDIT || mode == GACHA_CREDIT
}

fn confirm(mode: i32) -> bool {
    is_credit(mode) && (settings().free_play || CREDITS.load(Ordering::Relaxed) >= price(mode))
}

/// The modes of a product (credits only).
fn product_modes(product: i32) -> &'static [i32] {
    match product {
        GAME_START => &[START_CREDIT],
        ADD_CONSUME => &[GACHA_CREDIT],
        _ => &[],
    }
}

fn type_modes(consume_type: i32) -> &'static [i32] {
    match consume_type {
        CREDIT => &[START_CREDIT, GACHA_CREDIT],
        _ => &[],
    }
}

// ---- System ----

#[unsafe(no_mangle)]
pub extern "C" fn SystemSetGameSetting(_setting: P) {}

#[unsafe(no_mangle)]
pub extern "C" fn SystemGetOnlineUpdateState(_exists: P, _progress: P, _online: P) {}

#[unsafe(no_mangle)]
pub extern "C" fn SystemSetUnityState(_state: i32, _sub: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn SystemGetSystemState(_state: P, _sub: P) {}

#[unsafe(no_mangle)]
pub extern "C" fn SystemSetUnityError(error: i32, sub: i32) {
    log!("bbg: game error {error} ({sub})");
}

#[unsafe(no_mangle)]
pub extern "C" fn SystemGetSystemError(_error: P, _sub: P) {}

/// SystemLibraryInfo: seven ANSI strings of 32 characters (left empty).
#[unsafe(no_mangle)]
pub extern "C" fn SystemGetSystemLibraryInfo(_info: P) {
    settings();
}

#[unsafe(no_mangle)]
pub extern "C" fn SystemGetSystemGetShareUtl(_urls: P) {}

/// CLOSE_PHASE.NORMAL
#[unsafe(no_mangle)]
pub extern "C" fn SystemGetSystemShopClose() -> i32 {
    0
}

/// No shop maintenance.
#[unsafe(no_mangle)]
pub extern "C" fn SystemGetSystemShopMainte(_phase: P, _start: P, _end: P) -> i32 {
    0
}

/// EA3_STATUS_ONLINE, as the game's stand-in.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn SystemGetSystemEa3Status(status: *mut i32, code: *mut i32, sub: *mut i32) {
    for (p, v) in [(status, 3), (code, 0), (sub, 0)] {
        if !p.is_null() {
            unsafe { *p = v };
        }
    }
}

/// EAG_STAT_OK
#[unsafe(no_mangle)]
pub extern "C" fn SystemGetSystemEagState() -> i32 {
    0
}

/// EA3_PP_STATUS_AVAILABLE
#[unsafe(no_mangle)]
pub extern "C" fn SystemGetSystemPPState() -> i32 {
    3
}

#[unsafe(no_mangle)]
pub extern "C" fn SystemSetCardUseable(_useable: i32) {}

/// DateTime: the server's, local and local offset times (DateElement: time_sec u64, year u32,
/// weekday, month, day, hour, minute, sec; 24 bytes), all the current UTC time as the game's
/// stand-in.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn SystemGetNowTime(time: *mut u8) {
    if time.is_null() {
        return;
    }
    let mut st = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetSystemTime(&mut st) };
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut e = [0u8; 24];
    e[0..8].copy_from_slice(&secs.to_le_bytes());
    e[8..12].copy_from_slice(&(st.wYear as u32).to_le_bytes());
    e[12] = st.wDayOfWeek as u8;
    e[13] = st.wMonth as u8;
    e[14] = st.wDay as u8;
    e[15] = st.wHour as u8;
    e[16] = st.wMinute as u8;
    e[17] = st.wSecond as u8;
    for i in 0..3 {
        unsafe { std::ptr::copy_nonoverlapping(e.as_ptr(), time.add(24 * i), 24) };
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn SystemCpuUsageStart() {}

#[unsafe(no_mangle)]
pub extern "C" fn SystemGetCpuUsage(_usage: P) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn SystemCpuUsageEnd() {}

/// DeviceBootState: system, I/O, security, backup data, IC card, network all PASS (4);
/// BackUpDataIntiDisp false.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn SystemGetDeviceBootState(state: *mut i32) {
    if state.is_null() {
        return;
    }
    for i in 0..6 {
        unsafe { *state.add(i) = 4 };
    }
    unsafe { *(state.add(6) as *mut u8) = 0 };
}

#[unsafe(no_mangle)]
pub extern "C" fn SystemFullScreen() {}

#[unsafe(no_mangle)]
pub extern "C" fn SystemKeepAlive(_value: i32) {}

/// The watchdog's feed (`ExeptionChecker.WatchDog`): called once, as the game's stand-in does.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn SystemSetWatchDogFunc(func: Option<unsafe extern "system" fn()>) {
    if let Some(f) = func {
        unsafe { f() };
    }
}

/// The watchdog's lock: called at once, the watchdog stops counting.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn SystemSetWatchDogLock(func: Option<unsafe extern "system" fn()>) {
    if let Some(f) = func {
        unsafe { f() };
        log!("bbg: watchdog locked");
    }
}

/// The watchdog's unlock: never called (the game's stand-in calls it at once: the game quits
/// a minute later).
#[unsafe(no_mangle)]
pub extern "C" fn SystemSetWatchDogUnLock(_func: P) {}

#[unsafe(no_mangle)]
pub extern "C" fn SystemSceneStamp(_scene: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn SystemIsPossibleGameStart() -> i32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn SystemSceneAttach(_scene: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn SystemGetDataSize(_kind: i32) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn IsAsia() -> i32 {
    settings().asia as i32
}

// ---- Io ----

/// IoBtn: testBtnPress, serviceBtnPress, coinIn, coinBlockerIsOpen.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn IoGetBtnPress(btn: *mut u8) {
    if btn.is_null() {
        return;
    }
    let b = [TEST.load(Ordering::Relaxed) as u8, SERVICE.load(Ordering::Relaxed) as u8, COIN.load(Ordering::Relaxed) as u8, 1];
    unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), btn, b.len()) };
}

/// No card reader.
#[unsafe(no_mangle)]
pub extern "C" fn IoGetIcCardReaderState(_state: P) {}

/// The game's keyboard test / service keys (its development input), ignored.
#[unsafe(no_mangle)]
pub extern "C" fn IoSetKeyPress(_code: u64) {}

// ---- Consume ----

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetConsumeInfoNum(product: i32) -> i32 {
    product_modes(product).len() as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetConsumeInfoMode(product: i32, idx: i32) -> i32 {
    product_modes(product).get(idx as usize).copied().unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetProductNum(consume_type: i32) -> i32 {
    type_modes(consume_type).len() as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetProductConsumeMode(consume_type: i32, idx: i32) -> i32 {
    type_modes(consume_type).get(idx as usize).copied().unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeIsFreePlay() -> i32 {
    settings().free_play as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetCoinInSoundPlayNum() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeSetCoinInSoundPlayCall() {}

/// GAME_MODE: START 1, GACHA 2.
#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetConsumeModeToGameMode(mode: i32) -> i32 {
    match mode {
        START_CREDIT | START_PASELI | START_COMPENSATION => 1,
        GACHA_CREDIT | GACHA_PASELI => 2,
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetConsumeModeToConsumeType(mode: i32) -> i32 {
    match mode {
        START_CREDIT | GACHA_CREDIT => CREDIT,
        START_PASELI | GACHA_PASELI => PASELI,
        START_COMPENSATION => COMPENSATION,
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetConsumeModeToProductType(mode: i32) -> i32 {
    match mode {
        START_CREDIT | START_PASELI => GAME_START,
        GACHA_CREDIT | GACHA_PASELI => ADD_CONSUME,
        START_COMPENSATION => GAME_START_COMPENSATION,
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetDefaultStartConsumeMode() -> i32 {
    START_CREDIT
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeIsDefaultStartConfirm() -> i32 {
    confirm(START_CREDIT) as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetDefaultStartShortageNum() -> i32 {
    ConsumeGetShortageCoinNum(START_CREDIT)
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetPrice(mode: i32) -> i32 {
    price(mode)
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetPriceCredit(mode: i32) -> i32 {
    price(mode)
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetShortageCoinNum(mode: i32) -> i32 {
    (price(mode) - CREDITS.load(Ordering::Relaxed)).max(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetConfirm(mode: i32) -> i32 {
    confirm(mode) as i32
}

/// Coins toward the next credit: none (a coin is a credit).
#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetFractionCoin() -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetCreditNum() -> i32 {
    CREDITS.load(Ordering::Relaxed)
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetExtractCoinNum() -> i32 {
    1
}

/// E_INQUIRE_REQUEST_STATUS.SKIP: no balance to ask a server for.
#[unsafe(no_mangle)]
pub extern "C" fn ConsumeInquireUpdate_Request(_mode: i32, _continue: u8) -> i32 {
    0
}

/// XRPCSTATUS.OK
#[unsafe(no_mangle)]
pub extern "C" fn ConsumeInquireUpdate_Status() -> i32 {
    4
}

/// E_CONSUME_REQUEST_STATUS: SUCCESS (the price taken), ERROR_NO_ENOUGH_CREDIT,
/// ERROR_NOT_AVAILABLE_CONSUME_TYPE (PASELI).
#[unsafe(no_mangle)]
pub extern "C" fn ConsumeConsume_Request(mode: i32, _continue: u8) -> i32 {
    if !is_credit(mode) {
        return 4;
    }
    let p = price(mode);
    let taken = CREDITS.try_update(Ordering::Relaxed, Ordering::Relaxed, |c| (c >= p).then_some(c - p));
    match taken {
        Ok(c) => {
            if mode == START_CREDIT {
                GAME_OVER_RESERVATION.store(false, Ordering::Relaxed);
            }
            log!("bbg: {} paid, {} credit(s) left", if mode == START_CREDIT { "game" } else { "gacha" }, c - p);
            0
        }
        Err(_) => 3,
    }
}

/// E_EAG_EACOIN_STATUS.SUCCESSED
#[unsafe(no_mangle)]
pub extern "C" fn ConsumeConsume_Status(_mode: i32, _continue: u8) -> i32 {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeConsumePcbEvent_Send(_mode: i32, _continue: u8) {}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeSetConsumeMode(_mode: i32) {}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeSetGameOverReservation() {
    GAME_OVER_RESERVATION.store(true, Ordering::Relaxed);
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeGetGameOverReservation() -> i32 {
    GAME_OVER_RESERVATION.load(Ordering::Relaxed) as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeIsConsumeProcessing() -> i32 {
    0
}

/// Service credits left over by an interrupted game: none.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ConsumeGetCreditErrorInfo(a: *mut i32, b: *mut i32, c: *mut i32, d: *mut i32, e: *mut i32) {
    for p in [a, b, c, d, e] {
        if !p.is_null() {
            unsafe { *p = 0 };
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ConsumeEndAddService(_mode: i32) {}
