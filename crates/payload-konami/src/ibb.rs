//! The Bishi Bashi (IBB): the payload is the game's `libacio.dll` (ACIO bus, card reader, the
//! HBHI board: the players' buttons, coins, security plugs); the game also imports `libbms.dll`
//! (the BM-System client, `bms.rs`), pointed at this DLL's functions of the same names.
//!
//! Three players, each a red, a green, a blue and a yellow button. The game reads 15 switches
//! from the HBHI status (rva 0x6a0ff): switch `n` = player * 4 + color (red, green, blue,
//! yellow), the game's button `color * 3 + player` (table 0x49ae30). Switches 0-7 (players 1
//! and 2) are packet byte 8's bits 0-7; switches 8-11 (player 3) analog channels 0-3, pressed
//! below half way (`<= 0x7fc0`); then test (byte 1 0x20), service (0x10), coin mech (0x04).
//!
//! Native inputs for `native_map`: `test service coin red green blue yellow` (players 1-3).

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// The HBHI status read (`call [ac_io_hbhi_get_control_status_buffer]`, rva 0x6a0f9).
const STATUS_READ: usize = 0x6a0f9;

/// The game's code (launcher.exe, also its "All Avatars" edit).
pub fn detected() -> bool {
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    // SizeOfImage: another game's smaller executable is not read past its end
    let size = unsafe { ((exe + ((exe + 0x3c) as *const u32).read_unaligned() as usize + 0x50) as *const u32).read_unaligned() } as usize;
    size > STATUS_READ + 6
        && unsafe { std::slice::from_raw_parts((exe + STATUS_READ) as *const u8, 6) } == [0xff, 0x15, 0x8c, 0x83, 0x48, 0x00]
}

pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    /// A button: its color (red, green, blue, yellow).
    Button(usize),
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("red", Native::Button(0)),
    ("green", Native::Button(1)),
    ("blue", Native::Button(2)),
    ("yellow", Native::Button(3)),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("b1", "red"),
    ("b2", "green"),
    ("b3", "blue"),
    ("b4", "yellow"),
    // yellow starts a game and joins it, ends a mini game's turn ("finish button")
    ("start", "yellow"),
];

const PLAYERS: usize = 3;

struct Board {
    maps: Vec<ButtonMap<Native>>,
    coin_held: bool,
}

static BOARD: Mutex<Option<Board>> = Mutex::new(None);

/// The HBHI status packet.
pub(crate) fn packet() -> [u8; 15] {
    let mut guard = BOARD.lock().unwrap();
    let board = guard.get_or_insert_with(|| Board {
        maps: (0..PLAYERS)
            .map(|p| if p == 0 { ButtonMap::new(NATIVES, DEFAULT_MAP) } else { ButtonMap::with_defaults(NATIVES, DEFAULT_MAP) })
            .collect(),
        coin_held: false,
    });
    let mut raw = [0u8; 15];
    let mut pressed = [false; PLAYERS * 4];
    let mut coin = false;
    for player in 0..PLAYERS {
        let stick = wal_payload_common::input(player);
        board.maps[player].for_each_pressed(&stick, |n| match n {
            Native::Test => raw[1] |= 0x20,
            Native::Service => raw[1] |= 0x10,
            Native::Coin => {
                raw[1] |= 0x04;
                coin = true;
            }
            Native::Button(color) => pressed[player * 4 + color] = true,
        });
    }
    for (n, &down) in pressed.iter().enumerate().take(8) {
        if down {
            raw[8] |= 1 << n;
        }
    }
    // player 3: analog channels 0-3 (high byte, the low 2 bits in the next byte's top bits),
    // released at full scale
    for ch in 0..4 {
        if !pressed[8 + ch] {
            raw[ch * 2] = 0xff;
            raw[ch * 2 + 1] |= 0xc0;
        }
    }
    if coin && !board.coin_held {
        crate::hbhi::coin_inserted();
    }
    board.coin_held = coin;
    raw
}

/// The boot's clock check: the operator set the clock (setting 0x1d; error 0x16, 5-2503-0000
/// "The clock is un-setting up" otherwise): the PC's clock is the host's. Its `jne` (set)
/// becomes `jmp`.
const CLOCK_CHECK: usize = 0x2f940;

/// Writes `new` at `at` when the code there is `expected`.
fn patch(at: usize, expected: &[u8], new: &[u8], what: &str) {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    if unsafe { std::slice::from_raw_parts(at as *const u8, expected.len()) } != expected {
        log!("ibb: unexpected code at {at:#x}, {what} not patched");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const std::ffi::c_void, new.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(new.as_ptr(), at as *mut u8, new.len());
        VirtualProtect(at as *const std::ffi::c_void, new.len(), old, &mut old);
    }
    log!("ibb: {what} patched");
}

pub fn init(module: *mut std::ffi::c_void) {
    ACTIVE.store(true, Ordering::Relaxed);
    let mut hooked = 0;
    for (name, address) in crate::otomedius::exports(module) {
        if name.starts_with("bms_") && unsafe { iat::hook("libbms.dll", &name, address) }.is_some() {
            hooked += 1;
        }
    }
    log!("ibb: {hooked} libbms imports answered by the payload");
    let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    patch(exe + CLOCK_CHECK, &[0x75, 0x11], &[0xeb, 0x11], "the clock check");
    crate::avs::init();
}
