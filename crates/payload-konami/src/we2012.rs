//! Winning Eleven 2012 (WORLD SOCCER Winning Eleven ARCADE CHAMPIONSHIP 2012, game code KCK):
//! the game is `weac12_bootstrap_release.dll` (exports `dll_entry_init` / `dll_entry_main`), run
//! by `wal-avs-boot.exe` from `contents/` (its DLLs in `contents/modules/`). The payload is its
//! `libacio.dll`: the PJEI board (arcade stick, buttons A-F, coins, security plug, `pjei.rs`),
//! the PJEC PS2 pad port (`pjec.rs`), the I36I PS3 pad port (`i36i.rs`), the card reader; its
//! `libtouchpanel.dll` imports are pointed at this DLL's functions of the same names
//! (`touchpanel.rs`), as for Winning Eleven 2008.
//!
//! The cabinet runs the game from `D:\KCK-00<n>-<version>\contents`: the product code in that
//! path is the cabinet's region (`KCK-001` Japan, `KCK-002` Asia, `KCK-003` China) and picks
//! hundreds of things: the text and data (`data/convert/ac/j`, `english` or `ctrad`, `csimpl`),
//! the attract movies (from a directory without a code none is picked: the first movie after
//! the attract's title crashes, a null name), the touch screen, the LCD monitor. Its licence
//! check compares it with the destination of the soft id code (`J`, `A`, `C`): both come from
//! `WAL_KONAMI_DEST` (wal-avs-boot sets the destination), the game's check (rva 0xedfef0: the
//! current directory searched for the code) answers for that region's code.
//!
//! Touch screen: player 1's right stick over the screen (the profile's mouse or gun drives
//! `rx` / `ry`) and virtual button 7 (not the PS2 pad's L2 then).
//! * Asia, China: a serial touch panel (`ac_touchpanel_new2(640, 480)`), over the whole picture.
//! * Japan: a USB touch monitor (an Elo, seen by Windows as a mouse; the boot looks for its
//!   device, TOUCH PANEL ERROR 5-1513-0000 without it): the game reads `GetCursorPos` and its
//!   window's `WM_LBUTTONDOWN` / `WM_LBUTTONUP` (byte 1, VK_LBUTTON, of its key table). The PC's
//!   own mouse messages are not dispatched to the game. Its LCD monitor is on COM1.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use wal_protocol::{Axis, button};
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, MSG, SM_CXSCREEN, SM_CYSCREEN};

/// The virtual button touching the screen.
const TOUCH: u32 = button::B7;

/// weac12_bootstrap_release.dll's base, when this process runs it.
pub fn game_module() -> usize {
    let name: Vec<u16> = "weac12_bootstrap_release.dll\0".encode_utf16().collect();
    unsafe { GetModuleHandleW(name.as_ptr()) as usize }
}

pub fn init(module: *mut c_void) {
    // the serial touch panel (Asia, China): the touch over the whole picture
    crate::touchpanel::set_touch_button(TOUCH);
    crate::touchpanel::set_full_screen(true);
    // the touch button is not the PS2 pad's L2
    crate::pjec::reserve(TOUCH);
    let mut hooked = 0;
    for (name, address) in crate::otomedius::exports(module) {
        if name.starts_with("ac_touchpanel_") && unsafe { iat::hook("libtouchpanel.dll", &name, address) }.is_some() {
            hooked += 1;
        }
    }
    log!("we2012: {hooked} libtouchpanel imports answered by the payload");
    crate::net::adapters_fit();
    product_check_init();
    // Japan: the LCD monitor; the touch screen's USB device (its description among the PC's
    // devices, rva 0xedb1a0) present: sub esp, 0x30; mov eax, [__security_cookie]
    wal_payload_common::serial::install_in("COM1", lcd, &[game_module()]);
    if !patch(0xedb1a0, &[0x83, 0xec, 0x30, 0xa1], &[0xb0, 0x01, 0xc3]) {
        log!("we2012: touch screen device check differs, kept");
    }
    touch_init();
    controller_select_init();
    // the network notice (rva 0xfe5a00, drawn while its id is not 0): not for id 1, "Operating
    // in Local Mode" (e-Amusement offline, always here): cmp [ebx+0x68], 1; jbe (was 0; je)
    if !patch(0xfe5a16, &[0x83, 0x7b, 0x68, 0x00, 0x0f, 0x84], &[0x83, 0x7b, 0x68, 0x01, 0x0f, 0x86]) {
        log!("we2012: network notice code differs, the local mode notice stays");
    }
}

/// The game's left mouse button state (its key table's VK_LBUTTON byte).
static TOUCH_FLAG: AtomicUsize = AtomicUsize::new(0);
static ORIG_DISPATCH: AtomicUsize = AtomicUsize::new(0);

/// The touch position: player 1's right stick over the screen (the game's window fills it).
unsafe extern "system" fn get_cursor_pos(point: *mut POINT) -> i32 {
    let Some(point) = (unsafe { point.as_mut() }) else { return 0 };
    let stick = wal_payload_common::input(0);
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let at = |v: i16, size: i32| (((v as i32 + 32768) as i64 * size as i64 / 65536) as i32).clamp(0, size - 1);
    point.x = at(stick.axis(Axis::RightX), w);
    point.y = at(stick.axis(Axis::RightY), h);
    1
}

/// The PC's mouse messages (WM_MOUSEMOVE to WM_MOUSEHWHEEL) are not the game's: dropped.
unsafe extern "system" fn dispatch_message(msg: *const MSG) -> isize {
    if let Some(m) = unsafe { msg.as_ref() }
        && (0x200..=0x20e).contains(&m.message)
    {
        return 0;
    }
    let original: unsafe extern "system" fn(*const MSG) -> isize = unsafe { std::mem::transmute(ORIG_DISPATCH.load(Ordering::Relaxed)) };
    unsafe { original(msg) }
}

/// Button 7 down / up sets the game's touch flag, as its window procedure does on
/// WM_LBUTTONDOWN / WM_LBUTTONUP.
fn touch_thread() {
    let flag = TOUCH_FLAG.load(Ordering::Relaxed) as *mut u8;
    let mut held = false;
    loop {
        let pressed = wal_payload_common::input(0).pressed(TOUCH);
        if pressed != held {
            held = pressed;
            unsafe { std::ptr::write_volatile(flag, pressed as u8) };
        }
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
}

fn touch_init() {
    // the window procedure's WM_LBUTTONDOWN / UP: sete cl; mov [VK_LBUTTON's byte], cl
    let at = game_module() + 0xed561f;
    let code = unsafe { std::slice::from_raw_parts(at as *const u8, 5) };
    if code != [0x0f, 0x94, 0xc1, 0x88, 0x0d] {
        log!("we2012: mouse button code differs, no touch");
        return;
    }
    TOUCH_FLAG.store(unsafe { ((at + 5) as *const u32).read_unaligned() } as usize, Ordering::Relaxed);
    unsafe { iat::hook("user32.dll", "GetCursorPos", get_cursor_pos as *const () as usize) };
    match unsafe { iat::hook("user32.dll", "DispatchMessageA", dispatch_message as *const () as usize) } {
        Some(o) => ORIG_DISPATCH.store(o, Ordering::Relaxed),
        None => log!("we2012: DispatchMessageA import not found, the PC's mouse reaches the game"),
    }
    std::thread::spawn(touch_thread);
}

/// The LCD monitor on COM1 (its picture settings, set from the test menu): a command
/// `0<name> <value>\r\n` (`MODEL?`, `INFO`, `BRI`, `CON`, `BL`, `CSM`, `RED`, `GREEN`, `BLUE`)
/// is echoed `9<name> <value>\r\n`, then `9OK \r\n` (spice2x's answer). Without it the boot
/// stops on LCD MONITOR ERROR 5-1514-0000.
pub(crate) fn lcd(packet: &[u8]) -> Vec<u8> {
    let Some(command) = packet.strip_prefix(b"0") else { return Vec::new() };
    let command = String::from_utf8_lossy(command);
    let command = command.trim_end_matches(['\r', '\n']);
    let (name, value) = command.split_once(' ').unwrap_or((command, ""));
    format!("9{name} {value}\r\n9OK \r\n").into_bytes()
}

/// Writes `code` at the game's `rva` when it starts with `head`.
fn patch(rva: usize, head: &[u8], code: &[u8]) -> bool {
    let at = game_module() + rva;
    if unsafe { std::slice::from_raw_parts(at as *const u8, head.len()) } != head {
        return false;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, code.len());
        VirtualProtect(at as *const c_void, code.len(), old, &mut old);
    }
    true
}

/// The cabinet's product code, from the destination `WAL_KONAMI_DEST` (`J` Japan `KCK-001`,
/// `A` Asia `KCK-002`, `C` China `KCK-003`).
fn product() -> &'static [u16] {
    static PRODUCT: OnceLock<Vec<u16>> = OnceLock::new();
    PRODUCT.get_or_init(|| {
        let code = match std::env::var("WAL_KONAMI_DEST").unwrap_or_default().trim() {
            "A" => "KCK-002",
            "C" => "KCK-003",
            _ => "KCK-001",
        };
        log!("we2012: product code {code}");
        code.encode_utf16().collect()
    })
}

/// Whether the product code contains `needle` (a wide string), as the game searches its
/// directory for it.
extern "C" fn product_is(needle: *const u16) -> bool {
    if needle.is_null() {
        return false;
    }
    let len = (0..).take_while(|&i| unsafe { *needle.add(i) } != 0).count();
    let needle = unsafe { std::slice::from_raw_parts(needle, len) };
    len == 0 || product().windows(len).any(|w| w == needle)
}

// The game's check: the code (wide) in esi, the answer in al.
core::arch::global_asm!(
    ".globl {name}\n{name}:",
    "push esi",
    "call {check}",
    "add esp, 4",
    "ret",
    name = sym product_check,
    check = sym product_is,
);
unsafe extern "C" {
    fn product_check();
}

/// The product check (rva 0xedfef0: `GetCurrentDirectoryA` once, then the code searched in it)
/// jumps to `product_check`.
fn product_check_init() {
    let at = game_module() + 0xedfef0;
    // sub esp, 0x108; ...; mov eax, 1; test [the "directory read" flag], al
    let head: [u8; 6] = [0x81, 0xec, 0x08, 0x01, 0x00, 0x00];
    let flag: [u8; 7] = [0xb8, 0x01, 0x00, 0x00, 0x00, 0x84, 0x05];
    let code = |offset: usize, len: usize| unsafe { std::slice::from_raw_parts((at + offset) as *const u8, len) };
    if code(0, 6) != head || code(0x14, 7) != flag {
        log!("we2012: product code check differs, the game's directory decides (attract crash outside KCK-00x)");
        return;
    }
    product();
    let rel = (product_check as *const () as usize).wrapping_sub(at + 5) as i32;
    let mut jump = vec![0xe9];
    jump.extend_from_slice(&rel.to_le_bytes());
    patch(0xedfef0, &head, &jump);
}


/// Frames of the controller choice scene (AcSceneControllerTypeSelect_2012, vtable rva
/// 0x14d90ac: its slot 5 runs once a frame while it is on screen).
static CONTROLLER_FRAMES: AtomicUsize = AtomicUsize::new(0);
static CONTROLLER_UPDATE: AtomicUsize = AtomicUsize::new(0);

core::arch::global_asm!(
    ".globl {name}\n{name}:",
    "lock inc dword ptr [{frames}]",
    "jmp dword ptr [{update}]",
    name = sym controller_update,
    frames = sym CONTROLLER_FRAMES,
    update = sym CONTROLLER_UPDATE,
);
unsafe extern "C" {
    fn controller_update();
}

/// The controller choice before a match: touch buttons, the arcade controller (left) and the
/// home one (right), until its timer runs out. While the scene is on screen the payload touches
/// the arcade controller's button (every 2/3 s, 0.1 s each): taken as if the player had.
fn controller_select_init() {
    let slot = (game_module() + 0x14d90ac + 5 * 4) as *mut usize;
    // the scene's update (rva 0xfc3e60: push ecx; cmp dword ptr [...], 0)
    let update = game_module() + 0xfc3e60;
    if unsafe { *slot } != update || unsafe { std::slice::from_raw_parts(update as *const u8, 3) } != [0x51, 0x83, 0x3d] {
        log!("we2012: controller choice code differs, chosen by touch or timer");
        return;
    }
    CONTROLLER_UPDATE.store(update, Ordering::Relaxed);
    let mut old = 0;
    unsafe {
        VirtualProtect(slot.cast(), 4, PAGE_EXECUTE_READWRITE, &mut old);
        *slot = controller_update as *const () as usize;
        VirtualProtect(slot.cast(), 4, old, &mut old);
    }
    std::thread::spawn(|| {
        // the arcade controller's button, in fractions of the picture
        const BUTTON: (f32, f32) = (0.351, 0.5);
        let mut seen = CONTROLLER_FRAMES.load(Ordering::Relaxed);
        let mut start: Option<usize> = None;
        let mut idle = 0;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(16));
            let frames = CONTROLLER_FRAMES.load(Ordering::Relaxed);
            if frames != seen {
                seen = frames;
                idle = 0;
                let first = *start.get_or_insert(frames);
                let n = frames - first;
                let down = n % 40 >= 20 && n % 40 < 26;
                crate::touchpanel::set_auto_touch(Some((BUTTON.0, BUTTON.1, down)));
            } else if start.is_some() {
                idle += 1;
                if idle > 15 {
                    start = None;
                    crate::touchpanel::set_auto_touch(None);
                }
            }
        }
    });
}
