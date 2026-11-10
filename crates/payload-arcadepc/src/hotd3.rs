//! The House of the Dead 3 (SEGA's PC release, `exe\hod3pc.exe`, Direct3D 8, 32-bit, the MYTH
//! release: MD5 0228818e…) with the community "Hotd3pc ArcadeMod" 7.0 (Argonlefou):
//! `WAL_ARCADEPC_BOARD=hotd3`.
//!
//! The mod is a .NET launcher (`Hotd3Arcade_Launcher.exe`) that starts the game under its own
//! debugger and, at the process's first breakpoint, writes its patches into it (arcade title
//! and attract screens, coins and credits, free play, continue screen, the PC menu skipped,
//! fast reload without auto reload, saves beside the game, its settings in `Hod3Arcade.ini`
//! instead of the registry). Wine has no .NET in the prefix: the payload, loaded at the game's
//! entry point (`wal-loader`), applies the same patches itself, byte for byte (the same
//! instructions in its own code caves), its settings from the dump's `Hod3Arcade.ini` and the
//! profile (`WAL_HOTD3_*`, `WAL_FREEPLAY`, `WAL_CROSSHAIR`, the display mode from
//! `WAL_D3D9_WINDOWED_SIZE`). Its sprites (the arcade title's
//! logo and texts, per language) and its default rankings are the launcher's .NET resources,
//! read from the dump's `Hotd3Arcade_Launcher.exe` at start.
//!
//! The guns (as DemulShooter's `hod3pc` target): the game's aim writes and button reads are
//! removed, the payload writes both players' aim (-320..320, -240..240, y up) and buttons
//! (trigger, reload, the route change left / right) into the players' input blocks. A shot off
//! the screen reloads. Start and coin are the game's window keys (its `WM_KEYDOWN`: the
//! players' start keys of `Hod3Arcade.ini`, `5` a credit); both players' controls are the
//! keyboard (the mouse and pad modes' readings unused). The real keyboard and mouse are
//! filtered (`input_filter`), the mod's own start key message marked as the payload's.
//!
//! Saves (`HOD3.DAT`: settings and rankings, `SAVE.DAT`) beside the executable (the run
//! directory: kept between launches), not in the dump's `NVRAM`.
//!
//! The trigger is start too (the title, a join, a continue, the cutscenes' skip): always while the
//! in-game pause is disabled (start does nothing else in a game), else while its player is not
//! playing.
//!
//! Natives: `trigger reload action left right start coin offscreen`.

use std::sync::OnceLock;
use std::time::Duration;

use wal_payload_common::input_filter::post_key;
use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, input, log};
use windows_sys::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect};

// The game's data (RVAs of the MYTH release, the mod's defaults).
const ENTER_KEY_PRESSED: u32 = 0x3ECF48;
const FLAG_FREEPLAY: u32 = 0x3B7E17;
const CREDITS: u32 = 0x3B7DD0;
const CREDITS_TO_START: u32 = 0x3B7E1C;
const CREDITS_TO_CONTINUE: u32 = 0x3B7E1D;
const CONTINUE_TIMER_P1: u32 = 0x3B807C;
const CONTINUE_TIMER_P2: u32 = 0x3B82A8;
const CONTINUE_TIMER: u32 = 0x3B8924;
const CURRENT_SCENE: u32 = 0x3B8B74;
const NEXT_SCENE: u32 = 0x3B8780;
const P1_STATUS: u32 = 0x3B8038;
const P2_STATUS: u32 = 0x3B8264;
const P1_AMMO: u32 = 0x3B8090;
const P2_AMMO: u32 = 0x3B82BC;
const DRAW_FS2_SPRITES: u32 = 0xA5EE0;
const DRAW_FS_SPRITES: u32 = 0xA5EA0;
const GET_LOCAL_TIME: u32 = 0x3D710;
const POST_MESSAGE_PTR: u32 = 0x14925C;
const WINDOW_BASE_PTR: u32 = 0x536358;
const CURRENT_PATH: u32 = 0x59C5F4;
const SOUND_PLAYER: u32 = 0x5D2E18;
const FLT_16: u32 = 0x1C26BC;
const FLT_58: u32 = 0x1C28C4;
const FLT_10: u32 = 0x1C2718;
const FLT_46: u32 = 0x1C2938;
const ALLOW_START_GAME: u32 = 0x366748;
const START_GAME_BYTE_TABLE: u32 = 0x8E134;
const SPR_MENU_EXTRAS: u32 = 0x199C58;
/// The game's sound call (`WndProcCredits_CalledFunction`).
const PLAY_SFX: u32 = 0xA3D80;
const SFX_RELOAD: u32 = 0x210FA9;
const SFX_COIN: u32 = 0x10BA9;
const QUIT_GAME: u32 = 0xB2A30;

/// The extra sprites (offsets in the menu extras' sprite table).
const SPR_ATTRACT_RELOAD: u32 = 12;
const SPR_RANKINGS: u32 = 36;
const SPR_ATTRACT_SHELLS: u32 = 48;
const SPR_CREDITS: u32 = 120;
const SPR_FREEPLAY: u32 = 144;
const SPR_PRESS_START: u32 = 192;
const SPR_INSERT_COIN: u32 = 216;
const SPR_NUM0: u32 = 276;
const SPR_TRADEMARK: u32 = 396;
const SPR_LOGO: u32 = 408;

/// The data bank's fields (the mod's own variables, in a cave).
const DB_CREDITS_KEY: u32 = 0;
const DB_BLINK_TICK: u32 = 4;
const DB_FRAME_TICK: u32 = 8;
const DB_SPRITE_VISIBLE: u32 = 12;
const DB_SKIP_INTRO: u32 = 16;
const DB_SHOW_INTRO: u32 = 20;
const DB_SHOW_RELOAD_P1: u32 = 24;
const DB_SHOW_RELOAD_P2: u32 = 28;
const DB_DAT_PATH: u32 = 50;
const DB_SAVE_PATH: u32 = 305;
const DB_COIN_SOUND: u32 = 560;
const DB_GAME_DIR: u32 = 815;

/// The guns (DemulShooter's): aim (i16 x, then y) and input blocks (byte 0: 0x04 left, 0x08
/// right; byte 2 bit 0 the trigger; byte 3 bit 0 the reload).
const AIM: [u32; 2] = [0x367AA4, 0x367AD8];
const BUTTONS: [u32; 2] = [0x3ECF88, 0x3ED0F0];

/// `lParam` mark of the payload's key messages (`input_filter`'s): the mod's own start key
/// message passes the filter with it.
const KEY_MARK: u32 = 0x0A00_0000;
/// The coin key (the mod's window procedure: `5`).
const VK_COIN: u16 = 0x35;

fn base() -> u32 {
    static BASE: OnceLock<u32> = OnceLock::new();
    *BASE.get_or_init(|| unsafe { GetModuleHandleW(std::ptr::null()) } as u32)
}

/// An address in the executable.
fn at(rva: u32) -> u32 {
    base() + rva
}

fn write(addr: u32, bytes: &[u8]) {
    let mut old = 0;
    unsafe {
        VirtualProtect(addr as *const _, bytes.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), addr as *mut u8, bytes.len());
        VirtualProtect(addr as *const _, bytes.len(), old, &mut old);
    }
}

fn nops(rva: u32, len: usize) {
    write(at(rva), &vec![0x90; len]);
}

/// The bytes of a hex string ("C6 82 F0 00 00 00").
fn hex(s: &str) -> Vec<u8> {
    s.split_whitespace().map(|b| u8::from_str_radix(b, 16).expect("hex byte")).collect()
}

/// A code cave: executable memory written in order.
struct Cave {
    addr: u32,
    pos: u32,
}

impl Cave {
    fn new(size: usize) -> Cave {
        let addr = unsafe { VirtualAlloc(std::ptr::null(), size, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) } as u32;
        assert!(addr != 0, "hotd3: no memory for a code cave");
        Cave { addr, pos: 0 }
    }
    fn here(&self) -> u32 {
        self.addr + self.pos
    }
    fn bytes(&mut self, b: &[u8]) -> &mut Self {
        unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), self.here() as *mut u8, b.len()) };
        self.pos += b.len() as u32;
        self
    }
    fn hex(&mut self, s: &str) -> &mut Self {
        self.bytes(&hex(s))
    }
    fn u8(&mut self, v: u8) -> &mut Self {
        self.bytes(&[v])
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }
    fn f32(&mut self, v: f32) -> &mut Self {
        self.bytes(&v.to_le_bytes())
    }
    /// An instruction with a 32-bit relative target.
    fn rel(&mut self, op: &[u8], target: u32) -> &mut Self {
        let next = self.here() + op.len() as u32 + 4;
        self.bytes(op).u32(target.wrapping_sub(next))
    }
    fn call(&mut self, target: u32) -> &mut Self {
        self.rel(&[0xE8], target)
    }
    fn jmp(&mut self, target: u32) -> &mut Self {
        self.rel(&[0xE9], target)
    }
    fn jl(&mut self, target: u32) -> &mut Self {
        self.rel(&[0x0F, 0x8C], target)
    }
}

/// The game's code at `rva` (`len` bytes) jumps to `cave` (the rest NOPs).
fn hook(rva: u32, len: usize, cave: u32) {
    let from = at(rva);
    let mut b = vec![0xE9];
    b.extend_from_slice(&cave.wrapping_sub(from + 5).to_le_bytes());
    b.resize(len.max(5), 0x90);
    write(from, &b);
}

/// The mod's settings: `Hod3Arcade.ini`'s, then the profile's.
struct Config {
    volume_bgm: u8,
    volume_sfx: u8,
    volume_vce: u8,
    difficulty: u8,
    initial_life: u8,
    max_life: u32,
    free_play: bool,
    violence: u8,
    blood: u8,
    crosshair_design: u8,
    crosshair_speed: u8,
    hide_cursor: bool,
    hide_crosshairs: bool,
    disable_pause: bool,
    display_score: bool,
    credits_to_start: u32,
    credits_to_continue: u32,
    /// The game's registry settings (controls, display, language), in its order.
    reg: [u32; 38],
}

impl Config {
    fn language(&self) -> u32 {
        self.reg[37]
    }
}

/// The registry settings' names in `Hod3Arcade.ini` (index in [`Config::reg`]).
const REG_KEYS: &[(&str, usize)] = &[
    ("P1Controls", 0),
    ("P2Controls", 1),
    ("MouseShoot", 2),
    ("MouseReload", 3),
    ("Keyboard1pShoot", 4),
    ("Keyboard2pShoot", 5),
    ("Keyboard1pReload", 6),
    ("Keyboard2pReload", 7),
    ("Keyboard1pUp", 8),
    ("Keyboard2pUp", 9),
    ("Keyboard1pDown", 10),
    ("Keyboard2Down", 11),
    ("Keyboard1pLeft", 12),
    ("Keyboard2pLeft", 13),
    ("Keyboard1pRight", 14),
    ("Keyboard2pRight", 15),
    ("Keyboard1pStart", 16),
    ("Keyboard2pStart", 17),
    ("Keyboard1pCenter", 18),
    ("Keyboard2pCenter", 19),
    ("Gamepad1pShoot", 20),
    ("Gamepad2pShoot", 21),
    ("Gamepad1pReload", 22),
    ("Gamepad2pReload", 23),
    ("Gamepad1pStart", 24),
    ("Gamepad2pStart", 25),
    ("Gamepad1pCenter", 26),
    ("Gamepad2pCenter", 27),
    ("Resolution", 30),
    ("Fullscreen", 31),
    ("Framerate", 32),
    ("AntiAliasing", 33),
    ("Textures", 34),
    ("Colors", 35),
    ("Language", 37),
];

fn env_u32(name: &str) -> Option<u32> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

fn env_bool(name: &str) -> Option<bool> {
    std::env::var(name).ok().map(|v| matches!(v.trim(), "1" | "true" | "on"))
}

fn read_config(root: &str) -> Config {
    let mut c = Config {
        volume_bgm: 255,
        volume_sfx: 255,
        volume_vce: 255,
        difficulty: 2,
        initial_life: 3,
        max_life: 5,
        free_play: true,
        violence: 0,
        blood: 1,
        crosshair_design: 2,
        crosshair_speed: 255,
        hide_cursor: true,
        hide_crosshairs: true,
        disable_pause: true,
        display_score: false,
        credits_to_start: 1,
        credits_to_continue: 1,
        reg: [1, 0, 0, 1, 24, 37, 25, 38, 200, 72, 80, 208, 203, 75, 205, 77, 2, 3, 17, 21, 0, 0, 1, 1, 2, 2, 3, 3, 0, 0, 3, 1, 0, 1, 2, 1, 0, 0],
    };
    let ini = format!("{root}\\Hod3Arcade.ini");
    match std::fs::read_to_string(&ini) {
        Ok(text) => {
            for line in text.lines().map(str::trim) {
                let Some((key, value)) = line.split_once(':') else { continue };
                if line.starts_with(['[', ';']) {
                    continue;
                }
                let Ok(v) = value.trim().parse::<u32>() else { continue };
                if let Some(&(_, i)) = REG_KEYS.iter().find(|(k, _)| *k == key) {
                    c.reg[i] = v;
                    continue;
                }
                match key {
                    "VolumeBGM" => c.volume_bgm = v as u8,
                    "VolumeSFX" => c.volume_sfx = v as u8,
                    "VolumeVCE" => c.volume_vce = v as u8,
                    "Difficulty" => c.difficulty = v as u8,
                    "InitialLife" => c.initial_life = v as u8,
                    "MaxLife" => c.max_life = v,
                    "BloodColor" => c.blood = v as u8,
                    "Violence" => c.violence = v as u8,
                    "CreditsToStart" => c.credits_to_start = v,
                    "CreditsToContinue" => c.credits_to_continue = v,
                    "Freeplay" => c.free_play = v == 1,
                    "CrosshairDesign" => c.crosshair_design = v as u8,
                    "CrosshairSpeed" => c.crosshair_speed = v as u8,
                    "HideCrosshairs" => c.hide_crosshairs = v == 1,
                    "HideCursor" => c.hide_cursor = v == 1,
                    "DisableInGamePause" => c.disable_pause = v == 1,
                    "DisplayScore" => c.display_score = v == 1,
                    _ => {}
                }
            }
        }
        Err(e) => log!("hotd3: {ini}: {e} (the mod's defaults)"),
    }
    // both players on the keyboard: their aim and buttons are the payload's, start their keys
    c.reg[0] = 0;
    c.reg[1] = 0;
    if let Some(f) = wal_payload_common::free_play("WAL_ARCADEPC_FREEPLAY") {
        c.free_play = f;
    }
    c.hide_crosshairs = !wal_payload_common::crosshairs(!c.hide_crosshairs);
    if let Some(v) = env_u32("WAL_HOTD3_DIFFICULTY").filter(|&v| v <= 4) {
        c.difficulty = v as u8;
    }
    if let Some(v) = env_u32("WAL_HOTD3_LIFE").filter(|v| (1..=9).contains(v)) {
        c.initial_life = v as u8;
    }
    if let Some(v) = env_u32("WAL_HOTD3_MAX_LIFE").filter(|v| (1..=9).contains(v)) {
        c.max_life = v;
    }
    if let Some(v) = env_u32("WAL_HOTD3_CREDITS_TO_START").filter(|v| (1..=9).contains(v)) {
        c.credits_to_start = v;
    }
    if let Some(v) = env_u32("WAL_HOTD3_CREDITS_TO_CONTINUE").filter(|v| (1..=9).contains(v)) {
        c.credits_to_continue = v;
    }
    if let Some(v) = std::env::var("WAL_HOTD3_LANGUAGE").ok().and_then(|l| ["english", "french", "italian", "spanish"].iter().position(|n| l.trim().eq_ignore_ascii_case(n))) {
        c.reg[37] = v as u32;
    }
    // the game's display mode: the windowed device's size (`WAL_D3D9_WINDOWED_SIZE`) when it
    // is one of its modes (its full screen device made windowed in the screen's 4:3)
    if let Some(v) = std::env::var("WAL_D3D9_WINDOWED_SIZE").ok().and_then(|r| ["640x480", "800x600", "1024x768", "1280x960", "1280x1024"].iter().position(|n| r.trim() == *n)) {
        c.reg[30] = v as u32;
        c.reg[31] = 1;
    }
    if let Some(v) = env_bool("WAL_HOTD3_SCORE") {
        c.display_score = v;
    }
    if let Some(v) = env_bool("WAL_HOTD3_PAUSE") {
        c.disable_pause = !v;
    }
    c.initial_life = c.initial_life.max(1);
    c.max_life = c.max_life.max(1);
    c
}

/// A .NET assembly's `byte[]` resources (`System.Resources.ResourceReader`'s format, version 2),
/// by name.
fn net_resources(file: &[u8]) -> Vec<(String, Vec<u8>)> {
    fn i32_at(b: &[u8], p: usize) -> Option<i32> {
        Some(i32::from_le_bytes(b.get(p..p + 4)?.try_into().ok()?))
    }
    fn len7(b: &[u8], p: &mut usize) -> Option<usize> {
        let (mut v, mut shift) = (0usize, 0);
        loop {
            let byte = *b.get(*p)?;
            *p += 1;
            v |= ((byte & 0x7f) as usize) << shift;
            if byte & 0x80 == 0 {
                return Some(v);
            }
            shift += 7;
        }
    }
    let mut out = Vec::new();
    let Some(start) = file.windows(4).position(|w| w == 0xBEEFCACEu32.to_le_bytes()) else {
        return out;
    };
    let s = &file[start..];
    let parse = || -> Option<Vec<(String, Vec<u8>)>> {
        let mut p = 8;
        p += 4 + i32_at(s, p)? as usize;
        let _version = i32_at(s, p)?;
        let count = i32_at(s, p + 4)? as usize;
        let types = i32_at(s, p + 8)? as usize;
        p += 12;
        for _ in 0..types {
            p += len7(s, &mut p)?;
        }
        if p & 7 != 0 {
            p += 8 - (p & 7);
        }
        let positions = p + 4 * count;
        let data = i32_at(s, positions + 4 * count)? as usize;
        let names = positions + 4 * count + 4;
        let mut res = Vec::new();
        for i in 0..count {
            let mut q = names + i32_at(s, positions + 4 * i)? as usize;
            let n = len7(s, &mut q)?;
            let name = String::from_utf16_lossy(&s.get(q..q + n)?.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>());
            let mut d = data + i32_at(s, q + n)? as usize;
            // type code 0x20: byte[]
            if len7(s, &mut d)? == 0x20 {
                let len = i32_at(s, d)? as usize;
                res.push((name, s.get(d + 4..d + 4 + len)?.to_vec()));
            }
        }
        Some(res)
    };
    out = parse().unwrap_or_default();
    out
}

/// Where the mod keeps its variables.
struct Mod {
    c: Config,
    db: u32,
    draw1: u32,
    draw2: u32,
    draw_all: u32,
    /// Player 1's start key (virtual key).
    p1_start_vk: u8,
}

impl Mod {
    fn d(&self, field: u32) -> u32 {
        self.db + field
    }

    fn data_bank(&mut self, exe_dir: &str) {
        let cave = Cave::new(5376);
        self.db = cave.addr;
        let put = |off: u32, s: &str| unsafe {
            let p = (self.db + off) as *mut u8;
            std::ptr::copy_nonoverlapping(s.as_ptr(), p, s.len());
            p.add(s.len()).write(0);
        };
        unsafe {
            ((self.db + DB_SKIP_INTRO) as *mut u8).write(1);
            ((self.db + DB_SHOW_INTRO) as *mut u8).write(1);
            ((self.db + DB_CREDITS_KEY) as *mut u8).write(0);
        }
        put(DB_GAME_DIR, "C:\\Program Files (x86)\\SEGA\\THE HOUSE OF THE DEAD3_EU");
        put(DB_DAT_PATH, &format!("{exe_dir}\\HOD3.DAT"));
        put(DB_SAVE_PATH, &format!("{exe_dir}\\SAVE.DAT"));
        put(DB_COIN_SOUND, "..\\media\\coin002.aif");
    }

    fn registry_checks(&self, root: &str) {
        write(at(0x3CF3F), &hex("E9 55 03 00 00 90"));
        write(at(0x3CB6F), &hex("E9 55 03 00 00 90"));
        write(at(0xB2E26), &[0xEB]);
        nops(0xB1E38 - 172, 6);
        write(at(0xB1E38 - 118), &[0xEB]);
        write(at(0xB1E38), &[0xEB]);
        write(at(0xB1EB8), &hex("83 C4 14"));
        let mut path = root.as_bytes().to_vec();
        path.push(0);
        write(at(CURRENT_PATH), &path);
        let mut cave = Cave::new(2048);
        let mut num: u32 = 592;
        for (i, v) in self.c.reg.iter().enumerate() {
            cave.hex("C7 86");
            if i == 4 || i == 30 {
                num += 8;
            }
            cave.u32(num).u32(*v);
            num += 4;
        }
        cave.hex("8B 86 DC 02 00 00").jmp(at(0xB24E1 + 6));
        hook(0xB24E1, 6, cave.addr);
        nops(0xB25BA, 6);
        nops(0xB25EC, 6);
    }

    fn free_play_init(&self) {
        if self.c.free_play {
            write(at(0x3E0F1), &hex("C6 05 17 7E 7B 00 01 88 0D 13 7E 7B 00 89 08 5F C2 04 00"));
        }
    }

    /// The game's settings set after it reads `HOD3.DAT` (`reg`: the register holding it, 0x82
    /// edx or 0x87 edi in the `mov byte [reg+off]` encoding).
    fn settings(&self, cave: &mut Cave, reg: u8, free_play_off: Option<&str>) {
        let c = &self.c;
        let mut set = |off: u32, v: u8| {
            cave.bytes(&[0xC6, reg]).u32(off).u8(v);
        };
        set(0xF0, c.volume_bgm);
        set(0xF1, c.volume_sfx);
        set(0xF2, c.volume_vce);
        set(0x184, c.difficulty);
        set(0x186, c.initial_life - 1);
        if c.free_play {
            set(0x187, 0xFF);
        } else if let Some(v) = free_play_off {
            set(0x187, u8::from_str_radix(v, 16).unwrap());
        }
        set(0x188, c.violence);
        set(0x189, c.blood);
        set(0x18A, c.blood);
        set(0x190, c.crosshair_design);
        set(0x191, c.crosshair_speed);
    }

    fn override_loading_dat(&self) {
        for rva in [0x3CA26, 0x3CA42, 0x3D38D] {
            write(at(rva + 1), &self.d(DB_DAT_PATH).to_le_bytes());
        }
        self.free_play_init();
        let mut cave = Cave::new(2048);
        self.settings(&mut cave, 0x82, Some("05"));
        cave.hex("66 89 72 18").hex("85 DB").jmp(at(0x3CAE9 + 6));
        hook(0x3CAE9, 6, cave.addr);
    }

    fn override_default_dat(&self, rva: u32) {
        self.free_play_init();
        let mut cave = Cave::new(2048);
        cave.call(at(GET_LOCAL_TIME));
        self.settings(&mut cave, 0x87, None);
        cave.jmp(at(rva + 5));
        hook(rva, 5, cave.addr);
    }

    fn override_save_file(&self) {
        for rva in [0x3D621, 0x3D4B0, 0x3D4C8] {
            write(at(rva + 1), &self.d(DB_SAVE_PATH).to_le_bytes());
        }
    }

    fn wndproc_loop(&self) {
        write(at(0xB2A52), &[0xEB]);
        write(at(0xB29A2), &[0xEB]);
        write(at(0xB29C7), &[0xEB]);
    }

    /// The coin key (`5`): a credit (at most 99), the continue timers reset, the coin sound;
    /// Escape quits.
    fn wndproc_credits_button(&self) {
        let mut down = Cave::new(2048);
        down.hex("8B 54 24 18").hex("83 FA 35").hex("75 64");
        down.hex("83 3D").u32(self.d(DB_CREDITS_KEY)).hex("00").hex("75 5B");
        down.hex("83 3D").u32(at(CURRENT_SCENE)).hex("03").hex("74 52");
        down.hex("81 3D").u32(at(CREDITS)).hex("63 00 00 00").hex("7D 46");
        down.hex("C7 05").u32(self.d(DB_CREDITS_KEY)).hex("01 00 00 00");
        down.hex("83 05").u32(at(CREDITS)).hex("01");
        down.hex("C7 05").u32(at(CONTINUE_TIMER_P1)).hex("FF 9F 00 00");
        down.hex("C7 05").u32(at(CONTINUE_TIMER_P2)).hex("FF 9F 00 00");
        down.hex("C7 05").u32(at(CONTINUE_TIMER)).hex("FF 9F 00 00");
        down.hex("50 51 52 6A 00 68 A9 0B 01 00 68").u32(at(SOUND_PLAYER)).call(at(PLAY_SFX));
        down.hex("5A 59 58").hex("83 FA 1B").hex("75 05").jmp(at(QUIT_GAME));
        down.hex("83 FA 0D").jmp(at(0xB299B + 7));
        hook(0xB299B, 7, down.addr);
        let mut up = Cave::new(2048);
        up.hex("83 FB 35").hex("75 0A").hex("C7 05").u32(self.d(DB_CREDITS_KEY)).hex("00 00 00 00");
        up.hex("8B 54 24 18").hex("83 FA 0D").jmp(at(0xB2A4B + 7));
        hook(0xB2A4B, 7, up.addr);
    }

    fn force_coin_sound(&self) {
        let mut cave = Cave::new(2048);
        cave.hex("68 C8 00 00 00").hex("81 FD").u32(SFX_COIN).hex("75 05").hex("BB").u32(self.d(DB_COIN_SOUND));
        cave.jmp(at(0xA3FC8 + 5));
        hook(0xA3FC8, 5, cave.addr);
    }

    /// A sprite file the game loads replaced by `data` (its size at `size_rva`, its copy at
    /// `inject_rva`).
    fn inject_sprites(&self, data: &[u8], size_rva: u32, inject_rva: u32) {
        let mut file = Cave::new(data.len());
        file.bytes(data);
        let mut size = vec![0xB8];
        size.extend_from_slice(&(data.len() as u32).to_le_bytes());
        write(at(size_rva), &size);
        let mut cave = Cave::new(2048);
        cave.hex("57 56 51 BE").u32(file.addr).hex("8B FB B9").u32(data.len() as u32).hex("F3 A4 59 5E 5F").jmp(at(inject_rva + 5));
        hook(inject_rva, 5, cave.addr);
    }

    /// PRESS START / INSERT COIN.
    fn create_draw_sprite1(&mut self, sx: f32, sy: f32, transform: u8) {
        let mut c = Cave::new(2048);
        c.hex("83 3D").u32(self.d(DB_SPRITE_VISIBLE)).hex("00").hex("74 4E").hex("50 57");
        c.hex("83 3D").u32(at(FLAG_FREEPLAY)).hex("01").hex("74 14");
        c.hex("A0").u32(at(CREDITS_TO_START)).hex("39 05").u32(at(CREDITS)).hex("7D 07");
        c.hex("BF").u32(at(SPR_MENU_EXTRAS + SPR_INSERT_COIN)).hex("EB 05");
        c.hex("BF").u32(at(SPR_MENU_EXTRAS + SPR_PRESS_START));
        c.hex("68 00 00 80 3F 6A").u8(transform).hex("6A 00 68").f32(sx).hex("68").f32(sy).hex("68 17 B7 D1 38");
        c.hex("FF 74 24 2C FF 74 24 2C").call(at(DRAW_FS2_SPRITES)).hex("83 C4 20 5F 58 C3");
        self.draw1 = c.addr;
    }

    /// FREE PLAY / CREDITS nn.
    fn create_draw_sprite2(&mut self, sx: f32, sy: f32, transform: u8) {
        let lang = self.c.language();
        let mut c = Cave::new(2048);
        c.hex("57 83 3D").u32(at(FLAG_FREEPLAY)).hex("00").hex("74 0A");
        c.hex("BF").u32(at(SPR_MENU_EXTRAS + SPR_FREEPLAY)).hex("E9 92 00 00 00");
        c.hex("BF").u32(at(SPR_MENU_EXTRAS + SPR_CREDITS));
        c.hex("68 00 00 80 3F 6A").u8(transform).hex("6A 00 68").f32(sx).hex("68").f32(sy).hex("68 17 B7 D1 38");
        c.hex("FF 74 24 30 FF 74 24 30").hex("D9 04 24 D8 25").u32(at(FLT_16)).hex("D9 1C 24").call(at(DRAW_FS2_SPRITES));
        c.hex("D9 04 24 D8 05").u32(at(if lang == 3 { FLT_58 } else { FLT_46 })).hex("D9 1C 24");
        c.hex("A1").u32(at(CREDITS)).hex("BB 0A 00 00 00 F6 F3 0F B6 C0 B3 0C F6 E3 0F B6 F8 81 C7").u32(at(SPR_MENU_EXTRAS + SPR_NUM0));
        c.call(at(DRAW_FS2_SPRITES));
        c.hex("D9 04 24 D8 05").u32(at(FLT_10)).hex("D9 1C 24");
        c.hex("A1").u32(at(CREDITS)).hex("BB 0A 00 00 00 F6 F3 C1 E8 08 B3 0C F6 E3 0F B6 F8 81 C7").u32(at(SPR_MENU_EXTRAS + SPR_NUM0));
        c.call(at(DRAW_FS2_SPRITES)).hex("EB 20");
        c.hex("68 00 00 80 3F 6A").u8(transform).hex("6A 00 68").f32(sx).hex("68").f32(sy).hex("68 17 B7 D1 38");
        c.hex("FF 74 24 30 FF 74 24 30").call(at(DRAW_FS2_SPRITES)).hex("83 C4 20 5F C3");
        self.draw2 = c.addr;
    }

    fn create_draw_all(&mut self) {
        let mut c = Cave::new(2048);
        c.call(self.draw1).call(self.draw2).hex("C3");
        self.draw_all = c.addr;
    }

    /// The game's frame thread: the RELOAD sprites, the title's start skip, the attract's
    /// introduction, the blinking credits texts.
    fn quick_thread(&self) {
        let mut c = Cave::new(2048);
        c.hex("53 52");
        c.hex("83 3D").u32(at(P1_AMMO)).hex("00 7E 0A C7 05").u32(self.d(DB_SHOW_RELOAD_P1)).hex("00 00 00 00");
        c.hex("83 3D").u32(at(P2_AMMO)).hex("00 7E 0A C7 05").u32(self.d(DB_SHOW_RELOAD_P2)).hex("00 00 00 00");
        for (show, status, x) in [(DB_SHOW_RELOAD_P1, P1_STATUS, 70f32), (DB_SHOW_RELOAD_P2, P2_STATUS, 570f32)] {
            let t = if show == DB_SHOW_RELOAD_P1 { "6A 05" } else { "6A 07" };
            c.hex("83 3D").u32(self.d(show)).hex("00 74 38");
            c.hex("83 3D").u32(at(status)).hex("05 75 2F");
            c.hex("68 00 00 80 3F").hex(t).hex("6A 00 68 00 00 80 3F 68 00 00 80 3F 68 17 B7 D1 38 68").f32(240.0).hex("68").f32(x);
            c.hex("68 90 00 00 00").call(at(DRAW_FS_SPRITES)).hex("83 C4 24");
        }
        c.hex("83 3D").u32(at(CURRENT_SCENE)).hex("04 75 13");
        c.hex("83 3D").u32(self.d(DB_SKIP_INTRO)).hex("01 75 0A");
        c.hex("C7 05").u32(at(ENTER_KEY_PRESSED)).hex("01 00 00 00");
        // (the mod's jumps over this block 3 bytes longer: the push of the key mark)
        c.hex("83 3D").u32(at(CURRENT_SCENE)).hex("05 75 31");
        c.hex("83 3D").u32(self.d(DB_SKIP_INTRO)).hex("01 75 28");
        c.hex("C7 05").u32(self.d(DB_SKIP_INTRO)).hex("00 00 00 00");
        // PostMessageA(window, WM_KEYUP, player 1's start, the payload's key mark): the mod
        // posted lParam 0, which the input filter drops
        c.hex("50 A1").u32(at(WINDOW_BASE_PTR)).hex("8B 40 38");
        c.hex("68").u32(KEY_MARK).hex("6A").u8(self.p1_start_vk).hex("68 01 01 00 00 50");
        c.hex("A1").u32(at(POST_MESSAGE_PTR)).hex("FF D0 58");
        c.hex("83 3D").u32(at(CURRENT_SCENE)).hex("06 75 1D");
        c.hex("83 3D").u32(self.d(DB_SHOW_INTRO)).hex("01 75 14");
        c.hex("C7 05").u32(at(NEXT_SCENE)).hex("04 00 00 00");
        c.hex("C7 05").u32(self.d(DB_SHOW_INTRO)).hex("00 00 00 00");
        // GetTickCount (the import at 0x5491c0)
        c.hex("FF 15 C0 91 54 00");
        c.hex("83 3D").u32(self.d(DB_BLINK_TICK)).hex("00 75 0C");
        c.hex("A3").u32(self.d(DB_BLINK_TICK)).hex("A3").u32(self.d(DB_FRAME_TICK)).hex("EB 3A");
        c.hex("8B D8 2B 1D").u32(self.d(DB_BLINK_TICK)).hex("81 FB 00 03 00 00 7E 18");
        c.hex("A3").u32(self.d(DB_BLINK_TICK)).hex("8B 1D").u32(self.d(DB_SPRITE_VISIBLE)).hex("B9 01 00 00 00 29 D9 89 0D").u32(self.d(DB_SPRITE_VISIBLE));
        c.hex("8B D8 2B 1D").u32(self.d(DB_FRAME_TICK)).hex("83 FB 01 7C 35");
        c.hex("A3").u32(self.d(DB_FRAME_TICK)).hex("A1").u32(at(CURRENT_SCENE));
        c.hex("83 F8 02 74 26 83 F8 0A 74 21 83 F8 0B 74 1C");
        c.hex("68").f32(444.0).hex("68").f32(320.0).hex("68").f32(426.0).hex("68").f32(320.0);
        c.call(self.draw_all).hex("83 C4 10");
        c.hex("5A 5B 8B 44 24 14 8B 08").jmp(at(0x3E193 + 6));
        hook(0x3E193, 6, c.addr);
    }

    fn introduction_loop(&self) {
        write(at(0x76383), &[1]);
        write(at(0x763F9), &[1]);
        write(at(0x76505), &[7]);
    }

    fn title_screen(&self) {
        let logo = 0xA8D3D;
        nops(0x8E005, 16);
        write(at(logo), &hex("90 90 90 57 6A 00"));
        write(at(logo + 183), &hex("31 D2 90 90"));
        write(at(logo + 47), &hex("90 FF 74 24 2C"));
        write(at(logo + 53), &[10]);
        write(at(logo + 72), &390f32.to_le_bytes());
        write(at(logo + 77), &320f32.to_le_bytes());
        write(at(logo + 84), &at(SPR_MENU_EXTRAS + SPR_TRADEMARK).to_le_bytes());
        let mut c = Cave::new(2048);
        c.call(at(DRAW_FS2_SPRITES)).hex("C7 04 24").f32(320.0).hex("C7 44 24 04").f32(240.0);
        c.hex("BF").u32(at(SPR_MENU_EXTRAS + SPR_LOGO)).call(at(DRAW_FS2_SPRITES)).jmp(at(0xA8D95 + 5));
        hook(0xA8D95, 5, c.addr);
    }

    fn attract_mode_sprites(&self) {
        let mut c = Cave::new(2048);
        for (id, sprite, y, skip) in [("C4", SPR_ATTRACT_RELOAD, 160f32, "EB 62"), ("BF", SPR_ATTRACT_SHELLS, 284f32, "EB 1A")] {
            c.hex(&format!("81 3C 24 {id} 00 00 00")).hex("75 3F 57 51 52 BF").u32(at(SPR_MENU_EXTRAS + sprite));
            c.hex("8B 44 24 2C 50 6A 05 6A 00 68 00 00 80 3F 68 00 00 80 3F 68 AC C5 A7 37 68").f32(y).hex("68").f32(96.0);
            c.call(at(DRAW_FS2_SPRITES)).hex("83 C4 20 5A 59 5F C7 44 24 20 00 00 00 00").hex(skip);
        }
        c.hex("81 3C 24 CB 00 00 00 74 09 81 3C 24 C6 00 00 00 75 08 C7 44 24 20 00 00 00 00");
        c.call(at(DRAW_FS_SPRITES)).jmp(at(0xA960A + 5));
        hook(0xA960A, 5, c.addr);
    }

    fn disable_time_attack_attract(&self) {
        write(at(0xA9120), &hex("EB 23"));
        write(at(0x9B106), &hex("31 C0 90"));
    }

    fn credits_handling(&self) {
        nops(0x3E660, 6);
        nops(0x3E610, 2);
        nops(0x3E640, 2);
        nops(0x3E600, 7);
        nops(0x3E0E3, 10);
        write(at(CREDITS_TO_START), &[self.c.credits_to_start as u8]);
        write(at(CREDITS_TO_CONTINUE), &[self.c.credits_to_continue as u8]);
    }

    fn start_game_from_title(&self) {
        write(at(0x3E4F2), &[5]);
        write(at(0x8D9A4), &[5]);
        let mut c = Cave::new(2048);
        c.hex("83 FF 05 75 0F C7 05").u32(at(ALLOW_START_GAME)).hex("01 00 00 00").jmp(at(0x8E01E));
        c.hex("0F B6 80").u32(at(START_GAME_BYTE_TABLE)).jmp(at(0x8DFED + 7));
        hook(0x8DFED, 7, c.addr);
    }

    fn hide_cursor(&self) {
        write(at(0x32BDF), &[0]);
        write(at(0x32B7E), &[0]);
    }

    fn return_to_menu(&self) {
        let mut c = Cave::new(2048);
        c.hex("83 FE 02 75 0E 83 3D").u32(at(CURRENT_SCENE)).hex("0A 75 05 BE 05 00 00 00 83 FE 08");
        c.jl(at(0x32BB0 + 37)).jmp(at(0x32BB0 + 5));
        hook(0x32BB0, 5, c.addr);
    }

    fn max_life(&self) {
        let max = self.c.max_life as u8;
        write(at(0x8E39E), &[max]);
        let mut c = Cave::new(2048);
        c.hex("0F BE 82 86 01 00 00 83 F8").u8(max - 1).hex("7E 05 B8").u32((max - 1) as u32).jmp(at(0x8DC08 + 7));
        hook(0x8DC08, 7, c.addr);
    }

    fn fast_reload(&self) {
        let r = 0x204DCC;
        write(at(r + 4), &[17]);
        write(at(r + 28), &[17]);
        write(at(r + 24), &[9]);
        write(at(r), &[9]);
    }

    fn reload_effects(&self) {
        let mut c = Cave::new(2048);
        c.hex("50 51 8B 40 04 C1 E0 02 05").u32(self.d(DB_SHOW_RELOAD_P1)).hex("C7 00 01 00 00 00");
        c.hex("6A 00 68").u32(SFX_RELOAD).hex("68").u32(at(SOUND_PLAYER)).call(at(PLAY_SFX)).hex("59 58").jmp(at(0x8DF1E + 5));
        hook(0x8DF1E, 5, c.addr);
        let mut c = Cave::new(2048);
        c.hex("C7 40 68 00 00 00 00 83 3D").u32(at(CURRENT_SCENE)).hex("06 75 03 88 5D 18");
        c.hex("50 51 8B 40 04 C1 E0 02 05").u32(self.d(DB_SHOW_RELOAD_P1)).hex("C7 00 01 00 00 00 59 58").jmp(at(0x8DED4 + 7));
        hook(0x8DED4, 7, c.addr);
    }

    /// An in-game sprite (`sprite` for player 1 at `x1`, player 2 at `x2`) drawn instead of the
    /// game's.
    fn in_game_sprite(&self, rva: u32, y: f32, x1: f32, x2: f32, select: &dyn Fn(&mut Cave), player_test: &str) {
        let mut c = Cave::new(2048);
        c.hex("57 51 52 68 00 00 80 3F 6A 0A 6A 00 68").f32(0.25).hex("68").f32(0.25).hex("68 17 B7 D1 38");
        c.hex("68").f32(y).hex("68").f32(x1);
        select(&mut c);
        c.hex(player_test).hex("75 07 C7 04 24").f32(x2);
        c.call(at(DRAW_FS2_SPRITES)).hex("83 C4 20 5A 59 5F 68 00 00 00 00").jmp(at(rva + 5));
        hook(rva, 5, c.addr);
    }

    fn in_game_sprites(&self) {
        let x1 = -15f32;
        let x2 = if self.c.language() == 3 { 42f32 } else { 32f32 };
        // FREE PLAY, CREDITS: edi the player (1: player 2), the sprite in edi
        self.in_game_sprite(0x3E212, 444.0, 142.0, 496.0, &|c| {
            c.hex("83 FF 01 BF").u32(at(SPR_MENU_EXTRAS + SPR_FREEPLAY));
        }, "");
        self.in_game_sprite(0x3E25E, 444.0, 142.0 + x1, 496.0 + x1, &|c| {
            c.hex("83 FF 01 BF").u32(at(SPR_MENU_EXTRAS + SPR_CREDITS));
        }, "");
        // the credits' digits: eax the digit, the player on the stack
        write(at(0x3E2C3 - 6), &hex("90 90"));
        self.in_game_sprite(0x3E2C3, 444.0, 142.0 + x2, 496.0 + x2, &|c| {
            c.hex("B9 0C 00 00 00 F7 E1 05").u32(at(SPR_MENU_EXTRAS + SPR_NUM0)).hex("8B F8");
        }, "83 7C 24 2C 01");
        self.in_game_sprite(0x3E30B, 444.0, 142.0 + x2 + 10.0, 496.0 + x2 + 10.0, &|c| {
            c.hex("8B C6 B9 0C 00 00 00 F7 E1 05").u32(at(SPR_MENU_EXTRAS + SPR_NUM0)).hex("8B F8");
        }, "83 7C 24 2C 01");
        // PRESS START / INSERT COIN: edi the player, its status (9: not playing yet)
        let mut c = Cave::new(2048);
        c.hex("57 51 52 68 00 00 80 3F 6A 0A 6A 00 68").f32(0.25).hex("68").f32(0.25).hex("68 17 B7 D1 38");
        c.hex("68").f32(426.0).hex("68").f32(142.0);
        c.hex("8B C7 B9 2C 02 00 00 F7 E1 05").u32(at(P1_STATUS));
        c.hex("83 FF 01 BF").u32(at(SPR_MENU_EXTRAS + SPR_PRESS_START)).hex("75 07 C7 04 24").f32(496.0);
        c.hex("80 3D").u32(at(FLAG_FREEPLAY)).hex("01 74 1E 31 C9 8A 0D").u32(at(CREDITS_TO_CONTINUE));
        c.hex("83 38 09 75 06 8A 0D").u32(at(CREDITS_TO_START)).hex("39 0D").u32(at(CREDITS)).hex("7D 03 83 C7 18");
        c.call(at(DRAW_FS2_SPRITES)).hex("83 C4 20 5A 59 5F 68 00 00 00 00").jmp(at(0x3E1CC + 5));
        hook(0x3E1CC, 5, c.addr);
    }

    fn name_entry_title(&self) {
        write(at(0x80705 + 3), &[40]);
        write(at(0x80705 + 31), &hex("00 90 90 90"));
    }

    fn remove_endgame_save_popup(&self) {
        let mut c = Cave::new(2048);
        c.hex("8B 4E 24 83 3D").u32(at(CURRENT_SCENE)).hex("0A 75 02 31 C9 8B 56 20").jmp(at(0x2BA56 + 6));
        hook(0x2BA56, 6, c.addr);
    }

    fn ranking_title(&self) {
        let mut c = Cave::new(2048);
        c.hex("81 3C 24 57 03 00 00 75 2A C7 44 24 1C 06 00 00 00 C7 44 24 04").f32(320.0);
        c.hex("57 50 51 52 BF").u32(at(SPR_MENU_EXTRAS + SPR_RANKINGS)).hex("83 C4 14").call(at(DRAW_FS2_SPRITES));
        c.hex("83 EC 14 5A 59 58 5F EB 05").call(at(DRAW_FS_SPRITES)).jmp(at(0xA68CB + 5));
        hook(0xA68CB, 5, c.addr);
    }

    fn apply(&mut self, root: &str, exe_dir: &str, res: &[(String, Vec<u8>)]) {
        let get = |name: &str| res.iter().find(|(n, _)| n == name).map(|(_, d)| d.as_slice());
        if let Some(ranking) = get("DefaultRanking") {
            write(at(0x206E48), ranking);
        }
        self.data_bank(exe_dir);
        self.create_draw_sprite1(0.25, 0.25, 10);
        self.create_draw_sprite2(0.25, 0.25, 10);
        self.create_draw_all();
        self.registry_checks(root);
        if self.c.hide_cursor {
            self.hide_cursor();
        }
        self.override_loading_dat();
        for rva in [0x3D30D, 0x3D331, 0x3D355] {
            self.override_default_dat(rva);
        }
        self.override_save_file();
        self.wndproc_loop();
        self.wndproc_credits_button();
        self.force_coin_sound();
        self.credits_handling();
        let menu = ["spr_menuextras_en", "spr_menuextras_fr", "spr_menuextras_it", "spr_menuextras_es"];
        let menu = get(menu.get(self.c.language() as usize).copied().unwrap_or(menu[0])).or(get(menu[0]));
        if let Some(menu) = menu {
            self.inject_sprites(menu, 0x77737, 0x77759);
        }
        if let Some(logo) = get("spr_logo") {
            self.inject_sprites(logo, 0x75F3F, 0x75F5F);
        }
        self.quick_thread();
        self.introduction_loop();
        self.title_screen();
        self.attract_mode_sprites();
        self.start_game_from_title();
        self.disable_time_attack_attract();
        self.return_to_menu();
        if self.c.disable_pause {
            write(at(0x8C2B3), &hex("31 C0 90 90"));
        }
        if self.c.display_score {
            nops(0x8F1C7, 2);
        }
        self.max_life();
        self.fast_reload();
        // the guns not drawn (arcade mode display)
        nops(0x8FD29, 2);
        // no auto reload
        nops(0x8DEDB, 3);
        self.reload_effects();
        self.in_game_sprites();
        // the continue screen shown
        write(at(0x3E549), &hex("EB 05"));
        self.name_entry_title();
        self.remove_endgame_save_popup();
        // auto save after the game's end
        write(at(0x2E3A1), &hex("52 56 BE 60 30 9D 00 E8 33 F2 00 00 5E 5A EB 1A"));
        self.ranking_title();
        // no exit confirmation
        nops(0xB2AB2, 12);
        write(at(0xB2AB2), &hex("E9 E5 00 00 00"));
        if self.c.hide_crosshairs {
            write(at(0x8E989), &[0]);
        }
    }
}

/// DemulShooter's input hack: the game's aim writes and trigger / reload / left / right reads
/// removed, its input blocks' other bits cleared each frame.
fn guns_hack() {
    nops(0x6BE70, 4);
    nops(0x6BE7B, 4);
    let mut c = Cave::new(0x800);
    c.hex("8B E9 81 65 00 0C 00 01 01").jmp(at(0xB4BC1 + 5));
    hook(0xB4BC1, 5, c.addr);
    nops(0xB4C8C, 3);
    nops(0xB4CB1, 4);
    nops(0xB4C54, 3);
    nops(0xB4C0E, 3);
    for a in AIM {
        write(at(a), &[0; 4]);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Reload,
    Action,
    Left,
    Right,
    Start,
    Coin,
    Offscreen,
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("reload", Native::Reload),
    ("action", Native::Action),
    ("left", Native::Left),
    ("right", Native::Right),
    ("start", Native::Start),
    ("coin", Native::Coin),
    ("offscreen", Native::Offscreen),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("b2", "reload"),
    ("b3", "action"),
    ("b6", "offscreen"),
    ("left", "left"),
    ("right", "right"),
    ("start", "start"),
    ("coin", "coin"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// A scan code's virtual key (the game's start keys are scan codes).
fn scan_to_vk(scan: u32, default: u8) -> u8 {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::MapVirtualKeyW;
    match unsafe { MapVirtualKeyW(scan, 1) } {
        0 => default,
        vk => vk as u8,
    }
}

fn set_bit(addr: u32, bit: u8, on: bool) {
    let p = addr as *mut u8;
    unsafe {
        let v = p.read_volatile();
        let n = if on { v | bit } else { v & !bit };
        if n != v {
            p.write_volatile(n);
        }
    }
}

fn input_thread(start_vk: [u8; 2], fire_starts: bool) {
    let mut start_was = [false; 2];
    let mut coin_was = [false; 2];
    let mut aim = [(0i16, 0i16); 2];
    let mut state_was = [u32::MAX; 3];
    loop {
        std::thread::sleep(Duration::from_millis(8));
        let hwnd = gun::find_window();
        if hwnd.is_null() {
            continue;
        }
        let state = [CURRENT_SCENE, P1_STATUS, P2_STATUS].map(|rva| unsafe { (at(rva) as *const u32).read_volatile() });
        if state != state_was {
            log!("hotd3: scene {}, players' status {} {}", state[0], state[1], state[2]);
            state_was = state;
        }
        for p in 0..2 {
            let (mut trigger, mut reload, mut action, mut left, mut right, mut start, mut coin, mut off) = Default::default();
            map().for_each_pressed(&input(p), |n| match n {
                Native::Trigger => trigger = true,
                Native::Reload => reload = true,
                Native::Action => action = true,
                Native::Left => left = true,
                Native::Right => right = true,
                Native::Start => start = true,
                Native::Coin => coin = true,
                Native::Offscreen => off = true,
            });
            match gun::gun_of_player_in_picture(p) {
                Some((x, y)) => {
                    let gx = (641.0 * x - 320.5).round().clamp(-320.0, 320.0) as i16;
                    let gy = (240.5 - 481.0 * y).round().clamp(-240.0, 240.0) as i16;
                    aim[p] = (gx, gy);
                    unsafe { (at(AIM[p]) as *mut [i16; 2]).write_volatile([gx, gy]) };
                }
                None => off = true,
            }
            let b = at(BUTTONS[p]);
            set_bit(b + 2, 0x01, trigger && !off);
            set_bit(b + 3, 0x01, reload || (trigger && off));
            set_bit(b, 0x04, left || (action && aim[p].0 < 0));
            set_bit(b, 0x08, right || (action && aim[p].0 >= 0));
            // the trigger is start too: always without the in-game pause, else while the player
            // is not playing (status 5)
            let start = start || (trigger && (fire_starts || state[1 + p] != 5));
            if start != start_was[p] {
                post_key(hwnd, start_vk[p] as u16, start);
                start_was[p] = start;
            }
            if coin != coin_was[p] {
                post_key(hwnd, VK_COIN, coin);
                coin_was[p] = coin;
            }
        }
    }
}

/// The game's folder (the executable's parent's: `<root>\exe\hod3pc.exe`) and the executable's.
fn folders() -> (String, String) {
    let mut buf = [0u16; 520];
    let n = unsafe { GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let exe = String::from_utf16_lossy(&buf[..n]);
    let exe_dir = exe.rsplit_once('\\').map_or(".", |(d, _)| d).to_string();
    let root = exe_dir.rsplit_once('\\').map_or(".", |(d, _)| d).to_string();
    (root, exe_dir)
}

pub fn init() {
    let (root, exe_dir) = folders();
    let c = read_config(&root);
    let fire_starts = c.disable_pause;
    let start_vk = [scan_to_vk(c.reg[16], b'1'), scan_to_vk(c.reg[17], b'2')];
    let launcher = format!("{root}\\Hotd3Arcade_Launcher.exe");
    let res = std::fs::read(&launcher).map(|f| net_resources(&f)).unwrap_or_default();
    if res.is_empty() {
        log!("hotd3: no ArcadeMod resources in {launcher}: the mod not applied");
    } else {
        log!(
            "hotd3: ArcadeMod (resources {}), free play {}, difficulty {}, life {}/{}, language {}, resolution {}, full screen {}, crosshairs {}",
            res.iter().map(|(n, d)| format!("{n} {}", d.len())).collect::<Vec<_>>().join(", "),
            c.free_play,
            c.difficulty,
            c.initial_life,
            c.max_life,
            c.reg[37],
            c.reg[30],
            c.reg[31],
            !c.hide_crosshairs
        );
        let mut m = Mod { c, db: 0, draw1: 0, draw2: 0, draw_all: 0, p1_start_vk: start_vk[0] };
        m.apply(&root, &exe_dir, &res);
    }
    guns_hack();
    let exe = base() as usize;
    wal_payload_common::input_filter::init(exe, |_| false);
    std::thread::spawn(move || input_thread(start_vk, fire_starts));
    log!("hotd3: the guns in the players' input blocks, start keys {start_vk:?}, saves in {exe_dir}");
}
