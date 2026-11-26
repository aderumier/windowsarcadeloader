//! Sword Art Online Arcade: Deep Explorer (Bandai Namco, 2019, Unity 2017.4 / Mono, 64-bit),
//! `game\link.exe` (ROM `SAO134-1-NA-MPR0-L11`). The BNA1 cabinet's satellite (the game seat;
//! the terminal is the shop's card printer). The game imports nothing of ours: the payload is
//! loaded at the executable's entry point (`wal-loader64`, profile `payload_loader`), and
//! reproduces bananatools' `saohook` (public domain):
//!
//! - satellite or terminal: the game asks Unity's `SystemInfo` (an Intel processor and 6000 MB
//!   or more: the satellite), `saohook/systype.c`'s processor name and memory size, answered in
//!   `UnityPlayer.dll` (`HKLM\...\CentralProcessor\0\ProcessorNameString`, `GlobalMemoryStatusEx`).
//! - The Mono `DllImport`s of the game's plugins (`link_Data\Plugins`) are resolved through
//!   [`mono_imports`]; each library's first function hooks it (bananatools' `saohook/unity.c`
//!   hooks them as Unity loads them):
//!   - `bnusio.dll`: the USIO board, the real library on our emulated WinUSB board (`usio.rs`;
//!     bananatools replaces the library's functions, `board/usio.c`, the same switch bits).
//!   - `nbamUsbFinder_sup.dll`: the USB dongle's serial (`systype.c`'s `GetSerialNumber`),
//!     `2825 1 3 04 nnnn` (model 2825, region 1, type 3, 04 the satellite).
//!   - `bnReader.dll` (Sega's `libamw.dll`): the Aime reader on COM1 and its display on COM2
//!     (`board/sg-reader.c`, `board/vfd.c`), libamw's log volume.
//!   - `bnAMUpdater.dll`: the AMCUS auth daemon (`IAuth`) in-process (`amcus/iauth.c`), its
//!     ALL.Net answer pointing at the game server (`wal_namcoes3srv::sao`, ARTEMiS' `titles/sao`).
//! - The touch panel (the menus: `Input.GetMouseButton(0)`) from the cursor (`touch.rs`).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, Once};
use std::time::Duration;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log, mono_imports, sgreader};
use wal_protocol::{Axis, StickState};

use crate::{GAME_BASE, module, usio};

pub(crate) const GAME: &str = "link.exe";

/// The cabinet's dongle serial (bananatools' `dist/sao/bananatools.ini`; the game wants model
/// 2825 and 04, the satellite), its network id `ABLN` + the serial's last 7 digits (ARTEMiS'
/// `NETID_PREFIX`).
const SERIAL: &str = "282513040001";
const NET_ID: &str = "ABLN3040001";
const GAME_ID: &str = "SDEW";
const GAME_CD: &str = "SAO1";

/// Is this Sword Art Online Arcade's `link.exe` (its Unity data folder next to it)?
pub(crate) fn is_sao(exe: &str) -> bool {
    exe.eq_ignore_ascii_case(GAME)
        && std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("link_Data").is_dir())).unwrap_or(false)
}

// --- satellite: processor name and memory size (bananatools' systype.c) ---

/// bananatools' satellite processor name: Unity's `SystemInfo.processorType` has "Intel".
const CPU_NAME: &str = "Intel(R) Genuine Snake Oil";

static REG_QUERY_VALUE_EX_W: AtomicUsize = AtomicUsize::new(0);
static REG_QUERY_VALUE_EX_A: AtomicUsize = AtomicUsize::new(0);
static GLOBAL_MEMORY_STATUS_EX: AtomicUsize = AtomicUsize::new(0);
static GET_SYSTEM_METRICS: AtomicUsize = AtomicUsize::new(0);

/// `GetSystemMetrics(SM_DIGITIZER)`: an integrated, multi-input touch panel, ready
/// (bananatools' `hooklib/touch.c`, from spicetools' wintouchemu): Unity's
/// `Input.touchSupported`, the game's touch panel check (error 21-1 "TOUCH PANEL DISCONNECT").
unsafe extern "system" fn get_system_metrics(index: i32) -> i32 {
    const SM_DIGITIZER: i32 = 94;
    if index == SM_DIGITIZER {
        return 0x01 | 0x02 | 0x40 | 0x80;
    }
    let real: unsafe extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(GET_SYSTEM_METRICS.load(Ordering::Relaxed)) };
    unsafe { real(index) }
}

/// Writes `data` as a registry value's answer (`ERROR_MORE_DATA` when it does not fit).
unsafe fn reg_answer(data: &[u8], kind: *mut u32, out: *mut u8, size: *mut u32) -> i32 {
    const REG_SZ: u32 = 1;
    unsafe {
        if !kind.is_null() {
            *kind = REG_SZ;
        }
        if size.is_null() {
            return if out.is_null() { 0 } else { 87 };
        }
        let room = *size as usize;
        *size = data.len() as u32;
        if out.is_null() {
            return 0;
        }
        if room < data.len() {
            return 234; // ERROR_MORE_DATA
        }
        std::ptr::copy_nonoverlapping(data.as_ptr(), out, data.len());
    }
    0
}

unsafe extern "system" fn reg_query_value_ex_w(key: usize, name: *const u16, reserved: *mut u32, kind: *mut u32, out: *mut u8, size: *mut u32) -> i32 {
    if !name.is_null() {
        let len = (0..).take_while(|&i| unsafe { *name.add(i) } != 0).count();
        if String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(name, len) }).eq_ignore_ascii_case("ProcessorNameString") {
            let data: Vec<u8> = CPU_NAME.encode_utf16().chain(Some(0)).flat_map(u16::to_le_bytes).collect();
            return unsafe { reg_answer(&data, kind, out, size) };
        }
    }
    type F = unsafe extern "system" fn(usize, *const u16, *mut u32, *mut u32, *mut u8, *mut u32) -> i32;
    let real: F = unsafe { std::mem::transmute(REG_QUERY_VALUE_EX_W.load(Ordering::Relaxed)) };
    unsafe { real(key, name, reserved, kind, out, size) }
}

unsafe extern "system" fn reg_query_value_ex_a(key: usize, name: *const u8, reserved: *mut u32, kind: *mut u32, out: *mut u8, size: *mut u32) -> i32 {
    if !name.is_null() && unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes().eq_ignore_ascii_case(b"ProcessorNameString") {
        let data: Vec<u8> = CPU_NAME.bytes().chain(Some(0)).collect();
        return unsafe { reg_answer(&data, kind, out, size) };
    }
    type F = unsafe extern "system" fn(usize, *const u8, *mut u32, *mut u32, *mut u8, *mut u32) -> i32;
    let real: F = unsafe { std::mem::transmute(REG_QUERY_VALUE_EX_A.load(Ordering::Relaxed)) };
    unsafe { real(key, name, reserved, kind, out, size) }
}

/// `GlobalMemoryStatusEx`: 8000 MB of memory (bananatools' satellite; the game wants 6000).
unsafe extern "system" fn global_memory_status_ex(status: *mut u8) -> i32 {
    type F = unsafe extern "system" fn(*mut u8) -> i32;
    let real: F = unsafe { std::mem::transmute(GLOBAL_MEMORY_STATUS_EX.load(Ordering::Relaxed)) };
    let ok = unsafe { real(status) };
    if ok != 0 && !status.is_null() {
        // MEMORYSTATUSEX: length, load, total physical (u64 at +8)
        unsafe { *(status.add(8) as *mut u64) = 8000 << 20 };
    }
    ok
}

fn hook_satellite() {
    let unity = module("UnityPlayer.dll");
    if unity == 0 {
        return log!("sao: UnityPlayer.dll not loaded, cabinet type left alone");
    }
    let hooks: [(&str, &str, usize, &AtomicUsize); 4] = [
        ("user32.dll", "GetSystemMetrics", get_system_metrics as *const () as usize, &GET_SYSTEM_METRICS),
        ("advapi32.dll", "RegQueryValueExW", reg_query_value_ex_w as *const () as usize, &REG_QUERY_VALUE_EX_W),
        ("advapi32.dll", "RegQueryValueExA", reg_query_value_ex_a as *const () as usize, &REG_QUERY_VALUE_EX_A),
        ("kernel32.dll", "GlobalMemoryStatusEx", global_memory_status_ex as *const () as usize, &GLOBAL_MEMORY_STATUS_EX),
    ];
    for (dll, name, f, orig) in hooks {
        match unsafe { iat::hook_module(unity, dll, name, f) } {
            Some(o) => orig.store(o, Ordering::Relaxed),
            None => log!("sao: UnityPlayer {name} not hooked"),
        }
    }
    log!("sao: satellite cabinet ({CPU_NAME}, 8000 MB)");
}

// --- the USB dongle (nbamUsbFinder_sup.dll) ---

/// `GetSerialNumber(usercode, out)`: the serial, ANSI (systype.c's `my_GetSerialNumber`).
unsafe extern "system" fn get_serial_number(_usercode: u64, out: *mut u8) -> i32 {
    if !out.is_null() {
        unsafe {
            std::ptr::copy_nonoverlapping(SERIAL.as_ptr(), out, SERIAL.len());
            *out.add(SERIAL.len()) = 0;
        }
    }
    0
}

/// `Initialize`, `Release`: no USB key to look for.
unsafe extern "system" fn dongle_nothing() {}

/// `Read(slot, out)`: the key's data slots, empty.
unsafe extern "system" fn dongle_read(_slot: u8, _out: *mut u8) -> i32 {
    0
}

// --- the AMCUS auth daemon (bnAMUpdater.dll's CoCreateInstance) ---

static CO_CREATE_INSTANCE: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn co_create_instance(clsid: *const u8, outer: *mut std::ffi::c_void, context: u32, iid: *const u8, out: *mut *mut std::ffi::c_void) -> i32 {
    if !clsid.is_null() && unsafe { std::slice::from_raw_parts(clsid, 16) } == crate::amcus::CLSID {
        if !out.is_null() {
            let object = crate::amcus::daemon(&crate::amcus::OBJECT_NEW);
            unsafe { *out = (object as *const crate::amcus::Object).cast_mut().cast() };
        }
        log!("sao: in-process AMCUS auth (IAuth)");
        return 0;
    }
    type F = unsafe extern "system" fn(*const u8, *mut std::ffi::c_void, u32, *const u8, *mut *mut std::ffi::c_void) -> i32;
    let real: F = unsafe { std::mem::transmute(CO_CREATE_INSTANCE.load(Ordering::Relaxed)) };
    unsafe { real(clsid, outer, context, iid, out) }
}

fn hook_updater() {
    let updater = module("bnAMUpdater.dll");
    match unsafe { iat::hook_module(updater, "ole32.dll", "CoCreateInstance", co_create_instance as *const () as usize) } {
        Some(o) => CO_CREATE_INSTANCE.store(o, Ordering::Relaxed),
        None => log!("sao: bnAMUpdater CoCreateInstance not hooked"),
    }
}

// --- Sega's libamw (bnReader.dll): Aime reader, display, log volume ---

static GET_VOLUME_INFORMATION_A: AtomicUsize = AtomicUsize::new(0);

/// libamw looks for its log volume, labelled `SEGA_AM_LOG`: the dump's `L` folder (drive L:,
/// profile `drives`), as JoJo's Bizarre Adventure's libamw.
unsafe extern "system" fn get_volume_information_a(root: *const u8, name: *mut u8, size: u32, serial: *mut u32, max: *mut u32, flags: *mut u32, fs: *mut u8, fs_size: u32) -> i32 {
    const LABEL: &[u8] = b"SEGA_AM_LOG\0";
    let path = if root.is_null() { &[][..] } else { unsafe { std::ffi::CStr::from_ptr(root.cast()) }.to_bytes() };
    if path.eq_ignore_ascii_case(b"\\\\.\\L:\\") || path.eq_ignore_ascii_case(b"L:\\") {
        unsafe {
            if !name.is_null() && size as usize >= LABEL.len() {
                std::ptr::copy_nonoverlapping(LABEL.as_ptr(), name, LABEL.len());
            }
            if !serial.is_null() {
                *serial = 0x5741_4c00;
            }
            if !max.is_null() {
                *max = 255;
            }
            if !flags.is_null() {
                *flags = 0;
            }
            if !fs.is_null() && fs_size >= 5 {
                std::ptr::copy_nonoverlapping(b"NTFS\0".as_ptr(), fs, 5);
            }
        }
        return 1;
    }
    type F = unsafe extern "system" fn(*const u8, *mut u8, u32, *mut u32, *mut u32, *mut u32, *mut u8, u32) -> i32;
    let real: F = unsafe { std::mem::transmute(GET_VOLUME_INFORMATION_A.load(Ordering::Relaxed)) };
    unsafe { real(root, name, size, serial, max, flags, fs, fs_size) }
}

fn hook_reader() {
    let amw = module("libamw.dll");
    if amw == 0 {
        return log!("sao: libamw.dll not loaded, no card reader");
    }
    match unsafe { iat::hook_module(amw, "kernel32.dll", "GetVolumeInformationA", get_volume_information_a as *const () as usize) } {
        Some(o) => GET_VOLUME_INFORMATION_A.store(o, Ordering::Relaxed),
        None => log!("sao: libamw GetVolumeInformationA not hooked"),
    }
    sgreader::init("COM1", &[amw]);
    crate::vfd::init("COM2", &[amw]);
    card();
}

/// The player's Aime card (`WAL_NAMCOES3_ACCESS_CODE`, else one made once and kept in
/// `F:\aime.txt`): on the reader 3 seconds after the card button, and by itself for 20 seconds
/// after a coin or start (`WAL_NAMCOES3_AUTO_CARD`, default on: the game asks for the card after
/// its title).
fn card() {
    let code = crate::bpreader::player_card_in("F:\\aime.txt").access_code.clone();
    let auto = std::env::var("WAL_NAMCOES3_AUTO_CARD").map_or(true, |v| v.trim() != "0");
    log!("sao: Aime card {code}{}", if auto { ", inserted after start" } else { "" });
    std::thread::spawn(move || {
        let (mut start_held, mut card_held) = (false, false);
        loop {
            let input = wal_payload_common::input(0);
            let start = input.pressed(wal_protocol::button::START) || input.pressed(wal_protocol::button::COIN);
            let card = input.pressed(wal_protocol::button::CARD);
            if auto && start && !start_held {
                sgreader::insert(&code, Duration::from_secs(20));
            }
            if card && !card_held {
                sgreader::insert(&code, Duration::from_secs(3));
            }
            (start_held, card_held) = (start, card);
            std::thread::sleep(Duration::from_millis(50));
        }
    });
}

// --- the USIO board's inputs (bananatools' saohook/usio.c + saoio) ---

#[derive(Clone, Copy, PartialEq, Eq)]
enum Native {
    Test,
    Service,
    Coin,
    Touch,
    Switch(u32),
}

/// `GetSwIn`'s bits (the game's `BnamPeripheral.getMaskBit`; bananatools' `sao_usio_poll`).
const SW_TEST: u32 = 0x80;
const SW_SERVICE: u32 = 0x4000;
const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("touch", Native::Touch),
    // the three attack buttons, one per party member
    ("button1", Native::Switch(0x20_0000)),
    ("button2", Native::Switch(0x10_0000)),
    ("button3", Native::Switch(0x8_0000)),
    // the stick's buttons: step, guard, the third
    ("trigger1", Native::Switch(0x100)),
    ("trigger2", Native::Switch(0x80_0000)),
    ("trigger3", Native::Switch(0x40_0000)),
    // the operator buttons (test menu)
    ("menuup", Native::Switch(0x2000)),
    ("menudown", Native::Switch(0x1000)),
    ("enter", Native::Switch(0x200)),
];

/// Virtual stick -> cabinet; profiles override it with `native_map`.
const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("b1", "button1"),
    ("b2", "button2"),
    ("b3", "button3"),
    ("b4", "trigger1"),
    ("b5", "trigger2"),
    ("b6", "trigger3"),
    ("kp0", "touch"),
    ("up", "menuup"),
    ("down", "menudown"),
    ("start", "enter"),
];

struct State {
    map: ButtonMap<Native>,
    coins: u16,
    services: u16,
    coin_held: bool,
    service_held: bool,
    test_on: bool,
    test_held: bool,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
/// The touch native held (the USIO thread polls the sticks).
static TOUCH: AtomicBool = AtomicBool::new(false);

fn usio_poll() -> usio::Inputs {
    let mut guard = STATE.lock().unwrap();
    let s = guard.get_or_insert_with(|| State {
        map: ButtonMap::new(NATIVES, DEFAULT_MAP),
        coins: 0,
        services: 0,
        coin_held: false,
        service_held: false,
        test_on: false,
        test_held: false,
    });
    let stick: StickState = wal_payload_common::input(0);
    let mut sw = 0u32;
    let (mut coin, mut test, mut touch, mut service) = (false, false, false, false);
    s.map.for_each_pressed(&stick, |n| match n {
        Native::Test => test = true,
        Native::Coin => coin = true,
        Native::Service => service = true,
        Native::Touch => touch = true,
        Native::Switch(bit) => sw |= bit,
    });
    TOUCH.store(touch, Ordering::Relaxed);
    // the test switch latches (saoio's test toggle)
    if test && !s.test_held {
        s.test_on = !s.test_on;
        log!("sao: test switch {}", if s.test_on { "on" } else { "off" });
    }
    s.test_held = test;
    if s.test_on {
        sw |= SW_TEST;
    }
    // service: the switch, and a service credit per press (saoio)
    if service {
        sw |= SW_SERVICE;
    }
    if service && !s.service_held {
        s.services = s.services.wrapping_add(1) & 0x3FFF;
    }
    s.service_held = service;
    if coin && !s.coin_held {
        s.coins = s.coins.wrapping_add(1) & 0x3FFF;
    }
    s.coin_held = coin;
    // the stick: analog 0 (x, right high) and 1 (y, up high), the d-pad pushes it
    let mut analog = [0x8000u16; 8];
    analog[0] = stick.axis_u16(Axis::LeftX);
    analog[1] = !stick.axis_u16(Axis::LeftY);
    for (pressed, index, value) in [
        (stick.pressed(wal_protocol::button::UP), 1, 0xFFFF),
        (stick.pressed(wal_protocol::button::DOWN), 1, 0),
        (stick.pressed(wal_protocol::button::LEFT), 0, 0),
        (stick.pressed(wal_protocol::button::RIGHT), 0, 0xFFFF),
    ] {
        if pressed {
            analog[index] = value;
        }
    }
    usio::Inputs { switches: sw, coins: [s.coins, s.services], analog }
}

fn hook_usio() {
    let bnusio = module("bnusio.dll");
    if bnusio == 0 {
        return log!("sao: bnusio.dll not loaded, no I/O board");
    }
    usio::init(bnusio, usio_poll);
    // the game wants firmware 120..=126; bananatools' board name
    usio::set_memory(0x12, &126u16.to_le_bytes());
    usio::set_memory(0x1800, b"NBGI.REAL_USIO\0");
}

// --- free play (the test mode's coin settings) ---

/// The coin settings' file of the test mode (`SaveDataAccess`): `F:\TestModeData\2825-104_
/// CoinOpt.<generation>.bin`, the newest generation read: an AES-128 key and IV, then
/// AES-128-CBC (PKCS#7) of the 44-byte `TypeBase.SaveCoin` (Marshal layout, bools as 4 bytes),
/// whose bytes 8-9 are the sum of its bytes.
mod coin_settings {
    use aes::Aes128;
    use aes::cipher::generic_array::GenericArray;
    use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};

    const DIR: &str = "F:\\TestModeData";
    const SIZE: usize = 44;
    /// `freePlay`'s offset.
    const FREE_PLAY: usize = 12;

    /// The test mode's defaults (`SaveDataManager`'s constructor): version 3, continue bonus,
    /// coin value 1, ticket plan 10, card cost 1, continue cost 1, daily bonus, first campaign 10.
    fn defaults() -> [u8; SIZE] {
        let mut d = [0u8; SIZE];
        let put = |d: &mut [u8; SIZE], at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        d[10..12].copy_from_slice(&3u16.to_le_bytes());
        for (at, v) in [(16, 1), (20, 1), (24, 10), (28, 1), (32, 1), (36, 1), (40, 10)] {
            put(&mut d, at, v);
        }
        d
    }

    fn checksum(d: &mut [u8]) {
        d[8] = 0;
        d[9] = 0;
        let sum = d.iter().fold(0u16, |s, &b| s.wrapping_add(b as u16));
        d[8..10].copy_from_slice(&sum.to_le_bytes());
    }

    fn decrypt(file: &[u8]) -> Option<Vec<u8>> {
        if file.len() <= 32 || (file.len() - 32) % 16 != 0 {
            return None;
        }
        let aes = Aes128::new_from_slice(&file[..16]).ok()?;
        let mut prev: [u8; 16] = file[16..32].try_into().ok()?;
        let mut out = Vec::new();
        for chunk in file[32..].chunks(16) {
            let mut b = GenericArray::clone_from_slice(chunk);
            aes.decrypt_block(&mut b);
            out.extend(b.iter().zip(prev).map(|(x, p)| x ^ p));
            prev.copy_from_slice(chunk);
        }
        let pad = *out.last()? as usize;
        (1..=16).contains(&pad).then(|| out[..out.len() - pad].to_vec())
    }

    fn encrypt(data: &[u8], key: [u8; 16], iv: [u8; 16]) -> Vec<u8> {
        let aes = Aes128::new_from_slice(&key).unwrap();
        let pad = 16 - data.len() % 16;
        let mut plain = data.to_vec();
        plain.extend(std::iter::repeat_n(pad as u8, pad));
        let mut out = [key, iv].concat();
        let mut prev = iv;
        for chunk in plain.chunks(16) {
            let mut b = GenericArray::clone_from_slice(chunk);
            b.iter_mut().zip(prev).for_each(|(x, p)| *x ^= p);
            aes.encrypt_block(&mut b);
            prev.copy_from_slice(&b);
            out.extend_from_slice(&b);
        }
        out
    }

    /// Sets free play in the newest coin settings (a first generation with the defaults when
    /// there is none).
    pub(super) fn set_free_play(on: bool) {
        let _ = std::fs::create_dir_all(DIR);
        let newest = std::fs::read_dir(DIR)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_prefix("2825-104_CoinOpt.")?.strip_suffix(".bin")?.parse::<u32>().ok()
            })
            .max();
        let generation = newest.unwrap_or(1);
        let path = format!("{DIR}\\2825-104_CoinOpt.{generation}.bin");
        let current = newest.and_then(|_| std::fs::read(&path).ok()).and_then(|f| decrypt(&f)).filter(|d| d.len() == SIZE);
        let mut data = current.clone().unwrap_or_else(|| defaults().to_vec());
        data[FREE_PLAY..FREE_PLAY + 4].copy_from_slice(&(on as u32).to_le_bytes());
        checksum(&mut data);
        if current.as_deref() == Some(&data[..]) {
            return;
        }
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.as_nanos());
        let mut key = [0u8; 16];
        let mut iv = [0u8; 16];
        for i in 0..16 {
            key[i] = (seed >> (i % 8 * 8)) as u8 ^ (i as u8).wrapping_mul(77);
            iv[i] = (seed >> ((i + 3) % 8 * 8)) as u8 ^ (i as u8).wrapping_mul(31);
        }
        match std::fs::write(&path, encrypt(&data, key, iv)) {
            Ok(()) => wal_payload_common::log!("sao: free play {}", if on { "on" } else { "off" }),
            Err(e) => wal_payload_common::log!("sao: {path}: {e}"),
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn round_trip() {
            let mut d = super::defaults();
            super::checksum(&mut d);
            let f = super::encrypt(&d, [1; 16], [2; 16]);
            assert_eq!(super::decrypt(&f).unwrap(), d.to_vec());
        }
    }
}

// --- the Mono imports ---

static USIO: Once = Once::new();
static READER: Once = Once::new();
static UPDATER: Once = Once::new();

fn resolve(name: &str, module: &str) -> Option<usize> {
    match module {
        "bnusio.dll" => USIO.call_once(hook_usio),
        "bnreader.dll" => READER.call_once(hook_reader),
        "bnamupdater.dll" => UPDATER.call_once(hook_updater),
        "nbamusbfinder_sup.dll" => {
            return match name {
                "GetSerialNumber" => Some(get_serial_number as *const () as usize),
                "Initialize" | "Release" => Some(dongle_nothing as *const () as usize),
                "Read" => Some(dongle_read as *const () as usize),
                _ => None,
            };
        }
        _ => {}
    }
    None
}

pub(crate) fn attach(base: usize) {
    GAME_BASE.store(base, Ordering::Relaxed);
    hook_satellite();
    crate::amcus::settle_idle();
    // the cabinet's network (bananatools' netenv: subnet 192.168.170.0, the cabinet .11, the
    // router .1): the game wants 170 as its address's third byte (error 5-2 "CONTENT ROUTER
    // MISMATCH")
    crate::amcus::set_cabinet_network(["192.168.170.1", "192.168.170.1", "192.168.170.11", "255.255.255.0", "192.168.170.1", "192.168.170.1"]);
    crate::amcus::set_identity(crate::amcus::Identity {
        game_id: GAME_ID,
        game_cd: GAME_CD,
        game_ver: (33, 11),
        serial: SERIAL,
        scheme: "http://",
        path: "/",
        net_id: NET_ID,
    });
    // the common free play option (`WAL_NAMCOES3_FREEPLAY`, else `WAL_FREEPLAY`)
    if let Some(on) = wal_payload_common::free_play("WAL_NAMCOES3_FREEPLAY") {
        coin_settings::set_free_play(on);
    }
    let verbose = std::env::var("WAL_NAMCOES3_SERVER_LOG").is_ok_and(|v| v == "1");
    wal_namcoes3srv::sao::start(&crate::amcus::game_server(), "F:\\saosrv", |line| log!("{line}"), verbose);
    mono_imports::hook(resolve);
    let unity = module("UnityPlayer.dll");
    wal_payload_common::input_filter::init(unity, |_| false);
    crate::touch::init(unity, &TOUCH);
}
