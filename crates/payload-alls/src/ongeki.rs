//! O.N.G.E.K.I. (Sega, ALLS, 2018; game id SDDT): a 64-bit Mono Unity rhythm game (`mu3.exe`)
//! beside amdaemon (segatools' mu3hook, public domain):
//!
//! - the "Type 4" USB I/O board (amdaemon's, `io4.rs`): the cabinet's ten buttons and the lever
//!   ([`switches`], [`lever`])
//! - COM1: the 837-15396 Aime reader (amdaemon's, `aime.rs`)
//! - COM2: the VFD (the cabinet's character display, 200-6275, segatools' `board/vfd.c`): its
//!   commands (sync `1B` / `1A`, command, data) never answered but the version's (`5B`)
//! - COM3: the 837-15093-06 LED board (the side buttons' and pillars' lights, payload-common
//!   `led.rs`)
//! - the title server (`ongeki/server.rs`), answered in amdaemon's ALL.Net server
//! - the game's settings backup ([`backup_settings`]): a lone cabinet (its factory setting, group
//!   A without being the group's reference cabinet, waits forever at boot for the reference
//!   cabinet's settings: group OFF instead) and the lever's range calibrated (the factory's
//!   32768-32768: "lever range NG", the lever dead)
//! - the game's clock (Mono's): the cabinet's (Japan's, the time warp's), as amdaemon's
//! - the title server's requests plain ([`plain_requests`]: Re:Fresh 1.52 encrypts them)
//! - the three QR code card readers (webcams) absent: the player's webcams hidden (the game opened
//!   them all and decoded their frames all the time)
//!
//! The controls (player 1's virtual stick, segatools' XInput layout): the lever on the left
//! stick (or held fully left / right on l2 / r2: buttons 8 / 7); the left hand's red, green and
//! blue buttons on the d-pad's left, up and right, the right hand's on a pad's X, Y and A
//! (buttons 1, 2, 5), start too (the right red button decides in the menus); the side buttons
//! (the walls) on l1 / r1 (buttons 4 / 3); the menu buttons on the d-pad's down (left) and B
//! (right).

mod server;

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::mono::{self, P};
use wal_payload_common::{input, log};
use wal_protocol::{Axis, button as b};

/// The I/O board's switch words (players 1 and 2) for the cabinet's buttons (segatools'
/// mu3hook `io4.c`); the side buttons read 1 while released.
pub(crate) fn switches(sw: &mut [u16; 2]) {
    let p1 = input(0).buttons;
    let pressed = |mask: u32| p1 & mask != 0;
    // left red, green, blue; right red, blue (player 1's word)
    for (mask, bit) in [(b::LEFT, 0), (b::UP, 5), (b::RIGHT, 4), (b::B1 | b::START, 1), (b::B5, 15)] {
        if pressed(mask) {
            sw[0] |= 1 << bit;
        }
    }
    // right green (player 2's word)
    if pressed(b::B2) {
        sw[1] |= 1 << 0;
    }
    // menus: left (player 2's bit 14), right (player 1's bit 13)
    if pressed(b::DOWN) {
        sw[1] |= 1 << 14;
    }
    if pressed(b::B6) {
        sw[0] |= 1 << 13;
    }
    // the side buttons, active low: left (player 2's bit 15), right (player 1's bit 14)
    if !pressed(b::B4) {
        sw[1] |= 1 << 15;
    }
    if !pressed(b::B3) {
        sw[0] |= 1 << 14;
    }
}

/// The lever's ADC (channel 0): it increases from right to left (segatools: 0x7FFF - the
/// lever's signed position).
pub(crate) fn lever() -> u16 {
    let p1 = input(0);
    let mut pos = p1.axis_u16(Axis::LeftX) as i32 - 0x8000;
    if p1.pressed(b::B8) {
        pos = -0x8000;
    } else if p1.pressed(b::B7) {
        pos = 0x7FFF;
    }
    (0x7FFF - pos) as u16
}

/// The VFD's answers: its board information to each version request (`1B 5B`: `02`, `01.20`,
/// `01`); the text and pictures shown are dropped.
fn vfd(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for pair in data.windows(2) {
        if matches!(pair[0], 0x1A | 0x1B) && pair[1] == 0x5B {
            log!("ongeki: VFD version asked");
            out.extend_from_slice(b"\x0201.20\x01");
        }
    }
    out
}

/// The game's `kernel32` serial calls: its C# code opens the boards (Mono `DllImport`s,
/// resolved by Mono's `GetProcAddress`, not the executable's imports).
fn resolve(name: &str, module: &str) -> Option<usize> {
    if module == "kernel32.dll" { wal_payload_common::serial::replacement(name) } else { None }
}

/// The lever's calibrated range (direct values 0-65535; the test menu's check: the minimum below
/// 26024, the maximum above 39512): the stick's ends a little inside, reached at full tilt.
const LEVER_MIN: i32 = 0x1000;
const LEVER_MAX: i32 = 0xEFFF;
/// `MachineGroupID.OFF`.
const GROUP_OFF: i32 = 4;

/// `BackupSettingRecord`'s fields' offsets: machineGroupID, isStandardSettingMachine,
/// leverMoveMax, leverMoveMin; `BackupSetting._record`'s.
static OFFSETS: [AtomicUsize; 5] = [const { AtomicUsize::new(0) }; 5];

/// The factory settings of a record (group A, not the reference cabinet; the lever never
/// calibrated) made a lone cabinet's: true when changed.
fn fix_record(record: *mut u8) -> bool {
    let o = |i: usize| OFFSETS[i].load(Ordering::Relaxed);
    let mut changed = false;
    unsafe {
        let group: i32 = mono::read(record, o(0));
        let standard: u8 = mono::read(record, o(1));
        if group == 0 && standard == 0 {
            mono::write(record, o(0), GROUP_OFF);
            changed = true;
        }
        let max: i32 = mono::read(record, o(2));
        let min: i32 = mono::read(record, o(3));
        if max <= 39512 || min >= 26024 {
            mono::write(record, o(2), LEVER_MAX);
            mono::write(record, o(3), LEVER_MIN);
            changed = true;
        }
    }
    changed
}

/// `bool BackupSetting.verify()` (a valid record read at boot; the game's: true): the factory
/// settings fixed, false when changed (the game saves the record then).
unsafe extern "C" fn verify(this: P) -> i32 {
    let record: *mut u8 = unsafe { mono::read(this as *const u8, OFFSETS[4].load(Ordering::Relaxed)) };
    if record.is_null() || !fix_record(record) {
        return 1;
    }
    log!("ongeki: backup settings: group OFF, the lever's range {LEVER_MIN:#x}-{LEVER_MAX:#x}");
    0
}

/// The settings backup's factory values: `BackupSettingRecord.clear` (a new backup) and
/// `leverClear` given a lone cabinet's in their IL, before the game first runs them;
/// `BackupSetting.verify` replaced (a backup written before).
fn backup_settings() {
    let start = std::time::Instant::now();
    let (m, image) = loop {
        // Mono's loader only once its root domain exists, from a thread of Mono's
        if let Some(m) = mono::mono()
            && !m.domain().is_null()
            && m.attach()
        {
            let image = m.image("Assembly-CSharp");
            if !image.is_null() {
                break (m, image);
            }
        }
        if start.elapsed() > std::time::Duration::from_secs(120) {
            log!("ongeki: Assembly-CSharp not loaded: the backup's settings unchanged");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    let record = m.class(image, "MU3.AM", "BackupSettingRecord");
    let setting = m.class(image, "MU3.AM", "BackupSetting");
    let fields = [(record, "machineGroupID"), (record, "isStandardSettingMachine"), (record, "leverMoveMax"), (record, "leverMoveMin"), (setting, "_record")];
    for (i, (class, name)) in fields.into_iter().enumerate() {
        let field = m.field(class, name);
        if field.is_null() {
            log!("ongeki: {name} not found: the backup's settings unchanged");
            return;
        }
        OFFSETS[i].store(m.offset(field), Ordering::Relaxed);
    }
    // clear(): ldarg.0; ldc.i4.0 (MachineGroupID.A); call getEnumValue; stfld machineGroupID
    match m.il_code(m.method(record, "clear", 0)) {
        Some(il) if il.starts_with(&[0x02, 0x16, 0x28]) => il[1] = 0x16 + GROUP_OFF as u8,
        _ => log!("ongeki: BackupSettingRecord.clear unexpected: its group unchanged"),
    }
    // leverClear(): ldarg.0; ldc.i4 32768; stfld leverMoveMax; ldarg.0; ldc.i4 32768; stfld
    // leverMoveMin
    match m.il_code(m.method(record, "leverClear", 0)) {
        Some(il) if il.len() >= 18 && il[0..6] == [0x02, 0x20, 0x00, 0x80, 0x00, 0x00] && il[11..17] == [0x02, 0x20, 0x00, 0x80, 0x00, 0x00] => {
            il[2..6].copy_from_slice(&LEVER_MAX.to_le_bytes());
            il[13..17].copy_from_slice(&LEVER_MIN.to_le_bytes());
        }
        _ => log!("ongeki: BackupSettingRecord.leverClear unexpected: its lever range unchanged"),
    }
    if !m.replace(m.method(setting, "verify", 0), verify as *const () as usize) {
        log!("ongeki: BackupSetting.verify not replaced");
    }
    log!("ongeki: backup settings: new backups a lone cabinet's (group OFF, the lever calibrated)");
    plain_requests(m, image);
}

/// The title server's requests plain (Re:Fresh 1.52: `Config.initializeAfterAMDaemonReady` sets
/// `NetConfig.EncryptVersion` from the ROM version, AES-encrypted requests to hashed endpoint
/// names, and `UseTLS`): both setters' IL (`ldarg.0; stsfld; ret`) made `nop`s, the fields
/// staying 0 / false. Earlier versions (and the community's unpacked 1.50, patched the same
/// way) send them plain already.
fn plain_requests(m: &mono::Mono, image: P) {
    let config = m.class(image, "MU3", "NetConfig");
    if config.is_null() {
        return;
    }
    let mut plain = 0;
    for name in ["set_EncryptVersion", "set_UseTLS"] {
        if let Some(il) = m.il_code(m.method(config, name, 1))
            && il.len() == 7
            && il[0] == 0x02
            && il[1] == 0x80
            && il[6] == 0x2A
        {
            il[..6].fill(0x00);
            plain += 1;
        }
    }
    if plain == 2 {
        log!("ongeki: the title server's requests plain (NetConfig's encryption and TLS off)");
    } else {
        log!("ongeki: NetConfig's setters not the plain ones ({plain} of 2 changed): its requests as the game makes them");
    }
}

static GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static ADD_INTERNAL_CALL: AtomicUsize = AtomicUsize::new(0);

/// `mono_add_internal_call` (the Unity player registering its functions): Mono is up (its
/// loader used earlier hangs the game), the backup's settings patched from a thread then.
unsafe extern "C" fn add_internal_call(name: *const i8, method: P) {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !STARTED.swap(true, Ordering::Relaxed) {
        std::thread::spawn(backup_settings);
    }
    let orig: unsafe extern "C" fn(*const i8, P) = unsafe { std::mem::transmute(ADD_INTERNAL_CALL.load(Ordering::Relaxed)) };
    unsafe { orig(name, method) }
}

/// The game's `GetProcAddress` (Unity 5.6's player resolves Mono's exports itself).
unsafe extern "system" fn get_proc_address(module: P, name: *const u8) -> P {
    let orig: unsafe extern "system" fn(P, *const u8) -> P = unsafe { std::mem::transmute(GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    let f = unsafe { orig(module, name) };
    if !f.is_null() && (name as usize) >> 16 != 0 && unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes() == b"mono_add_internal_call" {
        ADD_INTERNAL_CALL.store(f as usize, Ordering::Relaxed);
        return add_internal_call as *const () as P;
    }
    f
}

/// The game's boards (the LED board, the VFD).
pub(crate) fn init() {
    std::thread::spawn(crate::hkb::mono_clock);
    wal_payload_common::webcam::hide_all();
    match unsafe { wal_payload_common::iat::hook("kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) } {
        Some(o) => GET_PROC_ADDRESS.store(o, Ordering::Relaxed),
        None => log!("ongeki: GetProcAddress not imported: the backup's settings unchanged"),
    }
    wal_payload_common::led::init("COM3");
    wal_payload_common::serial::install("COM2", vfd);
    wal_payload_common::mono_imports::hook(resolve);
    log!("ongeki: LED board on COM3, VFD on COM2");
}

/// amdaemon: the title server beside its ALL.Net server.
pub(crate) fn init_daemon() {
    server::init();
}
