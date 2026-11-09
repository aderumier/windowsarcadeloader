//! The gun sensor board of House of the Dead: Scarlet Dawn on COM1 (38400 bauds): both players'
//! guns, read by the game itself (`FGunSensor`), not through amdaemon.
//!
//! The game sends one byte commands, the board answers:
//! - `04` reset: `xx !xx` (a byte, its complement)
//! - `FD` / `FE` text settings (the player count `S P2\r\n`, the sensor frequency): `75 8A`
//! - `08` the guns' state, read at the next frame: 11 bytes, P1 x, P1 y, P2 x, P2 y (big endian,
//!   10 bits: x from the left, y from the bottom), a flags byte (bit 5 P1 trigger, bit 4 P2
//!   trigger, bit 7 P1 off the screen, bit 6 P2 off the screen), a spare byte, the complement of
//!   the sum of the other bytes.
//!
//! From the virtual sticks: the gun aims (`lx`, `ly`), b1 is the trigger, b2 points the gun off
//! the screen (the game's reload), as does a gun leaving the picture.
//!
//! The game maps the readings to the screen with each gun's calibration (the operator's, else
//! the factory's: targets inset from the screen's edges) and a screen size (the cabinet's
//! 1920x1080 and edge margins, set once): [`aim_fields`] makes both exact at each update, the
//! reading 0..1024 across the picture: the menus' canvas (1920 wide) or the 3D view's real size
//! in game (a lower resolution).

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{gun, input, log, serial};
use wal_protocol::button as b;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

const MAX: f32 = 1024.0;

fn checksum(data: &[u8]) -> u8 {
    !data.iter().fold(0u8, |s, b| s.wrapping_add(*b))
}

/// The guns' state (command 08).
fn state() -> Vec<u8> {
    let mut out = Vec::with_capacity(11);
    let mut flags = 0u8;
    for p in 0..2 {
        let buttons = input(p).buttons;
        let aim = gun::gun_of_player_in_picture(p).filter(|_| buttons & b::B2 == 0);
        // y from the bottom
        let (x, y) = aim.map_or((0, 0), |(x, y)| (((x.clamp(0.0, 1.0) * MAX) as u16).min(1023), 1023 - ((y.clamp(0.0, 1.0) * MAX) as u16).min(1023)));
        out.extend_from_slice(&x.to_be_bytes());
        out.extend_from_slice(&y.to_be_bytes());
        if aim.is_none() {
            flags |= 0x80 >> p;
        }
        if buttons & b::B1 != 0 {
            flags |= 0x20 >> p;
        }
    }
    out.extend([flags, 0]);
    out.push(checksum(&out));
    out
}

fn packet(data: &[u8]) -> Vec<u8> {
    let Some(&cmd) = data.first() else { return Vec::new() };
    match cmd {
        0x08 => state(),
        0x04 => vec![0x00, 0xFF],
        _ => {
            log!("gunsensor: command {:02x?}", &data[..data.len().min(16)]);
            vec![0x75, 0x8A]
        }
    }
}

/// The aim's update (`AAppPlayerController`, `this` in rdi): its first two loads of the screen
/// size and calibration, replaced by a jump to [`aim_stub`].
const AIM_SITE: usize = 0x1ded89;
/// `movd xmm8, [rdi+0xd4c]; movd xmm0, [rdi+0xd0c]`
const AIM_CODE: [u8; 17] = [0x66, 0x44, 0x0f, 0x6e, 0x87, 0x4c, 0x0d, 0x00, 0x00, 0x66, 0x0f, 0x6e, 0x87, 0x0c, 0x0d, 0x00, 0x00];
/// The game's state (`[+0x231 + player]`: playing), set when a game starts.
const GAME_STATE: usize = 0x3087750;

/// Where [`aim_stub`] goes back to.
#[unsafe(no_mangle)]
static WAL_HOTD_AIM_RETURN: AtomicUsize = AtomicUsize::new(0);

// The volatile registers kept (ecx: player 2's y reading, stored after the site), the stack
// aligned for the call, then the replaced instructions.
core::arch::global_asm!(
    ".globl {stub}",
    "{stub}:",
    "push rax",
    "push rcx",
    "push rdx",
    "push r8",
    "push r9",
    "push r10",
    "push r11",
    "push rbp",
    "mov rbp, rsp",
    "and rsp, -16",
    "sub rsp, 0x20",
    "mov rcx, rdi",
    "call {fields}",
    "mov rsp, rbp",
    "pop rbp",
    "pop r11",
    "pop r10",
    "pop r9",
    "pop r8",
    "pop rdx",
    "pop rcx",
    "pop rax",
    "movd xmm8, dword ptr [rdi + 0xd4c]",
    "movd xmm0, dword ptr [rdi + 0xd0c]",
    "jmp qword ptr [rip + WAL_HOTD_AIM_RETURN]",
    stub = sym aim_stub,
    fields = sym aim_fields,
);

unsafe extern "C" {
    fn aim_stub();
}

/// Identity calibrations (both guns: 0 at the left / top, 1024 at the right / bottom, no edge
/// margin) and the screen size the readings span.
extern "C" fn aim_fields(this: *mut u8) {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let state = unsafe { ((base + GAME_STATE) as *const usize).read() };
    let playing = state != 0 && unsafe { ((state + 0x231) as *const [u8; 2]).read() } != [0, 0];
    // the 3D view's pixels in game; the menus' canvas: 1920 wide, the view's aspect
    let view = gun::client_area(gun::find_window()).filter(|&(_, _, w, h)| w > 0.0 && h > 0.0);
    let (w, h) = match view {
        Some((_, _, w, h)) if playing => (w as i32, h as i32),
        Some((_, _, w, h)) => (1920, (1920.0 * h / w) as i32),
        None => (1920, 1080),
    };
    static LAST: AtomicUsize = AtomicUsize::new(0);
    let key = ((w as usize) << 16 << 16 | (h as usize) << 1) ^ playing as usize;
    if LAST.swap(key, Ordering::Relaxed) != key {
        log!("gunsensor: aim over {w}x{h} ({})", if playing { "game" } else { "menus" });
    }
    let fields: [(usize, i32); 16] = [
        // player 1: x low, high, center; y low, high, center
        (0xd04, 0), (0xd0c, 0x400), (0xd14, 0x200), (0xcf8, 0), (0xd00, 0x400), (0xd18, 0x200),
        // player 2
        (0xd2c, 0), (0xd34, 0x400), (0xd3c, 0x200), (0xd20, 0), (0xd28, 0x400), (0xd40, 0x200),
        // the margins (float), the screen
        (0xd44, 0), (0xd48, 0), (0xd4c, w), (0xd50, h),
    ];
    for (offset, value) in fields {
        unsafe { (this.add(offset) as *mut i32).write_unaligned(value) };
    }
}

fn hook_aim() {
    let at = unsafe { GetModuleHandleW(std::ptr::null()) } as usize + AIM_SITE;
    let code = unsafe { std::slice::from_raw_parts_mut(at as *mut u8, AIM_CODE.len()) };
    if code != AIM_CODE {
        log!("gunsensor: aim code not found (another version): the game's calibration kept");
        return;
    }
    WAL_HOTD_AIM_RETURN.store(at + AIM_CODE.len(), Ordering::Relaxed);
    // jmp [rip+0]; the stub's address; nop
    let mut jump = [0x90u8; 17];
    jump[..6].copy_from_slice(&[0xFF, 0x25, 0, 0, 0, 0]);
    jump[6..14].copy_from_slice(&(aim_stub as *const () as u64).to_le_bytes());
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, jump.len(), PAGE_EXECUTE_READWRITE, &mut old);
        code.copy_from_slice(&jump);
        VirtualProtect(at as *const c_void, jump.len(), old, &mut old);
    }
    log!("gunsensor: aim calibration hooked");
}

pub(crate) fn init() {
    serial::install("COM1", packet);
    hook_aim();
}
