//! The cabinet's clock (segatools' `platform/clock.c`, public domain): Japan's time zone
//! (UTC+9, no daylight saving; Sega's games expect it), and the clock never set (amdaemon sets
//! it to ALL.Net's time: Wine refuses, an RTC error), its privileges granted. In amdaemon and
//! the game.
//!
//! `WAL_ALLS_TIMEWARP=1` (segatools' `timewarp`): the cabinets' nightly maintenance hours, Japan
//! 02:00-07:00 (CHUNITHM PLUS closes before midnight), never seen: each day's 24 hours pass as
//! Japan's 07:00 to 23:00. Initial D THE ARCADE's attract stays black during them (a closed
//! cabinet waiting for its reboot), CHUNITHM's says play is over for the day.

use wal_payload_common::iat;

/// SYSTEMTIME: year, month, day of week, day, hour, minute, second, milliseconds.
type SystemTime = [u16; 8];

unsafe extern "system" {
    fn GetSystemTimeAsFileTime(out: *mut u64);
    fn FileTimeToSystemTime(time: *const u64, out: *mut SystemTime) -> i32;
    fn SetLastError(code: u32);
}

const HOUR: u64 = 36_000_000_000;
const DAY: u64 = 24 * HOUR;

static TIMEWARP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The current UTC time (FILETIME units), the maintenance hours skipped with the time warp:
/// the day biased 2 hours forward (Japan's 23:00-07:00 = UTC 14:00-22:00 becomes its last eight
/// hours), its time scaled by 16/24, the bias removed.
fn now() -> u64 {
    let mut now = 0u64;
    unsafe { GetSystemTimeAsFileTime(&mut now) };
    if !TIMEWARP.load(std::sync::atomic::Ordering::Relaxed) {
        return now;
    }
    let biased = now + 2 * HOUR;
    let (day, time) = (biased / DAY, biased % DAY);
    day * DAY + time * 16 / 24 - 2 * HOUR
}

/// The cabinet's current time in Unix seconds (time-warped too: ALL.Net's and the game
/// server's times agree with the game's clock).
pub(crate) fn unix_now() -> u64 {
    (now() / 10_000_000).saturating_sub(11_644_473_600)
}

unsafe extern "system" fn get_system_time_as_file_time(out: *mut u64) {
    if !out.is_null() {
        unsafe { *out = now() };
    }
}

unsafe extern "system" fn get_system_time(out: *mut SystemTime) {
    let now = now();
    unsafe { FileTimeToSystemTime(&now, out) };
}

unsafe extern "system" fn get_local_time(out: *mut SystemTime) {
    let now = now() + 9 * HOUR;
    unsafe { FileTimeToSystemTime(&now, out) };
}

unsafe extern "system" fn get_time_zone_information(info: *mut u8) -> u32 {
    if info.is_null() {
        return u32::MAX;
    }
    // TIME_ZONE_INFORMATION (172 bytes): only the bias (minutes west of UTC)
    unsafe {
        std::ptr::write_bytes(info, 0, 172);
        (info as *mut i32).write_unaligned(-9 * 60);
    }
    0 // TIME_ZONE_ID_UNKNOWN: no daylight saving
}

unsafe extern "system" fn set_time(_time: *const u8) -> i32 {
    1
}

static LOOKUP_PRIVILEGE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The privileges a time setter asks (amLib's `amRtcSetTimezoneInformation`: Wine has no
/// `SeTimeZonePrivilege`, error 1313, then CHUNITHM's error 8114): a privilege Wine does not
/// know gets Windows' value, `SE_TIME_ZONE_PRIVILEGE` (34).
unsafe extern "system" fn lookup_privilege_value_a(system: *const u8, name: *const u8, luid: *mut u32) -> i32 {
    let real: unsafe extern "system" fn(*const u8, *const u8, *mut u32) -> i32 = unsafe { std::mem::transmute(LOOKUP_PRIVILEGE.load(std::sync::atomic::Ordering::Relaxed)) };
    if unsafe { real(system, name, luid) } != 0 || luid.is_null() {
        return 1;
    }
    unsafe {
        *luid = 34;
        *luid.add(1) = 0;
        SetLastError(0);
    }
    1
}

/// The time privileges granted (the clock setters do nothing).
unsafe extern "system" fn adjust_token_privileges(_token: usize, _disable: i32, _new: *const u8, _len: u32, _prev: *mut u8, prev_len: *mut u32) -> i32 {
    unsafe {
        if !prev_len.is_null() {
            *prev_len = 0;
        }
        SetLastError(0);
    }
    1
}

/// The C runtimes whose clock the games read too (CHUNITHM's `_time64`, `localtime`: else its
/// RTC check sees the real time against ALL.Net's warped one, error 8114).
const RUNTIMES: [&str; 4] = ["msvcr100.dll", "msvcr110.dll", "msvcr120.dll", "ucrtbase.dll"];

pub(crate) fn init() {
    let warp = std::env::var("WAL_ALLS_TIMEWARP").is_ok_and(|v| v.trim() == "1");
    TIMEWARP.store(warp, std::sync::atomic::Ordering::Relaxed);
    let mut modules = vec![iat::game_module()];
    for name in RUNTIMES {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(wide.as_ptr()) } as usize;
        if base != 0 {
            modules.push(base);
        }
    }
    for base in modules {
        hook_module(base);
    }
    unsafe {
        if let Some(orig) = iat::hook("advapi32.dll", "LookupPrivilegeValueA", lookup_privilege_value_a as *const () as usize) {
            LOOKUP_PRIVILEGE.store(orig, std::sync::atomic::Ordering::Relaxed);
            iat::hook("advapi32.dll", "AdjustTokenPrivileges", adjust_token_privileges as *const () as usize);
        }
    }
}

/// The clock's hooks in the module at `base` (a runtime the game loads later: Chrono Regalia's
/// Mono, `mono.dll`, reading the time and time zone itself).
pub(crate) fn hook_module(base: usize) {
    let warp = TIMEWARP.load(std::sync::atomic::Ordering::Relaxed);
    unsafe {
        if warp {
            iat::hook_module(base, "kernel32.dll", "GetSystemTimeAsFileTime", get_system_time_as_file_time as *const () as usize);
            iat::hook_module(base, "kernel32.dll", "GetSystemTimePreciseAsFileTime", get_system_time_as_file_time as *const () as usize);
            iat::hook_module(base, "kernel32.dll", "GetSystemTime", get_system_time as *const () as usize);
        }
        iat::hook_module(base, "kernel32.dll", "GetLocalTime", get_local_time as *const () as usize);
        iat::hook_module(base, "kernel32.dll", "GetTimeZoneInformation", get_time_zone_information as *const () as usize);
        for name in ["SetLocalTime", "SetSystemTime", "SetTimeZoneInformation"] {
            iat::hook_module(base, "kernel32.dll", name, set_time as *const () as usize);
        }
    }
}
