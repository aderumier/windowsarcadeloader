//! PGA TOUR Golf Championship Edition's game: the C++ libraries in front of the cabinet database
//! (`GvrNetTournament.dll`, `GvrNetGameSession.dll`, `GvrNetLeaderBoard.dll`,
//! `GvrNetCabinetConfiguration.dll`, `GvrNetPlayerGameSummary.dll`, imported by the game, its
//! engine and the shell control). Each wraps .NET classes of `C:\gvr.net` (the cabinet's SQL
//! Server database) the dump lacks: the payload is installed under their names (profile
//! `payload_install`) and exports their classes' methods (`USBIOExtreme.def`, thiscall), for a
//! cabinet that never registered online:
//!
//! * no tournaments, leaderboards, course records nor World Tour progress (counts 0, texts "");
//! * the non-tournament playing conditions of a course and day (wind, pins, weather), the XML the
//!   cabinet's database loads from `C:\gvr.net\PgaDefaultFiles\PgaNonTourneyPlayingConditions\
//!   <course>\TDay<day>.xml`;
//! * the cabinet configuration the shell edits (`gvrnet.rs`: skins, mulligans, time out, play
//!   mode, volumes);
//! * the game's results (game session events) dropped.
//!
//! Sizes count the terminating zero: the callers allocate them, then read the text.

use std::ffi::c_char;

use wal_payload_common::log;

/// A `static char *` member exported as data.
#[repr(transparent)]
pub struct Text(*const c_char);
unsafe impl Sync for Text {}

#[unsafe(no_mangle)]
pub static GVRNET_ACTIVE: Text = Text(c"Active".as_ptr());
#[unsafe(no_mangle)]
pub static GVRNET_ALL: Text = Text(c"All".as_ptr());
#[unsafe(no_mangle)]
pub static GVRNET_DATA_TYPE_IMAGE: Text = Text(c"Image".as_ptr());
#[unsafe(no_mangle)]
pub static GVRNET_DATA_TYPE_LED: Text = Text(c"LED".as_ptr());
#[unsafe(no_mangle)]
pub static GVRNET_DATA_TYPE_TEXT: Text = Text(c"Text".as_ptr());
#[unsafe(no_mangle)]
pub static GVRNET_FUTURE: Text = Text(c"Future".as_ptr());
#[unsafe(no_mangle)]
pub static GVRNET_NATIONAL: Text = Text(c"GVRHOSTEDNATIONAL".as_ptr());
#[unsafe(no_mangle)]
pub static GVRNET_OPERATOR: Text = Text(c"OPERATORHOSTED".as_ptr());
#[unsafe(no_mangle)]
pub static GVRNET_PAST: Text = Text(c"Past".as_ptr());
#[unsafe(no_mangle)]
pub static GVRNET_STARTDATE: Text = Text(c"StartDate".as_ptr());
#[unsafe(no_mangle)]
pub static GVRNET_ALL18: Text = Text(c"All18".as_ptr());
#[unsafe(no_mangle)]
pub static GVRNET_BACK9: Text = Text(c"Back9".as_ptr());
#[unsafe(no_mangle)]
pub static GVRNET_FRONT9: Text = Text(c"Front9".as_ptr());

/// The vtable of the classes with a virtual destructor (`GvrNetTournament`,
/// `GvrNetTournamentData`, `GvrNetCabinetConfiguration`, `GvrNetCabinetConfigurationData`): their
/// scalar deleting destructor.
#[repr(transparent)]
struct Vtable([*const (); 1]);
unsafe impl Sync for Vtable {}
static VTABLE: Vtable = Vtable([deleting_destructor as *const ()]);

unsafe extern "thiscall" fn deleting_destructor(this: *mut usize, flags: u32) -> *mut usize {
    // `delete`: the game's C library's (MSVCRT `operator delete` is its `free`)
    if flags & 1 != 0 {
        unsafe extern "C" {
            fn free(p: *mut std::ffi::c_void);
        }
        unsafe { free(this.cast()) };
    }
    this
}

/// The constructor of a class with a virtual destructor: its vtable.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_new_virtual(this: *mut usize) -> *mut usize {
    if !this.is_null() {
        unsafe { *this = VTABLE.0.as_ptr() as usize };
    }
    this
}

/// The constructor of the other classes: nothing.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_new(this: *mut usize) -> *mut usize {
    this
}

// 0 (or false, or nothing) with 0 to 6 arguments, and 15 (`SetHoleEventCompleted`).
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_zero0(_this: usize) -> i32 {
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_zero1(_this: usize, _a: usize) -> i32 {
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_zero2(_this: usize, _a: usize, _b: usize) -> i32 {
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_zero3(_this: usize, _a: usize, _b: usize, _c: usize) -> i32 {
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_zero4(_this: usize, _a: usize, _b: usize, _c: usize, _d: usize) -> i32 {
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_zero5(_this: usize, _a: usize, _b: usize, _c: usize, _d: usize, _e: usize) -> i32 {
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_zero6(_this: usize, _a: usize, _b: usize, _c: usize, _d: usize, _e: usize, _f: usize) -> i32 {
    0
}
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "thiscall" fn gvrnet_zero15(
    _this: usize,
    _a: usize,
    _b: usize,
    _c: usize,
    _d: usize,
    _e: usize,
    _f: usize,
    _g: usize,
    _h: usize,
    _i: usize,
    _j: usize,
    _k: usize,
    _l: usize,
    _m: usize,
    _n: usize,
    _o: usize,
) -> i32 {
    0
}

// The size of an empty text (1) with 0 to 4 arguments.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_one0(_this: usize) -> i32 {
    1
}
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_one1(_this: usize, _a: usize) -> i32 {
    1
}
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_one2(_this: usize, _a: usize, _b: usize) -> i32 {
    1
}
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_one3(_this: usize, _a: usize, _b: usize, _c: usize) -> i32 {
    1
}
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_one4(_this: usize, _a: usize, _b: usize, _c: usize, _d: usize) -> i32 {
    1
}

unsafe fn empty(buf: *mut u8) {
    if !buf.is_null() {
        unsafe { *buf = 0 };
    }
}

// An empty text written to the first argument, 1 to 5 arguments.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_empty1(_this: usize, buf: *mut u8) {
    unsafe { empty(buf) }
}
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_empty2(_this: usize, buf: *mut u8, _b: usize) {
    unsafe { empty(buf) }
}
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_empty3(_this: usize, buf: *mut u8, _b: usize, _c: usize) {
    unsafe { empty(buf) }
}
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_empty4(_this: usize, buf: *mut u8, _b: usize, _c: usize, _d: usize) {
    unsafe { empty(buf) }
}
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_empty5(_this: usize, buf: *mut u8, _b: usize, _c: usize, _d: usize, _e: usize) {
    unsafe { empty(buf) }
}

/// `(int &, int)` and `(bool &, int)` getters: 0 / false.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_ref_zero(_this: usize, out: *mut i32, _index: i32) {
    if !out.is_null() {
        unsafe { *out = 0 };
    }
}

/// `GetCabinetID(int &)`: 0 (never registered).
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_ref_zero1(_this: usize, out: *mut i32) {
    if !out.is_null() {
        unsafe { *out = 0 };
    }
}

/// `GetCurrentPlayer(bool &, int)`: false (one byte).
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_bool_false(_this: usize, out: *mut u8, _index: i32) {
    if !out.is_null() {
        unsafe { *out = 0 };
    }
}

/// `GvrNetCourseStatData::GetXxx(int &value, char *first, char *last, int &holes)`: no record.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_course_stat(_this: usize, value: *mut i32, first: *mut u8, last: *mut u8, other: *mut i32) {
    unsafe {
        if !value.is_null() {
            *value = 0;
        }
        empty(first);
        empty(last);
        if !other.is_null() {
            *other = 0;
        }
    }
}

/// `GvrNetCourseStatData::GetXxxSize(int &first, int &last)`: empty names.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_course_stat_size(_this: usize, first: *mut i32, last: *mut i32) {
    unsafe {
        if !first.is_null() {
            *first = 1;
        }
        if !last.is_null() {
            *last = 1;
        }
    }
}

/// `GetCourseHoleAverageScore(double &, int, const char *, int)`: 0.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_average(_this: usize, out: *mut f64, _a: usize, _b: usize, _c: usize) {
    if !out.is_null() {
        unsafe { *out = 0.0 };
    }
}

/// The playing conditions of course `course`, day `day` (their XML; empty when none).
fn playing_condition(course: i32, day: i32) -> String {
    let path = format!("C:\\gvr.net\\PgaDefaultFiles\\PgaNonTourneyPlayingConditions\\{course:03}\\TDay{day:03}.xml");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    log!("gvrnet: playing conditions of course {course} day {day}: {} bytes", text.len());
    text
}

/// `GvrNetTournament::GetPlayingConditionNonTournamentSize(int course, int day)`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_condition_size(_this: usize, course: i32, day: i32) -> i32 {
    playing_condition(course, day).len() as i32 + 1
}

/// `GvrNetTournament::GetPlayingConditionNonTournament(char *, int course, int day)`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn gvrnet_condition(_this: usize, buf: *mut u8, course: i32, day: i32) {
    if buf.is_null() {
        return;
    }
    let text = playing_condition(course, day);
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr(), buf, text.len());
        *buf.add(text.len()) = 0;
    }
}

/// A cabinet configuration value (`name`'s `Value`): the number after its `~` (the enums'
/// index), else the value.
fn config(name: &str) -> i32 {
    let doc = crate::gvrnet::Doc::parse(&crate::gvrnet::load_config("CabinetConfig"));
    let value = doc.rows(name).first().and_then(|&r| doc.field(r, "Value")).map(|f| doc.elements[f].text.trim().to_string()).unwrap_or_default();
    let number = value.rsplit('~').next().unwrap_or("");
    number.trim().parse().or_else(|_| value.split_whitespace().last().unwrap_or("").parse()).unwrap_or(0)
}

macro_rules! config_getter {
    ($($f:ident => $name:literal,)*) => {$(
        #[unsafe(no_mangle)]
        pub unsafe extern "thiscall" fn $f(_this: usize, out: *mut i32) {
            if !out.is_null() {
                unsafe { *out = config($name) };
            }
        }
    )*};
}

config_getter! {
    gvrnet_announcer_volume => "AnnouncerVolume",
    gvrnet_coins_per_money_unit => "CoinsPerMoneyUnit",
    gvrnet_credit_display => "CreditDisplay",
    gvrnet_eighteen_hole_price => "EighteenHoleTournamentPrice",
    gvrnet_eighteen_stroke_skin => "EighteenStrokeSkin",
    gvrnet_international_setting => "InternationalSetting",
    gvrnet_mulligans => "Mulligans",
    gvrnet_nine_hole_price => "NineHoleTournamentPrice",
    gvrnet_nine_stroke_skin => "NineStrokeSkin",
    gvrnet_play_mode => "PlayMode",
    gvrnet_shootout_price => "ShootoutPrice",
    gvrnet_sound_volume => "SoundVolume",
    gvrnet_three_stroke_skin => "ThreeStrokeSkin",
    gvrnet_time_out_delay => "TimeOutDelay",
    gvrnet_time_out_delay_on => "TimeOutDelayOn",
}

#[cfg(test)]
mod tests {
    #[test]
    fn config_numbers() {
        let number = |value: &str| -> i32 {
            let n = value.rsplit('~').next().unwrap_or("");
            n.trim().parse().or_else(|_| value.split_whitespace().last().unwrap_or("").parse()).unwrap_or(0)
        };
        assert_eq!(number("Medium Low~2"), 2);
        assert_eq!(number("60"), 60);
        assert_eq!(number("On~0"), 0);
    }
}
