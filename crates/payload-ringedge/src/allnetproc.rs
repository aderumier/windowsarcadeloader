//! maimai FiNALE's network process, `ALLNetProc.exe`, emulated in the game's process (the
//! packed helper exits at once under Wine: its unpacker looks for a folder its own packed
//! launcher prepares). Enabled with `WAL_RINGEDGE_ALLNET=maimai`.
//!
//! The game and the helper share two blocks (`ALLNetComA2P`, App2Proc, 0x894 bytes, the game's;
//! `ALLNetComP2A`, Proc2App, 0x16F0 bytes, the helper's), found through the game's singleton
//! (`WAL_RINGEDGE_ALLNETPROC=<rva>`: `{flag, App2Proc*, Proc2App*}`, +0xB40E04 in FiNALE
//! 1.99B). The game asks for its mode in App2Proc[0] (2 online, 1 offline) and reads the
//! helper's status; its only requests are the players' accounting reports (App2Proc +0x10 + i *
//! 0x220, one slot per player: +0x08 / +0x0E sequence numbers, +0x20 a text), sent once the
//! previous one is acknowledged (Proc2App +0xCB0 + i * 10: +2 / +4), which [`run`] does at once:
//! unacknowledged for 5 seconds, the player's play goes on as a guest's (FUN_0047f790). Every 5
//! seconds the game checks that the helper runs with its named mutex `ALLNetProc` (else it starts
//! `app\ALLNetProc.exe` again): this process holds that mutex.
//!
//! Proc2App, as written here once the game asks to be online:
//! - +0x000: the state, App2Proc[0] echoed (2: ready; 0: no helper)
//! - +0x010: BoardInfo (0x40 bytes) and +0x058: AuthResp (0x800 bytes, the ALL.Net power-on's
//!   result), each AES-128-CBC encrypted with the game's keys, the IV at +0x16E0
//! - +0x858: the ALL.Net error code (0), +0xC5C: AuthResp valid (1), +0xC5D: accounting (1 once
//!   the boot's game settings came: the players' play reports, acknowledged at once, see
//!   [`run`]), +0xC5F: keychip error (0),
//!   +0xC60: the accounting state (6, OFF: idle; 1 REPORT is "network processing", no Aime),
//!   +0xC62: network down (0), +0xC63: BoardInfo valid (1), +0xC88 / +0xC90: the authentication's give-up count and remaining time (0: wait for
//!   the authentication), +0xC94 / +0xC98: the LAN / WAN DNS servers, +0xCA0: the server's
//!   address, +0xD3F: reboot request (0)
//!
//! AuthResp: +0x008 the country (`JPN`), +0x00C the shop's name, +0x434 the title server's URI
//! (the game posts to `<uri>MaimaiServlet/<Api>`, [`title_uri`]), +0x634 the server's address,
//! +0x640 the region id, +0x644 the four region names (64 bytes each), +0x744 the place id.
//! BoardInfo: +0x00 (5 characters), +0x06 the keychip id (12 characters: the User-Agent's), +0x12
//! and +0x22 (16 characters each: the main board's and the keychip's serials).
//! The AimeDB the game reaches itself (`aime.naominet.jp`, port 22345: the payload's, see
//! `crate::diva::allnet`).
//!
//! maimai PiNK's helper (`WAL_RINGEDGE_ALLNETPROC_LAYOUT=pink`, ALLNetProc.exe 1.60: its
//! FUN_0040a200 and FUN_00409f40 fill these): Proc2App is 0x12D4 bytes, nothing encrypted; its
//! AuthResp is 0x400 bytes at +0x050 (the same fields, shorter texts: +0x000 the place id as hex
//! text, +0x008 the country, +0x00C the shop's name, 40 characters, +0x0D4 the title server's URI,
//! +0x2D4 the server's address, +0x2E0 the region id, +0x2E4 the four region names, +0x3E4 the
//! place id), and every field after it 0x408 bytes lower than FiNALE's (+0x854 AuthResp valid,
//! +0x855 accounting, +0x858 the accounting state...), but the accounting acknowledgements at
//! +0x8A4, each with three counters echoed (+0x00 the slot's +0x02, +0x02 its +0x08, +0x04 its
//! +0x0E: FUN_005e6ed0, FUN_005e6f50, FUN_005e6e40; the player's login waits for all three, else
//! the player is not saved).

use std::sync::atomic::{AtomicUsize, Ordering};

use aes::cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};
use wal_payload_common::log;

/// The game's keys of the two encrypted records (its image's +0x854B70 BoardInfo, +0x854B80
/// AuthResp).
const BOARD_INFO_KEY: [u8; 16] = [0xbf, 0xc8, 0x62, 0xe8, 0xaf, 0x77, 0x64, 0x62, 0x87, 0x78, 0x58, 0xeb, 0x9e, 0x3f, 0xd5, 0x32];
const AUTH_RESP_KEY: [u8; 16] = [0x51, 0x3c, 0xea, 0x81, 0xde, 0x5f, 0xf9, 0xaa, 0x72, 0x3c, 0xb4, 0x38, 0x41, 0x11, 0x7d, 0xec];
const IV: [u8; 16] = [0; 16];

/// Where the helper's records and status sit in Proc2App: FiNALE's, or maimai PiNK's
/// (`WAL_RINGEDGE_ALLNETPROC_LAYOUT=pink`).
struct Layout {
    /// BoardInfo and AuthResp AES encrypted (the IV at +0x16E0)
    encrypted: bool,
    /// AuthResp's offset and size
    auth_resp: usize,
    auth_size: usize,
    /// AuthResp: the shop's name and its size, the title server's URI, the server's address, the
    /// region id, the region names, the place id
    name: (usize, usize),
    uri: usize,
    address: usize,
    region: usize,
    region_names: usize,
    place: usize,
    /// subtracted from FiNALE's offsets of the fields after AuthResp
    shift: usize,
    /// the players' accounting acknowledgements (10 bytes each)
    acks: usize,
}

const FINALE: Layout = Layout { encrypted: true, auth_resp: 0x058, auth_size: 0x800, name: (0x00C, 256), uri: 0x434, address: 0x634, region: 0x640, region_names: 0x644, place: 0x744, shift: 0, acks: 0xCB0 };
const PINK: Layout = Layout { encrypted: false, auth_resp: 0x050, auth_size: 0x400, name: (0x00C, 40), uri: 0x0D4, address: 0x2D4, region: 0x2E0, region_names: 0x2E4, place: 0x3E4, shift: 0x408, acks: 0x8A4 };

fn layout() -> &'static Layout {
    static PINK_LAYOUT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *PINK_LAYOUT.get_or_init(|| std::env::var("WAL_RINGEDGE_ALLNETPROC_LAYOUT").is_ok_and(|v| v.trim().eq_ignore_ascii_case("pink"))) { &PINK } else { &FINALE }
}

/// The title server's URI (the power-on's `uri`): `WAL_ALLS_TITLE_URI`, else
/// `http://<router>/SDEY/197/` (FiNALE's).
pub(crate) fn title_uri() -> String {
    wal_payload_common::allnet::title_uri("SDEY/197")
}

fn encrypt(key: &[u8; 16], plain: &[u8]) -> Vec<u8> {
    let cipher = aes::Aes128::new(GenericArray::from_slice(key));
    let mut prev = IV;
    let mut out = Vec::with_capacity(plain.len());
    for block in plain.chunks(16) {
        let mut b = [0u8; 16];
        b[..block.len()].copy_from_slice(block);
        for (x, p) in b.iter_mut().zip(prev) {
            *x ^= p;
        }
        let mut g = GenericArray::from(b);
        cipher.encrypt_block(&mut g);
        prev.copy_from_slice(&g);
        out.extend_from_slice(&g);
    }
    out
}

fn put(buf: &mut [u8], at: usize, text: &str, max: usize) {
    let bytes = text.as_bytes();
    let n = bytes.len().min(max - 1);
    buf[at..at + n].copy_from_slice(&bytes[..n]);
}

/// The router's address (the title server's, ALL.Net's), as the game keeps addresses: its
/// octets in order in memory.
fn router() -> [u8; 4] {
    wal_payload_common::dns::router()
}

fn auth_resp() -> Vec<u8> {
    let l = layout();
    let mut r = vec![0u8; l.auth_size];
    if !l.encrypted {
        put(&mut r, 0x000, "123", 8);
    }
    put(&mut r, 0x008, "JPN", 4);
    put(&mut r, l.name.0, "WindowsArcadeLoader", l.name.1);
    put(&mut r, l.uri, &title_uri(), 256);
    r[l.address..l.address + 4].copy_from_slice(&router());
    r[l.region..l.region + 4].copy_from_slice(&1u32.to_le_bytes());
    for (i, name) in ["W", "X", "Y", "Z"].iter().enumerate() {
        put(&mut r, l.region_names + i * 64, name, 64);
    }
    r[l.place..l.place + 4].copy_from_slice(&0x123u32.to_le_bytes());
    r
}

fn board_info() -> Vec<u8> {
    let mut b = vec![0u8; 0x40];
    put(&mut b, 0x00, "AAL2", 6);
    put(&mut b, 0x06, "A69E01A8888", 13);
    put(&mut b, 0x12, "AAVE01A00000001", 17);
    put(&mut b, 0x22, "A69E01A88888888", 17);
    b
}

/// Writes the helper's status into Proc2App (`p2a`), for the game's mode `mode`.
unsafe fn publish(p2a: *mut u8, mode: u32) {
    let l = layout();
    let set_at = |at: usize, bytes: &[u8]| unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), p2a.add(at), bytes.len()) };
    if l.encrypted {
        set_at(0x010, &encrypt(&BOARD_INFO_KEY, &board_info()));
        set_at(l.auth_resp, &encrypt(&AUTH_RESP_KEY, &auth_resp()));
        set_at(0x16E0, &IV);
    } else {
        set_at(0x010, &board_info());
        set_at(l.auth_resp, &auth_resp());
    }
    // the fields after AuthResp, at FiNALE's offsets
    let set = |at: usize, bytes: &[u8]| set_at(at - l.shift, bytes);
    set(0x858, &0u32.to_le_bytes());
    set(0xC5D, &[accounting_enabled()]);
    set(0xC5F, &[0]);
    set(0xC60, &[ACCOUNTING.load(Ordering::Relaxed) as u8]);
    set(0xC62, &[0]);
    set(0xC88, &0u32.to_le_bytes());
    set(0xC90, &0u32.to_le_bytes());
    set(0xC94, &[127, 0, 0, 1]);
    set(0xC98, &[127, 0, 0, 1]);
    set(0xCA0, &router());
    set(0xD3F, &[0]);
    // the records valid, then the helper ready
    set(0xC63, &[1]);
    set(0xC5C, &[1]);
    set_at(0x000, &mode.to_le_bytes());
}

/// The accounting enabled (Proc2App +0xC5D) once the boot's game settings came from the title
/// server: the game marks every player's accounting slot pending while authenticated, and holds
/// back its title server requests while accounting has pending slots (the boot's
/// `GetGameSettingApi` never sent: "Game Server WAIT").
fn accounting_enabled() -> u8 {
    crate::maimai_server::GAME_SETTING_SENT.load(Ordering::Relaxed) as u8
}

static SINGLETON: AtomicUsize = AtomicUsize::new(0);
/// The accounting state written (`WAL_RINGEDGE_ALLNETPROC_ACCOUNTING`, default 6: OFF, the idle state; 1 REPORT shows "network processing" and makes Aime unusable).
static ACCOUNTING: AtomicUsize = AtomicUsize::new(6);

/// The helper: its mutex held, then the status published whenever the game's blocks show no
/// helper (created, or cleared by the game's network manager) while the game asks to be online.
fn run() {
    let name: Vec<u16> = "ALLNetProc".encode_utf16().chain([0]).collect();
    let mutex = unsafe { windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 1, name.as_ptr()) };
    log!("allnetproc: mutex ALLNetProc held ({mutex:?})");
    let singleton = SINGLETON.load(Ordering::Relaxed);
    let mut logged = 0u32;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let object = unsafe { *(singleton as *const usize) };
        if object == 0 {
            continue;
        }
        let (a2p, p2a) = unsafe { (*((object + 4) as *const usize) as *const u32, *((object + 8) as *const usize) as *mut u8) };
        if a2p.is_null() || p2a.is_null() {
            continue;
        }
        let mode = unsafe { a2p.read_volatile() };
        let state = unsafe { (p2a as *const u32).read_volatile() };
        let valid = unsafe { p2a.add(0xC5C - layout().shift).read_volatile() };
        // the players' accounting requests (App2Proc +0x10 + i * 0x220: +0x08 / +0x0E their
        // sequence numbers) acknowledged at once (Proc2App +0xCB0 + i * 10: +2 / +4): the game
        // sends one only once the previous is (a player's play report, else after 5 s the
        // player plays as a guest)
        let shift = layout().shift;
        if mode >= 2 {
            unsafe { p2a.add(0xC5D - shift).write_volatile(accounting_enabled()) };
            for i in 0..4 {
                let slot = unsafe { (a2p as *const u8).add(0x10 + i * 0x220) };
                let ack = unsafe { p2a.add(layout().acks + i * 10) };
                unsafe {
                    // maimai PiNK's third counter: +0 echoes the slot's +2 (FUN_005e6ed0)
                    if layout().acks != FINALE.acks {
                        (ack as *mut u16).write_volatile((slot.add(2) as *const u16).read_volatile());
                    }
                    (ack.add(2) as *mut u16).write_volatile((slot.add(8) as *const u16).read_volatile());
                    (ack.add(4) as *mut u16).write_volatile((slot.add(0xE) as *const u16).read_volatile());
                }
            }
        }
        if mode >= 2 && (state != mode || valid != 1) {
            unsafe { publish(p2a, mode) };
            if logged < 5 {
                logged += 1;
                log!("allnetproc: game mode {mode}: authenticated, title server {}", title_uri());
            }
        }
    }
}

/// The emulated helper, when `WAL_RINGEDGE_ALLNETPROC` names the game's singleton.
pub(crate) fn init() {
    let Some(rva) = std::env::var("WAL_RINGEDGE_ALLNETPROC").ok().and_then(|v| usize::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok()) else {
        log!("allnetproc: WAL_RINGEDGE_ALLNETPROC not set: no network process");
        return;
    };
    let exe = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    SINGLETON.store(exe + rva, Ordering::Relaxed);
    if let Some(a) = std::env::var("WAL_RINGEDGE_ALLNETPROC_ACCOUNTING").ok().and_then(|v| v.trim().parse().ok()) {
        ACCOUNTING.store(a, Ordering::Relaxed);
    }
    let _ = title_uri();
    std::thread::spawn(run);
}
