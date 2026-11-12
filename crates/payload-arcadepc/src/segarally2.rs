//! Sega Rally 2 (`SEGA RALLY 2.EXE`, the 25th Anniversary Edition repack): the game's
//! libraries are MUSASHI COM servers (`MUSASHI\*.dll`, created by their CLSIDs).
//!
//! Inputs: `MUSASHI\MGInput.dll` reads the controllers through DirectInput
//! (`DirectInputCreateA`, force feedback effects on a wheel): the common fake DirectInput wheel
//! (`WAL_DINPUT_WHEEL`), its constant force sent to the launcher (`WAL_DINPUT_WHEEL_FFB_OUT`).
//! COM loads the library when the game creates its input object, past the payload's hooks of
//! the game's modules: loaded here first (COM gets the same module), its imports hooked.
//!
//! The game also reads the DirectInput keyboard (Enter starts, pauses, leaves the demo; Esc goes
//! back in the menus; without a keyboard device it crashes at the name entry, its keyboard state
//! null): the fake one's (`WAL_DINPUT_WHEEL_SYSTEM`), START as Enter, b5 as Esc.
//!
//! The aspect ratio is the profile's: the repack's widescreen fix (`wsock32.dll`) reads its
//! `SegaRally2Resolution.cfg` when the process starts, before the payload.

use wal_payload_common::{dinput, input, log};
use wal_protocol::button::{B5, START};
use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;

const DIK_ESCAPE: usize = 0x01;
const DIK_RETURN: usize = 0x1C;

/// The DirectInput keyboard's keys: player 1's START (Enter) and b5 (Esc).
fn keyboard(keys: &mut [u8; 256]) {
    let p1 = input(0);
    if p1.pressed(START) {
        keys[DIK_RETURN] = 0x80;
    }
    if p1.pressed(B5) {
        keys[DIK_ESCAPE] = 0x80;
    }
}

pub fn init() {
    dinput::set_keyboard(keyboard);
    let module = unsafe { LoadLibraryW(windows_sys::w!("MUSASHI\\MGInput.dll")) } as usize;
    if module == 0 {
        log!("segarally2: MUSASHI\\MGInput.dll not loaded: the game's own DirectInput");
        return;
    }
    dinput::hook_module(module);
    log!("segarally2: MGInput.dll's DirectInput hooked");
}
