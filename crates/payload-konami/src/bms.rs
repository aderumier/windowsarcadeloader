//! BM-System (Gashaaaan Refill!): `libbms.dll`, replaced by this payload.
//!
//! The cabinet runs `bms_IG2.exe`, a service the game reaches through libbms over the
//! `\\.\pipe\bmssvc` pipe: e-Amusement (ea3), the clock, event logs, rankings and the credits
//! (coins counted per slot, converted with the test menu's coin options). Here libbms answers
//! itself, without the service: e-Amusement offline ("operating in offline mode"), the clock
//! the PC's, no rankings or server messages, and the credits kept in the payload.
//!
//! Every call is cdecl. A request returns 0 (accepted); its `_wait` returns 0 done, 1 failed
//! (2 still running, never here).

use std::ffi::{CStr, c_char};
use std::sync::Mutex;

use wal_payload_common::log;

const DONE: i32 = 0;
const FAILED: i32 = 1;

/// The credits: coins inserted (all slots count as slot 1, the only one the game uses) and the
/// coin options (`bms_credit_update_option`: 13 bytes, freeplay, start1-4 the credits each kind
/// of start costs, player1-4 the slot each player pays from, slot1-4 coins per credit). A
/// player pays for a kind of start (`/coin/p<player>s<kind>a1`): kind 1 a game, 2 and 3 the
/// game's other plays (2 credits, 1 and 2 by default).
struct Credits {
    coins: u32,
    freeplay: bool,
    coins_per_credit: u32,
    start_cost: [u32; 4],
}

static CREDITS: Mutex<Credits> =
    Mutex::new(Credits { coins: 0, freeplay: false, coins_per_credit: 1, start_cost: [1, 1, 1, 1] });

impl Credits {
    fn credits(&self) -> u32 {
        self.coins / self.coins_per_credit
    }

    fn cost(&self, kind: u32) -> u32 {
        if self.freeplay { 0 } else { self.start_cost.get(kind.wrapping_sub(1) as usize).copied().unwrap_or(1) }
    }
}

fn text(s: *const c_char) -> String {
    if s.is_null() {
        return String::new();
    }
    let bytes = unsafe { CStr::from_ptr(s) }.to_bytes();
    // Shift-JIS messages: the ASCII part is enough for the log
    String::from_utf8_lossy(bytes).trim_end().to_string()
}

unsafe fn put<T>(out: *mut T, value: T) {
    if !out.is_null() {
        unsafe { out.write_unaligned(value) };
    }
}

// Boot: bms_init then bms_init_wait (0: finished, the service up).

#[unsafe(no_mangle)]
pub extern "C" fn bms_init(_config: usize, _a: usize, _b: usize, _flag: usize) -> i32 {
    log!("bms: BM-System answered by the payload (e-Amusement offline)");
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_init_wait() -> i32 {
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_init_error() -> i32 {
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_is_active() -> i32 {
    1
}

/// The last error's name (COMSERV, PCONFIRM...): none.
#[unsafe(no_mangle)]
pub extern "C" fn bms_get_last_error(out: *mut u8) -> i32 {
    unsafe { put(out, 0) };
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_log_info(fmt: *const c_char) {
    log!("bms: {}", text(fmt));
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_log_error(fmt: *const c_char) {
    log!("bms: error: {}", text(fmt));
}

// The clock: milliseconds since 1970 (UTC).

#[unsafe(no_mangle)]
pub extern "C" fn bms_time_get_clock(_flag: u32) -> i32 {
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_time_get_clock_wait(out: *mut u64) -> i32 {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    unsafe { put(out, now.as_millis() as u64) };
    DONE
}

// e-Amusement: status 2 with no error is "operating in offline mode" (0/1 checking the
// network, 3 online, 4 server maintenance).

#[unsafe(no_mangle)]
pub extern "C" fn bms_ea3_license_agree(_agree: u32) -> i32 {
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_ea3_license_agree_wait(out: *mut u32) -> i32 {
    unsafe { put(out, 1) };
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_ea3_get_status() -> i32 {
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_ea3_get_status_wait(status: *mut u32, update: *mut u32) -> i32 {
    unsafe {
        put(status, 2);
        put(update, 0);
    }
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_ea3_download_pause(_pause: u32) -> i32 {
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_ea3_download_pause_wait(out: *mut u32) -> i32 {
    unsafe { put(out, 0) };
    DONE
}

// Online services (server messages, network check, keepalive, rankings): unavailable.

#[unsafe(no_mangle)]
pub extern "C" fn bms_get_server_message(_buf: usize) -> i32 {
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_get_server_message_wait(_a: usize, _b: usize) -> i32 {
    FAILED
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_net_property_get(_out: usize) -> i32 {
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_net_property_get_wait(_out: usize) -> i32 {
    FAILED
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_keepalive_test_request(_kind: u32) -> i32 {
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_keepalive_test_request_wait(_out: usize) -> i32 {
    FAILED
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_ranking_get(_name: usize, _buf: usize, _count: u32) -> i32 {
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_ranking_get_wait(_a: usize, _b: usize, _c: usize) -> i32 {
    FAILED
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_playerdata_save(_a: usize, _b: usize, _c: usize) -> i32 {
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_playerdata_save_wait(out: *mut u32) -> i32 {
    unsafe { put(out, 0) };
    DONE
}

// Event logs (play statistics for the server): dropped.

macro_rules! eventlog {
    ($($name:ident),*) => {$(
        #[unsafe(no_mangle)]
        pub extern "C" fn $name() -> i32 {
            DONE
        }
    )*};
}

eventlog!(
    bms_eventlog_request_init,
    bms_eventlog_request_otdemo,
    bms_eventlog_request_testmode,
    bms_eventlog_request_error,
    bms_eventlog_request_game_start,
    bms_eventlog_request_game_end,
    bms_eventlog_request_transition,
    bms_eventlog_request_eventdata
);

// Credits.

#[unsafe(no_mangle)]
pub extern "C" fn bms_credit_update_option(option: *const u8) -> i32 {
    if option.is_null() {
        return FAILED;
    }
    let o = unsafe { std::slice::from_raw_parts(option, 13) };
    let mut c = CREDITS.lock().unwrap();
    c.freeplay = o[0] != 0;
    c.coins_per_credit = u32::from(o[9]).max(1);
    c.start_cost = [o[1], o[2], o[3], o[4]].map(u32::from);
    log!("bms: coin options: freeplay {}, {} coin(s) per credit, start {:?}", c.freeplay, c.coins_per_credit, &o[1..5]);
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_credit_update_option_blocking(option: *const u8) -> i32 {
    bms_credit_update_option(option)
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_credit_reset() -> i32 {
    CREDITS.lock().unwrap().coins = 0;
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_credit_reset_blocking() -> i32 {
    bms_credit_reset()
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_credit_coin_throw(_slot: u32, count: u32) -> i32 {
    let mut c = CREDITS.lock().unwrap();
    c.coins = c.coins.saturating_add(count);
    log!("bms: {count} coin(s), {} credit(s)", c.credits());
    DONE
}

/// The service button: one credit.
#[unsafe(no_mangle)]
pub extern "C" fn bms_credit_service() -> i32 {
    let mut c = CREDITS.lock().unwrap();
    c.coins = c.coins.saturating_add(c.coins_per_credit);
    log!("bms: service credit, {} credit(s)", c.credits());
    DONE
}

/// Whether `player` has the credits for a `kind` of start: 0 yes, 3 not enough.
#[unsafe(no_mangle)]
pub extern "C" fn bms_credit_confirm_blocking(_player: u32, kind: u32) -> i32 {
    let c = CREDITS.lock().unwrap();
    if c.credits() >= c.cost(kind) { DONE } else { 3 }
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_credit_consume(player: u32, kind: u32) -> i32 {
    let mut c = CREDITS.lock().unwrap();
    let cost = c.cost(kind);
    c.coins = c.coins.saturating_sub(cost * c.coins_per_credit);
    log!("bms: player {player} paid {cost} credit(s) (start {kind}), {} left", c.credits());
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_credit_consume_expired(_player: u32) -> i32 {
    DONE
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_credit_get_status(_slot: u32) -> i32 {
    DONE
}

/// The credits and the coins toward the next one.
#[unsafe(no_mangle)]
pub extern "C" fn bms_credit_get_status_wait(out: *mut [u32; 2]) -> i32 {
    let c = CREDITS.lock().unwrap();
    unsafe { put(out, [c.credits(), c.coins % c.coins_per_credit]) };
    DONE
}

macro_rules! credit_wait {
    ($($name:ident),*) => {$(
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(out: *mut u32) -> i32 {
            unsafe { put(out, 0) };
            DONE
        }
    )*};
}

credit_wait!(
    bms_credit_reset_wait,
    bms_credit_coin_throw_wait,
    bms_credit_service_wait,
    bms_credit_consume_wait,
    bms_credit_update_option_wait
);

// Otomedius's libbms: blocking credit calls (0 done, 3 not enough credits).

#[unsafe(no_mangle)]
pub extern "C" fn bms_credit_coin_throw_blocking(slot: u32, count: u32) -> i32 {
    bms_credit_coin_throw(slot, count)
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_credit_get_status_blocking(_slot: u32, out: *mut [u32; 2]) -> i32 {
    bms_credit_get_status_wait(out)
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_credit_consume_blocking(player: u32, kind: u32) -> i32 {
    bms_credit_consume(player, kind)
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_credit_service_blocking() -> i32 {
    bms_credit_service()
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_time_get_clock_blocking(_flag: u32, out: *mut u64) -> i32 {
    bms_time_get_clock_wait(out)
}

// e-Amusement pass, card management, matching (network battles), player data, the cabinets'
// TCP links: services of the e-Amusement network, unavailable (requests accepted, their
// results failed).

#[unsafe(no_mangle)]
pub extern "C" fn bms_ea3_get_url_eapass() -> *const u8 {
    std::ptr::null()
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_ea3_get_url_konaminetdx() -> *const u8 {
    std::ptr::null()
}

#[unsafe(no_mangle)]
pub extern "C" fn bms_ea3_get_eapass_valid() -> u16 {
    0
}

macro_rules! unavailable {
    ($($request:ident / $wait:ident),*) => {$(
        #[unsafe(no_mangle)]
        pub extern "C" fn $request() -> i32 {
            DONE
        }

        #[unsafe(no_mangle)]
        pub extern "C" fn $wait() -> i32 {
            FAILED
        }
    )*};
}

unavailable!(
    bms_cardmng_inquire / bms_cardmng_inquire_wait,
    bms_cardmng_cardactivate / bms_cardmng_cardactivate_wait,
    bms_cardmng_bindmodel / bms_cardmng_bindmodel_wait,
    bms_cardmng_playerlogin / bms_cardmng_playerlogin_wait,
    bms_cardmng_bindcard / bms_cardmng_bindcard_wait,
    bms_cardmng_playerlogout / bms_cardmng_playerlogout_wait,
    bms_matching_request / bms_matching_request_wait,
    bms_matching_query / bms_matching_query_wait,
    bms_playerdata_load / bms_playerdata_load_wait,
    bms_net_tcp_client_connect_request / bms_net_tcp_client_connect_wait
);

#[unsafe(no_mangle)]
pub extern "C" fn bms_matching_reset_wait() -> i32 {
    DONE
}

macro_rules! tcp_failed {
    ($($name:ident),*) => {$(
        #[unsafe(no_mangle)]
        pub extern "C" fn $name() -> i32 {
            -1
        }
    )*};
}

tcp_failed!(
    bms_net_tcp_initialize,
    bms_net_tcp_server_listen,
    bms_net_tcp_server_accept,
    bms_net_tcp_server_close_dstsocket_all,
    bms_net_tcp_client_disconnect,
    bms_net_tcp_sendto,
    bms_net_tcp_recvfrom
);
