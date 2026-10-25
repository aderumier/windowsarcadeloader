//! Dance Dance Revolution (2013, `MDX`): the game is `mdxja_945.dll` (the P3IO cabinets'
//! build; `mdxja_hm65.dll` the P4IO ones'), run by `wal-avs-boot.exe`, which loads this payload
//! (`libacio.dll`) after it (945 does not import it). The cabinet's boards, as spice2x (the
//! open-source reference) emulates them:
//!
//! - **P3IO** (USB, found with SetupAPI: `\\.\P3IO`, opened as `\\.\P3IO\p3io`): its inputs
//!   with `DeviceIoControl` (4 bytes, inverted: test, service, coin, each player's start, pad
//!   arrows and menu buttons), its commands as `AA <len> <data>` packets (`FF` escapes the next
//!   byte, inverted): version (`JDX`), watchdog, lights, security plugs, cabinet type (HD),
//!   video frequency, coin stock, and a serial port carrying an ACIO bus: the two card readers
//!   (ICCA, nodes 1 and 2) and the HD panel's light board (HDXS, node 3).
//! - **P4IO** (`\\.\P4IO\p4io`, hm65): `AA <cmd> <seq> <len>` commands answered in 64 bytes,
//!   its inputs with `DeviceIoControl` (16 bytes), the same ACIO bus; the pads' arrows are its
//!   MDXF sensor boards' (libacio's `ac_io_mdxf_*`). Its security plugs (spice2x's) are later
//!   DDRs': hm65 fails its check.
//! - **COM1** the dance pads' light board (FOOT: each 4-byte write answered `11`), **COM2** the
//!   satellite speakers' lights (SATE, an ACIO bus of 7 `DDRS` nodes), **COM3** the USB memory
//!   card port (USBMEM: text commands, "not connected").
//! - **Security.** The game plug's data is the game code in 6-bit ASCII (`c - 0x20`, four per
//!   24-bit little-endian word) after a signature of it and the plug's id; spice2x's plug is
//!   DDR X's (`GCJDXJAA`): the payload signs `GCMDXJAA` with the game's own function. The
//!   dongles are kbt.dll's `bt_get_ikey_status` (`kbt.rs`): both present, else 5-1505.
//!
//! SetupAPI answers the P3IO's two classes (and P4IO's) with one device, its description
//! "Generic Monitor" (the HD cabinet; `WAL_KONAMI_DDR_SD=1`: "Generic Television", the SD
//! one). The game's codecs (`k-clvsd.dll`, `xactengine2_10.dll` beside it) are registered
//! (their `DllRegisterServer`), as the cabinet's setup does. The card readers' PIN: the numpad,
//! or the configured card's typed 10 s after the game reads the card (its PIN screen).
//!
//! Controls per player: start; the pad's arrows (the stick's up, down, left, right); the
//! menu buttons left, right (`b1`, `b2`), up, down (`b3`, `b4`).

use std::collections::VecDeque;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log, serial};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

const P3IO_PORT: &str = "P3IO\\p3io";

/// The game DLL, when this process runs it.
pub fn game_module() -> usize {
    ["mdxja_hm65.dll", "mdxja_945.dll"]
        .iter()
        .map(|n| {
            let w: Vec<u16> = format!("{n}\0").encode_utf16().collect();
            unsafe { GetModuleHandleW(w.as_ptr()) as usize }
        })
        .find(|&m| m != 0)
        .unwrap_or(0)
}

// ── Inputs ────────────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    Start,
    /// The dance pad's arrow: up, down, left, right.
    Panel(u8),
    /// The cabinet's menu button: up, down, left, right.
    Menu(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("start", Native::Start),
    ("panel_up", Native::Panel(0)),
    ("panel_down", Native::Panel(1)),
    ("panel_left", Native::Panel(2)),
    ("panel_right", Native::Panel(3)),
    ("menu_up", Native::Menu(0)),
    ("menu_down", Native::Menu(1)),
    ("menu_left", Native::Menu(2)),
    ("menu_right", Native::Menu(3)),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("up", "panel_up"),
    ("down", "panel_down"),
    ("left", "panel_left"),
    ("right", "panel_right"),
    ("b1", "menu_left"),
    ("b2", "menu_right"),
    ("b3", "menu_up"),
    ("b4", "menu_down"),
];

static MAPS: Mutex<Option<[ButtonMap<Native>; 2]>> = Mutex::new(None);

/// The cabinet's switches, and each player's.
#[derive(Default)]
struct Inputs {
    test: bool,
    service: bool,
    coin: bool,
    start: [bool; 2],
    /// up, down, left, right
    panel: [[bool; 4]; 2],
    menu: [[bool; 4]; 2],
}

fn inputs() -> Inputs {
    let mut guard = MAPS.lock().unwrap();
    let maps = guard.get_or_insert_with(|| [ButtonMap::new(NATIVES, DEFAULT_MAP), ButtonMap::with_defaults(NATIVES, DEFAULT_MAP)]);
    let mut i = Inputs::default();
    for (player, map) in maps.iter().enumerate() {
        let stick = wal_payload_common::input(player);
        map.for_each_pressed(&stick, |n| match n {
            Native::Test => i.test = true,
            Native::Service => i.service = true,
            Native::Coin => i.coin = true,
            Native::Start => i.start[player] = true,
            Native::Panel(d) => i.panel[player][d as usize] = true,
            Native::Menu(d) => i.menu[player][d as usize] = true,
        });
    }
    i
}

/// The P3IO's input word (1: pressed; inverted on the wire): test 28, service 30, coin 29;
/// player 1 start 8, pad up/down/left/right 9-12, menu left/right 14, 15, up/down 24, 25;
/// player 2 start, pad and menu left/right 8 bits up, menu up/down 26, 27.
fn p3io_word() -> u32 {
    let i = inputs();
    let mut w = (i.test as u32) << 28 | (i.service as u32) << 30 | (i.coin as u32) << 29;
    for p in 0..2u32 {
        let (o, x) = (p * 8, p as usize);
        w |= (i.start[x] as u32) << (8 + o);
        for d in 0..4u32 {
            w |= (i.panel[x][d as usize] as u32) << (9 + o + d);
        }
        w |= (i.menu[x][2] as u32) << (14 + o) | (i.menu[x][3] as u32) << (15 + o);
        w |= (i.menu[x][0] as u32) << (24 + p * 2) | (i.menu[x][1] as u32) << (25 + p * 2);
    }
    w
}

/// The P4IO's input word (1: pressed): coin 24, service 25, test 28; player 1 start 0, menu
/// up/down/left/right 1-4; player 2 the same 8 bits up. The pad's arrows are the MDXF boards'
/// (libacio).
fn p4io_word() -> u32 {
    let i = inputs();
    let mut w = (i.coin as u32) << 24 | (i.service as u32) << 25 | (i.test as u32) << 28;
    for p in 0..2u32 {
        let (o, x) = (p * 8, p as usize);
        w |= (i.start[x] as u32) << o;
        for d in 0..4u32 {
            w |= (i.menu[x][d as usize] as u32) << (1 + o + d);
        }
    }
    w
}

// ── ACIO bus (behind the P3IO's serial port, and COM2) ─────────────────────────────────────────

/// A node group on an ACIO bus.
#[derive(Clone, Copy, PartialEq)]
enum Node {
    /// The card readers, player 1's then player 2's.
    Icca,
    /// The HD panel's light board.
    Hdxs,
    /// The satellite speakers' light boards.
    Sate,
}

struct Acio {
    /// `(group, its node count)`, in address order from 1.
    nodes: &'static [(Node, u8)],
    input: Vec<u8>,
    invert: bool,
    handshake: u32,
    output: VecDeque<u8>,
    reader: [Reader; 2],
}

#[derive(Default)]
struct Reader {
    /// The new readers' commands seen (0x160 key exchange, 0x161 read).
    new_type: bool,
    accept: bool,
    hold: bool,
    keypad_last: Option<usize>,
    capture: u8,
    keydown: u8,
    /// When the configured PIN was last typed.
    pin_armed: Option<std::time::Instant>,
}

impl Acio {
    const fn new(nodes: &'static [(Node, u8)]) -> Acio {
        Acio {
            nodes,
            input: Vec::new(),
            invert: false,
            handshake: 0,
            output: VecDeque::new(),
            reader: [
                Reader { new_type: false, accept: true, hold: false, keypad_last: None, capture: 8, keydown: 0, pin_armed: None },
                Reader { new_type: false, accept: true, hold: false, keypad_last: None, capture: 8, keydown: 0, pin_armed: None },
            ],
        }
    }

    fn write(&mut self, byte: u8) {
        let mut byte = byte;
        if self.invert {
            byte = !byte;
            self.invert = false;
            self.input.push(byte);
        } else if byte == 0xff {
            self.invert = true;
        } else {
            self.input.push(byte);
        }
        // garbage before a start of frame, repeated starts
        while self.input.first().is_some_and(|&b| b != 0xaa) {
            self.input.remove(0);
        }
        while self.input.len() > 1 && self.input[1] == 0xaa {
            self.input.remove(0);
        }
        // the bus handshake (a run of AA): answered AA AA
        if byte == 0xaa {
            self.handshake += 1;
        } else {
            self.handshake = 0;
        }
        if self.handshake > 1 {
            self.output.extend([0xaa, 0xaa]);
            self.handshake -= 1;
            return;
        }
        if self.input.len() < 6 {
            return;
        }
        let complete = if self.input[1] == 0x70 {
            self.input.len() >= 4 + self.input[2] as usize
        } else {
            self.input.len() >= 2 + 5 + self.input[5] as usize
        };
        if complete {
            let msg = std::mem::take(&mut self.input);
            self.parse(&msg);
        }
    }

    /// A response: `AA AA`, the message (`addr | 0x80`, code, pid, size, data) and its sum,
    /// `AA` / `FF` escaped.
    fn send(&mut self, addr: u8, code: u16, pid: u8, data: &[u8]) {
        let mut msg = vec![if addr != 0 { addr | 0x80 } else { 0 }, (code >> 8) as u8, code as u8, pid, data.len() as u8];
        msg.extend_from_slice(data);
        let sum = msg.iter().fold(0u8, |s, &b| s.wrapping_add(b));
        msg.push(sum);
        self.output.extend([0xaa, 0xaa]);
        for b in msg {
            if b == 0xaa || b == 0xff {
                self.output.extend([0xff, !b]);
            } else {
                self.output.push_back(b);
            }
        }
    }

    fn version(code: &str, kind: u32, minor: u8) -> Vec<u8> {
        let mut v = kind.to_le_bytes().to_vec();
        v.extend([0, 1, minor, 0]);
        let mut c = [0u8; 4];
        c[..code.len()].copy_from_slice(code.as_bytes());
        v.extend(c);
        v.extend([0u8; 32]);
        v
    }

    fn parse(&mut self, msg: &[u8]) {
        let body = &msg[1..msg.len() - 1];
        let sum = body.iter().fold(0u8, |s, &b| s.wrapping_add(b));
        if sum != msg[msg.len() - 1] {
            return;
        }
        let addr = body[0];
        if addr == 0x70 {
            return;
        }
        let code = u16::from_be_bytes([body[1], body[2]]);
        let pid = body[3];
        let data = body.get(5..).unwrap_or_default().to_vec();
        // the node's group and its unit in it
        let mut offset = 0u8;
        let mut target = None;
        for &(group, count) in self.nodes {
            if addr > offset && addr <= offset + count {
                target = Some((group, (addr - offset - 1) as usize));
                break;
            }
            offset += count;
        }
        let total: u8 = self.nodes.iter().map(|n| n.1).sum();
        match (target, code) {
            (_, 0x0001) if addr == 0 => self.send(0, code, pid, &[total]),
            (Some((Node::Icca, unit)), _) => self.icca(addr, code, pid, unit.min(1), &data),
            (Some((Node::Hdxs, _)), 0x0002) => self.send(addr, code, pid, &Acio::version("HDXS", 0x204, 6)),
            (Some((Node::Hdxs, _)), 0x0080) => self.send(addr, code, pid, &[]),
            (Some((Node::Sate, _)), 0x0002) => self.send(addr, code, pid, &Acio::version("DDRS", 0x105, 1)),
            _ => self.send(addr, code, pid, &[0]),
        }
    }

    /// A card reader's 16-byte status (spice2x's layout): state, sensors / card type, the card's
    /// id, flags, the keypad's last press and its keys.
    fn reader_status(&mut self, unit: usize) -> [u8; 16] {
        let card = crate::icca::reader_card(unit);
        let r = &mut self.reader[unit];
        // the keypad: a press is an edge (capture counter << 4 | key number)
        let key = crate::icca::keypad_digit(unit);
        const CODES: [u16; 10] = [0x100, 0x200, 0x2000, 2, 0x400, 0x4000, 4, 0x800, 0x8000, 8];
        const NUMS: [u8; 10] = [0, 1, 5, 9, 2, 6, 10, 3, 7, 11];
        r.keydown = 0;
        if key.is_some() && key != r.keypad_last {
            let k = key.unwrap();
            r.keydown = (r.capture << 4) | NUMS[k];
            r.capture = r.capture.wrapping_add(1) | 0x08;
        }
        r.keypad_last = key;
        let keys = key.map(|k| CODES[k]).unwrap_or(0);
        let mut s = [0u8; 16];
        if let Some(id) = card {
            s[2..10].copy_from_slice(&id);
        }
        if r.new_type {
            s[0] = if card.is_some() { 2 } else { 4 };
        } else {
            if card.is_some() && r.accept {
                r.hold = true;
            }
            if card.is_none() {
                r.hold = false;
            }
            (s[0], s[1]) = match (r.hold, card.is_some()) {
                (true, _) => (2, 0x30),
                (false, true) => (2, 0x10),
                (false, false) => (1, 0),
            };
        }
        s[11] = 3;
        s[12] = r.keydown;
        s[14] = (keys >> 8) as u8;
        s[15] = keys as u8;
        s
    }

    fn icca(&mut self, addr: u8, code: u16, pid: u8, unit: usize, data: &[u8]) {
        match code {
            0x0002 => self.send(addr, code, pid, &Acio::version("ICCA", 3, 6)),
            0x0131 => {
                // the card read: its PIN screen follows (the inquire 4 s on, the PIN screen 3 s
                // later, 15 s to type it): the configured PIN typed on the keypad 10 s on (once
                // per insertion)
                if crate::icca::reader_card(unit).is_some() {
                    let now = std::time::Instant::now();
                    if self.reader[unit].pin_armed.is_none_or(|t| now.duration_since(t).as_secs() >= 30) {
                        self.reader[unit].pin_armed = Some(now);
                        crate::icca::type_pin(unit, std::time::Duration::from_secs(10));
                        log!("ddr: player {}'s card read, its PIN typed", unit + 1);
                    }
                }
                let mut s = self.reader_status(unit);
                s[0] = 1;
                self.send(addr, code, pid, &s);
            }
            0x0134 | 0x0135 => {
                if code == 0x0135 && data.len() >= 2 {
                    match data[1] {
                        0x00 => self.reader[unit].accept = false,
                        0x11 => self.reader[unit].accept = true,
                        0x12 => self.reader[unit].hold = false,
                        _ => {}
                    }
                }
                let s = self.reader_status(unit);
                self.send(addr, code, pid, &s);
            }
            0x0160 => {
                self.reader[unit].new_type = true;
                self.send(addr, code, pid, &[0xbe, 0xef, 0xca, 0xfe]);
            }
            0x0161 => {
                self.reader[unit].new_type = true;
                self.send(addr, code, pid, &[]);
            }
            _ => self.send(addr, code, pid, &[0]),
        }
    }
}

// ── P3IO ──────────────────────────────────────────────────────────────────────────────────────

static P3IO_BUS: Mutex<Acio> = Mutex::new(Acio::new(&[(Node::Icca, 2), (Node::Hdxs, 1)]));
static SATE_BUS: Mutex<Acio> = Mutex::new(Acio::new(&[(Node::Sate, 7)]));

/// A P3IO answer: `AA <len>`, its data `AA` / `FF` escaped.
fn p3io_frame(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0xaa, data.len() as u8];
    for &b in data {
        if b == 0xaa || b == 0xff {
            out.extend([0xff, !b]);
        } else {
            out.push(b);
        }
    }
    out
}

/// The security plug's answer (spice2x's data): `1` present, then the black (game) or white
/// (e-Amusement) plug's 40 bytes.
const BLACK_PLUG: [u8; 40] = [
    0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xbe, 0xb5, 0xb2, 0xac, 0x16, 0x8c, 0xe7, 0xa8, 0x92, 0xb8, 0x1a, 0x86, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xf7,
];
const WHITE_PLUG: [u8; 40] = [
    0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xc3, 0xd4, 0x45, 0xe8, 0x7c, 0x17, 0x20, 0x08, 0x82, 0x20, 0x08, 0x82, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x1b,
];

/// The game plug's id (spice2x's).
const PLUG_ID: [u8; 8] = [0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7];

/// The game plug's 32 data bytes for `code` (the game's code, `GCMDXJAA`), signed by the game's
/// own plug check (`mdxja_945.dll`: its libp3io signs the plug's code with a key of its id and a
/// table, the function at rva 0x138310 called at 0x137883, and compares with the plug's data).
/// `None`: another build (the call site differs).
fn signed_plug(code: &[u8; 8]) -> Option<[u8; 32]> {
    let base = game_module();
    if base == 0 || unsafe { std::slice::from_raw_parts((base + 0x137883) as *const u8, 5) } != [0xe8, 0x88, 0x0a, 0x00, 0x00] {
        return None;
    }
    let table = unsafe { std::slice::from_raw_parts((base + 0x1abe08) as *const u8, 8) };
    let mut key = [0u8; 8];
    for i in 0..8 {
        key[i] = table[i] ^ code[i] ^ (0x7f - i as u8);
    }
    let mut id = PLUG_ID;
    id.reverse();
    let mut out = [0u8; 32];
    let f = base + 0x138310;
    // the key in ecx, (out, reversed id, code) on the stack, the caller pops them
    unsafe {
        std::arch::asm!(
            "push {code}",
            "push {id}",
            "push {out}",
            "call {f}",
            "add esp, 12",
            code = in(reg) code.as_ptr(),
            id = in(reg) id.as_ptr(),
            out = in(reg) out.as_mut_ptr(),
            f = in(reg) f,
            in("ecx") key.as_ptr(),
            clobber_abi("C"),
        );
    }
    Some(out)
}

/// The game plug's answer: its id and data. The spice2x data (DDR X's `GCJDXJAA`) checked against
/// the game's signature first, then the plug of this game's code.
fn black_plug() -> [u8; 40] {
    static PLUG: std::sync::OnceLock<[u8; 40]> = std::sync::OnceLock::new();
    *PLUG.get_or_init(|| {
        let mut plug = BLACK_PLUG;
        if let Some(check) = signed_plug(b"GCJDXJAA") {
            log!("ddr: plug signature {}", if check[..] == BLACK_PLUG[8..] { "matches" } else { "differs from spice2x's" });
        }
        match signed_plug(b"GCMDXJAA") {
            Some(data) => {
                plug[8..].copy_from_slice(&data);
                log!("ddr: game plug GCMDXJAA");
            }
            None => log!("ddr: plug code differs, DDR X's plug kept"),
        }
        plug
    })
}

fn sd_mode() -> bool {
    static SD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *SD.get_or_init(|| std::env::var("WAL_KONAMI_DDR_SD").is_ok_and(|v| v.trim() == "1"))
}

fn p3io(packet: &[u8]) -> Vec<u8> {
    if packet.len() < 2 || packet[0] != 0xaa {
        return Vec::new();
    }
    let mut parsed = Vec::new();
    let mut i = 2;
    while i < packet.len() {
        let mut b = packet[i];
        if b == 0xff && i + 1 < packet.len() {
            i += 1;
            b = !packet[i];
        }
        parsed.push(b);
        i += 1;
    }
    if parsed.len() < 2 {
        return Vec::new();
    }
    let data: Vec<u8> = match parsed[1] {
        // version: the board's code
        0x01 => vec![0, 0, b'J', b'D', b'X', 0, 0, 0],
        0x25 => {
            let mut d = vec![0, 0, 1];
            d.extend(if parsed.get(2).is_some_and(|f| f & 0x10 != 0) { black_plug() } else { WHITE_PLUG });
            d
        }
        // the cabinet type: 1 HD, 0 SD
        0x27 => vec![0, (!sd_mode()) as u8],
        0x29 => vec![0, 0, 0],
        0x31 => vec![0; 7],
        // the serial port's open (0) / close (FF)
        0x38 => vec![0, 0, if matches!(parsed.get(3), Some(0x00 | 0xff)) { 0 } else { 0xff }],
        // the serial port's write: the ACIO bus
        0x3a => {
            let len = parsed.get(3).copied().unwrap_or(0) as usize;
            let mut bus = P3IO_BUS.lock().unwrap();
            for &b in parsed.iter().skip(4).take(len) {
                bus.write(b);
            }
            vec![0, 0, len as u8]
        }
        // the serial port's read: the bus's answers
        0x3b => {
            let len = parsed.get(3).copied().unwrap_or(0) as usize;
            let mut bus = P3IO_BUS.lock().unwrap();
            let n = len.min(bus.output.len()).min(0xff);
            let mut d = vec![0, 0, n as u8];
            d.extend(bus.output.drain(..n));
            d
        }
        // watchdog, lights, mode and the others: done
        _ => vec![0, 0],
    };
    p3io_frame(&data)
}

// ── P4IO ──────────────────────────────────────────────────────────────────────────────────────

const P4IO_PORT: &str = "P4IO\\p4io";

static P4IO_BUS: Mutex<Acio> = Mutex::new(Acio::new(&[(Node::Icca, 2), (Node::Hdxs, 1)]));

/// The P4IO's security plugs (spice2x's data): the black (game) and white (e-Amusement) ones.
const P4IO_BLACK_PLUG: [u8; 32] = [
    0xc9, 0xfe, 0xc1, 0x5b, 0x8d, 0x8a, 0xe7, 0xa8, 0x92, 0xb8, 0x1a, 0x86, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x77,
];
const P4IO_WHITE_PLUG: [u8; 32] = [
    0x18, 0x98, 0x12, 0x71, 0xb3, 0xa2, 0x20, 0x08, 0x82, 0x20, 0x08, 0x82, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xda,
];

/// A P4IO command, `AA <cmd> <seq> <len> <data>`: its 64-byte answer, the same header and its
/// data.
fn p4io(packet: &[u8]) -> Vec<u8> {
    if packet.len() < 4 || packet[0] != 0xaa {
        return Vec::new();
    }
    let (cmd, seq, len) = (packet[1], packet[2], packet[3] as usize);
    let data = packet.get(4..4 + len).unwrap_or_default();
    let body: Vec<u8> = match cmd {
        // type, version, product, date, time
        0x01 => {
            let mut b = vec![0u8; 0x2c];
            b[..3].copy_from_slice(b"JDX");
            b[5..8].copy_from_slice(&[1, 0, 0]);
            b[8..12].copy_from_slice(b"prod");
            b[0x0c..0x16].copy_from_slice(b"2013-03-14");
            b[0x1c..0x24].copy_from_slice(b"10:00:00");
            b
        }
        0x18 => vec![0; 4],
        // the serial port (port 0: the ACIO bus): its data written, its answers read
        0x21 => {
            let mut b = vec![0, 0];
            if data.first() == Some(&0) {
                let mut bus = P4IO_BUS.lock().unwrap();
                for &x in &data[1..] {
                    bus.write(x);
                }
                let n = bus.output.len().min(58);
                b.extend(bus.output.drain(..n));
            }
            b
        }
        0x40 => {
            let mut b = vec![0xaa; 9];
            b[0] = 0;
            b
        }
        0x41 => {
            let mut b = vec![0u8];
            b.extend(if data.first() == Some(&0) { P4IO_BLACK_PLUG } else { P4IO_WHITE_PLUG });
            b
        }
        // nop, lights, the serial port's open and close
        _ => Vec::new(),
    };
    let mut out = vec![0u8; 64];
    out[..4].copy_from_slice(&[0xaa, cmd, seq, body.len() as u8]);
    out[4..4 + body.len()].copy_from_slice(&body);
    out
}

// ── MDXF: the P4IO cabinets' pad sensor boards (libacio) ───────────────────────────────────────

/// Each player's pad: a ring of 16 32-byte states (the arrows' sensors at bytes 4-5, the game's
/// time at 0x18) and its head.
struct Pad {
    ring: [[u8; 32]; 16],
    head: u8,
}

static PADS: Mutex<[Pad; 2]> = Mutex::new([Pad { ring: [[0; 32]; 16], head: 0 }, Pad { ring: [[0; 32]; 16], head: 0 }]);

/// The pad of an MDXF node (17 / 25 player 1, 18 / 26 player 2).
fn pad_of(node: i32) -> Option<usize> {
    match node {
        17 | 25 => Some(0),
        18 | 26 => Some(1),
        _ => None,
    }
}

/// The game's clock (the game DLL's `arkGetTickTime64`), else milliseconds since the first look.
fn game_time() -> u64 {
    static TICK: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let f = *TICK.get_or_init(|| {
        let m = game_module();
        (m != 0)
            .then(|| unsafe { windows_sys::Win32::System::LibraryLoader::GetProcAddress(m as _, c"arkGetTickTime64".as_ptr().cast()) })
            .flatten()
            .map(|f| f as usize)
            .unwrap_or(0)
    });
    if f != 0 {
        let f: unsafe extern "C" fn() -> u64 = unsafe { std::mem::transmute(f) };
        unsafe { f() }
    } else {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64
    }
}

/// A new state of the node's pad: its arrows' sensors (up `F0`, down `0F` at byte 4; left `F0`,
/// right `0F` at byte 5) and the time.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_mdxf_update_control_status_buffer(node: i32) -> bool {
    let Some(p) = pad_of(node) else { return false };
    let panel = inputs().panel[p];
    let mut pads = PADS.lock().unwrap();
    let pad = &mut pads[p];
    pad.head = (pad.head + 1) % 16;
    let entry = &mut pad.ring[pad.head as usize];
    *entry = [0; 32];
    entry[4] = if panel[0] { 0xf0 } else { 0 } | if panel[1] { 0x0f } else { 0 };
    entry[5] = if panel[2] { 0xf0 } else { 0 } | if panel[3] { 0x0f } else { 0 };
    entry[0x18..0x20].copy_from_slice(&game_time().to_le_bytes());
    true
}

/// The node's pad state `index` entries before `head` (`FF`: the latest); the head used.
#[unsafe(no_mangle)]
pub extern "C" fn ac_io_mdxf_get_control_status_buffer(node: i32, out: *mut u8, index: u8, head: u8) -> u64 {
    let Some(p) = pad_of(node) else {
        if !out.is_null() {
            unsafe { std::ptr::write_bytes(out, 0, 32) };
        }
        return (node.wrapping_sub(0x11) as u32 as u64) & !0xff;
    };
    let pads = PADS.lock().unwrap();
    let pad = &pads[p];
    let start = if head == 0xff { pad.head } else { head };
    let i = (start as usize + 16 - (index as usize & 15)) & 15;
    if !out.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(pad.ring[i].as_ptr(), out, 32) };
    }
    start as u64
}

/// `DeviceIoControl`: the P3IO's inputs (4 bytes, inverted, whatever the code); the P4IO's
/// (`FILE_DEVICE_UNKNOWN` function 0x801: 16 bytes) and its version (0x803).
unsafe extern "system" fn device_io_control(h: *mut c_void, code: u32, inb: *const c_void, inn: u32, out: *mut c_void, outn: u32, ret: *mut u32, ov: *mut c_void) -> i32 {
    let port = serial::port_of(h);
    if port.as_deref() == Some(P3IO_PORT) {
        if out.is_null() || outn < 4 {
            return 0;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
        unsafe { (out as *mut u32).write_unaligned(!p3io_word()) };
        if !ret.is_null() {
            unsafe { *ret = 4 };
        }
        return 1;
    }
    if port.as_deref() == Some(P4IO_PORT) {
        let n: u32 = match (code >> 16, (code >> 2) & 0x3fff) {
            (0x22, 0x801) if !out.is_null() && outn >= 16 => {
                // polled at kHz: 250 Hz is plenty
                std::thread::sleep(std::time::Duration::from_millis(4));
                unsafe {
                    std::ptr::write_bytes(out as *mut u8, 0, 16);
                    (out as *mut u32).write_unaligned(p4io_word());
                }
                16
            }
            (0x22, 0x803) if !out.is_null() && outn > 12 => {
                unsafe { std::ptr::copy_nonoverlapping(b"WAL p4io\0".as_ptr(), out as *mut u8, 9) };
                9
            }
            _ => 0,
        };
        if !ret.is_null() {
            unsafe { *ret = n };
        }
        return (n != 0) as i32;
    }
    let original: unsafe extern "system" fn(*mut c_void, u32, *const c_void, u32, *mut c_void, u32, *mut u32, *mut c_void) -> i32 =
        unsafe { std::mem::transmute(ORIG_IOCTL.load(Ordering::Relaxed)) };
    unsafe { original(h, code, inb, inn, out, outn, ret, ov) }
}

static ORIG_IOCTL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

// ── COM1 FOOT, COM2 SATE, COM3 USBMEM ─────────────────────────────────────────────────────────

fn foot(_packet: &[u8]) -> Vec<u8> {
    vec![0x11]
}

fn sate(packet: &[u8]) -> Vec<u8> {
    let mut bus = SATE_BUS.lock().unwrap();
    for &b in packet {
        bus.write(b);
    }
    bus.output.drain(..).collect()
}

fn usbmem(packet: &[u8]) -> Vec<u8> {
    let reply: &[u8] = if packet.starts_with(b"sver") {
        b"done GQHDXJAA WAL"
    } else if [b"on_a", b"on_b", b"offa", b"offb", b"lma ", b"lmb "].iter().any(|c| packet.starts_with(*c)) {
        b"done"
    } else {
        b"not connected"
    };
    let mut out = reply.to_vec();
    out.extend(b"\0\r>");
    out
}

// ── SetupAPI ──────────────────────────────────────────────────────────────────────────────────

/// The classes answered (P3IO's two, P4IO's), as GUID bytes.
fn classes() -> [[u8; 16]; 3] {
    let guid = |a: [u32; 4]| {
        let mut g = [0u8; 16];
        for (i, v) in a.iter().enumerate() {
            g[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        g
    };
    [
        guid([0x1fa4a480, 0x40c7ac60, 0x7952aca7, 0x5a57340f]),
        guid([0x4d36e96e, 0x11cee325, 0x0008c1bf, 0x1803e12b]),
        guid([0x8b7250a5, 0x46c94f61, 0x68e63a84, 0x206a4706]),
    ]
}

/// Fake device info sets: one per class (`DEVINFO + index`).
const DEVINFO: usize = 0x5e70_0000;

fn set_index(set: usize) -> Option<usize> {
    let i = set.checked_sub(DEVINFO)?;
    (i < 3).then_some(i)
}

fn interface_path(set: usize) -> &'static str {
    if set_index(set) == Some(2) { "\\\\.\\P4IO" } else { "\\\\.\\P3IO" }
}

static ORIG_SETUPAPI: [std::sync::atomic::AtomicUsize; 11] = [const { std::sync::atomic::AtomicUsize::new(0) }; 11];

fn orig<F: Copy>(i: usize) -> F {
    let p = ORIG_SETUPAPI[i].load(Ordering::Relaxed);
    unsafe { std::mem::transmute_copy(&p) }
}

unsafe extern "system" fn get_class_devs_w(guid: *const u8, enumerator: *const u16, hwnd: usize, flags: u32) -> usize {
    if !guid.is_null() {
        let g = unsafe { std::slice::from_raw_parts(guid, 16) };
        if let Some(i) = classes().iter().position(|c| c == g) {
            log!("ddr: SetupDiGetClassDevsW: the P3IO");
            unsafe { windows_sys::Win32::Foundation::SetLastError(0) };
            return DEVINFO + i;
        }
    }
    unsafe { orig::<unsafe extern "system" fn(*const u8, *const u16, usize, u32) -> usize>(0)(guid, enumerator, hwnd, flags) }
}

unsafe extern "system" fn enum_device_info(set: usize, index: u32, data: *mut u8) -> i32 {
    if set_index(set).is_some() {
        if index != 0 {
            unsafe { windows_sys::Win32::Foundation::SetLastError(259) }; // ERROR_NO_MORE_ITEMS
            return 0;
        }
        return 1;
    }
    unsafe { orig::<unsafe extern "system" fn(usize, u32, *mut u8) -> i32>(1)(set, index, data) }
}

unsafe extern "system" fn enum_interfaces(set: usize, dev: usize, guid: *const u8, index: u32, data: *mut u8) -> i32 {
    if set_index(set).is_some() {
        if index != 0 {
            unsafe { windows_sys::Win32::Foundation::SetLastError(259) };
            return 0;
        }
        return 1;
    }
    unsafe { orig::<unsafe extern "system" fn(usize, usize, *const u8, u32, *mut u8) -> i32>(2)(set, dev, guid, index, data) }
}

/// `SetupDiGetDeviceInterfaceDetailW`: `cbSize`, then the device path (UTF-16).
unsafe extern "system" fn interface_detail_w(set: usize, data: *const u8, detail: *mut u8, size: u32, required: *mut u32, devinfo: *mut u8) -> i32 {
    if set_index(set).is_some() {
        let path: Vec<u16> = interface_path(set).encode_utf16().chain([0]).collect();
        let needed = 4 + path.len() * 2;
        unsafe {
            if !required.is_null() {
                *required = needed as u32;
            }
            if detail.is_null() || (size as usize) < needed {
                windows_sys::Win32::Foundation::SetLastError(122); // ERROR_INSUFFICIENT_BUFFER
                return 0;
            }
            std::ptr::copy_nonoverlapping(path.as_ptr() as *const u8, detail.add(4), path.len() * 2);
        }
        return 1;
    }
    unsafe { orig::<unsafe extern "system" fn(usize, *const u8, *mut u8, u32, *mut u32, *mut u8) -> i32>(3)(set, data, detail, size, required, devinfo) }
}

/// `SetupDiGetDeviceRegistryPropertyA`: the device description (the cabinet's monitor: HD or
/// SD), an empty driver.
unsafe extern "system" fn registry_property_a(set: usize, dev: *mut u8, property: u32, kind: *mut u32, buf: *mut u8, size: u32, required: *mut u32) -> i32 {
    if set_index(set).is_some() {
        log!("ddr: SetupDiGetDeviceRegistryPropertyA({property})");
        let value: &[u8] = match property {
            0 => if sd_mode() { b"Generic Television\0" } else { b"Generic Monitor\0" }, // SPDRP_DEVICEDESC
            1 => b"USB\\VID_0000&PID_7305\0", // SPDRP_HARDWAREID
            _ => b"\0",
        };
        unsafe {
            if !required.is_null() {
                *required = value.len() as u32;
            }
            if buf.is_null() || (size as usize) < value.len() {
                windows_sys::Win32::Foundation::SetLastError(122);
                return 0;
            }
            if !kind.is_null() {
                *kind = 1; // REG_SZ
            }
            std::ptr::copy_nonoverlapping(value.as_ptr(), buf, value.len());
        }
        return 1;
    }
    unsafe { orig::<unsafe extern "system" fn(usize, *mut u8, u32, *mut u32, *mut u8, u32, *mut u32) -> i32>(4)(set, dev, property, kind, buf, size, required) }
}

unsafe extern "system" fn destroy_info_list(set: usize) -> i32 {
    if set_index(set).is_some() {
        return 1;
    }
    unsafe { orig::<unsafe extern "system" fn(usize) -> i32>(5)(set) }
}

unsafe extern "system" fn info_list_detail_w(set: usize, detail: *mut u8) -> i32 {
    if set_index(set).is_some() {
        return 1;
    }
    unsafe { orig::<unsafe extern "system" fn(usize, *mut u8) -> i32>(6)(set, detail) }
}

/// `CM_Get_Device_ID_ExW(devinst, buffer, len, flags, machine)`: the board's device id.
unsafe extern "system" fn cm_get_device_id(devinst: u32, buf: *mut u16, len: u32, flags: u32, machine: usize) -> u32 {
    log!("ddr: CM_Get_Device_ID_ExW({devinst:#x})");
    let id: Vec<u16> = "USB\\VID_0000&PID_7305\\0".encode_utf16().chain([0]).collect();
    if !buf.is_null() && (len as usize) >= id.len() {
        unsafe { std::ptr::copy_nonoverlapping(id.as_ptr(), buf, id.len()) };
        return 0; // CR_SUCCESS
    }
    let _ = (flags, machine);
    0x1a // CR_BUFFER_SMALL
}

/// The board's restart through the class installer (install parameters, `DIF_PROPERTYCHANGE`):
/// done.
unsafe extern "system" fn install_params(set: usize, dev: *mut u8, params: *mut u8) -> i32 {
    if set_index(set).is_some() {
        log!("ddr: SetupDi install parameters");
        return 1;
    }
    unsafe { orig::<unsafe extern "system" fn(usize, *mut u8, *mut u8) -> i32>(8)(set, dev, params) }
}

unsafe extern "system" fn set_class_install_params(set: usize, dev: *mut u8, params: *mut u8, size: u32) -> i32 {
    if set_index(set).is_some() {
        return 1;
    }
    unsafe { orig::<unsafe extern "system" fn(usize, *mut u8, *mut u8, u32) -> i32>(9)(set, dev, params, size) }
}

unsafe extern "system" fn call_class_installer(function: u32, set: usize, dev: *mut u8) -> i32 {
    if set_index(set).is_some() {
        log!("ddr: SetupDiCallClassInstaller({function:#x})");
        return 1;
    }
    unsafe { orig::<unsafe extern "system" fn(u32, usize, *mut u8) -> i32>(10)(function, set, dev) }
}

/// The debug keys (`GetAsyncKeyState`, `GetKeyState`): none pressed.
unsafe extern "system" fn no_key(_key: i32) -> i16 {
    0
}

// ── Codecs ────────────────────────────────────────────────────────────────────────────────────

/// Registers the game's codecs beside it (`DllRegisterServer`, regsvr32's work).
fn register_codecs(game: usize) {
    let mut buf = [0u16; 520];
    let n = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleFileNameW(game as _, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let Some(dir) = std::path::PathBuf::from(String::from_utf16_lossy(&buf[..n])).parent().map(|d| d.to_path_buf()) else { return };
    for name in ["k-clvsd.dll", "xactengine2_10.dll"] {
        let path = dir.join(name);
        if !path.exists() {
            continue;
        }
        let w: Vec<u16> = path.to_string_lossy().encode_utf16().chain([0]).collect();
        let module = unsafe { windows_sys::Win32::System::LibraryLoader::LoadLibraryW(w.as_ptr()) };
        let register = (!module.is_null()).then(|| unsafe { windows_sys::Win32::System::LibraryLoader::GetProcAddress(module, c"DllRegisterServer".as_ptr().cast()) }).flatten();
        match register {
            Some(f) => {
                let f: unsafe extern "system" fn() -> i32 = unsafe { std::mem::transmute(f) };
                let hr = unsafe { f() };
                log!("ddr: {name} registered ({hr:#x})");
            }
            None => log!("ddr: {name} not registered (no DllRegisterServer)"),
        }
    }
}

pub fn init(module: crate::HINSTANCE, game: usize) {
    // the dongles (kbt.dll's `bt_get_ikey_status`): both present, else SECURITY ERROR 5-1505
    crate::kbt::init(module);
    register_codecs(game);
    serial::install_in(P3IO_PORT, p3io, &[game]);
    serial::install_in(P4IO_PORT, p4io, &[game]);
    serial::install_in("COM1", foot, &[game]);
    serial::install_in("COM2", sate, &[game]);
    serial::install_in("COM3", usbmem, &[game]);
    let hooks: [(&str, usize); 11] = [
        ("SetupDiGetClassDevsW", get_class_devs_w as *const () as usize),
        ("SetupDiEnumDeviceInfo", enum_device_info as *const () as usize),
        ("SetupDiEnumDeviceInterfaces", enum_interfaces as *const () as usize),
        ("SetupDiGetDeviceInterfaceDetailW", interface_detail_w as *const () as usize),
        ("SetupDiGetDeviceRegistryPropertyA", registry_property_a as *const () as usize),
        ("SetupDiDestroyDeviceInfoList", destroy_info_list as *const () as usize),
        ("SetupDiGetDeviceInfoListDetailW", info_list_detail_w as *const () as usize),
        ("CM_Get_Device_ID_ExW", cm_get_device_id as *const () as usize),
        ("SetupDiGetDeviceInstallParamsW", install_params as *const () as usize),
        ("SetupDiSetClassInstallParamsW", set_class_install_params as *const () as usize),
        ("SetupDiCallClassInstaller", call_class_installer as *const () as usize),
    ];
    for (i, (name, f)) in hooks.into_iter().enumerate() {
        match unsafe { iat::hook_module(game, "setupapi.dll", name, f) } {
            Some(o) => ORIG_SETUPAPI[i].store(o, Ordering::Relaxed),
            None => log!("ddr: {name} not imported"),
        }
    }
    match unsafe { iat::hook_module(game, "kernel32.dll", "DeviceIoControl", device_io_control as *const () as usize) } {
        Some(o) => ORIG_IOCTL.store(o, Ordering::Relaxed),
        None => log!("ddr: DeviceIoControl not imported"),
    }
    for name in ["GetAsyncKeyState", "GetKeyState"] {
        unsafe { iat::hook_module(game, "user32.dll", name, no_key as *const () as usize) };
    }
}
