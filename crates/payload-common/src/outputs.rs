//! Cabinet outputs by their MAME names (lamps, gun recoil, the drive board's bytes), for the
//! launcher's network outputs (`WAL_OUTPUTS`: MAMEHooker, BackForceFeeder): each value sent to
//! the launcher when it changes ([`Message::Named`]), every known one again when the payload
//! (re)connects. The names are those MAME and its tools use:
//!
//! * `RawLamps`, `RawLamps1`... (a JVS board's general purpose output bytes) and `lamp0`,
//!   `lamp1`... (each output, lamp n bit 7 - n % 8 of byte n / 8): [`gpo`], [`lamps`]
//! * `RawDrive`: each byte the game sends to a Sega drive board, in order ([`drive`])
//! * the games' own names (`P1_LmpStart`, `P1_GunRecoil`...): [`set`], [`pulse`]; DemulShooter's
//!   event outputs (`P1_CtmRecoil`, `P1_Damaged`) always pulses with its timings ([`set`])
//!
//! Nothing is kept or sent with the option off.

use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use wal_protocol::Message;

use crate::log;

/// Known values, in the order first set (sent again on reconnection).
static VALUES: Mutex<Vec<(String, i32)>> = Mutex::new(Vec::new());

/// The launcher's network outputs are on (`WAL_OUTPUTS=1`).
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("WAL_OUTPUTS").is_ok_and(|v| matches!(v.trim(), "1" | "true" | "on"));
        if on {
            log!("outputs: network outputs on");
        }
        on
    })
}

fn send(name: &str, value: i32) {
    if let Some(stream) = crate::WRITER.lock().unwrap().as_mut() {
        let _ = stream.write_all(&Message::Named { name: name.to_string(), value }.encode());
    }
}

/// Sets output `name` to `value`, sent when it changes.
pub fn set(name: &str, value: i32) {
    if !enabled() {
        return;
    }
    // DemulShooter's event outputs: a value other than 0 starts a pulse, 0 is the pulse's end
    if let Some((on, off)) = event_timing(name) {
        if value != 0 {
            start_pulse(name, on, off);
        }
        return;
    }
    store_and_send(name, value);
}

/// The value sent when it changed.
fn store_and_send(name: &str, value: i32) {
    let mut values = VALUES.lock().unwrap();
    match values.iter_mut().find(|(n, _)| n == name) {
        Some((_, v)) if *v == value => return,
        Some((_, v)) => *v = value,
        None => values.push((name.to_string(), value)),
    }
    send(name, value);
}

/// DemulShooter's event outputs (its `AsyncGameOutput`): 1 for `on` ms, then 0, a new event
/// ignored until `off` ms more: the custom recoil (`P1_CtmRecoil`: `WAL_OUTPUTS_RECOIL_ON` /
/// `WAL_OUTPUTS_RECOIL_OFF`, DemulShooter's 10 / 30 ms by default) and a hit (`P1_Damaged`:
/// `WAL_OUTPUTS_DAMAGED`, 200 ms, then 100 ms). The GUI's Common options.
fn event_timing(name: &str) -> Option<(u64, u64)> {
    static TIMES: OnceLock<(u64, u64, u64)> = OnceLock::new();
    let &(recoil_on, recoil_off, damaged) = TIMES.get_or_init(|| {
        let ms = |key: &str, default: u64| std::env::var(key).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default);
        (ms("WAL_OUTPUTS_RECOIL_ON", 10), ms("WAL_OUTPUTS_RECOIL_OFF", 30), ms("WAL_OUTPUTS_DAMAGED", 200))
    });
    if name.ends_with("_CtmRecoil") {
        Some((recoil_on, recoil_off))
    } else if name.ends_with("_Damaged") {
        Some((damaged, 100))
    } else {
        None
    }
}

/// When each pulsing output takes a new pulse again.
static PULSE_READY: Mutex<Vec<(String, std::time::Instant)>> = Mutex::new(Vec::new());

/// `name` at 1 for `on` ms, then 0; ignored while a pulse and its `off` ms after it run.
fn start_pulse(name: &str, on: u64, off: u64) {
    let now = std::time::Instant::now();
    {
        let mut ready = PULSE_READY.lock().unwrap();
        let until = now + Duration::from_millis(on + off);
        match ready.iter_mut().find(|(n, _)| n == name) {
            Some((_, t)) if *t > now => return,
            Some((_, t)) => *t = until,
            None => ready.push((name.to_string(), until)),
        }
    }
    store_and_send(name, 1);
    let name = name.to_string();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(on));
        store_and_send(&name, 0);
    });
}

/// Output `name`'s last value, when set.
pub fn value(name: &str) -> Option<i32> {
    VALUES.lock().unwrap().iter().find(|(n, _)| n == name).map(|(_, v)| *v)
}

/// Output `name` at 1 for `ms` milliseconds, then 0 (a gun's recoil solenoid, a ticket motor):
/// another pulse ignored while it runs; DemulShooter's event outputs (`P1_CtmRecoil`,
/// `P1_Damaged`) with its own timings.
pub fn pulse(name: &'static str, ms: u64) {
    if !enabled() {
        return;
    }
    let (on, off) = event_timing(name).unwrap_or((ms, 0));
    start_pulse(name, on, off);
}

/// General purpose output byte `index` of a JVS board (byte 0: outputs 1 to 8 in bits 7 to 0):
/// `RawLamps<index>` and each changed `lamp<n>`.
pub fn gpo(index: usize, byte: u8) {
    if !enabled() {
        return;
    }
    let raw = if index == 0 { "RawLamps".to_string() } else { format!("RawLamps{index}") };
    let old = VALUES.lock().unwrap().iter().find(|(n, _)| *n == raw).map(|(_, v)| *v as u8);
    if old == Some(byte) {
        return;
    }
    set(&raw, byte as i32);
    let changed = old.map_or(0xFF, |o| o ^ byte);
    for bit in (0..8).rev().filter(|b| changed & (1 << b) != 0) {
        let (n, v) = (index * 8 + 7 - bit, (byte >> bit & 1) as i32);
        set(&format!("lamp{n}"), v);
        // the profile's name of that output (`WAL_OUTPUTS_LAMPS`)
        for (_, name) in lamp_names().iter().filter(|(i, _)| *i == n) {
            set(name, v);
            // a gun's recoil solenoid: DemulShooter's custom recoil too
            if let Some(player) = name.strip_suffix("_GunRecoil") {
                set(&format!("{player}_CtmRecoil"), v);
            }
        }
    }
}

/// The names a game profile gives its lamps (`WAL_OUTPUTS_LAMPS="0:P1_LmpStart,1:P1_GunRecoil,
/// ..."`: `lamp<n>` also sent by that name, several joined by `+`), the cabinet's meaning of
/// each JVS output.
fn lamp_names() -> &'static [(usize, String)] {
    static NAMES: OnceLock<Vec<(usize, String)>> = OnceLock::new();
    NAMES.get_or_init(|| {
        std::env::var("WAL_OUTPUTS_LAMPS")
            .unwrap_or_default()
            .split(',')
            .filter_map(|e| e.split_once(':'))
            .filter_map(|(n, names)| Some((n.trim().parse::<usize>().ok()?, names)))
            .flat_map(|(n, names)| names.split('+').map(move |name| (n, name.trim().to_string())))
            .collect()
    })
}

/// A JVS board's general purpose output bytes, from byte 0 (a `32` command's data).
pub fn gpo_bytes(bytes: &[u8]) {
    for (i, b) in bytes.iter().enumerate() {
        gpo(i, *b);
    }
}

/// Lamps of a board that is not a JVS one, sent as a JVS board's: lamp n is bit n of `bits`,
/// `count` lamps.
pub fn lamps(bits: u64, count: usize) {
    let mut bytes = vec![0u8; count.div_ceil(8)];
    for n in (0..count).filter(|n| bits >> n & 1 != 0) {
        bytes[n / 8] |= 0x80 >> (n % 8);
    }
    gpo_bytes(&bytes);
}

/// Bytes the game sends to a Sega drive board, in order: `RawDrive`, each of them (repeated
/// values too: the frames are the bytes' sequence).
pub fn drive(bytes: &[u8]) {
    if !enabled() {
        return;
    }
    let mut values = VALUES.lock().unwrap();
    for b in bytes {
        match values.iter_mut().find(|(n, _)| n == "RawDrive") {
            Some((_, v)) => *v = *b as i32,
            None => values.push(("RawDrive".to_string(), *b as i32)),
        }
        send("RawDrive", *b as i32);
    }
}

/// Every known value to the launcher, at the connection (values set before it are not lost).
pub(crate) fn resend() {
    if !enabled() {
        return;
    }
    let values = VALUES.lock().unwrap().clone();
    for (name, value) in values {
        send(&name, value);
    }
}

/// Reading the game's memory for its outputs (the values DemulShooter reads, at the known
/// builds' addresses): every read checked first, 0 when the address is not readable.
#[cfg(windows)]
pub mod mem {
    /// The game executable's image base.
    pub fn base() -> usize {
        unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) as usize }
    }

    /// The game executable's link time (its PE header's TimeDateStamp): which build runs.
    pub fn timestamp() -> u32 {
        let b = base();
        u32(b + 8 + u32(b + 0x3C) as usize)
    }

    pub fn u8(addr: usize) -> u8 {
        if crate::crash::readable(addr, 1) { unsafe { std::ptr::read_volatile(addr as *const u8) } } else { 0 }
    }

    pub fn u32(addr: usize) -> u32 {
        if crate::crash::readable(addr, 4) { unsafe { std::ptr::read_unaligned(addr as *const u32) } } else { 0 }
    }

    /// A pointer (the process's width: a 64-bit game's are 8 bytes).
    pub fn ptr(addr: usize) -> usize {
        if crate::crash::readable(addr, size_of::<usize>()) { unsafe { std::ptr::read_unaligned(addr as *const usize) } } else { 0 }
    }

    pub fn f32(addr: usize) -> f32 {
        f32::from_bits(u32(addr))
    }

    /// The pointer at `addr`, then the pointer at each `+ offset` in turn (DemulShooter's
    /// `ReadPtrChain`); 0 at the first null pointer.
    pub fn chain(addr: usize, offsets: &[usize]) -> u32 {
        let mut p = u32(addr);
        for o in offsets {
            if p == 0 {
                return 0;
            }
            p = u32(p as usize + o);
        }
        p
    }
}

/// Runs `step` every `ms` milliseconds on a thread of its own while the outputs are on (the
/// games whose outputs are read from their memory).
pub fn poll(name: &'static str, ms: u64, mut step: impl FnMut() + Send + 'static) {
    if !enabled() {
        return;
    }
    log!("outputs: {name} read every {ms} ms");
    std::thread::spawn(move || {
        loop {
            step();
            std::thread::sleep(Duration::from_millis(ms));
        }
    });
}

/// [`poll`], when the game executable is the build whose addresses `step` reads (its link time,
/// [`mem::timestamp`]); else logged, nothing read.
#[cfg(windows)]
pub fn poll_build(name: &'static str, timestamp: u32, ms: u64, step: impl FnMut() + Send + 'static) {
    if !enabled() {
        return;
    }
    let found = mem::timestamp();
    if found != timestamp {
        log!("outputs: {name}: another build (link time {found:#x}, not {timestamp:#x}), its outputs not read");
        return;
    }
    poll(name, ms, step);
}

/// A value that pulses an output when it goes down (DemulShooter's custom recoil on the ammo
/// count, damage on the life): `update` with the new value, `None` while it is not followed.
#[derive(Default)]
pub struct Falling {
    last: Option<i64>,
}

impl Falling {
    pub fn update(&mut self, value: Option<i64>, name: &'static str) {
        if let (Some(v), Some(last)) = (value, self.last)
            && v < last
        {
            pulse(name, 50);
        }
        self.last = value;
    }
}

/// The game's own code reporting its outputs (DemulShooter's code caves, 32-bit games): a jump at
/// a site of whole instructions (5 bytes at least) to a stub of the caller's bytes, then the
/// site's own instructions (a 5-byte `call` relocated, any other position independent bytes as
/// they are) and a jump back. [`stub`] is the general form (the bytes get the address of 16
/// dwords of their own, [`Slots`]); [`player_flag`] sets a player's flag. `None` when the site's
/// bytes are not `original` (another build: left as it is).
#[cfg(all(windows, target_arch = "x86"))]
pub mod hook {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect};

    /// A hook's 16 dwords (its flags, or the values its stub copies).
    #[derive(Clone, Copy)]
    pub struct Slots(pub usize);

    impl Slots {
        /// Dword `i`.
        pub fn get(self, i: usize) -> u32 {
            unsafe { std::ptr::read_volatile((self.0 + 4 * i) as *const u32) }
        }

        /// Dword `i` was set since the last call (cleared): a player's flag.
        pub fn take(self, i: usize) -> bool {
            i < 16 && unsafe { std::ptr::replace((self.0 + 4 * i) as *mut u32, 0) } != 0
        }
    }

    pub type Flags = Slots;

    /// Where [`player_flag`] reads the player.
    #[derive(Clone, Copy)]
    pub enum Player {
        /// Player 1.
        Fixed,
        /// The value of a register (0 eax, 1 ecx, 2 edx, 3 ebx, 5 ebp, 6 esi, 7 edi).
        Reg(u8),
        /// The byte at a register (not esp) plus a displacement.
        ByteAt(u8, i8),
    }

    /// # Safety
    /// `site` must be the start of the instructions of `original` in the game's code, and the
    /// stub's bytes must keep the registers and flags the following code reads.
    pub unsafe fn stub(site: usize, original: &[u8], build: impl FnOnce(usize) -> Vec<u8>) -> Option<Slots> {
        let len = original.len();
        if len < 5 || !crate::crash::readable(site, len) || unsafe { std::slice::from_raw_parts(site as *const u8, len) } != original {
            return None;
        }
        let block = unsafe { VirtualAlloc(std::ptr::null(), 0x200, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) } as usize;
        if block == 0 {
            return None;
        }
        let (slots, code) = (block, block + 0x40);
        let mut c = build(slots);
        let rel = |c: &Vec<u8>, target: usize| target.wrapping_sub(code + c.len() + 4) as u32;
        if original[0] == 0xE8 && len == 5 {
            let target = (site + 5).wrapping_add(i32::from_le_bytes(original[1..5].try_into().unwrap()) as usize);
            c.push(0xE8);
            let r = rel(&c, target);
            c.extend_from_slice(&r.to_le_bytes());
        } else {
            c.extend_from_slice(original);
        }
        c.push(0xE9);
        let r = rel(&c, site + len);
        c.extend_from_slice(&r.to_le_bytes());
        unsafe {
            std::ptr::copy_nonoverlapping(c.as_ptr(), code as *mut u8, c.len());
            let mut old = 0;
            VirtualProtect(site as *const _, len, PAGE_EXECUTE_READWRITE, &mut old);
            std::ptr::write_bytes((site + 5) as *mut u8, 0x90, len - 5);
            std::ptr::write_unaligned((site + 1) as *mut u32, code.wrapping_sub(site + 5) as u32);
            *(site as *mut u8) = 0xE9;
            VirtualProtect(site as *const _, len, old, &mut old);
        }
        Some(Slots(slots))
    }

    /// A stub setting dword `slots[player]` to 1, when `skip_unless_zero` is `None` or the dword
    /// at `[reg * 8 + that address]` is 0 (a register player only), registers and flags kept.
    ///
    /// # Safety
    /// As [`stub`].
    pub unsafe fn player_flag(site: usize, original: &[u8], player: Player, skip_unless_zero: Option<usize>) -> Option<Slots> {
        unsafe {
            stub(site, original, |slots| {
                let s = (slots as u32).to_le_bytes();
                let mut c: Vec<u8> = vec![0x9C]; // pushfd
                match player {
                    Player::Fixed => {
                        // mov dword [slots], 1
                        c.extend_from_slice(&[0xC7, 0x05]);
                        c.extend_from_slice(&s);
                        c.extend_from_slice(&[1, 0, 0, 0]);
                    }
                    Player::Reg(reg) => {
                        if let Some(addr) = skip_unless_zero {
                            // cmp dword [reg * 8 + addr], 0; jne past the flag (17 bytes)
                            c.extend_from_slice(&[0x83, 0x3C, 0xC5 | reg << 3]);
                            c.extend_from_slice(&(addr as u32).to_le_bytes());
                            c.extend_from_slice(&[0x00, 0x0F, 0x85, 17, 0, 0, 0]);
                        }
                        // push reg; shl reg, 2; add reg, slots; mov dword [reg], 1; pop reg
                        c.extend_from_slice(&[0x50 + reg, 0xC1, 0xE0 + reg, 0x02, 0x81, 0xC0 + reg]);
                        c.extend_from_slice(&s);
                        c.extend_from_slice(&[0xC7, reg, 1, 0, 0, 0, 0x58 + reg]);
                    }
                    Player::ByteAt(reg, disp) => {
                        // push eax; movzx eax, byte [reg + disp]; shl eax, 2; add eax, slots;
                        // mov dword [eax], 1; pop eax
                        c.extend_from_slice(&[0x50, 0x0F, 0xB6, 0x40 | reg, disp as u8, 0xC1, 0xE0, 0x02, 0x05]);
                        c.extend_from_slice(&s);
                        c.extend_from_slice(&[0xC7, 0x00, 1, 0, 0, 0, 0x58]);
                    }
                }
                c.push(0x9D); // popfd
                c
            })
        }
    }
}
