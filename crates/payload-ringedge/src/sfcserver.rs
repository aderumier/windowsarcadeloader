//! Shining Force Cross's title server (the ALL.Net power-on's uri `/SBVB/`, the router's port
//! 80, `wal_payload_common::allnet`'s server). Enabled with `WAL_RINGEDGE_ALLNET=shiningforce`.
//!
//! The game's HTTP client (`RINGEDGE-PRJF/1.0`, `NrsHttp`) posts each request as a fixed size
//! big-endian structure, zlib packed (`allnet::set_title_rd2`), and reads the answer the same
//! way (`NetData*Response::Binary2Class`: exactly the structure's size, else refused). Each
//! structure starts with the protocol version (`0xC93D8B0E` Raid 1.05, `0xDDBC372D` Elysion
//! Ver.C: the answers' sizes per game, [`PROTOCOLS`]) and its command: a request's even, its
//! answer's the next odd one. The player's data is the card's: `start_response` hands back the
//! `card_body` the request read from the card, the game writes the card itself.
//!
//! `WAL_RINGEDGE_SFC_UNCOMPRESS=<rva>`: the game's call to zlib's `uncompress` for the answers
//! (`NrsHttp`, 0x8aad1a in Raid, 0x9e74ea in Elysion) passes an uninitialized output length (4 under Wine: every
//! answer refused, `Z_BUF_ERROR`): the call goes to [`uncompress`], the output buffer's size.
//!
//! `WAL_RINGEDGE_SFC_NETWORK=<rva>`: the shop network the game expects its address in (the
//! string `192.168.68.0`, compared on 3 bytes: else NETWORK(SYS) NG) becomes the machine's
//! (its CRI matching library binds the machine's real address: a made-up one fails).
//!
//! `WAL_RINGEDGE_SFC_CARD=<key query rva>:<keyboard rva>:<card task's call rva>`: the IC card. The game's card manager
//! also holds a development card device (`iccd::usb::DeviceUSB`): a card in files, `iccard.txt`
//! (its 20 digit id) and `iccard.bin` (its data) at the root of the first drive C: to M: or Z:
//! holding them, the profile's card drive; the profile patches the manager to hand it out
//! instead of the Maxell reader's. Its task inserts or ejects that card on the development
//! keyboard's Delete key (0x2E, the keyboard object's pressed query): player 1's card input is
//! that key press for the card task's call (its return address), through [`key_pressed`] in
//! place of the query.
//!
//! `WAL_RINGEDGE_SFC_CARD_AUTO=1`: the card put in on player 1's first start (the card input
//! still ejects it).
//!
//! `WAL_RINGEDGE_SFC_NEW_CARD=<drive>:<format version>:<size>:<offset>=<byte>,...`: a new card
//! in that drive's root when it holds none (the profile's card drive: the player's card, kept
//! there): `iccard.txt` a random id, `iccard.bin` a blank card of the game's format. The card:
//! its key record twice (31 bytes at 0 and 0x1F: the format version, the card id 0, the launch
//! code 0x1FE2000054F1, then the low 16 bits of the CRC-32 of its first 29 bytes, big endian),
//! then the body up to the last 2 bytes (zero but for the listed bytes, the game's blank card)
//! and the low 16 bits of its CRC-32.

use wal_payload_common::log;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

/// The answers' output buffer (`NrsHttp` + 0x4d570, as large as its receive buffer).
const OUTPUT_SIZE: usize = 0x4d44c;

/// zlib's `uncompress(dest, &dest_len, source, source_len)` (`Z_OK` 0, `Z_DATA_ERROR` -3),
/// `dest` [`OUTPUT_SIZE`] bytes whatever `dest_len` says.
unsafe extern "C" fn uncompress(dest: *mut u8, dest_len: *mut u32, source: *const u8, source_len: u32) -> i32 {
    let packed = unsafe { std::slice::from_raw_parts(source, source_len as usize) };
    match miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(packed, OUTPUT_SIZE) {
        Ok(data) => {
            unsafe {
                std::ptr::copy_nonoverlapping(data.as_ptr(), dest, data.len());
                *dest_len = data.len() as u32;
            }
            0
        }
        Err(e) => {
            log!("sfc: answer not unpacked: {e:?}");
            -3
        }
    }
}

/// The call at `WAL_RINGEDGE_SFC_UNCOMPRESS` to [`uncompress`], the network string at
/// `WAL_RINGEDGE_SFC_NETWORK`.
pub(crate) fn init() {
    network();
    uncompress_call();
    card();
}

fn rva(name: &str) -> Option<usize> {
    std::env::var(name).ok().and_then(|v| usize::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok())
}

unsafe fn write(at: usize, bytes: &[u8]) {
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const _, bytes.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
        VirtualProtect(at as *const _, bytes.len(), old, &mut old);
    }
}

fn network() {
    let Some(rva) = rva("WAL_RINGEDGE_SFC_NETWORK") else { return };
    let Some(ip) = crate::pcp::local_address() else { return };
    let at = wal_payload_common::iat::game_module() + rva;
    let o = ip.octets();
    // the original's room (`192.168.68.0`, 12 characters): a shorter one, NUL padded
    let mut text = format!("{}.{}.{}.0", o[0], o[1], o[2]).into_bytes();
    if text.len() > 12 {
        log!("sfc: network {} longer than the game's string", String::from_utf8_lossy(&text));
        return;
    }
    text.resize(13, 0);
    unsafe { write(at, &text) };
    log!("sfc: shop network {}.{}.{}.0", o[0], o[1], o[2]);
}

fn uncompress_call() {
    let Some(rva) = rva("WAL_RINGEDGE_SFC_UNCOMPRESS") else { return };
    let at = wal_payload_common::iat::game_module() + rva;
    if unsafe { *(at as *const u8) } != 0xE8 {
        log!("sfc: no call at +{rva:#x}");
        return;
    }
    let rel = (uncompress as *const () as usize).wrapping_sub(at + 5) as i32;
    unsafe { write(at + 1, &rel.to_le_bytes()) };
    log!("sfc: answers unpacked by the payload (call at +{rva:#x})");
}

/// The development keyboard (the game's keyboard object 0), the card task's call of its
/// pressed query (its return address) and the card input's pending press.
static KEYBOARD: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static CARD_CALLER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static CARD_PRESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// The keyboard object's pressed query (thiscall: ecx the keyboard, the key on the stack, `ret 4`)
// to [`key_pressed`] with its caller's return address.
core::arch::global_asm!(
    ".globl {name}",
    "{name}:",
    "push dword ptr [esp + 4]",
    "push dword ptr [esp + 4]",
    "push ecx",
    "call {query}",
    "add esp, 12",
    "ret 4",
    name = sym key_query_stub,
    query = sym key_pressed,
);

unsafe extern "C" {
    fn key_query_stub();
}

/// The keyboard's pressed query (`this + 4`: a bit per key, 0x67 keys); the card task's query of
/// Delete on keyboard 0 also answered by a pending card press (once).
extern "C" fn key_pressed(this: usize, caller: usize, key: u32) -> u32 {
    use std::sync::atomic::Ordering;
    if key == 0x2E && this == KEYBOARD.load(Ordering::Relaxed) && caller == CARD_CALLER.load(Ordering::Relaxed) && CARD_PRESS.swap(false, Ordering::Relaxed) {
        log!("sfc: card inserted or ejected");
        return 1;
    }
    if key >= 0x67 {
        return 0;
    }
    let bits = (this + 4) as *const u32;
    unsafe { (*bits.add((key >> 5) as usize) & (1 << (key & 31)) != 0) as u32 }
}

fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |mut crc, &b| {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
        crc
    })
}

/// A blank card of `version`, `size` bytes, the body's listed bytes set.
fn blank_card(version: u32, size: usize, bytes: &[(usize, u8)]) -> Vec<u8> {
    let mut card = vec![0u8; size];
    let mut key = [0u8; 31];
    key[0..4].copy_from_slice(&version.to_be_bytes());
    key[12..20].copy_from_slice(&0x1FE2_0000_54F1u64.to_be_bytes());
    let crc = crc32(&key[..29]) as u16;
    key[29..31].copy_from_slice(&crc.to_be_bytes());
    card[0..31].copy_from_slice(&key);
    card[31..62].copy_from_slice(&key);
    for &(at, b) in bytes {
        if (62..size - 2).contains(&at) {
            card[at] = b;
        }
    }
    let crc = crc32(&card[62..size - 2]) as u16;
    card[size - 2..].copy_from_slice(&crc.to_be_bytes());
    card
}

/// `WAL_RINGEDGE_SFC_NEW_CARD`: the card files, when the card drive holds none.
fn new_card() {
    let Ok(spec) = std::env::var("WAL_RINGEDGE_SFC_NEW_CARD") else { return };
    let mut parts = spec.trim().splitn(4, ':');
    let (Some(drive), Some(version), Some(size)) = (parts.next(), parts.next(), parts.next()) else { return };
    let (Ok(version), Ok(size)) = (u32::from_str_radix(version.trim_start_matches("0x"), 16), size.parse::<usize>()) else { return };
    let bytes: Vec<(usize, u8)> = parts
        .next()
        .unwrap_or("")
        .split(',')
        .filter_map(|b| {
            let (at, v) = b.split_once('=')?;
            Some((usize::from_str_radix(at.trim().trim_start_matches("0x"), 16).ok()?, u8::from_str_radix(v.trim(), 16).ok()?))
        })
        .collect();
    let root = std::path::PathBuf::from(format!("{}:\\", drive.trim()));
    let (id_path, data_path) = (root.join("iccard.txt"), root.join("iccard.bin"));
    if id_path.exists() && data_path.exists() || size < 64 {
        return;
    }
    // 20 digits from the time (a card per machine is enough)
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let id = format!("{:020}", nanos % 100_000_000_000_000_000_000);
    match std::fs::write(&id_path, &id).and_then(|_| std::fs::write(&data_path, blank_card(version, size, &bytes))) {
        Ok(()) => log!("sfc: new card {id} in {}", root.display()),
        Err(e) => log!("sfc: no new card in {}: {e}", root.display()),
    }
}

fn card() {
    use std::sync::atomic::Ordering;
    new_card();
    let Ok(value) = std::env::var("WAL_RINGEDGE_SFC_CARD") else { return };
    let rvas: Vec<usize> = value.split(':').filter_map(|v| usize::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok()).collect();
    let [query, keyboard, caller] = rvas[..] else {
        log!("sfc: WAL_RINGEDGE_SFC_CARD wants <query>:<keyboard>:<caller> rvas");
        return;
    };
    let base = wal_payload_common::iat::game_module();
    KEYBOARD.store(base + keyboard, Ordering::Relaxed);
    CARD_CALLER.store(base + caller, Ordering::Relaxed);
    let at = base + query;
    let mut jump = [0xE9u8, 0, 0, 0, 0];
    jump[1..].copy_from_slice(&((key_query_stub as *const () as usize).wrapping_sub(at + 5) as i32).to_le_bytes());
    unsafe { write(at, &jump) };
    // the card put in on player 1's first start (a press at boot is lost: the card scene
    // waits for its own insertion)
    let mut auto = std::env::var("WAL_RINGEDGE_SFC_CARD_AUTO").is_ok_and(|v| v.trim() == "1");
    std::thread::spawn(move || {
        let (mut held, mut start_held) = (false, false);
        loop {
            let input = wal_payload_common::input(0);
            let pressed = input.pressed(wal_protocol::button::CARD);
            let start = input.pressed(wal_protocol::button::START);
            if pressed && !held || auto && start && !start_held {
                CARD_PRESS.store(true, Ordering::Relaxed);
                auto = false;
            }
            (held, start_held) = (pressed, start);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    });
    log!("sfc: IC card in files, the card input inserts / ejects it");
}

/// One game's protocol: its version (the first field of every structure) and its commands
/// (request command, name, answer size: the `NetData*Response` classes' sizes, vtable + 0x30).
struct Protocol {
    version: u32,
    /// `card_body`'s size (after `card_key` in `start_request`, after the header in
    /// `start_response`)
    card_body: usize,
    commands: &'static [(u32, &'static str, usize)],
}

const PROTOCOLS: &[Protocol] = &[
    // Raid 1.05
    Protocol {
        version: 0xC93D_8B0E,
        card_body: 6604,
        commands: &[
            (0x00, "init", 0xc8),
            (0x02, "ping", 0x684),
            (0x04, "start", 0x2335),
            (0x06, "end", 0x1c),
            (0x08, "item", 0x19dc),
            (0x0a, "recognize", 0x64),
            (0x0c, "passcode_result", 0x10),
            (0x0e, "shop", 0x24a0),
            (0x10, "game_conf", 0x5e8),
            (0x12, "event_info", 0x1eb),
            (0x14, "campaign_info", 0x92),
            (0x16, "boost_mode_info", 0x26),
            (0x18, "fnet_regist", 0x10),
            (0x1a, "fnet_unregist", 0x10),
            (0x1c, "reissue_entry", 0x14),
            (0x1e, "reissue_exec", 0x1a18),
            (0x20, "get_avatar_info", 0x1a14),
        ],
    },
    // Elysion Ver.C
    Protocol {
        version: 0xDDBC_372D,
        card_body: 8342,
        commands: &[
            (0x00, "init", 0x1b8),
            (0x02, "ping", 0x7a0),
            (0x04, "start", 0x2a9d),
            (0x06, "end", 0x4e),
            (0x08, "item", 0x20a6),
            (0x0a, "recognize", 0x64),
            (0x0c, "passcode_result", 0x10),
            (0x0e, "shop", 0x1be4),
            (0x10, "game_conf", 0xd0a),
            (0x12, "event_info", 0x1eb),
            (0x14, "campaign_info", 0x98),
            (0x16, "boost_mode_info", 0x26),
            (0x18, "assist_mode_info", 0x50),
            (0x1a, "fnet_regist", 0x10),
            (0x1c, "fnet_unregist", 0x10),
            (0x1e, "reissue_entry", 0x14),
            (0x20, "reissue_exec", 0x20e2),
            (0x22, "get_avatar_info", 0x20de),
            (0x24, "auto_save", 0x4e),
            (0x26, "jedi_regist", 0x10),
            (0x28, "jedi_unregist", 0x10),
            (0x2a, "client_error_log", 0x79c),
        ],
    },
];

fn be32(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |v| u32::from_be_bytes([v[0], v[1], v[2], v[3]]))
}

/// The answer to one request (path, unpacked body).
pub(crate) fn answer(_path: &str, body: &[u8]) -> Vec<u8> {
    let (version, command) = (be32(body, 0), be32(body, 4));
    let Some(protocol) = PROTOCOLS.iter().find(|p| p.version == version) else {
        log!("sfc: unknown protocol {version:#x} ({} bytes)", body.len());
        return Vec::new();
    };
    let Some(&(_, name, size)) = protocol.commands.iter().find(|c| c.0 == command) else {
        log!("sfc: unknown command {command:#x} ({} bytes)", body.len());
        return Vec::new();
    };
    log!("sfc: {name}_request ({} bytes): {}", body.len(), body.iter().map(|b| format!("{b:02x}")).collect::<String>());
    let mut out = vec![0u8; size];
    out[0..4].copy_from_slice(&version.to_be_bytes());
    out[4..8].copy_from_slice(&(command + 1).to_be_bytes());
    // then status (0: ok) and eco_flag, each answer's own fields
    match name {
        "init" => init_answer(&mut out),
        "recognize" => recognize(body, &mut out),
        "start" | "item" => card_body_back(body, &mut out, protocol.card_body),
        _ => {}
    }
    out
}

/// The requests' common part (`keychip_serial`, `board_serial`, `place_id`, `rom_version`,
/// `mdata_revision_*`, `client_conf`), after the protocol and the command: a request's own
/// fields start here.
const REQUEST_FIELDS: usize = 66;

/// `card_key` (56 bytes): `fmt_ver`, `card_id`, `access_code[20]`, `launch_code` (u64),
/// `play_count`, `commit_count`, `has_data`, `use_count`, `develop`.
const CARD_KEY: usize = 56;

/// The card's id on the server: its access code's CRC-32 (31 bits, never 0).
fn card_id(key: &[u8]) -> u32 {
    let crc = key[8..28].iter().fold(!0u32, |mut crc, &b| {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
        crc
    });
    (!crc & 0x7FFF_FFFF).max(1)
}

/// `recognize_response`: the request's `card_key` back, with its card id, then `passcode`
/// (`code[8]`, `limit_timestamp`, `fail_timestamp`, `is_enable`): none.
fn recognize(body: &[u8], out: &mut [u8]) {
    let Some(key) = body.get(REQUEST_FIELDS..REQUEST_FIELDS + CARD_KEY) else { return };
    out[16..16 + CARD_KEY].copy_from_slice(key);
    out[20..24].copy_from_slice(&card_id(key).to_be_bytes());
}

/// `start_response` / `item_response`: the request's `card_body` (the card's data) back, the
/// player's data on the server; the rest (`user_game_info`, the counters, `event_result`) zero.
fn card_body_back(body: &[u8], out: &mut [u8], card_body: usize) {
    let at = REQUEST_FIELDS + CARD_KEY;
    match body.get(at..at + card_body) {
        Some(data) if out.len() >= 16 + card_body => out[16..16 + card_body].copy_from_slice(data),
        _ => log!("sfc: request without its card body ({} bytes)", body.len()),
    }
}

/// A fixed size string field.
fn put_str(out: &mut [u8], at: usize, len: usize, s: &str) {
    let b = s.as_bytes();
    let n = b.len().min(len - 1);
    out[at..at + n].copy_from_slice(&b[..n]);
}

/// `init_response`'s `sys_conf`: `mdata_order_uri[128]`, `vs_ip[32]` (the matching server, the
/// router), `vs_game_class`, `coin_conf` (`permit_flag`, `open_date`, `end_date`).
fn init_answer(out: &mut [u8]) {
    let r = wal_payload_common::dns::router();
    put_str(out, 144, 32, &format!("{}.{}.{}.{}", r[0], r[1], r[2], r[3]));
}
