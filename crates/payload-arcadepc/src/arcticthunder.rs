//! Arctic Thunder (Midway, 2001, `snow.exe`, Glide 3, 32-bit): `WAL_ARCADEPC_BOARD=arcticthunder`,
//! x86 build.
//!
//! - **Board.** The game runs on Midway's `goose.dll` (processes, CMOS, I/O). The cabinet's I/O
//!   board (on a serial port, `magicbus.dll`) belongs to the coin and security manager
//!   `wincsm.exe`, a separate program: goose finds it by name (`0x1000c860`: `EnumProcesses`,
//!   `OpenProcess`), reads `HKLM\Software\Midway Games\WinCSM\IOBlock` (the address, in wincsm,
//!   of two pointers: the I/O block, the control block) and from then on reads and writes
//!   wincsm's memory (`ReadProcessMemory` / `WriteProcessMemory`). The payload is wincsm: both
//!   blocks are its own memory, goose's search answers a handle of the game's own process and
//!   the registry value is written at start.
//! - **I/O block.** +8: the 0x330 bytes goose copies (`0x1000bbd0`) when the control block's
//!   flag (+0) is 0, then sets it to 1: the switch words (+0 / +2 the players' switches, +4 / +6
//!   the coin and operator switches, active low), each switch's press and release counts (+0x10c
//!   / +0x20c the players', +0x18c / +0x28c the others': goose calls the game's handlers), the
//!   analog inputs (+0x31c, four 10-bit values; +0x30c the same). +0x338: the pricing (0x94
//!   bytes), +0x3cc: the credits (paid, service, ...).
//! - **Control block.** +0x18 the credits to subtract (the game waits until the total changes),
//!   +0x1c / +0x20 clear the credits, +0x24 a service credit (the game waits for 0), +0x28 the coin
//!   lock (1 asked, 2 granted, 0 free), +0xc the lamps and the feedback (ignored).
//! - **Switches.** Players: bit 7 attack (the handlebar's button), bits 0-3 up, down, left, right,
//!   4-6 buttons A-C. Coin and operator word (the diagnostics' names): bit 0 start, 4 service
//!   credit, 5 / 6 volume down / up, 7 test, 8-12 the coin slots and the bill validator. Analog:
//!   0 the handlebar (0 left, 1020 right), 1 the throttle (0..1020).
//! - **Pricing.** One credit a coin, one credit to start and to continue (the cabinet's: four).
//! - **Free play.** The game's own free play (`COIN_FREE_ONLY` of `game.env`): the game reads its
//!   `game.env` settings through `getenv` first (`0x473f14`), `WAL_FREEPLAY` sets the variable
//!   (msvcrt's `_putenv`, the CRT's own environment).
//! - **Protection.** The textures (`*.wms`, all of them) are RC4-encrypted with a key derived from
//!   the cabinet's hard disk serial number (goose `0x10008f60`: the ATA / SCSI serial read through
//!   `winio.sys` or the SCSI miniport, its digits a number modulo 2^28; MD5 of its 4 bytes, the
//!   base provider's 40-bit RC4 key), and marked read-only and hidden (`gobj.dll` decrypts only
//!   those, else it reads them as they are: the header check fails, the fonts were blank boxes and
//!   the first track texture an assertion). The dump's serial number gives 0x0a6a209a (found by
//!   trying the 2^28 numbers against the textures' known first bytes, `05 80 00 00`): goose's
//!   serial reader returns it, and gobj's `GetFileAttributesA` reports an encrypted texture (its
//!   first dword not `0x8005`) read-only and hidden. goose asks the default RSA provider
//!   (`CryptAcquireContextA` without a name): Windows 2000's was the base provider (40-bit RC4
//!   keys), Wine's is the enhanced one (128-bit): goose's calls get the base provider.
//! - **Floating point.** goose's process loop (`ploop_process`) unmasks the invalid operation and
//!   divide by zero exceptions (`_control87`, its `goose.ini` defaults: the dump has none). Wine's
//!   `_control87` unmasks them in SSE's MXCSR too (Windows 2000's msvcrt: the x87 only): SSE code
//!   of the runner's DLLs then died (`STATUS_FLOAT_MULTIPLE_TRAPS`), and the Glide wrappers'
//!   x87 code divides by zero (nGlide: goose's exception handler, the game closed). goose's
//!   `_control87` keeps every exception masked and the MXCSR as it was.
//!
//! - **Operator menu.** On the cabinet wincsm watches the test switch, closes the game and runs
//!   `diagnostics\diag.exe` (with the game's `diagdll.dll`): not done, `test` reaches the game's
//!   switch word only.
//!
//! Natives: `start coin coin2 service test volume_down volume_up attack up down left right
//! button_a button_b button_c throttle`; the left stick steers, `accel` is the throttle.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{input, log};
use wal_protocol::Axis;
use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::Registry::{HKEY, HKEY_LOCAL_MACHINE, KEY_WRITE, REG_DWORD, RegCloseKey, RegCreateKeyExA, RegSetValueExA};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// goose's wincsm search (returns its process handle, -1 when absent) and its first bytes.
const GOOSE_FIND_WINCSM: usize = 0xC860;
const GOOSE_FIND_WINCSM_CODE: [u8; 3] = [0x55, 0x8B, 0xEC];
/// goose's disk serial reader (the textures' password) and its first bytes, the dump's password.
const GOOSE_DISK_SERIAL: usize = 0x8F60;
const GOOSE_DISK_SERIAL_CODE: [u8; 4] = [0x57, 0x56, 0x55, 0x53];
const PASSWORD: u32 = 0x0A6A_209A;
/// A texture's first dword, decrypted.
const WMS_MAGIC: u32 = 0x8005;

/// The I/O block: +8 goose's copy, +0x338 the pricing, +0x3cc the credits.
const IO_SIZE: usize = 0x400;
const COPY: usize = 8;
const PRICING: usize = 0x338;
const CREDITS: usize = 0x3CC;
/// Offsets in goose's copy.
const PLAYER_PRESS: usize = 0x10C;
const DIP_PRESS: usize = 0x18C;
const PLAYER_RELEASE: usize = 0x20C;
const DIP_RELEASE: usize = 0x28C;
const ANALOG_RAW: usize = 0x30C;
const ANALOG: usize = 0x31C;
/// The control block.
const CTRL_SIZE: usize = 0x40;
const CTRL_FRESH: usize = 0;
const CTRL_SUBTRACT: usize = 0x18;
const CTRL_CLEAR_ALL: usize = 0x1C;
const CTRL_CLEAR_BONUS: usize = 0x20;
const CTRL_SERVICE: usize = 0x24;
const CTRL_COIN_LOCK: usize = 0x28;

#[repr(C, align(16))]
struct Blocks {
    io: [u8; IO_SIZE],
    ctrl: [u8; CTRL_SIZE],
    /// What the registry value points to: the I/O block, the control block.
    pointers: [u32; 2],
}

static mut BLOCKS: Blocks = Blocks { io: [0; IO_SIZE], ctrl: [0; CTRL_SIZE], pointers: [0; 2] };

fn io() -> *mut u8 {
    unsafe { (&raw mut BLOCKS.io).cast() }
}

fn ctrl() -> *mut u8 {
    unsafe { (&raw mut BLOCKS.ctrl).cast() }
}

fn rd32(base: *mut u8, at: usize) -> u32 {
    unsafe { std::ptr::read_volatile(base.add(at).cast::<u32>()) }
}

fn wr32(base: *mut u8, at: usize, v: u32) {
    unsafe { std::ptr::write_volatile(base.add(at).cast::<u32>(), v) }
}

fn rd16(base: *mut u8, at: usize) -> u16 {
    unsafe { std::ptr::read_volatile(base.add(at).cast::<u16>()) }
}

fn wr16(base: *mut u8, at: usize, v: u16) {
    unsafe { std::ptr::write_volatile(base.add(at).cast::<u16>(), v) }
}

#[derive(Clone, Copy, PartialEq)]
enum Native {
    /// A bit of the players' switches.
    Player(u8),
    /// A bit of the coin and operator switches.
    Dip(u8),
    Throttle,
}

const NATIVES: &[(&str, Native)] = &[
    ("start", Native::Dip(0)),
    ("service", Native::Dip(4)),
    ("volume_down", Native::Dip(5)),
    ("volume_up", Native::Dip(6)),
    ("test", Native::Dip(7)),
    ("coin", Native::Dip(8)),
    ("coin2", Native::Dip(9)),
    ("up", Native::Player(0)),
    ("down", Native::Player(1)),
    ("left", Native::Player(2)),
    ("right", Native::Player(3)),
    ("button_a", Native::Player(4)),
    ("button_b", Native::Player(5)),
    ("button_c", Native::Player(6)),
    ("attack", Native::Player(7)),
    ("throttle", Native::Throttle),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("start", "start"),
    ("coin", "coin"),
    ("service", "service"),
    ("test", "test"),
    ("b1", "attack"),
    ("b2", "throttle"),
    ("b3", "button_c"),
    ("b4", "button_b"),
    ("b5", "volume_down"),
    ("b6", "volume_up"),
    ("up", "up"),
    ("down", "down"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// The switches pressed (active high): the players' word, the coin and operator word, the
/// throttle held.
fn switches() -> (u32, u32, bool) {
    let (mut player, mut dip, mut throttle) = (0u32, 0u32, false);
    map().for_each_pressed(&input(0), |n| match n {
        Native::Player(bit) => player |= 1 << bit,
        Native::Dip(bit) => dip |= 1 << bit,
        Native::Throttle => throttle = true,
    });
    (player, dip, throttle)
}

/// The handlebar's value (0 left .. 1020 right) from the left stick.
fn steering(x: i16) -> u32 {
    ((x as i32 + 32768) >> 8) as u32 * 4
}

/// The throttle's value (0..1020) from `accel` (0..32767), full when held.
fn throttle(accel: i16, held: bool) -> u32 {
    if held { 1020 } else { (accel.max(0) as u32 * 255 / 32767) * 4 }
}

/// The pricing goose hands the game: one credit a coin, one to start and to continue.
fn pricing() -> [u8; 0x94] {
    let mut p = [0u8; 0x94];
    // units per coin (left, right, center, extra, bill), units per credit, per bonus, minimum
    // units, credits to start, to continue, maximum credits, coins per bill, show fractions,
    // the coin counters' units, ...
    let words: [u32; 20] = [1, 1, 1, 1, 4, 1, 0, 0, 1, 1, 32, 4, 1, 1, 1, 1, 1, 0, 4, 0];
    for (i, w) in words.iter().enumerate() {
        p[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    let text = b"1 CREDIT / 1 COIN";
    p[0x5C..0x5C + text.len()].copy_from_slice(text);
    p
}

/// wincsm's loop: the switches into goose's copy, the coins into credits, the game's requests.
fn run() {
    let (io, ctrl) = (io(), ctrl());
    // the counts of presses / releases not yet taken by goose
    let mut presses = [[0u32; 32]; 2];
    let mut releases = [[0u32; 32]; 2];
    let (mut last_player, mut last_dip) = (0u32, 0u32);
    let mut first = true;
    loop {
        let stick = input(0);
        let (player, dip, held) = switches();
        for (k, (now, last)) in [(player, last_player), (dip, last_dip)].into_iter().enumerate() {
            for bit in 0..32 {
                let m = 1u32 << bit;
                if now & m != 0 && last & m == 0 {
                    presses[k][bit] += 1;
                    // a coin: a credit (the coin slots, not the bill)
                    if k == 1 && (8..=11).contains(&bit) {
                        let c = rd32(io, CREDITS);
                        wr32(io, CREDITS, c + 1);
                    }
                }
                if now & m == 0 && last & m != 0 {
                    releases[k][bit] += 1;
                }
            }
        }
        (last_player, last_dip) = (player, dip);

        // goose took the last copy: the next one
        if rd32(ctrl, CTRL_FRESH) != 0 || first {
            let copy = unsafe { io.add(COPY) };
            wr32(copy, 0, !player);
            wr32(copy, 4, !dip);
            for bit in 0..32 {
                wr32(copy, PLAYER_PRESS + bit * 4, presses[0][bit]);
                wr32(copy, PLAYER_RELEASE + bit * 4, releases[0][bit]);
                wr32(copy, DIP_PRESS + bit * 4, presses[1][bit]);
                wr32(copy, DIP_RELEASE + bit * 4, releases[1][bit]);
            }
            presses = [[0; 32]; 2];
            releases = [[0; 32]; 2];
            let analog = [steering(stick.axis(Axis::LeftX)), throttle(stick.axis(Axis::Accel), held), 512, 512];
            for (i, v) in analog.iter().enumerate() {
                wr32(copy, ANALOG + i * 4, *v);
                wr32(copy, ANALOG_RAW + i * 4, *v);
            }
            wr32(ctrl, CTRL_FRESH, 0);
            first = false;
        }

        // the game's requests
        let subtract = rd16(ctrl, CTRL_SUBTRACT) as i16 as i32;
        if subtract != 0 {
            let (paid, service) = (rd32(io, CREDITS) as i32, rd32(io, CREDITS + 4) as i32);
            let from_service = subtract.min(service).max(0);
            wr32(io, CREDITS + 4, (service - from_service) as u32);
            wr32(io, CREDITS, (paid - (subtract - from_service)).max(0) as u32);
            wr16(ctrl, CTRL_SUBTRACT, 0);
            log!("arcticthunder: {subtract} credit(s) used");
        }
        if rd32(ctrl, CTRL_SERVICE) != 0 {
            wr32(io, CREDITS + 4, rd32(io, CREDITS + 4) + 1);
            wr32(ctrl, CTRL_SERVICE, 0);
            log!("arcticthunder: service credit");
        }
        for at in [CTRL_CLEAR_ALL, CTRL_CLEAR_BONUS] {
            if rd32(ctrl, at) != 0 {
                if at == CTRL_CLEAR_ALL {
                    for i in 0..5 {
                        wr32(io, CREDITS + i * 4, 0);
                    }
                }
                wr32(ctrl, at, 0);
            }
        }
        if rd16(ctrl, CTRL_COIN_LOCK) == 1 {
            wr16(ctrl, CTRL_COIN_LOCK, 2);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

static GOOSE: AtomicUsize = AtomicUsize::new(0);
static GET_FILE_ATTRIBUTES: AtomicUsize = AtomicUsize::new(0);
/// The `LoadLibraryA` imports of snow.exe and goose.dll (the originals).
static LOAD_LIBRARY: [AtomicUsize; 2] = [const { AtomicUsize::new(0) }; 2];

unsafe extern "system" fn load_library_exe(name: *const u8) -> usize {
    unsafe { load_library(0, name) }
}

unsafe extern "system" fn load_library_goose(name: *const u8) -> usize {
    unsafe { load_library(1, name) }
}

/// `LoadLibraryA`: `gobj.dll` (the object library, loaded at `goose_init`) gets the attributes
/// hook before it loads its fonts.
unsafe fn load_library(i: usize, name: *const u8) -> usize {
    let f: unsafe extern "system" fn(*const u8) -> usize = unsafe { std::mem::transmute(LOAD_LIBRARY[i].load(Ordering::Relaxed)) };
    let module = unsafe { f(name) };
    if module != 0 && !name.is_null() && unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy().to_ascii_lowercase().contains("gobj") {
        hook_gobj(module);
    }
    module
}

fn hook_gobj(gobj: usize) {
    if GET_FILE_ATTRIBUTES.load(Ordering::Relaxed) != 0 {
        return;
    }
    match unsafe { wal_payload_common::iat::hook_module(gobj, "KERNEL32.dll", "GetFileAttributesA", get_file_attributes as *const () as usize) } {
        Some(o) => {
            GET_FILE_ATTRIBUTES.store(o, Ordering::Relaxed);
            log!("arcticthunder: gobj.dll's encrypted textures");
        }
        None => log!("arcticthunder: no gobj.dll GetFileAttributesA import, textures not decrypted"),
    }
}

/// Whether `path` is an encrypted texture (a `.wms` whose first dword is not the format's).
fn encrypted_texture(path: &str) -> bool {
    use std::io::Read;
    if !path.to_ascii_lowercase().ends_with(".wms") {
        return false;
    }
    let mut head = [0u8; 4];
    std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)).is_ok_and(|_| u32::from_le_bytes(head) != WMS_MAGIC)
}

/// gobj's `GetFileAttributesA`: the encrypted textures read-only and hidden, as on the cabinet.
unsafe extern "system" fn get_file_attributes(name: *const u8) -> u32 {
    let f: unsafe extern "system" fn(*const u8) -> u32 = unsafe { std::mem::transmute(GET_FILE_ATTRIBUTES.load(Ordering::Relaxed)) };
    let attributes = unsafe { f(name) };
    if attributes != u32::MAX && !name.is_null() {
        let path = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy();
        if encrypted_texture(&path) {
            // FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_HIDDEN, not FILE_ATTRIBUTE_ARCHIVE
            return (attributes | 0x3) & !0x20;
        }
    }
    attributes
}

static ACQUIRE_CONTEXT: AtomicUsize = AtomicUsize::new(0);

/// goose's `CryptAcquireContextA`: the default RSA provider is Windows 2000's, the base one.
unsafe extern "system" fn acquire_context(prov: *mut usize, container: *const u8, provider: *const u8, kind: u32, flags: u32) -> i32 {
    let f: unsafe extern "system" fn(*mut usize, *const u8, *const u8, u32, u32) -> i32 = unsafe { std::mem::transmute(ACQUIRE_CONTEXT.load(Ordering::Relaxed)) };
    let provider = if provider.is_null() && kind == 1 { c"Microsoft Base Cryptographic Provider v1.0".as_ptr().cast() } else { provider };
    unsafe { f(prov, container, provider, kind, flags) }
}

static CONTROL87: AtomicUsize = AtomicUsize::new(0);

/// goose's `_control87`: the exceptions kept masked (`_MCW_EM`), the MXCSR as it was.
unsafe extern "C" fn control87(new: u32, mask: u32) -> u32 {
    let new = new | (mask & 0x8_001F);
    let f: unsafe extern "C" fn(u32, u32) -> u32 = unsafe { std::mem::transmute(CONTROL87.load(Ordering::Relaxed)) };
    let mut mxcsr = 0u32;
    unsafe {
        std::arch::asm!("stmxcsr [{0}]", in(reg) &raw mut mxcsr);
        let old = f(new, mask);
        std::arch::asm!("ldmxcsr [{0}]", in(reg) &raw const mxcsr);
        old
    }
}

/// msvcrt's `_putenv` (the game's `getenv` reads the CRT's copy of the environment).
fn put_env(setting: &std::ffi::CStr) {
    unsafe {
        let crt = GetModuleHandleW(windows_sys::w!("msvcrt.dll"));
        if let Some(f) = GetProcAddress(crt, c"_putenv".as_ptr().cast()) {
            let putenv: unsafe extern "C" fn(*const u8) -> i32 = std::mem::transmute(f);
            putenv(setting.as_ptr().cast());
        }
    }
}

fn patch(at: usize, bytes: &[u8]) {
    unsafe {
        let mut old = 0;
        VirtualProtect(at as *const _, bytes.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
        VirtualProtect(at as *const _, bytes.len(), old, &mut old);
    }
}

/// The registry value goose reads: the address of the two block pointers.
fn publish_blocks() -> bool {
    unsafe {
        BLOCKS.pointers = [io() as u32, ctrl() as u32];
        let address = (&raw const BLOCKS.pointers) as u32;
        let mut key: HKEY = std::ptr::null_mut();
        let path = c"Software\\Midway Games\\WinCSM\\IOBlock";
        if RegCreateKeyExA(HKEY_LOCAL_MACHINE, path.as_ptr().cast(), 0, std::ptr::null(), 0, KEY_WRITE, std::ptr::null(), &mut key, std::ptr::null_mut()) != 0 {
            return false;
        }
        let ok = RegSetValueExA(key, std::ptr::null(), 0, REG_DWORD, (&raw const address).cast(), 4) == 0;
        RegCloseKey(key);
        ok
    }
}

pub fn init() {
    let goose = unsafe { GetModuleHandleW(windows_sys::w!("goose.dll")) } as usize;
    if goose == 0
        || unsafe { std::slice::from_raw_parts((goose + GOOSE_FIND_WINCSM) as *const u8, 3) } != GOOSE_FIND_WINCSM_CODE
        || unsafe { std::slice::from_raw_parts((goose + GOOSE_DISK_SERIAL) as *const u8, 4) } != GOOSE_DISK_SERIAL_CODE
    {
        log!("arcticthunder: unknown goose.dll, no board");
        return;
    }
    GOOSE.store(goose, Ordering::Relaxed);

    // the textures' password: the cabinet disk's serial number
    let mut code = [0xB8, 0, 0, 0, 0, 0xC3];
    code[1..5].copy_from_slice(&PASSWORD.to_le_bytes());
    patch(goose + GOOSE_DISK_SERIAL, &code);
    match unsafe { wal_payload_common::iat::hook_module(goose, "MSVCRT.dll", "_control87", control87 as *const () as usize) } {
        Some(o) => CONTROL87.store(o, Ordering::Relaxed),
        None => log!("arcticthunder: no goose.dll _control87 import"),
    }
    match unsafe { wal_payload_common::iat::hook_module(goose, "ADVAPI32.dll", "CryptAcquireContextA", acquire_context as *const () as usize) } {
        Some(o) => ACQUIRE_CONTEXT.store(o, Ordering::Relaxed),
        None => log!("arcticthunder: no goose.dll CryptAcquireContextA import"),
    }
    let gobj = unsafe { GetModuleHandleW(windows_sys::w!("gobj.dll")) } as usize;
    if gobj != 0 {
        hook_gobj(gobj);
    } else {
        let exe = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
        let hooks: [(usize, usize); 2] = [(exe, load_library_exe as *const () as usize), (goose, load_library_goose as *const () as usize)];
        for (i, (module, f)) in hooks.into_iter().enumerate() {
            if let Some(o) = unsafe { wal_payload_common::iat::hook_module(module, "KERNEL32.dll", "LoadLibraryA", f) } {
                LOAD_LIBRARY[i].store(o, Ordering::Relaxed);
            }
        }
    }

    unsafe {
        let p = io().add(PRICING);
        std::ptr::copy_nonoverlapping(pricing().as_ptr(), p, 0x94);
    }
    if !publish_blocks() {
        log!("arcticthunder: the IOBlock registry value could not be written");
    }
    // goose's wincsm: this process (a real handle: goose takes -1 for none)
    let mut me: HANDLE = std::ptr::null_mut();
    unsafe { DuplicateHandle(GetCurrentProcess(), GetCurrentProcess(), GetCurrentProcess(), &mut me, 0, 0, DUPLICATE_SAME_ACCESS) };
    let mut code = [0xB8, 0, 0, 0, 0, 0xC3];
    code[1..5].copy_from_slice(&(me as u32).to_le_bytes());
    patch(goose + GOOSE_FIND_WINCSM, &code);

    match wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") {
        Some(true) => put_env(c"COIN_FREE_ONLY=1"),
        Some(false) => put_env(c"COIN_FREE_ONLY=0"),
        None => {}
    }
    std::thread::spawn(run);
    log!("arcticthunder: wincsm emulated (blocks at {:#x})", io() as usize);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analog() {
        assert_eq!(steering(-32768), 0);
        assert_eq!(steering(0), 512);
        assert_eq!(steering(32767), 1020);
        assert_eq!(throttle(0, false), 0);
        assert_eq!(throttle(32767, false), 1020);
        assert_eq!(throttle(0, true), 1020);
    }

    #[test]
    fn pricing_text() {
        let p = pricing();
        assert_eq!(&p[0x20..0x24], &1u32.to_le_bytes());
        assert_eq!(&p[0x5C..0x6D], b"1 CREDIT / 1 COIN");
    }
}
