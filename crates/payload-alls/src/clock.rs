//! The cabinet's clock (segatools' `platform/clock.c`, public domain): Japan's time zone
//! (UTC+9, no daylight saving; Sega's games expect it), and the clock never set (amdaemon sets
//! it to ALL.Net's time: Wine refuses, an RTC error). In amdaemon and the game.
//!
//! `WAL_ALLS_TIMEWARP=1` (segatools' `timewarp`): the cabinets' nightly maintenance hours, Japan
//! 02:00-07:00, never seen: each day's 24 hours pass as its 19 others. Initial D THE ARCADE's
//! attract stays black during them (a closed cabinet waiting for its reboot).

use wal_payload_common::iat;

/// SYSTEMTIME: year, month, day of week, day, hour, minute, second, milliseconds.
type SystemTime = [u16; 8];

unsafe extern "system" {
    fn GetSystemTimeAsFileTime(out: *mut u64);
    fn FileTimeToSystemTime(time: *const u64, out: *mut SystemTime) -> i32;
}

const HOUR: u64 = 36_000_000_000;
const DAY: u64 = 24 * HOUR;

static TIMEWARP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The current UTC time (FILETIME units), the maintenance hours skipped with the time warp:
/// the day biased 2 hours forward (Japan's 02:00-07:00 = UTC 17:00-22:00 becomes its last five
/// hours), its time scaled by 19/24, the bias removed.
fn now() -> u64 {
    let mut now = 0u64;
    unsafe { GetSystemTimeAsFileTime(&mut now) };
    if !TIMEWARP.load(std::sync::atomic::Ordering::Relaxed) {
        return now;
    }
    let biased = now + 2 * HOUR;
    let (day, time) = (biased / DAY, biased % DAY);
    day * DAY + time * 19 / 24 - 2 * HOUR
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

pub(crate) fn init() {
    let warp = std::env::var("WAL_ALLS_TIMEWARP").is_ok_and(|v| v.trim() == "1");
    TIMEWARP.store(warp, std::sync::atomic::Ordering::Relaxed);
    unsafe {
        if warp {
            iat::hook("kernel32.dll", "GetSystemTimeAsFileTime", get_system_time_as_file_time as *const () as usize);
            iat::hook("kernel32.dll", "GetSystemTimePreciseAsFileTime", get_system_time_as_file_time as *const () as usize);
            iat::hook("kernel32.dll", "GetSystemTime", get_system_time as *const () as usize);
        }
        iat::hook("kernel32.dll", "GetLocalTime", get_local_time as *const () as usize);
        iat::hook("kernel32.dll", "GetTimeZoneInformation", get_time_zone_information as *const () as usize);
        for name in ["SetLocalTime", "SetSystemTime", "SetTimeZoneInformation"] {
            iat::hook("kernel32.dll", name, set_time as *const () as usize);
        }
    }
}
