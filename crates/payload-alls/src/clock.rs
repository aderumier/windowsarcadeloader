//! The cabinet's clock (segatools' `platform/clock.c`, public domain): Japan's time zone
//! (UTC+9, no daylight saving; Sega's games expect it), and the clock never set (amdaemon sets
//! it to ALL.Net's time: Wine refuses, an RTC error). In amdaemon and the game.

use wal_payload_common::iat;

/// SYSTEMTIME: year, month, day of week, day, hour, minute, second, milliseconds.
type SystemTime = [u16; 8];

unsafe extern "system" {
    fn GetSystemTimeAsFileTime(out: *mut u64);
    fn FileTimeToSystemTime(time: *const u64, out: *mut SystemTime) -> i32;
}

const HOUR: u64 = 36_000_000_000;

unsafe extern "system" fn get_local_time(out: *mut SystemTime) {
    let mut now = 0u64;
    unsafe {
        GetSystemTimeAsFileTime(&mut now);
        now += 9 * HOUR;
        FileTimeToSystemTime(&now, out);
    }
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
    unsafe {
        iat::hook("kernel32.dll", "GetLocalTime", get_local_time as *const () as usize);
        iat::hook("kernel32.dll", "GetTimeZoneInformation", get_time_zone_information as *const () as usize);
        for name in ["SetLocalTime", "SetSystemTime", "SetTimeZoneInformation"] {
            iat::hook("kernel32.dll", name, set_time as *const () as usize);
        }
    }
}
