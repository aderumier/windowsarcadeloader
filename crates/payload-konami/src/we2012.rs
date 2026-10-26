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
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect};
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
    // the game calibrates the panel in screen pixels (its targets 128 pixels from the corners)
    crate::touchpanel::set_screen_calibration(true);
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
    // the touch pointer shown (WAL_CROSSHAIR)
    crate::crosshair::init();
    controller_select_init();
    network_on_init();
    pin_keypad_init();
    player_data_defaults_init();
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

/// The network options (`/dev/nvram/networkopt.xml`, the test menu's NETWORK OPTIONS) as the
/// game reads them (rva 0xf03c70: loaded once, 132 bytes): their first byte turns the network
/// on (the dump's: all 0, "Operating in Local Mode": no card, no e-Amusement). With an
/// e-Amusement server (`WAL_KONAMI_EASRV=1`, `WAL_KONAMI_SERVICES`) it is on.
const NETWORK_OPTIONS: usize = 0xf03c70;
/// The getter's first instruction (`sub esp, 0x88`), moved to its trampoline.
const NETWORK_OPTIONS_HEAD: [u8; 6] = [0x81, 0xec, 0x88, 0x00, 0x00, 0x00];
static NETWORK_OPTIONS_TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);

extern "C" fn network_options() -> *mut u8 {
    let original: extern "C" fn() -> *mut u8 = unsafe { std::mem::transmute(NETWORK_OPTIONS_TRAMPOLINE.load(Ordering::Relaxed)) };
    let options = original();
    if !options.is_null() {
        unsafe { *options = 1 };
    }
    options
}

fn network_on_init() {
    let server = std::env::var("WAL_KONAMI_EASRV").is_ok_and(|v| v.trim() == "1") || std::env::var("WAL_KONAMI_SERVICES").is_ok_and(|v| !v.trim().is_empty());
    if !server {
        return;
    }
    let at = game_module() + NETWORK_OPTIONS;
    if unsafe { std::slice::from_raw_parts(at as *const u8, 6) } != NETWORK_OPTIONS_HEAD {
        log!("we2012: network options code differs, the test menu's setting kept");
        return;
    }
    let stub = unsafe { VirtualAlloc(std::ptr::null(), 16, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) } as usize;
    if stub == 0 {
        return;
    }
    // the moved instruction, then a jump back past it
    let mut code = NETWORK_OPTIONS_HEAD.to_vec();
    code.push(0xe9);
    code.extend_from_slice(&((at + 6).wrapping_sub(stub + 11) as i32).to_le_bytes());
    unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), stub as *mut u8, code.len()) };
    NETWORK_OPTIONS_TRAMPOLINE.store(stub, Ordering::Relaxed);
    let mut jump = vec![0xe9];
    jump.extend_from_slice(&((network_options as *const () as usize).wrapping_sub(at + 5) as i32).to_le_bytes());
    jump.push(0x90);
    patch(NETWORK_OPTIONS, &NETWORK_OPTIONS_HEAD, &jump);
    log!("we2012: network on (e-Amusement server)");
}

/// The PIN screens (AcSceneIcCardConfirmPassword_2012, a card's PIN, and
/// AcSceneIcCardRegistPassword_2012, a new card's, typed twice: vtables rva 0x14d9644,
/// 0x14d993c): their slot 5 runs once a frame while they are on screen, the payload's
/// `pin_frame` first.
static CONFIRM_UPDATE: AtomicUsize = AtomicUsize::new(0);
static REGIST_UPDATE: AtomicUsize = AtomicUsize::new(0);

core::arch::global_asm!(
    ".globl {confirm}\n{confirm}:",
    "push ecx",
    "push ecx",
    "call {frame}",
    "add esp, 4",
    "pop ecx",
    "jmp dword ptr [{confirm_update}]",
    ".globl {regist}\n{regist}:",
    "push ecx",
    "push ecx",
    "call {frame}",
    "add esp, 4",
    "pop ecx",
    "jmp dword ptr [{regist_update}]",
    confirm = sym confirm_password_update,
    regist = sym regist_password_update,
    frame = sym pin_frame,
    confirm_update = sym CONFIRM_UPDATE,
    regist_update = sym REGIST_UPDATE,
);
unsafe extern "C" {
    fn confirm_password_update();
    fn regist_password_update();
}

/// The keypad's key press (rva 0xfb35a0: the scene in esi, the key in edi, 0-9 the digit keys):
/// the key lit, its digit added to the PIN (the keypad shuffles its digits: one of 14 patterns,
/// the scene's +0x234, each key's digit a string of the game's table, rva 0x172b718).
const KEY_PRESS: usize = 0xfb35a0;
/// The scene's count of the PIN's digits typed.
const DIGITS_TYPED: usize = 0x1d8;
/// A digit typed this long after the screen comes up, then after the previous one.
const PIN_START: std::time::Duration = std::time::Duration::from_millis(1000);
const PIN_DIGIT: std::time::Duration = std::time::Duration::from_millis(400);

/// The key showing `digit` on the PIN screen `scene`.
fn pin_digit_key(scene: usize, digit: usize) -> Option<usize> {
    let pattern = unsafe { *((scene + 0x234) as *const i32) };
    if !(0..14).contains(&pattern) {
        return None;
    }
    let table = (game_module() + 0x172b718) as *const *const u8;
    (0..10).find(|&k| unsafe { *(*table.add(pattern as usize * 10 + k)) } == b'0' + digit as u8)
}

/// The digit's commit (rva 0xfb3360: the scene in edi): one more of the PIN's circles filled.
const DIGIT_COMMIT: usize = 0xfb3360;

/// A digit key touched, as the keypad's touch handler does: the key press, then the commit.
fn press_key(scene: usize, key: usize) {
    let functions = [game_module() + KEY_PRESS, game_module() + DIGIT_COMMIT];
    unsafe {
        std::arch::asm!(
            "push esi",
            "push edi",
            "push ebx",
            "mov ebx, ecx",
            "push eax",
            "mov esi, eax",
            "mov edi, edx",
            "call dword ptr [ebx]",
            "mov edi, dword ptr [esp]",
            "call dword ptr [ebx + 4]",
            "pop eax",
            "pop ebx",
            "pop edi",
            "pop esi",
            in("eax") scene,
            in("edx") key,
            in("ecx") functions.as_ptr(),
            clobber_abi("C"),
        );
    }
}

/// The configured card's PIN (`WAL_KONAMI_CARD_PIN`, 4 digits).
fn configured_pin() -> Option<&'static [usize]> {
    static PIN: OnceLock<Option<Vec<usize>>> = OnceLock::new();
    PIN.get_or_init(|| {
        let pin = std::env::var("WAL_KONAMI_CARD_PIN").ok()?;
        let digits: Vec<usize> = pin.trim().bytes().filter(u8::is_ascii_digit).map(|b| (b - b'0') as usize).collect();
        (digits.len() == 4).then_some(digits)
    })
    .as_deref()
}

/// A frame of a PIN screen (the game's thread): the configured card's PIN typed on its keypad,
/// a digit every `PIN_DIGIT` (the new card's confirmation too: the count starts again), else
/// the numpad's digits (`kp0`..`kp9`) as they are pressed.
extern "C" fn pin_frame(scene: usize) {
    struct State {
        scene: usize,
        since: std::time::Instant,
        last: std::time::Instant,
        pressed: Option<std::time::Instant>,
        held: Option<usize>,
    }
    static STATE: std::sync::Mutex<Option<State>> = std::sync::Mutex::new(None);
    let now = std::time::Instant::now();
    let mut state = STATE.lock().unwrap();
    // a new screen (another object, or the previous one left a while ago)
    if !state.as_ref().is_some_and(|s| s.scene == scene && now.duration_since(s.last) < std::time::Duration::from_secs(1)) {
        *state = Some(State { scene, since: now, last: now, pressed: None, held: None });
    }
    let s = state.as_mut().unwrap();
    s.last = now;
    let typed = unsafe { *((scene + DIGITS_TYPED) as *const i32) };
    if !(0..4).contains(&typed) {
        return;
    }
    let digit = match configured_pin() {
        Some(pin) => {
            if now.duration_since(s.since) < PIN_START || s.pressed.is_some_and(|t| now.duration_since(t) < PIN_DIGIT) {
                return;
            }
            Some(pin[typed as usize])
        }
        None => {
            let key = crate::icca::keypad_digit(0).filter(|&d| d < 10);
            let pressed = key.filter(|_| s.held != key);
            s.held = key;
            pressed
        }
    };
    let Some(key) = digit.and_then(|d| pin_digit_key(scene, d)) else { return };
    s.pressed = Some(now);
    press_key(scene, key);
}

/// Hooks the PIN screens' frames.
fn pin_keypad_init() {
    let hooks = [(0x14d9644 + 5 * 4, 0xfb48c0, &CONFIRM_UPDATE, confirm_password_update as *const () as usize), (0x14d993c + 5 * 4, 0xfaf770, &REGIST_UPDATE, regist_password_update as *const () as usize)];
    for (slot, update, original, ours) in hooks {
        let slot = (game_module() + slot) as *mut usize;
        let update = game_module() + update;
        if unsafe { *slot } != update {
            log!("we2012: PIN screen code differs, the PIN is touched by the player");
            return;
        }
        original.store(update, Ordering::Relaxed);
        let mut old = 0;
        unsafe {
            VirtualProtect(slot.cast(), 4, PAGE_EXECUTE_READWRITE, &mut old);
            *slot = ours;
            VirtualProtect(slot.cast(), 4, old, &mut old);
        }
    }
}

/// The card's login (AcCardEntryLoadUserDataSeq_2012, vtable rva 0x14db054: its slot 8, rva
/// 0xf51920, runs it a frame at a time): the answers it reads (`userdata.load`, `takeover`,
/// `userenv.get`) hold some 240 values and lists each, each missing one an error (the game then
/// ends). A new player, or one saved by an older version, has only some of them: while that
/// sequence runs, a missing value reads 0 (the game's own default for a new player, a date its
/// release) and a missing group (or list entry) is the answer's empty group (`empty`, the
/// server's), through the game's libavs imports (`property_search`, `property_node_refer`).
const LOGIN_SEQ_SLOT: usize = 0x14db054 + 8 * 4;
const LOGIN_SEQ_UPDATE: usize = 0xf51920;
static IN_LOGIN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static ORIG_SEARCH: AtomicUsize = AtomicUsize::new(0);
static ORIG_REFER: AtomicUsize = AtomicUsize::new(0);
static LOGIN_UPDATE: AtomicUsize = AtomicUsize::new(0);
/// The answer's root (the node of the first search in a frame of the login), for its empty group.
static ANSWER_ROOT: AtomicUsize = AtomicUsize::new(0);

core::arch::global_asm!(
    ".globl {stub}\n{stub}:",
    "push ecx",
    "call {enter}",
    "pop ecx",
    "call dword ptr [{update}]",
    "push eax",
    "call {leave}",
    "pop eax",
    "ret",
    stub = sym login_update,
    enter = sym login_enter,
    leave = sym login_leave,
    update = sym LOGIN_UPDATE,
);
unsafe extern "C" {
    fn login_update();
}

extern "C" fn login_enter() {
    ANSWER_ROOT.store(0, Ordering::Relaxed);
    IN_LOGIN.store(true, Ordering::Relaxed);
}

extern "C" fn login_leave() {
    IN_LOGIN.store(false, Ordering::Relaxed);
}

/// The cabinet's environment (`userenv.get`, its answer parsed at rva 0x10bd150, called from
/// rva 0xeaf6d8 only): asked again after a game too, outside the login; the same defaults.
const USERENV_PARSE: usize = 0x10bd150;
const USERENV_PARSE_CALL: usize = 0xeaf6d8;

extern "stdcall" fn userenv_parse(answer: usize, out: usize) -> u8 {
    let parse: extern "stdcall" fn(usize, usize) -> u8 = unsafe { std::mem::transmute(game_module() + USERENV_PARSE) };
    let inside = IN_LOGIN.swap(true, Ordering::Relaxed);
    if !inside {
        ANSWER_ROOT.store(0, Ordering::Relaxed);
    }
    let ok = parse(answer, out);
    IN_LOGIN.store(inside, Ordering::Relaxed);
    ok
}

/// property_search: a group missing from an answer of the login answered by its empty group:
/// nothing in it, its values read 0.
extern "C" fn property_search(prop: *mut c_void, node: *mut c_void, path: *const u8) -> *mut c_void {
    let search: extern "C" fn(*mut c_void, *mut c_void, *const u8) -> *mut c_void = unsafe { std::mem::transmute(ORIG_SEARCH.load(Ordering::Relaxed)) };
    let found = search(prop, node, path);
    if !IN_LOGIN.load(Ordering::Relaxed) || node.is_null() {
        return found;
    }
    if ANSWER_ROOT.load(Ordering::Relaxed) == 0 {
        ANSWER_ROOT.store(node as usize, Ordering::Relaxed);
    }
    if !found.is_null() {
        return found;
    }
    let empty = c"empty".as_ptr().cast();
    let beside = search(prop, node, empty);
    if !beside.is_null() {
        return beside;
    }
    search(prop, ANSWER_ROOT.load(Ordering::Relaxed) as *mut c_void, empty)
}

/// property_node_refer: a value missing from an answer of the login read as 0 (a date, the
/// game's release).
extern "C" fn property_node_refer(prop: *mut c_void, node: *mut c_void, name: *const u8, kind: u32, out: *mut u8, size: u32) -> i32 {
    let refer: extern "C" fn(*mut c_void, *mut c_void, *const u8, u32, *mut u8, u32) -> i32 = unsafe { std::mem::transmute(ORIG_REFER.load(Ordering::Relaxed)) };
    let r = refer(prop, node, name, kind, out, size);
    if r >= 0 || !IN_LOGIN.load(Ordering::Relaxed) || out.is_null() {
        return r;
    }
    unsafe { std::ptr::write_bytes(out, 0, size as usize) };
    // the dates (strings: `YYYY-MM-DD`, 11 bytes; `YYYY-MM-DD hh:mm:ss`, 20): ones the game parses
    let date: &[u8] = match (kind, size) {
        (0xb, 11) => b"2013-04-01",
        (0xb, 20) => b"2013-04-01 00:00:00",
        _ => b"",
    };
    unsafe { std::ptr::copy_nonoverlapping(date.as_ptr(), out, date.len()) };
    size as i32
}

fn player_data_defaults_init() {
    let server = std::env::var("WAL_KONAMI_EASRV").is_ok_and(|v| v.trim() == "1") || std::env::var("WAL_KONAMI_SERVICES").is_ok_and(|v| !v.trim().is_empty());
    if !server {
        return;
    }
    let slot = (game_module() + LOGIN_SEQ_SLOT) as *mut usize;
    if unsafe { *slot } != game_module() + LOGIN_SEQ_UPDATE {
        log!("we2012: card login code differs, a new player's card fails");
        return;
    }
    let (Some(search), Some(refer)) = (unsafe { iat::hook("libavs-win32.dll", "XC058ba50000fb", property_search as *const () as usize) }, unsafe {
        iat::hook("libavs-win32.dll", "XC058ba5000113", property_node_refer as *const () as usize)
    }) else {
        log!("we2012: libavs property imports not found");
        return;
    };
    ORIG_SEARCH.store(search, Ordering::Relaxed);
    ORIG_REFER.store(refer, Ordering::Relaxed);
    let call = game_module() + USERENV_PARSE_CALL;
    let target = unsafe { (call + 5).wrapping_add(*((call + 1) as *const i32) as usize) };
    if unsafe { *(call as *const u8) } == 0xe8 && target == game_module() + USERENV_PARSE {
        let mut code = vec![0xe8];
        code.extend_from_slice(&((userenv_parse as *const () as usize).wrapping_sub(call + 5) as i32).to_le_bytes());
        patch(USERENV_PARSE_CALL, &[0xe8], &code);
    } else {
        log!("we2012: userenv reader differs, after a game it fails");
    }
    LOGIN_UPDATE.store(unsafe { *slot }, Ordering::Relaxed);
    let mut old = 0;
    unsafe {
        VirtualProtect(slot.cast(), 4, PAGE_EXECUTE_READWRITE, &mut old);
        *slot = login_update as *const () as usize;
        VirtualProtect(slot.cast(), 4, old, &mut old);
    }
}
