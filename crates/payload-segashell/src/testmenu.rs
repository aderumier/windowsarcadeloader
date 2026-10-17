//! The cabinet's test menu: the Shell's (`Shell.exe`), run in place of the game.
//!
//! On the cabinet the Shell watches the TEST button while the game runs, closes the game and
//! shows its test menu (settings, bookkeeping, gun calibration...), then launches the game
//! again. Here the payload plays the Shell for the game, so:
//!
//! * In the game, TEST (F2) asks the launcher to start the test menu program
//!   (`WAL_SEGASHELL_TEST_MENU=<path from the game directory>`, `../shell/Shell.exe`) and
//!   ends the game.
//! * In that program (the payload is loaded by `wal-loader` there too), its own patches
//!   (`WAL_SEGASHELL_SHELL_PATCHES`, the `WAL_PATCHES` format: straight into the test menu
//!   after its start, its data folders next to its own), its I/O board report
//!   (`WAL_SEGASHELL_SHELL_INPUT=<rva>`) written from the virtual sticks, and the
//!   calls leaving the test menu (`WAL_SEGASHELL_SHELL_EXIT=<rva>,...`: call instructions,
//!   the game launch and the restart) replaced by a return to the game.
//!
//! The Shell writes its settings and the gun calibration in `ShellData/*.ini`, which the game
//! reads at start (`../ShellData/ShellData.ini`).

use std::time::Duration;

use wal_payload_common::{log, patches};
use wal_protocol::{Axis, StickState, button};
use windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};

/// The test menu program, from the game's directory (`WAL_SEGASHELL_TEST_MENU`).
fn test_menu() -> Option<String> {
    std::env::var("WAL_SEGASHELL_TEST_MENU").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// This process is the test menu program (its file name).
pub(crate) fn is_test_menu() -> bool {
    test_menu().is_some_and(|m| {
        let name = m.rsplit(['/', '\\']).next().unwrap_or_default();
        name.eq_ignore_ascii_case(&patches::exe_name())
    })
}

fn rva(var: &str) -> Vec<usize> {
    std::env::var(var)
        .unwrap_or_default()
        .split(',')
        .filter_map(|v| usize::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok())
        .collect()
}

fn base() -> usize {
    unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as usize }
}

/// The launcher starts `args` (relaunch request) once this process has ended.
fn leave(args: &str) {
    if !wal_payload_common::connected() {
        log!("testmenu: launcher not connected, staying");
        return;
    }
    wal_payload_common::request_relaunch(args);
    // the request reaches the launcher before the process ends
    std::thread::sleep(Duration::from_millis(300));
    unsafe { TerminateProcess(GetCurrentProcess(), 0) };
}

/// In the game: TEST starts the test menu program. A TEST held at start (back from the test
/// menu) is not a press.
pub(crate) fn watch_game() {
    let Some(menu) = test_menu() else { return };
    std::thread::spawn(move || {
        let mut held = true;
        loop {
            std::thread::sleep(Duration::from_millis(20));
            let down = (0..2).any(|p| wal_payload_common::input(p).pressed(button::TEST));
            if down && !held {
                log!("testmenu: TEST, starting {menu}");
                leave(&format!("exe={menu}"));
            }
            held = down;
        }
    });
}

/// In the test menu program.
pub(crate) fn init_shell() {
    // its files from its own directory (started from the game's)
    let mut path = [0u16; 520];
    let n = unsafe { GetModuleFileNameW(std::ptr::null_mut(), path.as_mut_ptr(), path.len() as u32) } as usize;
    if let Some(sep) = path[..n].iter().rposition(|c| *c == b'\\' as u16) {
        path[sep] = 0;
        unsafe { windows_sys::Win32::System::Environment::SetCurrentDirectoryW(path.as_ptr()) };
    }
    if let Ok(spec) = std::env::var("WAL_SEGASHELL_SHELL_PATCHES") {
        patches::apply_list(&spec);
    }
    if let [report_at, buttons_at] = rva("WAL_SEGASHELL_SHELL_INPUT")[..] {
        // it only reads the board (and turns its buttons into its own) once the board is found
        // (never here): both written in its place
        let (report_at, buttons_at) = (base() + report_at, base() + buttons_at);
        std::thread::spawn(move || {
            loop {
                unsafe { report(report_at as *mut u8, buttons_at as *mut u8) };
                std::thread::sleep(Duration::from_millis(4));
            }
        });
        log!("testmenu: I/O board report at {report_at:#x}, buttons at {buttons_at:#x} from the virtual sticks");
    }
    for at in rva("WAL_SEGASHELL_SHELL_EXIT") {
        unsafe { call_to(base() + at, back_to_game as *const () as usize) };
        log!("testmenu: call at +{at:#x} returns to the game");
    }
}

/// Leaving the test menu: the game again.
extern "C" fn back_to_game() -> u32 {
    log!("testmenu: test menu exited, back to the game");
    leave("");
    0
}

/// TEST stays released until it has been seen released (held from the game's press).
static TEST_ARMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Gun axis, -32768..32767, as the board's 0-255 (0 left / top).
fn gun(stick: &StickState, axis: Axis) -> u8 {
    ((stick.axis(axis) as i32 + 32768) >> 8) as u8
}

/// The Shell's I/O board report (15 bytes, from its HID board): byte 1/2 P1 gun Y/X, 3/4 P2
/// gun Y/X, 6/7 buttons 1-16 (bit `n - 1`): 1/2 start, 3/4 trigger, 5/6 lever down/up,
/// 13 TEST, 14 SERVICE. And its buttons as it reads them (from the board's and the keyboard's):
/// button `n` bit `n` of 3 bytes.
unsafe fn report(buf: *mut u8, buttons: *mut u8) {
    use std::sync::atomic::Ordering;
    let sticks = [0, 1].map(wal_payload_common::input);
    let any = |b: u32| sticks.iter().any(|s| s.pressed(b));
    let test = any(button::TEST);
    if !test {
        TEST_ARMED.store(true, Ordering::Relaxed);
    }
    let pressed = [
        sticks[0].pressed(button::START),
        sticks[1].pressed(button::START),
        sticks[0].pressed(button::B1),
        sticks[1].pressed(button::B1),
        any(button::DOWN),
        any(button::UP),
        false,
        false,
        false,
        false,
        false,
        false,
        test && TEST_ARMED.load(Ordering::Relaxed),
        any(button::SERVICE),
    ];
    let bits = pressed.iter().enumerate().fold(0u16, |b, (i, on)| b | (u16::from(*on) << i));
    let mut r = [0u8; 15];
    r[1] = gun(&sticks[0], Axis::LeftY);
    r[2] = gun(&sticks[0], Axis::LeftX);
    r[3] = gun(&sticks[1], Axis::LeftY);
    r[4] = gun(&sticks[1], Axis::LeftX);
    r[6..8].copy_from_slice(&bits.to_le_bytes());
    unsafe {
        std::ptr::copy_nonoverlapping(r.as_ptr(), buf, r.len());
        std::ptr::copy_nonoverlapping((u32::from(bits) << 1).to_le_bytes().as_ptr(), buttons, 3);
    }
}

/// Retargets the `call rel32` at `at` to `target`.
unsafe fn call_to(at: usize, target: usize) {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    unsafe {
        if *(at as *const u8) != 0xE8 {
            log!("testmenu: no call at {at:#x}");
            return;
        }
        let mut old = 0;
        VirtualProtect(at as *const _, 5, PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::write_unaligned((at + 1) as *mut i32, target.wrapping_sub(at + 5) as i32);
        VirtualProtect(at as *const _, 5, old, &mut old);
    }
}
