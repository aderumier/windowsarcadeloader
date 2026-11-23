//! The "UAF" cabinet framework (Hot Shots Fire Rescue: Unity 2019.2, IL2CPP, 32-bit):
//! `WAL_ARCADEPC_BOARD=uaf`.
//!
//! `GameAssembly.dll` resolves the game's `DllImport`s with `GetProcAddress`, hooked
//! (`payload-common`'s `mono_imports.rs`):
//!
//! - **IO board** (`io_board.dll`): `openIOBoard` 0 (opened), `readIOBoard` the board's 32-byte
//!   report, `writeIOBoard` (lamps, motors, pumps, tickets) accepted. The game reads an input
//!   as a bit of a report byte, the places its scene gives (`UAF_ControlManager`, read live):
//!   byte 2 the operator's test 0x20, service 0x40, repair 0x80; byte 3 the coins of players
//!   1 and 2 (0x01, 0x02), their start keys (0x40, 0x80), the water tank's level switches (0x04 low, 0x10 normal, 0x08
//!   high: filled, 0x14);
//!   byte 29 the coins of players 3 and 4 (0x01, 0x02), the four cannons' triggers (0x04
//!   0x08 0x10 0x20), the start keys of players 3 and 4 (0x40, 0x80).
//! - **SuperDog dongle** (`api_dsp_windows.dll`, stdcall): logged in, its data file (65524)
//!   zeros: the licence perpetual (`CheckDog`).
//! - **Aim.** The cannons' potentiometers (report bytes 8..24) go through a 13-point grid
//!   calibration; `UAF_ControlManager.MoveGunController(player, current, out target, ...)`
//!   (RVA 0x42E910, the 2022-11-23 v3.3 `GameAssembly.dll`) is replaced by the payload's: the
//!   target is the cannon's place on the picture in the game's centered pixels (x -960..960,
//!   y -540..540 up).
//!
//! - **Network outputs** (`WAL_OUTPUTS`), DemulShooter's code caves in the same build as stubs
//!   (`outputs::hook`) writing [`OUT`]: `UAF_ControlManager.OpenLight` / `CloseLight` (player,
//!   lamp, state: 1 on, 2 blinking) the players' lamps (1 `P1_LmpStart`, 2 `P1_LmpGun`),
//!   `StartSeatShake` / `CloseSeatShake` the seats (`P1_ChairShake`), `StartShakeApparatusOut` /
//!   `CloseShake` the cannons' motors (`P1_GunMotor`), `WaterMistControl` the mist
//!   (`SmokeSwitch`), `SgPlayer.PlayEffectHitted` a hit (player, -1 all: `P1_Damaged` while
//!   playing), the managers' `Update` their instances: the game manager's coins (+0x40,
//!   `P1_Credits`) and players playing (+0x5C), the control manager's water pumps (+0x1E8, a
//!   [player, 2] byte array: the big gun, the small one: `P1_BigGun`, `P1_WaterFire`).
//!
//! Natives: `trigger start coin` (each player), `test service repair` (the operator's).

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log, mono_imports};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

const PLAYERS: usize = 4;
const REPORT: usize = 32;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Start,
    Coin,
    /// An operator's key: its bit of byte 2.
    Key(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("test", Native::Key(0x20)),
    ("service", Native::Key(0x40)),
    ("repair", Native::Key(0x80)),
];

const DEFAULTS: &[(&str, &str)] = &[("b1", "trigger"), ("start", "start"), ("coin", "coin"), ("test", "test"), ("service", "service")];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// Each player's (coin, start, trigger) places: (byte, bit).
const PLACES: [[(usize, u8); 3]; PLAYERS] = [
    [(3, 0x01), (3, 0x40), (29, 0x04)],
    [(3, 0x02), (3, 0x80), (29, 0x08)],
    [(29, 0x01), (29, 0x40), (29, 0x10)],
    [(29, 0x02), (29, 0x80), (29, 0x20)],
];

/// The IO board's report: the inputs now.
fn report() -> [u8; REPORT] {
    let mut r = [0u8; REPORT];
    // the water tank filled: above its low (0x04) and normal (0x10) levels, under its high (0x08)
    r[3] |= 0x14;
    for (player, places) in PLACES.iter().enumerate() {
        map().for_each_pressed(&input(player), |n| match n {
            Native::Coin => r[places[0].0] |= places[0].1,
            Native::Start => r[places[1].0] |= places[1].1,
            Native::Trigger => r[places[2].0] |= places[2].1,
            Native::Key(key) if player == 0 => r[2] |= key,
            Native::Key(_) => {}
        });
    }
    r
}

static READS: AtomicU64 = AtomicU64::new(0);

extern "C" fn open_board() -> i32 {
    log!("uaf: IO board opened");
    0
}

extern "C" fn none() -> i32 {
    0
}

/// `readIOBoard(byte[] data, length)`: the array marshaled as its elements.
extern "C" fn read_board(data: *mut u8, length: i32) -> i32 {
    let r = report();
    let n = (length.max(0) as usize).min(REPORT);
    unsafe {
        std::ptr::write_bytes(data, 0, length.max(0) as usize);
        std::ptr::copy_nonoverlapping(r.as_ptr(), data, n);
    }
    if READS.fetch_add(1, Ordering::Relaxed) == 0 {
        log!("uaf: first report to the game ({length} bytes)");
    }
    length
}

extern "C" fn write_board(_data: *const u8, length: i32) -> i32 {
    length
}

// The SuperDog dongle: stdcall (`DllImport`'s default), the callee pops its arguments

extern "system" fn dog_login(_feature: i32, _scope: *const c_void, _vendor: *const c_void, handle: *mut i32) -> i32 {
    unsafe { *handle = 1 };
    0
}
extern "system" fn dog_logout(_handle: i32) -> i32 {
    0
}
extern "system" fn dog_read(_handle: i32, _file: i32, _offset: i32, length: i32, buffer: *mut u8) -> i32 {
    unsafe { std::ptr::write_bytes(buffer, 0, length.max(0) as usize) };
    0
}
extern "system" fn dog_write(_handle: i32, _file: i32, _offset: i32, _length: i32, _buffer: *const u8) -> i32 {
    0
}
extern "system" fn dog_get_size(_handle: i32, _file: i32, size: *mut i32) -> i32 {
    unsafe { *size = 256 };
    0
}
extern "system" fn dog_encrypt(_handle: i32, _data: *mut u8, _length: i32) -> i32 {
    0
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    let f = |f: usize| Some(f);
    match module {
        "io_board.dll" => match name {
            "openIOBoard" => f(open_board as *const () as usize),
            "readIOBoard" => f(read_board as *const () as usize),
            "writeIOBoard" => f(write_board as *const () as usize),
            _ => f(none as *const () as usize),
        },
        "api_dsp_windows.dll" => match name {
            "dog_login_scope" => f(dog_login as *const () as usize),
            "dog_logout" => f(dog_logout as *const () as usize),
            "dog_read" => f(dog_read as *const () as usize),
            "dog_write" => f(dog_write as *const () as usize),
            "dog_get_size" => f(dog_get_size as *const () as usize),
            "dog_encrypt" => f(dog_encrypt as *const () as usize),
            _ => None,
        },
        _ => None,
    }
}

// The aim

/// `UAF_ControlManager.MoveGunController` and its prologue in the known `GameAssembly.dll`.
const MOVE_GUN: usize = 0x42E910;
const MOVE_GUN_PROLOGUE: [u8; 6] = [0x55, 0x8B, 0xEC, 0x83, 0xE4, 0xF8];

/// Each cannon's last target (x, y bits), kept while it points off the picture.
static TARGETS: [AtomicU64; PLAYERS] = [const { AtomicU64::new(0) }; PLAYERS];

/// `bool MoveGunController(this, int player, Vector3 current, out Vector2 target, float speed,
/// int gun, MethodInfo*)`.
#[allow(clippy::too_many_arguments)]
extern "C" fn move_gun(this: *mut u8, player: i32, _cx: f32, _cy: f32, _cz: f32, target: *mut [f32; 2], _speed: f32, _gun: i32, _method: usize) -> u8 {
    let Some(slot) = usize::try_from(player).ok().filter(|p| *p < PLAYERS) else {
        return 0;
    };
    if let Some((x, y)) = gun::gun_of_player_in_picture(slot) {
        let t = [(x - 0.5) * 1920.0, (0.5 - y) * 1080.0];
        TARGETS[slot].store((t[0].to_bits() as u64) << 32 | t[1].to_bits() as u64, Ordering::Relaxed);
    }
    let bits = TARGETS[slot].load(Ordering::Relaxed);
    unsafe {
        *target = [f32::from_bits((bits >> 32) as u32), f32::from_bits(bits as u32)];
        // the game's "moved" flag (`flag`)
        *this.add(0x1D4) = 1;
    }
    1
}

/// Jumps `MoveGunController` to the payload's, once `GameAssembly.dll` is loaded.
fn replace_move_gun() {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    for _ in 0..20000 {
        let base = unsafe { GetModuleHandleW(windows_sys::w!("GameAssembly.dll")) } as usize;
        if base == 0 {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        let code = (base + MOVE_GUN) as *mut u8;
        if unsafe { std::slice::from_raw_parts(code, MOVE_GUN_PROLOGUE.len()) } != MOVE_GUN_PROLOGUE {
            log!("uaf: unknown GameAssembly.dll: the aim not replaced");
            return;
        }
        let rel = (move_gun as *const () as usize).wrapping_sub(code as usize + 5) as u32;
        let mut jump = [0xE9u8, 0, 0, 0, 0];
        jump[1..].copy_from_slice(&rel.to_le_bytes());
        let mut old = 0;
        unsafe {
            VirtualProtect(code.cast(), jump.len(), PAGE_EXECUTE_READWRITE, &mut old);
            std::ptr::copy_nonoverlapping(jump.as_ptr(), code, jump.len());
            VirtualProtect(code.cast(), jump.len(), old, &mut old);
        }
        log!("uaf: the cannons' aim is the payload's");
        return;
    }
}

// The network outputs

/// What the stubs write: the lamps (`[player * 4 + lamp]`, bytes 0..16), the seats' (16..20) and
/// the cannons' motors (20..24), the hits (24..28), the mist (dword 7), the game manager (dword
/// 8), the control manager (dword 9).
static OUT: [AtomicU32; 10] = [const { AtomicU32::new(0) }; 10];

#[cfg(target_arch = "x86")]
fn outputs(base: usize) {
    use wal_payload_common::outputs::hook::stub;
    use wal_payload_common::outputs::{mem, poll, set};
    let out = OUT.as_ptr() as u32;
    let a = |o: u32| (out + o).to_le_bytes();
    // pushfd; push eax; push ecx; mov ecx, [ebp+0C] (player); cmp ecx, 3; ja; mov eax, [ebp+10]
    // (lamp); cmp eax, 3; ja; lea ecx, [eax+ecx*4]; mov eax, [ebp+14] (state) or xor eax, eax;
    // mov [ecx+lamps], al; pop ecx; pop eax; popfd
    let light = |on: bool| {
        let mut c = vec![0x9C, 0x50, 0x51, 0x8B, 0x4D, 0x0C, 0x83, 0xF9, 0x03, 0x77, if on { 20 } else { 19 }];
        c.extend_from_slice(&[0x8B, 0x45, 0x10, 0x83, 0xF8, 0x03, 0x77, if on { 12 } else { 11 }, 0x8D, 0x0C, 0x88]);
        c.extend_from_slice(if on { &[0x8B, 0x45, 0x14][..] } else { &[0x31, 0xC0][..] });
        c.extend_from_slice(&[0x88, 0x81]);
        c.extend_from_slice(&a(0));
        c.extend_from_slice(&[0x59, 0x58, 0x9D]);
        c
    };
    // pushfd; push eax; mov eax, [ebp+0C] (player); cmp eax, 3; ja; mov byte [eax+motors], on;
    // pop eax; popfd
    let motor = |first: u32, on: u8| {
        let mut c = vec![0x9C, 0x50, 0x8B, 0x45, 0x0C, 0x83, 0xF8, 0x03, 0x77, 0x07, 0xC6, 0x80];
        c.extend_from_slice(&a(first));
        c.extend_from_slice(&[on, 0x58, 0x9D]);
        c
    };
    // pushfd; push eax; mov eax, [ebp+disp]; mov [dword], eax; pop eax; popfd
    let store = |disp: u8, dword: u32| {
        let mut c = vec![0x9C, 0x50, 0x8B, 0x45, disp, 0xA3];
        c.extend_from_slice(&a(dword * 4));
        c.extend_from_slice(&[0x58, 0x9D]);
        c
    };
    // pushfd; push eax; mov eax, [ebp+10] (player); cmp eax, -1; jne; mov dword [hits],
    // 0x01010101; jmp; cmp eax, 3; ja; mov byte [eax+hits], 1; pop eax; popfd
    let hit = {
        let mut c = vec![0x9C, 0x50, 0x8B, 0x45, 0x10, 0x83, 0xF8, 0xFF, 0x75, 0x0C, 0xC7, 0x05];
        c.extend_from_slice(&a(24));
        c.extend_from_slice(&[1, 1, 1, 1, 0xEB, 0x0C, 0x83, 0xF8, 0x03, 0x77, 0x07, 0xC6, 0x80]);
        c.extend_from_slice(&a(24));
        c.extend_from_slice(&[1, 0x58, 0x9D]);
        c
    };
    let sites: [(usize, &[u8], Vec<u8>); 10] = [
        (0x428844, &[0x8B, 0x40, 0x08, 0x8B, 0x75, 0x10], light(true)),
        (0x428A34, &[0x8B, 0x40, 0x08, 0x8B, 0x7D, 0x10], light(false)),
        (0x430CB8, &[0x8B, 0x87, 0x0C, 0x02, 0x00, 0x00], motor(16, 1)),
        (0x430E48, &[0x8B, 0x87, 0x0C, 0x02, 0x00, 0x00], motor(16, 0)),
        (0x430A48, &[0x8B, 0x87, 0x08, 0x02, 0x00, 0x00], motor(20, 1)),
        (0x430978, &[0x8B, 0x87, 0x08, 0x02, 0x00, 0x00], motor(20, 0)),
        (0x42FA03, &[0x83, 0xE4, 0xF0, 0x83, 0xEC, 0x38], store(0x0C, 7)),
        (0x3BD333, &[0x83, 0xE4, 0xF0, 0x83, 0xEC, 0x28], hit),
        (0x43A14A, &[0x8B, 0x7D, 0x08, 0x83, 0xC4, 0x04], store(0x08, 8)),
        (0x428DB2, &[0x8B, 0x75, 0x08, 0x33, 0xD2], store(0x08, 9)),
    ];
    for (rva, original, code) in sites {
        if unsafe { stub(base + rva, original, |_| code) }.is_none() {
            log!("uaf: outputs: unknown code at {rva:#x}, no outputs");
            return;
        }
    }
    log!("uaf: outputs");
    let byte = |i: usize| unsafe { *(OUT.as_ptr() as *const u8).add(i) };
    poll("hot shots fire rescue", 16, move || {
        // a lamp's state 2: blinking, 100 ms on, 100 ms off (DemulShooter's)
        let blink = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() / 100 % 2) as i32;
        let lamp = |v: u8| if v == 2 { 1 - blink } else { v as i32 };
        let game = OUT[8].load(Ordering::Relaxed) as usize;
        let (coins, gaming) = if game != 0 { (mem::u32(game + 0x40) as usize, mem::u32(game + 0x5C) as usize) } else { (0, 0) };
        let pumps = OUT[9].load(Ordering::Relaxed) as usize;
        let pumps = if pumps != 0 { mem::u32(pumps + 0x1E8) as usize } else { 0 };
        // the pumps' [player, 2] array: its bounds (+8) the lengths
        let (rows, row) = if pumps != 0 { let b = mem::u32(pumps + 8) as usize; if b != 0 { (mem::u32(b) as usize, mem::u32(b + 8) as usize) } else { (0, 0) } } else { (0, 0) };
        for p in 0..PLAYERS {
            let n = |f: &str| format!("P{}_{f}", p + 1);
            set(&n("LmpStart"), lamp(byte(p * 4 + 1)));
            set(&n("LmpGun"), lamp(byte(p * 4 + 2)));
            set(&n("ChairShake"), byte(16 + p) as i32);
            set(&n("GunMotor"), byte(20 + p) as i32);
            let (big, small) = if p < rows.min(PLAYERS) { (mem::u8(pumps + 0x10 + row * p), mem::u8(pumps + 0x10 + row * p + 1)) } else { (0, 0) };
            set(&n("BigGun"), big as i32);
            set(&n("WaterFire"), small as i32);
            let playing = gaming != 0 && p < mem::u32(gaming + 0xC) as usize && mem::u8(gaming + 0x10 + p) == 1;
            if byte(24 + p) == 1 && playing {
                set(&n("Damaged"), 1);
            }
            if byte(24 + p) == 1 {
                unsafe { *(OUT.as_ptr() as *mut u8).add(24 + p) = 0 };
            }
            let credits = if coins != 0 && p < mem::u32(coins + 0xC) as usize { mem::u32(coins + 0x10 + 4 * p) as i32 } else { 0 };
            set(&n("Credits"), credits);
        }
        set("SmokeSwitch", OUT[7].load(Ordering::Relaxed) as i32);
    });
}

/// The outputs' stubs, once `GameAssembly.dll` is loaded.
fn start_outputs() {
    if !wal_payload_common::outputs::enabled() {
        return;
    }
    for _ in 0..20000 {
        let base = unsafe { GetModuleHandleW(windows_sys::w!("GameAssembly.dll")) } as usize;
        if base != 0 {
            #[cfg(target_arch = "x86")]
            outputs(base);
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

pub fn init() {
    mono_imports::hook(resolve);
    std::thread::spawn(replace_move_gun);
    std::thread::spawn(start_outputs);
    log!("uaf: IO board, SuperDog, aim");
}
