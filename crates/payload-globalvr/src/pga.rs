//! PGA TOUR Golf Championship Edition III: the HASP4 dongle of the shell's control (`ShellControl.ocx`)
//! and of the game (`pga_tour_golf.exe`).
//!
//! Both link the HASP4 API (the `hasp(service, seed, port, pw1, pw2, &p1, &p2, &p3, &p4)` stub,
//! cdecl, found by its code) and read the dongle's 112-byte memory (by blocks; the first
//! edition's game: by words) with the passwords 0x293D / 0x4B28 (else 0x4A4F / 0x5DD7):
//!
//! * the control: the dongle present (`HaspID` status 0), its country (`GetCountryCode`: the two
//!   bytes after "CTRY", "US"), its expiry date (`CheckDate`: the day numbers after "DATE"; none:
//!   no expiry);
//! * the game: its code, the shell's `Operator\Games\TW2002\Code1` registry value ("PGA02.02";
//!   Championship Edition II: "PGA02.01", the first: "PGATOUR1"), somewhere in the memory (the
//!   first: in its first 8 bytes; else it quits with exit code 4, "dongle problem").
//!
//! The shell control starts the game with the payload, through `wal-loader` ([`create_process`]).
//!
//! The game shows its window seconds before its Direct3D device (loading the course): the
//! window's white background was on the screen, painted black ([`black_window`]).
//!
//! Championship Edition II's `DXWrap.dll` rounds DirectDraw's video memory up to 16 MB with
//! `(memory + 8 MB) & 0x0F000000`: from 256 MB on (Wine: up to 4 GB) it is 0, and the display
//! mode fails ("not enough video memory"). The rounding is replaced by 240 MB
//! ([`video_memory`]).

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// The start of the `hasp()` stub (`push ebp; mov ebp,esp; push eax/ebx/ecx/edx/edi/esi;
/// mov esi,[ebp+1c]; mov edi,[esi]; mov ebx,0; mov ebx,[ebp+8]; mov bh,bl; mov bl,0;
/// add ebx,[ebp+10]`).
const STUB: [u8; 29] = [
    0x55, 0x8B, 0xEC, 0x50, 0x53, 0x51, 0x52, 0x57, 0x56, 0x8B, 0x75, 0x1C, 0x8B, 0x3E, 0xBB, 0x00, 0x00, 0x00, 0x00, 0x8B,
    0x5D, 0x08, 0x8A, 0xFB, 0xB3, 0x00, 0x03, 0x5D, 0x10,
];

const IS_HASP: i32 = 1;
const READ_WORD: i32 = 3;
const WRITE_WORD: i32 = 4;
const HASP_STATUS: i32 = 5;
const HASP_ID: i32 = 6;
const READ_BLOCK: i32 = 50;
const WRITE_BLOCK: i32 = 51;

/// The port the dongle answers on when asked for any (0): the first USB key.
const USB_PORT: i32 = 201;

/// The dongle's memory: the games' codes (the first edition, which reads only the first 8 bytes,
/// Championship Edition III, II), the country.
const fn memory() -> [u8; 0x70] {
    let mut m = [0u8; 0x70];
    let head = b"PGATOUR1PGA02.02PGA02.01CTRYUS";
    let mut i = 0;
    while i < head.len() {
        m[i] = head[i];
        i += 1;
    }
    m
}

static MEMORY: std::sync::Mutex<[u8; 0x70]> = std::sync::Mutex::new(memory());

/// The `hasp()` of the module at `base` (`size` bytes) answered by [`hasp`].
pub(crate) fn hook(base: usize, size: usize, name: &str) {
    let image = unsafe { std::slice::from_raw_parts(base as *const u8, size) };
    let Some(at) = image.windows(STUB.len()).position(|w| w == STUB) else {
        log!("pga: {name}: hasp() not found, dongle not emulated");
        return;
    };
    let stub = base + at;
    let mut code = [0xE9u8, 0, 0, 0, 0];
    code[1..].copy_from_slice(&((hasp as *const () as usize).wrapping_sub(stub + 5) as i32).to_le_bytes());
    let mut old = 0;
    unsafe {
        VirtualProtect(stub as *const c_void, 5, PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), stub as *mut u8, 5);
        VirtualProtect(stub as *const c_void, 5, old, &mut old);
    }
    log!("pga: {name}: hasp() at {stub:#x} answered");
}

/// The size of the image of the module at `base` (`SizeOfImage`).
fn image_size(base: usize) -> usize {
    unsafe {
        let nt = base + ((base + 0x3c) as *const u32).read_unaligned() as usize;
        ((nt + 0x50) as *const u32).read_unaligned() as usize
    }
}

/// The dump is PGA TOUR's (its cabinet database folder, `C:\gvr.net`), or [`is_first_edition`].
pub(crate) fn is_dump() -> bool {
    static DUMP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DUMP.get_or_init(|| std::path::Path::new("C:\\gvr.net\\Country").exists() || is_first_edition())
}

/// The dump is the first Championship Edition's (1.2: no database, its operator menu in
/// `C:\TW2002_Shell\Operator`).
pub(crate) fn is_first_edition() -> bool {
    static FIRST: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FIRST.get_or_init(|| std::path::Path::new("C:\\TW2002_Shell\\Operator\\frame.html").exists())
}

/// The shell control's dongle, its module loaded at `base`, and its game start.
pub(crate) fn hook_control(base: usize) {
    if !is_dump() {
        return;
    }
    hook(base, image_size(base), "ShellControl.ocx");
    if is_first_edition() {
        watchdog_archives();
    }
    // the control starts the game with the C library's `_spawnl` (`CreateProcessW` in Wine's)
    let msvcrt = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(wide("msvcrt.dll").as_ptr()) } as usize;
    if msvcrt != 0
        && let Some(o) = unsafe { iat::hook_module(msvcrt, "kernel32.dll", "CreateProcessW", create_process as *const () as usize) }
    {
        ORIG_CREATE_PROCESS.store(o, Ordering::Relaxed);
    }
}

/// The first edition's game archives, which the cabinet's watchdog (`WatchDog.exe`, started with
/// Windows) keeps in memory for the game: the files mapped by the shell under the names the
/// game opens (`GVR_Resources.dll`: `OpenFileMappingA("WATCHDOG_PLAYERS.GVR")`, else `fopen` of
/// that name, and a crash without it).
fn watchdog_archives() {
    use std::os::windows::io::IntoRawHandle;
    use windows_sys::Win32::System::Memory::{CreateFileMappingA, PAGE_READONLY};

    const BIN: &str = "C:\\Program Files\\EA SPORTS\\Tiger Woods PGA TOUR 2002\\Bin";
    for (file, name) in [("golfers.gvr", "WATCHDOG_GOLFERS.GVR"), ("players.gvr", "WATCHDOG_PLAYERS.GVR"), ("test.gvr", "WATCHDOG_TEST.GVR")] {
        let Ok(archive) = std::fs::File::open(format!("{BIN}\\{file}")) else {
            log!("pga: no {BIN}\\{file}");
            continue;
        };
        // the file and its mapping kept open for the shell's life, like the watchdog's
        let handle = archive.into_raw_handle();
        let name_z = format!("{name}\0");
        let mapping = unsafe { CreateFileMappingA(handle, std::ptr::null(), PAGE_READONLY, 0, 0, name_z.as_ptr()) };
        if mapping.is_null() {
            log!("pga: {file} not mapped (error {})", unsafe { windows_sys::Win32::Foundation::GetLastError() });
        } else {
            log!("pga: {file} mapped as {name}");
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn wide_str(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let len = (0..).take_while(|&i| unsafe { *p.add(i) } != 0).count();
    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(p, len) })
}

static ORIG_CREATE_PROCESS: AtomicUsize = AtomicUsize::new(0);

/// The C library's `CreateProcessW`: the game started through `wal-loader` (which waits for it and
/// exits with its exit code, the shell's game result).
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn create_process(
    app: *const u16,
    cmd: *mut u16,
    pa: usize,
    ta: usize,
    inherit: i32,
    flags: u32,
    env: usize,
    dir: *const u16,
    si: usize,
    pi: usize,
) -> i32 {
    type CreateProcessW = unsafe extern "system" fn(*const u16, *mut u16, usize, usize, i32, u32, usize, *const u16, usize, usize) -> i32;
    let orig: CreateProcessW = unsafe { std::mem::transmute(ORIG_CREATE_PROCESS.load(Ordering::Relaxed)) };
    let (app_s, cmd_s) = (wide_str(app), wide_str(cmd));
    log!("pga: the shell starts {app_s:?} {cmd_s:?}");
    let Some(command) = crate::beachhead::loader_command(&app_s, &cmd_s, &crate::beachhead::payload_path()) else {
        return unsafe { orig(app, cmd, pa, ta, inherit, flags, env, dir, si, pi) };
    };
    log!("pga: {command}");
    let mut command = wide(&command);
    // the loader (a console program) without a console window over the game
    let flags = flags | windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
    unsafe { orig(std::ptr::null(), command.as_mut_ptr(), pa, ta, inherit, flags, env, dir, si, pi) }
}

/// The game's dongle, in the game.
pub(crate) fn init() {
    let exe = crate::frontend_fit::exe_name();
    if exe != "pga_tour_golf.exe" && exe != "tw2002.exe" {
        return;
    }
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    hook(base, crate::exe_size(), &exe);
    crate::netds::init(base);
    video_memory();
    archives_dated();
    ripple_edge();
    std::thread::spawn(black_window);
}

/// `and ecx, 0xFFFF; cmp ecx, 2003` (`GVR_Resources.dll`'s date check: the year of
/// `GetSystemTime`).
const ARCHIVE_YEAR: [u8; 12] = [0x81, 0xE1, 0xFF, 0xFF, 0x00, 0x00, 0x81, 0xF9, 0xD3, 0x07, 0x00, 0x00];

/// The first edition's archives (`GVR_Resources`) opened only in January and February 2003
/// (else none, and the game crashes on its device): the date check answers true, and the
/// archives built before the payload (static objects of `GVR_Resources.dll` and `Engine.dll`,
/// `push "WATCHDOG_..."; mov ecx, object; call constructor`) are opened again
/// (`GVR_Resources::Initialize`).
fn archives_dated() {
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

    let res = unsafe { GetModuleHandleA(c"GVR_Resources.dll".as_ptr().cast()) } as usize;
    if res == 0 {
        return;
    }
    let image = unsafe { std::slice::from_raw_parts(res as *const u8, image_size(res)) };
    let Some(at) = image.windows(ARCHIVE_YEAR.len()).position(|w| w == ARCHIVE_YEAR) else {
        return;
    };
    // the check's start (`push ebp; mov ebp, esp`): `mov al, 1; ret`
    let Some(start) = (at.saturating_sub(0x40)..at).rev().find(|&i| image[i..i + 3] == [0x55, 0x8B, 0xEC]) else {
        return;
    };
    let code: [u8; 3] = [0xB0, 0x01, 0xC3];
    let target = res + start;
    let mut old = 0;
    unsafe {
        VirtualProtect(target as *const c_void, code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), target as *mut u8, code.len());
        VirtualProtect(target as *const c_void, code.len(), old, &mut old);
    }
    log!("pga: GVR_Resources.dll: archive date check at {target:#x} answered");

    let Some(initialize) = (unsafe { GetProcAddress(res as _, c"?Initialize@GVR_Resources@@QAE_NPBD@Z".as_ptr().cast()) }) else {
        return;
    };
    let initialize: unsafe extern "thiscall" fn(usize, *const u8) -> bool = unsafe { std::mem::transmute(initialize) };
    for module in [c"GVR_Resources.dll", c"Engine.dll"] {
        let base = unsafe { GetModuleHandleA(module.as_ptr().cast()) } as usize;
        if base == 0 {
            continue;
        }
        let size = image_size(base);
        let image = unsafe { std::slice::from_raw_parts(base as *const u8, size) };
        let inside = |p: usize| (base..base + size - 16).contains(&p);
        for i in 0..size.saturating_sub(11) {
            if image[i] != 0x68 || image[i + 5] != 0xB9 || image[i + 10] != 0xE8 {
                continue;
            }
            let name = u32::from_le_bytes(image[i + 1..i + 5].try_into().unwrap()) as usize;
            let object = u32::from_le_bytes(image[i + 6..i + 10].try_into().unwrap()) as usize;
            if !inside(name) || !inside(object) || !image[name - base..].starts_with(b"WATCHDOG_") {
                continue;
            }
            // the archive (+0x10c) not opened
            if unsafe { *((object + 0x10c) as *const usize) } != 0 {
                continue;
            }
            let ok = unsafe { initialize(object, name as *const u8) };
            let name = String::from_utf8_lossy(&image[name - base..].split(|&b| b == 0).next().unwrap_or_default()).into_owned();
            log!("pga: {}: archive {name} opened ({ok})", module.to_string_lossy());
        }
    }
}

/// `mov eax, [ebp-0x78]; sub eax, [ebp-0x14]; xor ecx, ecx; mov cl, [eax]` (`Engine.dll`'s
/// water ripple map from `Courses\dudv2.dds`: the pixel above).
const RIPPLE_ABOVE: [u8; 10] = [0x8B, 0x45, 0x88, 0x2B, 0x45, 0xEC, 0x33, 0xC9, 0x8A, 0x08];

/// The address of the first edition's ripple map read of the pixel above (0: none).
static RIPPLE_READ: AtomicUsize = AtomicUsize::new(0);

/// The first edition's ripple map (du/dv from the locked `Courses\dudv2.dds` texture) reads the
/// row above its first row: memory the Windows driver had before the texture, none in Wine's
/// (an access violation loading the course). That read faulting reads the pixel itself.
fn ripple_edge() {
    use windows_sys::Win32::System::Diagnostics::Debug::AddVectoredExceptionHandler;

    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(c"Engine.dll".as_ptr().cast()) } as usize;
    if base == 0 {
        return;
    }
    let image = unsafe { std::slice::from_raw_parts(base as *const u8, image_size(base)) };
    let Some(at) = image.windows(RIPPLE_ABOVE.len()).position(|w| w == RIPPLE_ABOVE) else {
        return;
    };
    RIPPLE_READ.store(base + at + 8, Ordering::Relaxed);
    unsafe { AddVectoredExceptionHandler(1, Some(ripple_fault)) };
    log!("pga: Engine.dll: ripple map edge at {:#x}", base + at + 8);
}

unsafe extern "system" fn ripple_fault(info: *mut windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS) -> i32 {
    const EXCEPTION_ACCESS_VIOLATION: i32 = 0xC000_0005_u32 as i32;
    const EXCEPTION_CONTINUE_EXECUTION: i32 = -1;
    const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
    let (record, context) = unsafe { (&*(*info).ExceptionRecord, &mut *(*info).ContextRecord) };
    if record.ExceptionCode != EXCEPTION_ACCESS_VIOLATION || context.Eip as usize != RIPPLE_READ.load(Ordering::Relaxed) {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    // `mov cl, [eax]` with the pixel ([ebp-0x78])
    let pixel = unsafe { *((context.Ebp as usize - 0x78) as *const *const u8) };
    context.Ecx = u32::from(unsafe { *pixel });
    context.Eip += 2;
    EXCEPTION_CONTINUE_EXECUTION
}

/// `add eax, 0x800000; and eax, 0x0F000000` (DXWrap's video memory rounding, Championship
/// Edition II).
const VIDEO_MEMORY_ROUNDING: &[u8] = &[0x05, 0x00, 0x00, 0x80, 0x00, 0x25, 0x00, 0x00, 0x00, 0x0F];

/// DXWrap's video memory rounding replaced by `mov eax, 0x0F000000` (240 MB, the most it keeps).
fn video_memory() {
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleA(c"DXWrap.dll".as_ptr().cast()) } as usize;
    if base == 0 {
        return;
    }
    let image = unsafe { std::slice::from_raw_parts(base as *const u8, image_size(base)) };
    let Some(at) = image.windows(VIDEO_MEMORY_ROUNDING.len()).position(|w| w == VIDEO_MEMORY_ROUNDING) else {
        return;
    };
    let code: [u8; 10] = [0xB8, 0x00, 0x00, 0x00, 0x0F, 0x90, 0x90, 0x90, 0x90, 0x90];
    let target = base + at;
    let mut old = 0;
    unsafe {
        VirtualProtect(target as *const c_void, code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), target as *mut u8, code.len());
        VirtualProtect(target as *const c_void, code.len(), old, &mut old);
    }
    log!("pga: DXWrap.dll: video memory 240 MB (rounding at {target:#x})");
}

/// The game's window, once shown: its class background black, filled black.
fn black_window() {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{BLACK_BRUSH, FillRect, GetDC, GetStockObject, ReleaseDC};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GCLP_HBRBACKGROUND, GetClientRect, GetWindowThreadProcessId, IsWindowVisible, SetClassLongW,
    };

    unsafe extern "system" fn found(hwnd: HWND, out: LPARAM) -> i32 {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if pid == std::process::id() && unsafe { IsWindowVisible(hwnd) } != 0 && unsafe { GetClientRect(hwnd, &mut rect) } != 0 && rect.right >= 640 {
            unsafe { *(out as *mut HWND) = hwnd };
            return 0;
        }
        1
    }

    for _ in 0..600 {
        let mut hwnd: HWND = std::ptr::null_mut();
        unsafe { EnumWindows(Some(found), &mut hwnd as *mut HWND as LPARAM) };
        if !hwnd.is_null() {
            let black = unsafe { GetStockObject(BLACK_BRUSH) };
            unsafe { SetClassLongW(hwnd, GCLP_HBRBACKGROUND, black as i32) };
            let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            unsafe { GetClientRect(hwnd, &mut rect) };
            let dc = unsafe { GetDC(hwnd) };
            unsafe { FillRect(dc, &rect, black) };
            unsafe { ReleaseDC(hwnd, dc) };
            log!("pga: game window {hwnd:p} painted black");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "C" fn hasp(
    service: i32,
    _seed: i32,
    port: i32,
    _pw1: i32,
    _pw2: i32,
    p1: *mut i32,
    p2: *mut i32,
    p3: *mut i32,
    p4: *mut i32,
) {
    let (a1, a2, a4) = unsafe { (*p1, *p2, *p4) };
    let port = if port == 0 { USB_PORT } else { port };
    let out = match service {
        IS_HASP => [1, 1, 0, 0],
        // 112-byte memory, MemoHASP, its port
        HASP_STATUS => [1, 1, port, 0],
        // id low / high, status 0
        HASP_ID => [0x4c57, 0x4741, 0, 0],
        // p1 word address, p2 the word (the first edition's game)
        READ_WORD | WRITE_WORD => {
            let mut memory = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
            let at = a1 as usize * 2;
            match memory.get_mut(at..at + 2) {
                Some(word) if service == READ_WORD => [a1, i32::from(u16::from_le_bytes([word[0], word[1]])), 0, a4],
                Some(word) => {
                    word.copy_from_slice(&(a2 as u16).to_le_bytes());
                    [a1, a2, 0, a4]
                }
                // address out of the memory
                None => [a1, a2, -6, a4],
            }
        }
        READ_BLOCK | WRITE_BLOCK => {
            // p1 start word, p2 words, p4 buffer
            let (start, len, buf) = (a1 as usize * 2, a2 as usize * 2, a4 as usize as *mut u8);
            let mut memory = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
            for i in 0..len {
                let Some(byte) = memory.get_mut(start + i) else { break };
                unsafe {
                    if service == READ_BLOCK {
                        *buf.add(i) = *byte;
                    } else {
                        *byte = *buf.add(i);
                    }
                }
            }
            [a1, a2, 0, a4]
        }
        _ => [0, 0, 0, 0],
    };
    unsafe { [*p1, *p2, *p3, *p4] = out };
    if service == READ_WORD {
        static LOGGED: std::sync::Once = std::sync::Once::new();
        LOGGED.call_once(|| log!("pga: hasp memory read by words"));
    } else if service != READ_BLOCK {
        log!("pga: hasp service {service} port {port} -> {out:?}");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn memory_layout() {
        let m = super::memory();
        assert_eq!(&m[..24], b"PGATOUR1PGA02.02PGA02.01");
        assert_eq!(&m[24..30], b"CTRYUS");
        assert!(!m.windows(4).any(|w| w == b"DATE"));
    }
}
