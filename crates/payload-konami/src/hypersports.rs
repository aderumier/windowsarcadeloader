//! Hyper Sports 2011 Remake (Konami China, 2011, game code KC2): `AMSports.exe` (Direct3D 9) on
//! Konami's `gf.dll` framework (window, Direct3D, DirectInput, sound, the cabinet's boards).
//! The game imports no I/O library the payload could replace: wal-loader starts it with the
//! payload (`payload_loader`), the common hooks go to gf.dll (where the window, the Direct3D
//! device and DirectInput are), and the executable's gf.dll imports below are answered.
//!
//! - The I/O board: a Python 4 USB board (`GfPython4`: switches, coin slots, coin counters and
//!   blockers, lamps, the security plugs). Its functions the game imports are answered: the
//!   board ready (state 2), the security plugs read (plug 0 `KC2:C:A:A`, the game's code), the
//!   coins (`getCoinStock` / `consumeCoinStock`, slot 0: the game combines the slots), the
//!   outputs accepted. The board thread (`init`) is not started: the switch thread below takes
//!   its place.
//! - The license check: the game reads `hid.bin` from the cabinet's D:, E: or F: drive,
//!   decrypts it (RSA, Blowfish) and compares it with the PC's hardware id, at boot and from a
//!   thread while running; it failing is error 5-1505 (SECURITY ERROR). The dump has no
//!   `hid.bin`: both checks report it valid.
//! - The boot's clock check (setting 0x20, error 5-2503 "the clock is un-setting up": the
//!   operator sets it in the test menu): the PC's clock is the host's, the check is skipped.
//! - The LED controller (`GfLedController`, on the board's serial channel 2): ready, 2 nodes,
//!   its outputs accepted.
//! - The switches: each player has three buttons (blue, green, red, left to right: left, OK,
//!   right in the menus), plus the cabinet's test and service buttons and the coin switch. The
//!   board thread gives them to gf.dll's input as its extension device (input device 2:
//!   `GfInput::pushExtensionInputData`, the button and its state); the game's two
//!   `GfuVirtualInput`s (one per player) read them there. The payload's switch thread does the
//!   same from the virtual sticks, the game's own pressed / released / repeat logic follows.
//!   The developers' keys (the PC keyboard: Z / X / C, numpad 1 / 2 / 3, T, S) are not used:
//!   the fake DirectInput (`WAL_DINPUT_DISABLE=1`) has a keyboard (gf.dll's input start throws
//!   without one) without a key pressed.
//!
//! Native inputs for `native_map`: `test service coin red blue green` (players 1 and 2).

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{dinput, iat, log};

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    Red,
    Blue,
    Green,
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("red", Native::Red),
    ("blue", Native::Blue),
    ("green", Native::Green),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("b1", "blue"),
    ("b2", "green"),
    ("b3", "red"),
];

const PLAYERS: usize = 2;

/// The board's switches (extension device buttons): each player's blue, green and red buttons
/// (the game's virtual buttons 2, 1 and 0), test, service, coin.
const SW_BLUE: [u8; PLAYERS] = [5, 15];
const SW_GREEN: [u8; PLAYERS] = [6, 16];
const SW_RED: [u8; PLAYERS] = [7, 17];
const SW_TEST: u8 = 20;
const SW_SERVICE: u8 = 21;
const SW_COIN: u8 = 23;

struct Board {
    maps: Vec<ButtonMap<Native>>,
    coin_held: bool,
    /// The coins inserted, not yet consumed by the game.
    coins: u16,
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);

fn with_board<R>(f: impl FnOnce(&mut Board) -> R) -> R {
    let mut guard = BOARD.lock().unwrap();
    let board = guard.get_or_insert_with(|| Board {
        maps: (0..PLAYERS)
            .map(|p| if p == 0 { ButtonMap::new(NATIVES, DEFAULT_MAP) } else { ButtonMap::with_defaults(NATIVES, DEFAULT_MAP) })
            .collect(),
        coin_held: false,
        coins: 0,
    });
    f(board)
}

/// The switches closed now (bit per switch), the coins counted.
fn switches() -> u32 {
    let sticks: Vec<_> = (0..PLAYERS).map(wal_payload_common::input).collect();
    with_board(|b| {
        let mut bits = 0u32;
        for (player, stick) in sticks.iter().enumerate() {
            b.maps[player].for_each_pressed(stick, |n| {
                let sw = match n {
                    Native::Test => SW_TEST,
                    Native::Service => SW_SERVICE,
                    Native::Coin => SW_COIN,
                    Native::Blue => SW_BLUE[player],
                    Native::Green => SW_GREEN[player],
                    Native::Red => SW_RED[player],
                };
                bits |= 1 << sw;
            });
        }
        let coin = bits & 1 << SW_COIN != 0;
        if coin && !b.coin_held {
            b.coins = b.coins.saturating_add(1);
            log!("hypersports: coin");
        }
        b.coin_held = coin;
        bits
    })
}

/// `GfInput::pushExtensionInputData(time, type, index, value)` (cdecl; type 0: a button).
type PushInput = extern "C" fn(u32, u32, u8, i16);

/// The board thread's work: each switch change given to gf.dll's extension device.
fn switch_thread(push: PushInput) {
    let mut last = 0u32;
    loop {
        let now = switches();
        let changed = now ^ last;
        for sw in (0..32u8).filter(|&sw| changed & 1 << sw != 0) {
            push(0, 0, sw, i16::from(now & 1 << sw != 0));
        }
        last = now;
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
}

// GfPython4 (static members: cdecl)

/// `GfPython4::STATE`: 2 the board ready (3 busy, 4 and 5 errors).
const STATE_READY: u32 = 2;

static PUSH_INPUT: AtomicUsize = AtomicUsize::new(0);

/// The board started (the game's input is up): the switch thread runs.
extern "C" fn python4_init() {
    match PUSH_INPUT.load(Ordering::Relaxed) {
        0 => log!("hypersports: no GfInput::pushExtensionInputData, no switches"),
        push => {
            let push: PushInput = unsafe { std::mem::transmute(push) };
            std::thread::spawn(move || switch_thread(push));
            log!("hypersports: Python 4 I/O board emulated");
        }
    }
}

extern "C" fn python4_state() -> u32 {
    STATE_READY
}

/// A command queued (its id, 0: refused): the commands are done at once.
extern "C" fn command1(_a: u32) -> u64 {
    1
}
extern "C" fn command2(_a: u32, _b: u32) -> u64 {
    1
}

extern "C" fn nothing1(_a: u32) {}
extern "C" fn nothing2(_a: u32, _b: u32) {}
extern "C" fn nothing0() {}

/// The two security plugs (`Security`, 0x28 bytes each: an 8-byte id, 32 bytes of data): the
/// game keeps each as a string of 20 characters (its test menu, its network data).
unsafe extern "C" fn python4_security(out: *mut u8) {
    if out.is_null() {
        return;
    }
    let mut plugs = [0u8; 0x50];
    plugs[..9].copy_from_slice(b"KC2:C:A:A");
    unsafe { std::ptr::copy_nonoverlapping(plugs.as_ptr(), out, plugs.len()) };
}

/// The coins of `slot` not consumed yet (the game reads slot 0).
extern "C" fn coin_stock(slot: i32) -> u16 {
    if slot == 0 { with_board(|b| b.coins) } else { 0 }
}

/// `count` coins of `slot` used: true when there were.
extern "C" fn consume_coins(slot: u32, count: u16) -> bool {
    slot == 0
        && with_board(|b| {
            if b.coins < count {
                return false;
            }
            b.coins -= count;
            true
        })
}

// GfLedController (static members: cdecl)

/// `GfLedController::STATE`: 2 ready (1 starting).
extern "C" fn led_init(_channel: u32, _baud: u32, _parity: u32) -> bool {
    true
}

extern "C" fn led_state() -> u32 {
    2
}

extern "C" fn led_nodes() -> u32 {
    2
}

/// An output command queued (0: refused).
extern "C" fn led_output(_node: u32, _data: u32) -> u32 {
    1
}

/// The license checks (thiscall, 1: `hid.bin` valid, "security OK!"), at boot and in the
/// running game's thread: `mov eax, 1; ret`.
const LICENSE_CHECKS: [usize; 2] = [0x45b0f0, 0x45f190];

/// The boot's clock set check: its `jne` (set) becomes `jmp`.
const CLOCK_CHECK: usize = 0x45d35e;

/// Writes `new` at `at` when the code there is `expected`.
fn patch(at: usize, expected: &[u8], new: &[u8], what: &str) {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    if unsafe { std::slice::from_raw_parts(at as *const u8, expected.len()) } != expected {
        log!("hypersports: unexpected code at {at:#x}, {what} not patched");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, new.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(new.as_ptr(), at as *mut u8, new.len());
        VirtualProtect(at as *const c_void, new.len(), old, &mut old);
    }
    log!("hypersports: {what} patched");
}

/// The game is AMSports.exe.
pub fn detected() -> bool {
    crate::file_name(std::ptr::null_mut()).eq_ignore_ascii_case("AMSports.exe")
}

/// gf.dll's base: the module the common hooks go to.
pub fn game_module() -> usize {
    unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(c"gf.dll".as_ptr().cast()) as usize }
}

pub fn init() {
    dinput::set_keyboard(|_| {});
    let push = unsafe {
        windows_sys::Win32::System::LibraryLoader::GetProcAddress(
            game_module() as _,
            c"?pushExtensionInputData@GfInput@@SAXIW4OBJECT_TYPE@1@EF@Z".as_ptr().cast(),
        )
    };
    PUSH_INPUT.store(push.map_or(0, |f| f as usize), Ordering::Relaxed);
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as usize };
    let f = |p: *const ()| p as usize;
    let hooks: &[(&str, usize)] = &[
        ("?init@GfPython4@@SAXXZ", f(python4_init as *const ())),
        ("?getState@GfPython4@@SA?AW4STATE@1@XZ", f(python4_state as *const ())),
        ("?getSecurityPlugStart@GfPython4@@SA_KE@Z", f(command1 as *const ())),
        ("?getSecurity@GfPython4@@SAXPAUSecurity@1@@Z", f(python4_security as *const ())),
        ("?getCoinStock@GfPython4@@SAGH@Z", f(coin_stock as *const ())),
        ("?consumeCoinStock@GfPython4@@SA_NIG@Z", f(consume_coins as *const ())),
        ("?setCoinBlockersStart@GfPython4@@SA_K_N0@Z", f(command2 as *const ())),
        ("?setCoinCounterEnable@GfPython4@@SAX_N0@Z", f(nothing2 as *const ())),
        ("?setCoinCounterCombine@GfPython4@@SAX_N@Z", f(nothing1 as *const ())),
        ("?setOutputStart@GfPython4@@SA_KG@Z", f(command1 as *const ())),
        ("?setWatchDogTimer@GfPython4@@SAXE@Z", f(nothing1 as *const ())),
        ("?init@GfLedController@@SA_NIW4SERIAL_BAUD@GfPython4@@W4SERIAL_PARITY@3@@Z", f(led_init as *const ())),
        ("?getState@GfLedController@@SA?AW4STATE@1@XZ", f(led_state as *const ())),
        ("?getNrNodes@GfLedController@@SAIXZ", f(led_nodes as *const ())),
        ("?quit@GfLedController@@SAXXZ", f(nothing0 as *const ())),
        ("?setOutputStart@GfLedController@@SAIEI@Z", f(led_output as *const ())),
        ("?setOutputStart@GfLedController@@SAIEQBE@Z", f(led_output as *const ())),
    ];
    let mut hooked = 0;
    for (name, address) in hooks {
        if unsafe { iat::hook_module(exe, "gf.dll", name, *address) }.is_some() {
            hooked += 1;
        } else {
            log!("hypersports: {name} not imported");
        }
    }
    for at in LICENSE_CHECKS {
        patch(at, &[0x53, 0x8b, 0xdc, 0x83, 0xec, 0x08], &[0xb8, 0x01, 0x00, 0x00, 0x00, 0xc3], "a license check");
    }
    patch(CLOCK_CHECK, &[0x75, 0x14], &[0xeb, 0x14], "the clock check");
    log!("hypersports: {hooked} gf.dll imports answered (I/O board, LED controller), switches on its input's extension device");
}
