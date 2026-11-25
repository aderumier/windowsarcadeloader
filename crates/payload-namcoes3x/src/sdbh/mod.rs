//! Super Dragon Ball Heroes (Chinese release, ROM 2.02, `app\Game.exe`).
//!
//! The game reaches every cabinet device through the cabinet library `neodcd.dll`, which runs
//! as on the cabinet. The payload answers what it would find on the cabinet:
//! - its network library, `DDTelIFDll.dll` ([`ddtel`]): the communication service's table;
//! - the play mat's card scanner ([`fp`]): the deck on a virtual mat;
//! - the Hero License reader ([`ic`]): the player's save card, a file of the dump;
//! - the I/O board ([`io`]): a NeoJV on JVS, the cabinet's buttons;
//! - the card dispensers ([`dispenser`]): ready, every payout done;
//! - the operator's activation card ([`activation`]): always in stock.
//!
//! The card database's texts are the English mod's ([`english`]).
//!
//! The deck is chosen in a card binder drawn over the game, and the play mat shown and played
//! in a panel ([`overlay`], [`ui`], the card pack's pictures: [`cards`]).
//!
//! The cabinet's `X:` drive is the game's root (its `Game.ini`, `version.ini`, `tel\ini`).

mod activation;
mod cards;
mod ddtel;
mod dispenser;
mod english;
mod fp;
mod ic;
mod io;
mod overlay;
mod ui;

use wal_payload_common::log;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleA;

/// This build: the executable's link time and image size.
const TIMESTAMP: u32 = 0x62b2f387;
const IMAGE_SIZE: u32 = 0x1f37000;

/// Whether the process is Super Dragon Ball Heroes' `Game.exe` (this build).
pub fn is_sdbh() -> bool {
    let base = super::game_base();
    let read = |at: usize| super::safe_read::<u32>(base + at);
    let Some(pe) = read(0x3c) else { return false };
    let pe = pe as usize;
    pe < 0x1000 && read(pe) == Some(0x4550) && read(pe + 8) == Some(TIMESTAMP) && read(pe + 24 + 56) == Some(IMAGE_SIZE)
}

/// Before the common start: the `X:` drive in the cabinet drives the payload redirects.
fn add_x_drive() {
    let root = ddtel::game_root();
    let root = root.to_string_lossy().trim_end_matches('\\').to_string();
    let var = wal_protocol::env::DRIVES;
    let mut drives = std::env::var(var).unwrap_or_default();
    if !drives.split('|').any(|d| d.starts_with("X=") || d.starts_with("x=")) {
        if !drives.is_empty() {
            drives.push('|');
        }
        drives.push_str(&format!("X={root}"));
        unsafe { std::env::set_var(var, drives) };
    }
}

pub fn init() {
    add_x_drive();
    wal_payload_common::start("namcoes3-sdbh");
    let neodcd = unsafe { GetModuleHandleA(c"neodcd.dll".as_ptr().cast()) } as usize;
    if neodcd == 0 {
        return log!("sdbh: neodcd.dll not loaded, nothing to answer");
    }
    ddtel::init(neodcd);
    fp::init(super::game_base());
    ic::init(super::game_base());
    io::init();
    dispenser::init(super::game_base());
    activation::init(super::game_base());
    overlay::init();
    english::init(super::game_base());
    std::thread::spawn(input_loop);
    log!("sdbh: Super Dragon Ball Heroes ROM 2.02");
}

/// The player's saves: `WindowsLoader\sdbh` of the dump.
pub(crate) fn save_dir() -> std::path::PathBuf {
    ddtel::game_root().join("WindowsLoader").join("sdbh")
}

/// The payload's own controls: the Hero License on and off the reader.
fn input_loop() {
    use wal_protocol::button;
    let mut held = 0u32;
    loop {
        let buttons = wal_payload_common::input(0).buttons;
        let pressed = buttons & !held;
        held = buttons;
        if pressed & button::CARD != 0 {
            ic::toggle();
        }
        overlay::input(&wal_payload_common::input(0));
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}
