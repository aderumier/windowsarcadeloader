//! Cooper's 9 (2006): the payload is its `inpout32.dll`, started before the game by wal-loader.
//!
//! `game.exe` (Direct3D 9) is a memory dump of the HASP envelope-protected game: its sections
//! are the running image, its import directory is empty and its import address table holds the
//! dumping machine's addresses (Windows XP DLLs, envelope stubs). Before the game's code runs:
//!
//! - The imports: a real import directory is written where the header points (`.protect`, the
//!   envelope's section, empty once loaded), its lookup tables naming each slot of the table
//!   (`IMPORTS`), and the slots are resolved. The common hooks then find the game's imports as
//!   in any executable. The names come from the code calling each slot (arguments, results,
//!   the strings passed); the few slots only the HASP library's own code uses are left to a
//!   stub that logs the call (`unresolved`).
//! - The entry point: the dump's is the game's `WinMain` (the C runtime start-up already ran
//!   when the game was dumped), which takes the process entry's argument as its instance: the
//!   executable's base is passed instead (resources, window class).
//! - The game's data: its cabinet mounted `data\data.bin` as drive Z: (`load.exe`, through
//!   `system`) and read `z:/data/bin/`; the dump has the files in `data\bin`: the path is made
//!   relative, `system` does nothing.
//! - The dongle: the game is linked with the HASP HL library; `hasp_login`, `hasp_logout`,
//!   `hasp_read`, `hasp_write` and `hasp_decrypt` are answered (`dongle`): the memory the game
//!   checks ("GWP", its version, the region `WAL_KONAMI_REGION`) and a decrypted reply with the
//!   bytes the game compares.
//! - The I/O board, on the parallel port (`LoadLibraryA("inpout32")`, `Inp32` / `Out32` on
//!   0x378-0x37a): the game writes a command byte to the data port and reads the reply there;
//!   the status port tells it a gun fired (`Board`). Its poll thread runs this protocol.
//! - The guns: the board reports each gun's raw position, which the game maps to its 1280x512
//!   screen through each gun's calibration (test menu). The calibration is skipped: the board
//!   reports the pointer over the whole picture (0-255 both ways), the game's own scale (x5,
//!   x2) makes it the screen position. The light gun flash (a frame of one color at each shot)
//!   is skipped unless `WAL_KONAMI_GUN_FLASH=1`.
//! - The settings and bookkeeping (`expdat`, `extdat`, in the game's folder) go to the drive of
//!   `WAL_KONAMI_SAVE_DRIVE` (the profile maps it to the dump's save folder).
//! - The movies (AVI, MS MPEG-4 v2 and WMV9, DirectShow): MP42 needs a runner whose winedmo
//!   decodes it (GE patch `0098-winedmo-expose-mp42-video-decoding`).
//!
//! Native inputs for `native_map`: `test service coin start trigger offscreen`.

use std::ffi::{CStr, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log};
use wal_protocol::Axis;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// The image (the dump has no relocations: always at its base).
const BASE: usize = 0x400000;

/// The import address table's slots, per DLL (each group ends with a null slot). An empty name:
/// a slot only the HASP library's code calls (bypassed), never identified.
const IMPORTS: &[(&str, usize, &[&str])] = &[
    (
        "advapi32.dll",
        0x512000,
        &[
            "RegOpenKeyA",
            "RegCreateKeyExA",
            "RegDeleteValueA",
            "RegOpenKeyExA",
            "RegCloseKey",
            "RegQueryValueExA",
            "OpenProcessToken",
            "LookupPrivilegeValueA",
            "AdjustTokenPrivileges",
            "RegSetValueExA",
            "RegEnumKeyExA",
            "RegQueryInfoKeyA",
            "RegDeleteKeyA",
        ],
    ),
    ("dinput8.dll", 0x512038, &["DirectInput8Create"]),
    ("dsound.dll", 0x512040, &["DirectSoundCreate"]),
    (
        "gdi32.dll",
        0x512048,
        &[
            "ExtTextOutW",
            "MoveToEx",
            "ExtTextOutA",
            "CreateCompatibleDC",
            "SetMapMode",
            "CreateFontIndirectW",
            "CreateFontIndirectA",
            "GetFontLanguageInfo",
            "GetTextMetricsW",
            "SetBkMode",
            "SetBkColor",
            "SetTextColor",
            "GetCharacterPlacementW",
            "GetCharacterPlacementA",
            "SelectObject",
            "DeleteDC",
            "CreateDIBSection",
            "GetGlyphOutlineW",
            "GetTextMetricsA",
            "GetObjectW",
            "GetObjectA",
            "DeleteObject",
            "GetDeviceCaps",
            "SetTextAlign",
        ],
    ),
    (
        "kernel32.dll",
        0x5120ac,
        &[
            "SizeofResource",
            "LoadResource",
            "FindResourceA",
            "LoadLibraryExA",
            "lstrcmpiA",
            "lstrcpynA",
            "IsDBCSLeadByte",
            "QueryPerformanceFrequency",
            "GetLocalTime",
            "QueryPerformanceCounter",
            "MulDiv",
            "CloseHandle",
            "WaitForSingleObject",
            "GetProcAddress",
            "LoadLibraryA",
            "CreateThread",
            "SetLocalTime",
            "GetCurrentProcess",
            "GetCurrentThreadId",
            "SetEvent",
            "ResetEvent",
            "WaitForMultipleObjects",
            "CreateEventA",
            "ReleaseSemaphore",
            "GetSystemInfo",
            "",
            "SetThreadPriority",
            "GetThreadPriority",
            "GetCurrentThread",
            "GetTickCount",
            "IsProcessorFeaturePresent",
            "WriteFile",
            "",
            "CreateFileA",
            "FreeLibrary",
            "ReadFile",
            "SetFilePointer",
            "ExpandEnvironmentStringsA",
            "HeapAlloc",
            "GetProcessHeap",
            "HeapFree",
            "TlsAlloc",
            "TlsGetValue",
            "TlsSetValue",
            "",
            "DeviceIoControl",
            "",
            "GetSystemTimeAsFileTime",
            "GetStartupInfoA",
            "ExitProcess",
            "",
            "",
            "",
            "",
            "GetCurrentProcessId",
            "",
            "FindClose",
            "",
            "",
            "",
            "",
            "",
            "",
            "OutputDebugStringA",
            "WideCharToMultiByte",
            "EnterCriticalSection",
            "LeaveCriticalSection",
            "GetLastError",
            "GetModuleFileNameA",
            "lstrlenA",
            "MultiByteToWideChar",
            "lstrlenW",
            "InterlockedDecrement",
            "InterlockedIncrement",
            "DeleteCriticalSection",
            "InitializeCriticalSection",
            "RaiseException",
            "GetVersionExA",
            "GetThreadLocale",
            "GetLocaleInfoA",
            "GetACP",
            "InterlockedExchange",
            "GetModuleHandleA",
            "Sleep",
        ],
    ),
    (
        "msvcr71.dll",
        0x512200,
        &[
            "__dllonexit",
            "_onexit",
            "_c_exit",
            "_exit",
            "_XcptFilter",
            "_ismbblead",
            "_cexit",
            "_acmdln",
            "_amsg_exit",
            "__getmainargs",
            "_initterm",
            "__setusermatherr",
            "_adjust_fdiv",
            "__p__commode",
            "__p__fmode",
            "__set_app_type",
            "memset",
            "??1type_info@@UAE@XZ",
            "?terminate@@YAXXZ",
            "__security_error_handler",
            "strstr",
            "sprintf",
            "system",
            "_except_handler3",
            "??3@YAXPAX@Z",
            "free",
            "??_V@YAXPAX@Z",
            "malloc",
            "_resetstkoflw",
            "",
            "??_U@YAPAXI@Z",
            "realloc",
            "_CxxThrowException",
            "srand",
            "rand",
            "fclose",
            "fread",
            "fopen",
            "??2@YAPAXI@Z",
            "vsprintf",
            "strncpy",
            "strncmp",
            "wcsncpy",
            "fflush",
            "fwrite",
            "_purecall",
            "_CIacos",
            "_finite",
            "_controlfp",
            "_CIpow",
            "_strdup",
            "setlocale",
            "",
            "longjmp",
            "_setjmp3",
            "isdigit",
            "isspace",
            "sscanf",
            "floor",
            "iswpunct",
            "iswdigit",
            "iswalpha",
            "iswspace",
            "exit",
        ],
    ),
    ("oleaut32.dll", 0x512304, &["VarUI4FromStr"]),
    (
        "user32.dll",
        0x51230c,
        &[
            "DefWindowProcA",
            "PostQuitMessage",
            "SetCursor",
            "ShowCursor",
            "CreateWindowExA",
            "RegisterClassExA",
            "MessageBoxA",
            "ShowWindow",
            "RegisterWindowMessageA",
            "MsgWaitForMultipleObjects",
            "GetQueueStatus",
            "PostThreadMessageA",
            "GetDC",
            "ReleaseDC",
            "CharNextA",
            "PeekMessageA",
            "GetMessageA",
            "TranslateMessage",
            "DispatchMessageA",
            "GetClientRect",
            "UpdateWindow",
        ],
    ),
    (
        "winmm.dll",
        0x512364,
        &[
            "mmioClose",
            "mmioAdvance",
            "timeGetTime",
            "timeBeginPeriod",
            "timeEndPeriod",
            "mmioAscend",
            "mmioRead",
            "mmioDescend",
            "timeSetEvent",
            "timeKillEvent",
            "mmioOpenA",
            "mmioGetInfo",
            "mmioSeek",
            "mmioSetInfo",
        ],
    ),
    ("d3d9.dll", 0x5123a0, &["Direct3DCreate9"]),
    (
        "ole32.dll",
        0x5123a8,
        &["CoFreeUnusedLibraries", "CoTaskMemRealloc", "CoTaskMemAlloc", "CoTaskMemFree", "CoCreateInstance", "CoUninitialize", "CoInitialize"],
    ),
];

/// The envelope's section: zeros once loaded (the dump's file holds them there), writable; the
/// header's import directory points into it.
const PROTECT: usize = 0x29fb000;
const PROTECT_END: usize = 0x2b2a000;

/// The game's `WinMain` (the dump's entry point) and what follows its first 5 bytes (wal-loader
/// restores those): `push 0; push esi; push <module>`, esi being the instance.
const WIN_MAIN: usize = 0x402480;
const WIN_MAIN_BYTES: [u8; 13] = [0x56, 0x8b, 0x74, 0x24, 0x08, 0x6a, 0x00, 0x56, 0x68, 0x70, 0xe3, 0x55, 0x00];

/// The game's data folder, as the cabinet mounted it.
const DATA_PATH: usize = 0x5168d0;

/// The HASP HL API, linked in the game (stdcall).
const HASP_LOGIN: usize = 0x4a3730;
const HASP_LOGOUT: usize = 0x4a37c0;
const HASP_ENCRYPT: usize = 0x4a38a0;
const HASP_DECRYPT: usize = 0x4a39a0;
const HASP_READ: usize = 0x4a4540;
const HASP_WRITE: usize = 0x4a4610;

/// The guns' calibration (input manager, per gun): from the raw position's conversion to the
/// clamp of the calibrated one (0-255, then x5 / x2 to the screen).
const CALIBRATION: usize = 0x419b57;
const CALIBRATION_BYTES: [u8; 8] = [0x0f, 0xb6, 0x48, 0x1a, 0xf3, 0x0f, 0x2a, 0xd1];
const CALIBRATED: usize = 0x419bc1;

/// The boot's PLEASE CALIBRATE THE GUN: the settings' "guns calibrated" bit (the test menu's
/// GUN CALIBRATION sets it). The aim needs no calibration: always set.
const CALIBRATION_CHECK: usize = 0x41dc00;
const CALIBRATION_CHECK_BYTES: [u8; 6] = [0xa1, 0x90, 0x81, 0x3e, 0x02, 0xc1];

/// The gun flash: the frame the game clears to one color when the board starts a gun's
/// measurement (the light gun sees the screen); `0x4190b0` tells the renderer to draw it.
const GUN_FLASH: usize = 0x4190b0;
const GUN_FLASH_BYTES: [u8; 6] = [0x8b, 0x0d, 0x5c, 0x7f, 0x3e, 0x02];

/// The settings and bookkeeping files (`expdat`, `extdat` in the game's folder): the pushes of
/// their names (save: 0x41dab8, 0x41db10; load: 0x41de3a) and the names.
const SAVE_FILE_PUSHES: [(usize, usize); 4] = [(0x41dabd, 0x51aaf0), (0x41db15, 0x51aae8), (0x41de41, 0x51aae8), (0x41de48, 0x51aaf0)];

/// This payload is inpout32.dll, in Cooper's 9 (its `WinMain` and data path in place).
pub fn detected(module: windows_sys::Win32::Foundation::HINSTANCE) -> bool {
    if !crate::file_name(module).eq_ignore_ascii_case("inpout32.dll") {
        return false;
    }
    let exe = unsafe { GetModuleHandleA(std::ptr::null()) } as usize;
    exe == BASE
        && unsafe { std::slice::from_raw_parts((WIN_MAIN + 5) as *const u8, 8) } == &WIN_MAIN_BYTES[5..]
        && unsafe { CStr::from_ptr(DATA_PATH as *const _) }.to_bytes() == b"z:/data/bin/"
}

/// Writable copy of the code or data at `at`.
unsafe fn write(at: usize, bytes: &[u8]) {
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, bytes.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
        VirtualProtect(at as *const c_void, bytes.len(), old, &mut old);
    }
}

/// Space in `.protect`, handed out in order.
struct Arena(usize);

impl Arena {
    fn put(&mut self, bytes: &[u8]) -> usize {
        let at = self.0;
        assert!(at + bytes.len() <= PROTECT_END);
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len()) };
        self.0 = (at + bytes.len() + 3) & !3;
        at
    }
}

/// The unresolved slot called (its address pushed by its stub).
extern "C" fn unresolved(slot: usize) {
    log!("coopers9: unresolved import (slot {slot:#x}) called");
}

/// Writes the import directory and resolves the import address table. Before the common hooks
/// (they look the game's imports up there). Returns (resolved, missing) slot counts.
pub fn imports() -> (usize, Vec<String>) {
    let nt = BASE + unsafe { ((BASE + 0x3c) as *const u32).read() } as usize;
    // PE32: the import directory, data directory 1
    let dir = BASE + unsafe { ((nt + 24 + 96 + 8) as *const u32).read() } as usize;
    // the descriptors where the header points, the rest from the section's start
    let mut arena = Arena(PROTECT);
    let mut descriptors = Vec::new();
    let mut resolved = 0;
    let mut missing = Vec::new();
    for &(dll, iat, names) in IMPORTS {
        let dll_c = std::ffi::CString::new(dll).unwrap();
        let name_at = arena.put(dll_c.as_bytes_with_nul());
        let module = unsafe { LoadLibraryA(dll_c.as_ptr().cast()) };
        let mut lookup = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let slot = iat + 4 * i;
            let function = if name.is_empty() {
                // push slot; mov eax, unresolved; call eax; add esp, 4; ret
                let mut stub = vec![0x68];
                stub.extend_from_slice(&(slot as u32).to_le_bytes());
                stub.push(0xb8);
                stub.extend_from_slice(&(unresolved as *const () as u32).to_le_bytes());
                stub.extend_from_slice(&[0xff, 0xd0, 0x83, 0xc4, 0x04, 0xc3]);
                lookup.push((arena.put(b"\0\0wal_unresolved\0") - BASE) as u32);
                arena.put(&stub)
            } else {
                let mut by_name = vec![0u8, 0];
                by_name.extend_from_slice(name.as_bytes());
                by_name.push(0);
                lookup.push((arena.put(&by_name) - BASE) as u32);
                let c = std::ffi::CString::new(*name).unwrap();
                match unsafe { GetProcAddress(module, c.as_ptr().cast()) } {
                    Some(f) => {
                        resolved += 1;
                        f as usize
                    }
                    None => {
                        missing.push(format!("{dll}!{name}"));
                        0
                    }
                }
            };
            unsafe { write(slot, &(function as u32).to_le_bytes()) };
        }
        lookup.push(0);
        let lookup_bytes: Vec<u8> = lookup.iter().flat_map(|r| r.to_le_bytes()).collect();
        let lookup_at = arena.put(&lookup_bytes);
        // OriginalFirstThunk, TimeDateStamp, ForwarderChain, Name, FirstThunk
        for v in [lookup_at - BASE, 0, 0, name_at - BASE, iat - BASE] {
            descriptors.extend_from_slice(&(v as u32).to_le_bytes());
        }
    }
    descriptors.extend_from_slice(&[0; 20]);
    unsafe { write(dir, &descriptors) };
    (resolved, missing)
}

/// `WinMain` gets the executable's base as its instance: `mov esi, base; push 0; push esi;
/// push <module>; jmp back`.
fn win_main_instance() {
    let mut thunk = vec![0xbe];
    thunk.extend_from_slice(&(BASE as u32).to_le_bytes());
    thunk.extend_from_slice(&WIN_MAIN_BYTES[5..]);
    thunk.push(0xe9);
    let thunk_at = PROTECT_END - 0x100;
    let back = WIN_MAIN + WIN_MAIN_BYTES.len();
    thunk.extend_from_slice(&(back.wrapping_sub(thunk_at + thunk.len() + 4) as u32).to_le_bytes());
    unsafe {
        std::ptr::copy_nonoverlapping(thunk.as_ptr(), thunk_at as *mut u8, thunk.len());
        crate::f24::jump(WIN_MAIN + 5, thunk_at);
        write(WIN_MAIN + 10, &[0x90; 3]);
    }
}

/// `expdat` / `extdat` in the folder of `WAL_KONAMI_SAVE_DRIVE` (the profile maps that drive to
/// the dump's save folder) rather than the game's own (the run directory).
fn save_files() {
    let Some(drive) = std::env::var("WAL_KONAMI_SAVE_DRIVE").ok().and_then(|d| d.trim().chars().next()).filter(char::is_ascii_alphabetic) else {
        return;
    };
    let mut arena = Arena(PROTECT_END - 0x200);
    for (push, name) in SAVE_FILE_PUSHES {
        let operand = push + 1;
        if unsafe { *(push as *const u8) } != 0x68 || unsafe { (operand as *const u32).read_unaligned() } != name as u32 {
            log!("coopers9: unexpected code at {push:#x}, the save files stay in the game folder");
            return;
        }
    }
    for (push, name) in SAVE_FILE_PUSHES {
        let file = unsafe { CStr::from_ptr(name as *const _) }.to_string_lossy();
        let path = format!("{drive}:\\{file}\0");
        let at = arena.put(path.as_bytes());
        unsafe { write(push + 1, &(at as u32).to_le_bytes()) };
    }
    log!("coopers9: settings and bookkeeping in {drive}:\\");
}

// The dongle (HASP HL, stdcall).

/// The dongle's read-only memory (file 0xfff0) as the game checks it: "GWP", the version
/// (A1 04 01), the region (index in "JUAKEGF"), 04, the bytes at 9 and 15 (0x64), 00 01 at
/// 0x2e.
static DONGLE: Mutex<[u8; 0x70]> = Mutex::new([0; 0x70]);

const REGIONS: &str = "JUAKEGF";

extern "system" fn hasp_login(_feature: u32, _vendor_code: *const u8, handle: *mut u32) -> u32 {
    if !handle.is_null() {
        unsafe { *handle = 1 };
    }
    0
}

extern "system" fn hasp_logout(_handle: u32) -> u32 {
    0
}

extern "system" fn hasp_encrypt(_handle: u32, _buffer: *mut u8, _length: u32) -> u32 {
    0
}

/// The game decrypts a constant and checks its first and eighth bytes ('C', '9').
extern "system" fn hasp_decrypt(_handle: u32, buffer: *mut u8, length: u32) -> u32 {
    let plain = b"Cooper 9";
    if !buffer.is_null() {
        let out = unsafe { std::slice::from_raw_parts_mut(buffer, length as usize) };
        for (i, b) in out.iter_mut().enumerate() {
            *b = plain.get(i).copied().unwrap_or(0);
        }
    }
    0
}

extern "system" fn hasp_read(_handle: u32, file: u32, offset: u32, length: u32, buffer: *mut u8) -> u32 {
    let memory = DONGLE.lock().unwrap();
    let (offset, length) = (offset as usize, length as usize);
    if file != 0xfff0 || offset + length > memory.len() || buffer.is_null() {
        log!("coopers9: hasp_read({file:#x}, {offset:#x}, {length}) outside the memory");
        return 0x1d; // HASP_MEM_RANGE
    }
    unsafe { std::ptr::copy_nonoverlapping(memory[offset..].as_ptr(), buffer, length) };
    0
}

extern "system" fn hasp_write(_handle: u32, file: u32, offset: u32, length: u32, buffer: *const u8) -> u32 {
    let mut memory = DONGLE.lock().unwrap();
    let (offset, length) = (offset as usize, length as usize);
    if file != 0xfff0 || offset + length > memory.len() || buffer.is_null() {
        return 0x1d;
    }
    unsafe { std::ptr::copy_nonoverlapping(buffer, memory[offset..].as_mut_ptr(), length) };
    0
}

fn dongle() {
    let region = std::env::var("WAL_KONAMI_REGION").ok().and_then(|r| REGIONS.find(r.trim().to_ascii_uppercase().as_str()).filter(|_| r.trim().len() == 1));
    let region = region.unwrap_or(REGIONS.find('E').unwrap());
    let mut memory = DONGLE.lock().unwrap();
    memory[..10].copy_from_slice(&[b'G', b'W', b'P', 0xa1, 0x04, 0x01, region as u8, 0x04, 0x00, 0x64]);
    memory[10..16].copy_from_slice(&[0x00, 0x64, 0x00, 0x64, 0x00, 0x64]);
    memory[0x2e..0x30].copy_from_slice(&[0x00, 0x01]);
    log!("coopers9: dongle region {}", &REGIONS[region..region + 1]);
    let hooks: [(usize, &[u8], usize); 6] = [
        (HASP_LOGIN, &[0x56, 0x57, 0x68, 0x68, 0xb5], hasp_login as *const () as usize),
        (HASP_LOGOUT, &[0x51, 0x56, 0x57, 0x68, 0xe4], hasp_logout as *const () as usize),
        (HASP_ENCRYPT, &[0x83, 0xec, 0x08, 0x56, 0x57], hasp_encrypt as *const () as usize),
        (HASP_DECRYPT, &[0x83, 0xec, 0x08, 0x56, 0x57], hasp_decrypt as *const () as usize),
        (HASP_READ, &[0x51, 0x56, 0x68, 0x84, 0xc0], hasp_read as *const () as usize),
        (HASP_WRITE, &[0x51, 0x56, 0x68, 0xf8, 0xc0], hasp_write as *const () as usize),
    ];
    for (at, expected, target) in hooks {
        if unsafe { std::slice::from_raw_parts(at as *const u8, expected.len()) } == expected {
            unsafe { crate::f24::jump(at, target) };
        } else {
            log!("coopers9: unexpected code at {at:#x}, HASP function not answered");
        }
    }
}

// The C runtime's `system`: the cabinet's `load.exe` (mounts data.bin as Z:, unmounts it).

extern "C" fn system(command: *const std::ffi::c_char) -> i32 {
    let command = if command.is_null() { String::new() } else { unsafe { CStr::from_ptr(command) }.to_string_lossy().into_owned() };
    log!("coopers9: system(\"{command}\") skipped");
    0
}

/// The PC clock is the host's: the test menu's clock setting is not applied.
extern "system" fn set_local_time(_time: *const c_void) -> i32 {
    1
}

// The I/O board (parallel port).

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    Start,
    Trigger,
    Offscreen,
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("start", Native::Start),
    ("trigger", Native::Trigger),
    ("offscreen", Native::Offscreen),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("b1", "trigger"),
    ("b2", "offscreen"),
    ("b6", "offscreen"),
];

const PLAYERS: usize = 2;

const DATA_PORT: u16 = 0x378;

static MAPS: std::sync::OnceLock<Vec<ButtonMap<Native>>> = std::sync::OnceLock::new();

/// The command byte last written to the data port.
static COMMAND: AtomicU8 = AtomicU8::new(0);
/// The status port was read once (the game's board detection).
static STATUS_READ: AtomicBool = AtomicBool::new(false);
/// A gun fired: the status port's "measured" state (bit 7 low) until the triggers are read
/// released.
static FIRED: AtomicBool = AtomicBool::new(false);

/// A player's controls this step.
#[derive(Default)]
struct Player {
    pressed: Vec<Native>,
    /// The pointer on the picture (0..1), None off it.
    aim: Option<(f32, f32)>,
}

fn player(p: usize) -> Player {
    let stick = wal_payload_common::input(p);
    let mut pressed = Vec::new();
    if let Some(map) = MAPS.get() {
        map[p].for_each_pressed(&stick, |n| pressed.push(n));
    }
    let fx = (stick.axis(Axis::LeftX) as f32 + 32768.0) / 65535.0;
    let fy = (stick.axis(Axis::LeftY) as f32 + 32768.0) / 65535.0;
    let on = (0.0..1.0).contains(&fx) && (0.0..1.0).contains(&fy) && !pressed.contains(&Native::Offscreen);
    Player { aim: on.then_some((fx, fy)), pressed }
}

/// The reply to `command` on the data port.
fn reply(command: u8) -> u8 {
    let players: Vec<Player> = (0..PLAYERS).map(player).collect();
    // the `offscreen` control is a whole shot off the screen (the protect / reload shot)
    let trigger = |p: usize| players[p].pressed.iter().any(|n| matches!(n, Native::Trigger | Native::Offscreen));
    let raw = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    match command {
        // the guns' positions: P1 x, y, P2 x, y
        1..=4 => {
            let p = (command as usize - 1) / 2;
            match players[p].aim {
                Some((x, y)) => raw(if command % 2 == 1 { x } else { y }),
                None => 0,
            }
        }
        // the shots: per gun (bits 0-3, 4-7) the trigger (bit 0), on the screen (bit 2); off
        // the screen, the game takes no position
        5 => {
            let mut v = 0;
            for p in 0..PLAYERS {
                if trigger(p) {
                    v |= (if players[p].aim.is_some() { 4 } else { 1 }) << (4 * p);
                }
            }
            if v == 0 {
                FIRED.store(false, Ordering::Relaxed);
            }
            v
        }
        // the triggers
        6 => {
            let v = (0..PLAYERS).filter(|&p| trigger(p)).fold(0, |v, p| v | 1 << (4 * p));
            FIRED.store(v != 0, Ordering::Relaxed);
            v
        }
        // the switches: starts (bits 0, 1), test, service, coin
        7 | 8 => {
            if (0..PLAYERS).any(trigger) {
                FIRED.store(true, Ordering::Relaxed);
            }
            let mut v = 0xe0;
            for (p, player) in players.iter().enumerate() {
                for n in &player.pressed {
                    v |= match n {
                        Native::Start => 1 << p,
                        Native::Test => 0x04,
                        Native::Service => 0x08,
                        Native::Coin => 0x10,
                        _ => 0,
                    };
                }
            }
            v
        }
        _ => 0,
    }
}

/// inpout32's `Inp32(port)`.
extern "system" fn inp32(port: i16) -> i16 {
    if port as u16 == DATA_PORT {
        return reply(COMMAND.load(Ordering::Relaxed)) as i16;
    }
    // the status port, read once per poll of the game's I/O thread (which never sleeps)
    std::thread::sleep(std::time::Duration::from_millis(2));
    if !STATUS_READ.swap(true, Ordering::Relaxed) {
        // the board is there (bits 1-2)
        return 0x0f;
    }
    // bit 7 low: a gun measured a shot (the game reads the positions); bit 5: the switches'
    // second bank
    if FIRED.load(Ordering::Relaxed) { 0x20 } else { 0xdf }
}

/// inpout32's `Out32(port, value)`.
extern "system" fn out32(port: i16, value: i16) {
    if port as u16 == DATA_PORT {
        COMMAND.store(value as u8, Ordering::Relaxed);
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn coopers9_inp32(port: i16) -> i16 {
    inp32(port)
}

#[unsafe(no_mangle)]
pub extern "system" fn coopers9_out32(port: i16, value: i16) {
    out32(port, value)
}

pub fn init(resolved: usize, missing: &[String]) {
    log!("coopers9: {resolved} imports resolved");
    for m in missing {
        log!("coopers9: import {m} not found");
    }
    unsafe {
        if iat::hook("msvcr71.dll", "system", system as *const () as usize).is_none() {
            log!("coopers9: system not hooked");
        }
        iat::hook("kernel32.dll", "SetLocalTime", set_local_time as *const () as usize);
    }
    win_main_instance();
    unsafe { write(DATA_PATH, b"./data/bin/\0") };
    save_files();
    if !std::env::var("WAL_KONAMI_GUN_FLASH").is_ok_and(|v| v.trim() == "1") {
        if unsafe { std::slice::from_raw_parts(GUN_FLASH as *const u8, GUN_FLASH_BYTES.len()) } == GUN_FLASH_BYTES {
            // xor eax, eax; ret
            unsafe { write(GUN_FLASH, &[0x33, 0xc0, 0xc3]) };
        } else {
            log!("coopers9: unexpected code at {GUN_FLASH:#x}, the gun flash stays");
        }
    }
    dongle();
    if unsafe { std::slice::from_raw_parts(CALIBRATION as *const u8, CALIBRATION_BYTES.len()) } == CALIBRATION_BYTES {
        unsafe { crate::f24::jump(CALIBRATION, CALIBRATED) };
    } else {
        log!("coopers9: unexpected calibration code, guns aim through their calibration");
    }
    if unsafe { std::slice::from_raw_parts(CALIBRATION_CHECK as *const u8, CALIBRATION_CHECK_BYTES.len()) } == CALIBRATION_CHECK_BYTES {
        // mov eax, 1; ret
        unsafe { write(CALIBRATION_CHECK, &[0xb8, 0x01, 0x00, 0x00, 0x00, 0xc3]) };
    } else {
        log!("coopers9: unexpected code at {CALIBRATION_CHECK:#x}, the calibration check stays");
    }
    let _ = MAPS.set(
        (0..PLAYERS)
            .map(|p| if p == 0 { ButtonMap::new(NATIVES, DEFAULT_MAP) } else { ButtonMap::with_defaults(NATIVES, DEFAULT_MAP) })
            .collect(),
    );
    log!("coopers9: I/O board (parallel port) and dongle emulated (Cooper's 9)");
}
