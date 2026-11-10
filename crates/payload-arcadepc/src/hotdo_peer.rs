//! The House of the Dead: Overkill's online co-op partner played locally (`WAL_HOTDO_PEER=1`):
//! player 2 as a fake network peer inside the one game instance.
//!
//! The PC build has no local second player, but its online co-op works over Steam P2P (a LAN
//! Steam emulator's lobby and P2P packets: gbe_fork's steam_api.dll in the dump). The host's
//! game simulates the remote player from its replicated input (player object 4's field 0x0d:
//! the crosshair's x, y and the buttons, applied to that player's controller, 0x4d13c0): the
//! peer here only has to speak the protocol. The exe's imports of steam_api.dll are hooked:
//!
//! * `SteamNetworking()`: the real object's method table patched (send / is-available / read /
//!   accept / close / session state): packets to the fake peer's Steam ID go to the peer, its
//!   packets come first out of the reads.
//! * `SteamMatchmaking()`: `SetLobbyData` gives the host's new lobby.
//! * `SteamAPI_RegisterCallback`: the game's callback objects, so the peer fires them itself
//!   (`LobbyChatUpdate_t` 505: player 2 joins; `P2PSessionRequest_t` 1202).
//! * `SteamAPI_RunCallbacks`: the peer's tick, on the game's thread.
//!
//! Packets: [type][sub][len u16][seq u16][source slot][destination slot] then the payload. Sub
//! 0x22: reliable, acknowledged by a 0x0E (the acknowledged packet's seq); 0x83 events are
//! acknowledged by a 0x82 (the event's id at +8).

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

/// The fake peer's Steam ID (the recorded client's, whose messages are replayed).
const PEER_ID: u64 = 76561198000000102;
/// P2PSessionState_t's fake remote address.
const PEER_IP: u32 = 0x7f00_0002;
const PEER_PORT: u16 = 47585;

static ORIG_REGISTER: AtomicUsize = AtomicUsize::new(0);
static ORIG_RUN_CALLBACKS: AtomicUsize = AtomicUsize::new(0);
static ORIG_UNREGISTER: AtomicUsize = AtomicUsize::new(0);
static ORIG_NETWORKING: AtomicUsize = AtomicUsize::new(0);
static ORIG_MATCHMAKING: AtomicUsize = AtomicUsize::new(0);
/// The real networking / matchmaking objects' original methods (patched slots).
static NET_METHODS: [AtomicUsize; 8] = [const { AtomicUsize::new(0) }; 8];
static MM_SET_LOBBY_DATA: AtomicUsize = AtomicUsize::new(0);
static NET_PATCHED: AtomicBool = AtomicBool::new(false);
static MM_PATCHED: AtomicBool = AtomicBool::new(false);
/// The host's lobby (0: none yet).
static LOBBY: AtomicU64 = AtomicU64::new(0);

/// The game's callback objects: (CCallbackBase*, callback id).
static CALLBACKS: Mutex<Vec<(usize, i32)>> = Mutex::new(Vec::new());

// The recorded client's packets (a two-instance session's capture), replayed with the peer's
// own sequence numbers.
const JOIN_ACCEPT: &str = "05080c000000010001fc2200";
const JOIN_INFO: &str = "1a221800010001000101c17bffffffff00000000ffffffff";
const PROFILE: &str = "25223401020001000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000506c617965722031000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001000000000000000000000000000000000000000000000000000000000000000000000000000000000000000100000000000000000000000000000000000000000000000000000000000000654c5e0201001001";
const LOBBY_A: &str = "25001800080001001400000001000000000000008e274342";
const LOBBY_B: &str = "25001c00110001001400000003000000000000008e2743425701004f";
const SHOP_ENTER_1: &str = "2822f40003000100ffffe8000200000008000000ffffffff080000000100000000000000ffffffff000000003b00000001000000000000003b00000001000000a800000000000000030000003a0000003a000000f8f9220084fe967a486728003cfa22005fb3957a0300000041b3957ad0d29c2f24fa2200f68dc97b70ee702f01000000030000000000000048fa2200328fc97ba0e4702fa0e4702f00000000b0ed702f84fa22000000000014ee702f64fa2200758dc97bbced702fa0e4702f01000000020000000000000088fa2200cd8ec97b84fa2200a0e4702f0000000008eb702fc4fa220000000000d4ed702f9cfa2200";
const SHOP_ENTER_2: &str = "2822f40004000100ffffe800050000000e000000ffffffff0103060200000000000000000003967a486728003cfa22005fb3957a0300000041b3957ad0d29c2f24fa2200f68dc97b70ee702f01000000030000000000000048fa2200328fc97ba0e4702fa0e4702f00000000b0ed702f84fa22000000000014ee702f64fa2200758dc97bbced702fa0e4702f01000000020000000000000088fa2200cd8ec97b84fa2200a0e4702f0000000008eb702fc4fa220000000000d4ed702f9cfa220066ff5ed4b8fa2200d0ff9e7b02000000701e722f10366b2f20fb2200e99c9a7b00ecd37b00000000fffffffff0ebd37b00000000";
const SHOP_ENTER_3: &str = "2822f40005000100ffffe8000400000008000000ffffffff0300000001000000030000000000000048fa2200328fc97ba0e4702fa0e4702f00000000b0ed702f84fa22000000000014ee702f64fa2200758dc97bbced702fa0e4702f01000000020000000000000088fa2200cd8ec97b84fa2200a0e4702f0000000008eb702fc4fa220000000000d4ed702f9cfa220066ff5ed4b8fa2200d0ff9e7b02000000701e722f10366b2f20fb2200e99c9a7b00ecd37b00000000fffffffff0ebd37b0000000032ff5ed4fcfa2200735bb07b05000000ffffffffe8fa22000e0000000100000003000000805bb07b0103060200000000";
const SHOP_BROWSE_1: &str = "2822f40006000100ffffe8000200000008000000ffffffff0600000009000000000000003b00000001000000a800000000000000030000003a0000003a000000f8f9220084fe967a486728003cfa22005fb3957a0300000041b3957ad0d29c2f0000000000000000c0682f00a8000000000000003a00000008fa22009a51067accfa22004cfa22000000000000000000d0d29c2f54fa220085418b7a85418b7ad880da7ad080da7ab0fa22008a4fd479d080da7ad0d29c2f000000000000000000000000b60a917ad0d29c2f00000000000000000000000001000100d0d29c2fd0d29c2f00000000000000003c428b7a3881da7a";
const SHOP_BROWSE_2: &str = "2822f40007000100ffffe8000200000008000000ffffffff060000000a0000000000000048fa2200328fc97ba0e4702fa0e4702f00000000b0ed702f84fa22000000000014ee702f64fa2200758dc97bbced702fa0e4702f01000000020000000000000088fa2200cd8ec97b84fa2200a0e4702f0000000008eb702fc4fa220000000000d4ed702f9cfa22001932c47b3ccd6a2f0000000000000000bcfa22007cc4c87b3ccd6a2f0000000000000000000000000000000070cf6a2fd8fa2200a0549d2f1cac682f60ac682f3cfb22000000000070cf6a2f1cac682f002ccc7b19f6be7bffffff7fffffff7fced0d37b04fb2200";
const SHOP_BROWSE_3: &str = "2822f40008000100ffffe8000200000008000000ffffffff06000000000000000000000048fa2200328fc97ba0e4702fa0e4702f00000000b0ed702f84fa22000000000014ee702f64fa2200758dc97bbced702fa0e4702f01000000020000000000000088fa2200cd8ec97b84fa2200a0e4702f0000000008eb702fc4fa220000000000d4ed702f9cfa22001932c47b3ccd6a2f0000000000000000bcfa22007cc4c87b3ccd6a2f0000000000000000000000000000000070cf6a2fd8fa2200a0549d2f1cac682f60ac682f3cfb22000000000070cf6a2f1cac682f002ccc7b19f6be7bffffff7fffffff7fced0d37b04fb2200";
const SHOP_READY_1: &str = "2822f40009000100ffffe80003000000040000000100000000000000010000000000220058f922007cfa220003000000ffffffff7cf92200c1b9957a7cf92200c1b9957a00010000000000000000000074f9220002000000a800000000000000d0cf2e000000000098f922009ebc957a03000000a4f92200d0ad957a03000000a8000000f080a269f0f9220010b5957ac0f9220003000000fa682f003a0000000300000000000000ffffffff000000003b00000001000000000000003b00000001000000a800000000000000030000003a0000003a000000f8f9220084fe967a486728003cfa22005fb3957a0300000041b3957a";
const SHOP_READY_2: &str = "2822f4000a000100ffffe800050000000e000000ffffffff02040306020000000000000000040000a4f92200d0ad957a03000000a8000000f080a269f0f9220010b5957ac0f9220003000000fa682f003a0000000300000000000000ffffffff000000003b00000001000000000000003b00000001000000a800000000000000030000003a0000003a000000f8f9220084fe967a486728003cfa22005fb3957a0300000041b3957afaff5ed42cfa2200ef459f7b0300000001000000d81d8b2f0400000020048b2f0000000048fa22007fef9a7b0100000000000000a0d9772f00b86b2f00000000b4fa2200529d9a7b64fa2200";
const SHOP_READY_3: &str = "2822f4000b000100ffffe8000400000008000000ffffffff04000000010000003a0000000300000000000000ffffffff000000003b00000001000000000000003b00000001000000a800000000000000030000003a0000003a000000f8f9220084fe967a486728003cfa22005fb3957a0300000041b3957afaff5ed42cfa2200ef459f7b0300000001000000d81d8b2f0400000020048b2f0000000048fa22007fef9a7b0100000000000000a0d9772f00b86b2f00000000b4fa2200529d9a7b64fa2200aeff5ed490fa2200735bb07b05000000ffffffff7cfa22000e0000000100000004000000805bb07b0204030602000000";
const SHOP_READY_AGAIN: &str = "2822f40012000100ffffe80003000000040000000100000001000000030000000000000048fa2200328fc97ba0e4702fa0e4702f00000000b0ed702f84fa22000000000014ee702f64fa2200758dc97bbced702fa0e4702f01000000020000000000000088fa2200cd8ec97b84fa2200a0e4702f0000000008eb702fc4fa220000000000d4ed702f9cfa22001932c47b3ccd6a2f0000000000000000bcfa220052ff5ed4e4fa2200f9479e7bc0437a2f300e7a2fc0c27b2ff4fa220004d6b17b00000000000000000100000020c97b2fc0599d2f88c37b2ffcc77b2f02000000eec97b2f04c37b2fc0599d2fffffffffb0467a2f";
const LEVEL_24: &str = "24221000130001009f77d27b00000000";
const LEVEL_HELLO: &str = "81001600440001000100000048454c4c4f0000000000";
const LEVEL_EVENT7_3: &str = "830218004500010002000000070000000300000000000000";
const LEVEL_BUNDLE: &str = "23221f00140001001f00000000000000000004040000000407020000000000";
const LEVEL_EVENT7_2: &str = "830218005000010003000000070000000200000000000000";

fn bytes(h: &str) -> Vec<u8> {
    (0..h.len()).step_by(2).filter_map(|i| u8::from_str_radix(&h[i..i + 2], 16).ok()).collect()
}

struct Peer {
    /// Packets from the host to the peer, not handled yet.
    inbox: VecDeque<Vec<u8>>,
    /// Packets from the peer to the host, not read yet.
    outbox: VecDeque<Vec<u8>>,
    /// The peer's counters: every packet's, the reliable (sub 0x22) packets'.
    seq: u16,
    reliable_seq: u16,
    /// Joined the lobby (callbacks fired).
    joined: bool,
    /// When the lobby was seen, the last connect sent (the connect repeats until the accept).
    lobby_seen: Option<std::time::Instant>,
    last_connect: Option<std::time::Instant>,
    /// The host's connection accept (04) received.
    accepted: bool,
    /// The replay's stages done: profile, gun shop entered, ready, level started.
    profile_sent: bool,
    shop_entered: bool,
    ready_sent: bool,
    level_started: bool,
    /// The peer's next event id (0x83 at +8; the replayed level start used 2 and 3).
    event_id: u32,
    /// The last input bundle sent (player 2's crosshair and buttons, during the level).
    last_input: Option<std::time::Instant>,
    /// Player 2's actions held (bit n: the input action n of the tick's list).
    actions_held: u8,
}

static PEER: Mutex<Peer> = Mutex::new(Peer {
    inbox: VecDeque::new(),
    outbox: VecDeque::new(),
    seq: 0,
    reliable_seq: 1,
    joined: false,
    lobby_seen: None,
    last_connect: None,
    accepted: false,
    profile_sent: false,
    shop_entered: false,
    ready_sent: false,
    level_started: false,
    event_id: 4,
    last_input: None,
    actions_held: 0,
});

fn hex(d: &[u8]) -> String {
    d.iter().map(|b| format!("{b:02x}")).collect()
}

impl Peer {
    /// Queues a packet to the host: its seq from the reliable counter (sub 0x22) or the packet
    /// counter, the peer's slot (1) to the host's (0).
    fn send(&mut self, mut packet: Vec<u8>) {
        if packet.len() >= 8 {
            let seq = if packet[1] == 0x22 {
                let s = self.reliable_seq;
                self.reliable_seq = self.reliable_seq.wrapping_add(1);
                s
            } else {
                let s = self.seq;
                self.seq = self.seq.wrapping_add(1);
                s
            };
            packet[4..6].copy_from_slice(&seq.to_le_bytes());
            packet[6] = 0x01;
            packet[7] = 0x00;
        }
        log!("hotdo peer: -> {}", hex(&packet));
        self.outbox.push_back(packet);
    }

    fn send_all(&mut self, packets: &[&str]) {
        for p in packets {
            self.send(bytes(p));
        }
    }

    /// The host's packet `p`: acknowledgements, keep-alives and the replay.
    fn handle(&mut self, p: &[u8]) {
        log!("hotdo peer: <- {}", hex(p));
        if p.len() < 8 {
            return;
        }
        let (kind, sub, seq) = (p[0], p[1], u16::from_le_bytes([p[4], p[5]]));
        // reliable packets: acknowledged by their seq
        if sub == 0x22 {
            let mut ack = vec![0x0e, 0x00, 0x0c, 0x00, 0, 0, 0x01, 0x00];
            ack.extend_from_slice(&seq.to_le_bytes());
            ack.extend_from_slice(&[0x22, 0x00]);
            self.send(ack);
        }
        // events and hellos: acknowledged by their id
        if (kind == 0x83 || kind == 0x81) && p.len() >= 12 {
            let mut ack = vec![0x82, 0x00, 0x10, 0x00, 0, 0, 0x01, 0x00];
            ack.extend_from_slice(&p[8..12]);
            ack.extend_from_slice(&[0; 4]);
            self.send(ack);
        }
        // the host's keep-alive ping: the client's answer (a ping of 16 ms)
        if kind == 0x0b && sub == 0x08 {
            self.send(bytes("0b00140000000100010000440f00000098020000"));
        }
        // gun shop records: [ff ff e8 00][type][length][data]
        let record = if kind == 0x28 && p.len() >= 20 { Some(u32::from_le_bytes([p[12], p[13], p[14], p[15]])) } else { None };
        match kind {
            // connection accepted: the client's accept and join info
            0x04 if !self.accepted => {
                self.accepted = true;
                self.send_all(&[JOIN_ACCEPT, JOIN_INFO]);
            }
            // the host's lobby state: the client's profile and lobby state
            0x25 if self.accepted && !self.profile_sent => {
                self.profile_sent = true;
                self.send_all(&[PROFILE, LOBBY_A, LOBBY_B]);
            }
            0x28 => {
                if !self.shop_entered {
                    self.shop_entered = true;
                    self.send_all(&[SHOP_ENTER_1, SHOP_ENTER_2, SHOP_ENTER_3, SHOP_BROWSE_1, SHOP_BROWSE_2, SHOP_BROWSE_3]);
                }
                match record {
                    // the host's ready check: player 2 ready
                    Some(3) if !self.ready_sent => {
                        self.ready_sent = true;
                        self.send_all(&[SHOP_READY_1, SHOP_READY_2, SHOP_READY_3]);
                    }
                    Some(3) => self.send_all(&[SHOP_READY_AGAIN]),
                    // records the client echoes
                    Some(6) => self.send(p.to_vec()),
                    _ => {}
                }
            }
            // the host player's state (reliable bundle about object 3, player 1): the same state
            // for object 4 (player 2): the level's sync (intro, then playing)
            0x23 if sub == 0x22 && p.len() > 0x13 && p[0x12] == 0x03 => {
                let mut mirror = p.to_vec();
                mirror[0x12] = 0x04;
                self.send(mirror);
            }
            // the host player's events 7 (its weapon / state) and 4 (the level's sync points):
            // the same from player 2
            0x83 if p.len() >= 20 && matches!(u32::from_le_bytes([p[12], p[13], p[14], p[15]]), 4 | 7) => {
                let mut event = vec![0x83, 0x02, 0x18, 0x00, 0, 0, 0x01, 0x00];
                event.extend_from_slice(&self.event_id.to_le_bytes());
                self.event_id += 1;
                event.extend_from_slice(&p[12..20]);
                event.extend_from_slice(&[0; 4]);
                self.send(event);
            }
            // the host's node list event (9: node, flag, player 0): the same from player 2 (the
            // recorded client confirmed each one)
            0x83 if p.len() >= 28 && u32::from_le_bytes([p[12], p[13], p[14], p[15]]) == 9 && p[21] == 0 => {
                let mut event = vec![0x83, 0x02, 0x1c, 0x00, 0, 0, 0x01, 0x00];
                event.extend_from_slice(&self.event_id.to_le_bytes());
                self.event_id += 1;
                event.extend_from_slice(&p[12..28]);
                event[21] = 0x01;
                self.send(event);
            }
            // the level starts: the client's hello and its first events
            0x81 if !self.level_started => {
                self.level_started = true;
                player_2_local();
                self.send_all(&[LEVEL_24, LEVEL_HELLO, LEVEL_EVENT7_3, LEVEL_BUNDLE, LEVEL_EVENT7_2]);
            }
            _ => {}
        }
    }
}

/// Fires the game's callbacks of `id` with `param` (MSVC: slot 1 is `Run(void*)`).
fn fire(id: i32, param: &[u8]) {
    let targets: Vec<usize> = CALLBACKS.lock().unwrap().iter().filter(|(_, i)| *i == id).map(|(o, _)| *o).collect();
    log!("hotdo peer: callback {id} to {} object(s)", targets.len());
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    for object in targets {
        unsafe {
            let vtable = *(object as *const *const usize);
            log!(
                "hotdo peer: callback object {object:#x} methods {:#x} {:#x} {:#x} (static VAs), flags {:#x} id {}",
                (*vtable).wrapping_sub(base) + 0x400000,
                (*vtable.add(1)).wrapping_sub(base) + 0x400000,
                (*vtable.add(2)).wrapping_sub(base) + 0x400000,
                *((object + 4) as *const u8),
                *((object + 8) as *const i32)
            );
            let run: unsafe extern "thiscall" fn(usize, *const u8) = std::mem::transmute(*vtable.add(1));
            run(object, param.as_ptr());
        }
    }
}

/// The peer's tick (the game's thread, after Steam's callbacks): player 2 joins the host's
/// lobby 2 s after it appears (the host's network then listens), its connect repeated each
/// second until the host's accept.
fn tick() {
    use std::time::{Duration, Instant};
    let lobby = LOBBY.load(Ordering::Relaxed);
    let mut peer = PEER.lock().unwrap();
    if lobby != 0 && peer.lobby_seen.is_none() {
        peer.lobby_seen = Some(Instant::now());
    }
    // the host's network listening (P2PSessionRequest_t's callback registered)
    let listening = CALLBACKS.lock().unwrap().iter().any(|&(_, id)| id == 1202);
    if !peer.joined && listening && peer.lobby_seen.is_some_and(|t| t.elapsed() >= Duration::from_secs(2)) {
        peer.joined = true;
        drop(peer);
        // LobbyChatUpdate_t: lobby, user changed, making change, k_EChatMemberStateChangeEntered
        let mut update = Vec::new();
        update.extend_from_slice(&lobby.to_le_bytes());
        update.extend_from_slice(&PEER_ID.to_le_bytes());
        update.extend_from_slice(&PEER_ID.to_le_bytes());
        update.extend_from_slice(&1u32.to_le_bytes());
        update.extend_from_slice(&[0; 4]);
        fire(505, &update);
        fire(1202, &PEER_ID.to_le_bytes());
        peer = PEER.lock().unwrap();
    }
    if peer.joined && !peer.accepted && peer.last_connect.is_none_or(|t| t.elapsed() >= Duration::from_secs(1)) {
        peer.last_connect = Some(Instant::now());
        // the client's connect (its own seq field)
        log!("hotdo peer: -> 03000800e0a20000");
        peer.outbox.push_back(vec![0x03, 0x00, 0x08, 0x00, 0xe0, 0xa2, 0x00, 0x00]);
    }
    while let Some(p) = peer.inbox.pop_front() {
        peer.handle(&p);
    }
    // player 2's input, every frame (the host applies it as it comes: no smoothing):
    // replicated object 4's field 0x0d (12 bytes: the crosshair's x, y in the game window's
    // pixels from its centre, y up, and the buttons: bit 0 the trigger).
    if peer.level_started && peer.last_input.is_none_or(|t| t.elapsed() >= Duration::from_millis(15)) {
        peer.last_input = Some(Instant::now());
        let stick = wal_payload_common::input(1);
        let hwnd = wal_payload_common::gun::find_window();
        let (w, h) = wal_payload_common::gun::client_area(hwnd).map_or((1920.0, 1080.0), |(_, _, w, h)| (w, h));
        let on_screen = wal_payload_common::gun::gun_of_player_in_picture(1);
        let (x, y) = on_screen.map_or((-w, 0.0), |(x, y)| ((x - 0.5) * w, (0.5 - y) * h));
        let trigger = stick.pressed(wal_protocol::button::B1);
        // the local players' input actions made on player 2's object (a remote player's input
        // carries only the crosshair and the trigger): reload (b2, b6 or a shot off the
        // screen), special (b3), next weapon (right or start), previous weapon (left)
        use crate::hotdo::{ACTION_NEXT_WEAPON_RVA, ACTION_PREVIOUS_WEAPON_RVA, ACTION_RELOAD_RVA, ACTION_SPECIAL_RVA, player_action};
        use wal_protocol::button::{B2, B3, B6, LEFT, RIGHT, START};
        let actions = [
            (stick.pressed(B2) || stick.pressed(B6) || (trigger && on_screen.is_none()), ACTION_RELOAD_RVA),
            (stick.pressed(B3), ACTION_SPECIAL_RVA),
            (stick.pressed(RIGHT) || stick.pressed(START), ACTION_NEXT_WEAPON_RVA),
            (stick.pressed(LEFT), ACTION_PREVIOUS_WEAPON_RVA),
        ];
        for (i, &(pressed, rva)) in actions.iter().enumerate() {
            if pressed && peer.actions_held & 1 << i == 0 {
                player_action(1, rva);
            }
        }
        peer.actions_held = actions.iter().enumerate().fold(0, |held, (i, &(pressed, _))| held | (pressed as u8) << i);
        let buttons = (trigger && on_screen.is_some()) as u32;
        let mut bundle = vec![0x23, 0x00, 0x22, 0x00, 0, 0, 0x01, 0x00];
        bundle.extend_from_slice(&[0; 10]);
        bundle.extend_from_slice(&[0x04, 0x0d]);
        bundle.extend_from_slice(&(x as f32).to_le_bytes());
        bundle.extend_from_slice(&(y as f32).to_le_bytes());
        bundle.extend_from_slice(&buttons.to_le_bytes());
        bundle.extend_from_slice(&[0, 0]);
        let seq = peer.seq;
        peer.seq = peer.seq.wrapping_add(1);
        bundle[4..6].copy_from_slice(&seq.to_le_bytes());
        peer.outbox.push_back(bundle);
    }
}

/// The game runs only its own player's score and health (the other player's owner reports
/// them): its "this player is mine" checks made to pass for player 2 too, the host now its
/// owner in all but the input.
fn player_2_local() {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    static DONE: std::sync::Once = std::sync::Once::new();
    DONE.call_once(|| {
        let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
        // (RVA, the jne skipping a remote player): 0x5248c0's visible score (+0x244 of the
        // players' stats), 0x58b320's health change (player +0x20c, dying at 0)
        for (rva, original) in [(0x124915, [0x75, 0x67]), (0x18b356, [0x75, 0x4c])] {
            let at = (base + rva) as *mut [u8; 2];
            if unsafe { at.read() } != original {
                log!("hotdo peer: unexpected code at RVA {rva:#x}: {:02x?}", unsafe { at.read() });
                continue;
            }
            let mut old = 0;
            unsafe {
                VirtualProtect(at as *const _, 2, PAGE_EXECUTE_READWRITE, &mut old);
                at.write([0x90, 0x90]);
                VirtualProtect(at as *const _, 2, old, &mut old);
            }
        }
        log!("hotdo peer: player 2's score and health run by the host");
    });
}

// The hooks

unsafe extern "C" fn register_callback(object: usize, id: i32) {
    let mut callbacks = CALLBACKS.lock().unwrap();
    callbacks.retain(|(o, _)| *o != object);
    callbacks.push((object, id));
    drop(callbacks);
    let orig: unsafe extern "C" fn(usize, i32) = unsafe { std::mem::transmute(ORIG_REGISTER.load(Ordering::Relaxed)) };
    unsafe { orig(object, id) };
}

unsafe extern "C" fn unregister_callback(object: usize) {
    CALLBACKS.lock().unwrap().retain(|(o, _)| *o != object);
    let orig: unsafe extern "C" fn(usize) = unsafe { std::mem::transmute(ORIG_UNREGISTER.load(Ordering::Relaxed)) };
    unsafe { orig(object) };
}

unsafe extern "C" fn run_callbacks() {
    let orig: unsafe extern "C" fn() = unsafe { std::mem::transmute(ORIG_RUN_CALLBACKS.load(Ordering::Relaxed)) };
    unsafe { orig() };
    tick();
}

fn method(slot: usize) -> usize {
    NET_METHODS[slot].load(Ordering::Relaxed)
}

/// SendP2PPacket(CSteamID, data, len, send type, channel)
unsafe extern "thiscall" fn net_send(this: usize, id_lo: u32, id_hi: u32, data: *const u8, len: u32, kind: u32, channel: i32) -> u8 {
    if (id_hi as u64) << 32 | id_lo as u64 == PEER_ID {
        let packet = unsafe { std::slice::from_raw_parts(data, len as usize) }.to_vec();
        PEER.lock().unwrap().inbox.push_back(packet);
        return 1;
    }
    let orig: unsafe extern "thiscall" fn(usize, u32, u32, *const u8, u32, u32, i32) -> u8 = unsafe { std::mem::transmute(method(0)) };
    unsafe { orig(this, id_lo, id_hi, data, len, kind, channel) }
}

/// IsP2PPacketAvailable(size*, channel)
unsafe extern "thiscall" fn net_available(this: usize, size: *mut u32, channel: i32) -> u8 {
    if channel == 0
        && let Some(p) = PEER.lock().unwrap().outbox.front()
    {
        static ONCE: AtomicBool = AtomicBool::new(false);
        if !ONCE.swap(true, Ordering::Relaxed) {
            log!("hotdo peer: host polls: {} bytes available", p.len());
        }
        if !size.is_null() {
            unsafe { *size = p.len() as u32 };
        }
        return 1;
    }
    let orig: unsafe extern "thiscall" fn(usize, *mut u32, i32) -> u8 = unsafe { std::mem::transmute(method(1)) };
    unsafe { orig(this, size, channel) }
}

/// ReadP2PPacket(dest, capacity, size*, CSteamID* remote, channel)
unsafe extern "thiscall" fn net_read(this: usize, dest: *mut u8, capacity: u32, size: *mut u32, remote: *mut u64, channel: i32) -> u8 {
    if channel == 0 {
        let mut peer = PEER.lock().unwrap();
        if let Some(p) = peer.outbox.front()
            && p.len() as u32 <= capacity
        {
            let p = peer.outbox.pop_front().unwrap();
            log!("hotdo peer: host read {} bytes", p.len());
            unsafe {
                std::ptr::copy_nonoverlapping(p.as_ptr(), dest, p.len());
                if !size.is_null() {
                    *size = p.len() as u32;
                }
                if !remote.is_null() {
                    *remote = PEER_ID;
                }
            }
            return 1;
        }
    }
    let orig: unsafe extern "thiscall" fn(usize, *mut u8, u32, *mut u32, *mut u64, i32) -> u8 = unsafe { std::mem::transmute(method(2)) };
    unsafe { orig(this, dest, capacity, size, remote, channel) }
}

/// AcceptP2PSessionWithUser(CSteamID) / CloseP2PSessionWithUser(CSteamID)
unsafe extern "thiscall" fn net_accept(this: usize, id_lo: u32, id_hi: u32) -> u8 {
    if (id_hi as u64) << 32 | id_lo as u64 == PEER_ID {
        return 1;
    }
    let orig: unsafe extern "thiscall" fn(usize, u32, u32) -> u8 = unsafe { std::mem::transmute(method(3)) };
    unsafe { orig(this, id_lo, id_hi) }
}

unsafe extern "thiscall" fn net_close(this: usize, id_lo: u32, id_hi: u32) -> u8 {
    if (id_hi as u64) << 32 | id_lo as u64 == PEER_ID {
        log!("hotdo peer: session closed by the host");
        return 1;
    }
    let orig: unsafe extern "thiscall" fn(usize, u32, u32) -> u8 = unsafe { std::mem::transmute(method(4)) };
    unsafe { orig(this, id_lo, id_hi) }
}

/// GetP2PSessionState(CSteamID, P2PSessionState_t*): the peer always connected.
unsafe extern "thiscall" fn net_session_state(this: usize, id_lo: u32, id_hi: u32, state: *mut u8) -> u8 {
    if (id_hi as u64) << 32 | id_lo as u64 == PEER_ID {
        if !state.is_null() {
            // active, connecting, error, relay, bytes queued, packets queued, remote IP, port
            let mut s = [0u8; 20];
            s[0] = 1;
            s[12..16].copy_from_slice(&PEER_IP.to_le_bytes());
            s[16..18].copy_from_slice(&PEER_PORT.to_le_bytes());
            unsafe { std::ptr::copy_nonoverlapping(s.as_ptr(), state, s.len()) };
        }
        return 1;
    }
    let orig: unsafe extern "thiscall" fn(usize, u32, u32, *mut u8) -> u8 = unsafe { std::mem::transmute(method(6)) };
    unsafe { orig(this, id_lo, id_hi, state) }
}

/// SetLobbyData(CSteamID lobby, key, value): the host's lobby.
unsafe extern "thiscall" fn mm_set_lobby_data(this: usize, id_lo: u32, id_hi: u32, key: *const i8, value: *const i8) -> u8 {
    let lobby = (id_hi as u64) << 32 | id_lo as u64;
    if LOBBY.swap(lobby, Ordering::Relaxed) != lobby {
        log!("hotdo peer: host lobby {lobby}");
    }
    let orig: unsafe extern "thiscall" fn(usize, u32, u32, *const i8, *const i8) -> u8 = unsafe { std::mem::transmute(MM_SET_LOBBY_DATA.load(Ordering::Relaxed)) };
    unsafe { orig(this, id_lo, id_hi, key, value) }
}

/// A copy of `object`'s method table (64 slots) with `patches`, installed on the object.
unsafe fn patch_vtable(object: usize, patches: &[(usize, usize)], originals: &mut dyn FnMut(usize, usize)) {
    unsafe {
        let vtable = *(object as *const *const usize);
        let copy: &'static mut [usize; 64] = Box::leak(Box::new([0usize; 64]));
        for (i, slot) in copy.iter_mut().enumerate() {
            *slot = *vtable.add(i);
        }
        for &(slot, f) in patches {
            originals(slot, copy[slot]);
            copy[slot] = f;
        }
        *(object as *mut *const usize) = copy.as_ptr();
    }
}

unsafe extern "C" fn steam_networking() -> usize {
    let orig: unsafe extern "C" fn() -> usize = unsafe { std::mem::transmute(ORIG_NETWORKING.load(Ordering::Relaxed)) };
    let object = unsafe { orig() };
    if object != 0 && !NET_PATCHED.swap(true, Ordering::Relaxed) {
        let patches = [
            (0, net_send as *const () as usize),
            (1, net_available as *const () as usize),
            (2, net_read as *const () as usize),
            (3, net_accept as *const () as usize),
            (4, net_close as *const () as usize),
            (6, net_session_state as *const () as usize),
        ];
        unsafe { patch_vtable(object, &patches, &mut |slot, f| NET_METHODS[slot].store(f, Ordering::Relaxed)) };
        log!("hotdo peer: SteamNetworking {object:#x} patched");
    }
    object
}

unsafe extern "C" fn steam_matchmaking() -> usize {
    let orig: unsafe extern "C" fn() -> usize = unsafe { std::mem::transmute(ORIG_MATCHMAKING.load(Ordering::Relaxed)) };
    let object = unsafe { orig() };
    if object != 0 && !MM_PATCHED.swap(true, Ordering::Relaxed) {
        // ISteamMatchmaking009: SetLobbyData is slot 20
        let patches = [(20, mm_set_lobby_data as *const () as usize)];
        unsafe { patch_vtable(object, &patches, &mut |_, f| MM_SET_LOBBY_DATA.store(f, Ordering::Relaxed)) };
        log!("hotdo peer: SteamMatchmaking {object:#x} patched");
    }
    object
}

pub fn init() {
    let hooks: [(&str, usize, &AtomicUsize); 5] = [
        ("SteamAPI_UnregisterCallback", unregister_callback as *const () as usize, &ORIG_UNREGISTER),
        ("SteamAPI_RegisterCallback", register_callback as *const () as usize, &ORIG_REGISTER),
        ("SteamAPI_RunCallbacks", run_callbacks as *const () as usize, &ORIG_RUN_CALLBACKS),
        ("SteamNetworking", steam_networking as *const () as usize, &ORIG_NETWORKING),
        ("SteamMatchmaking", steam_matchmaking as *const () as usize, &ORIG_MATCHMAKING),
    ];
    for (name, hook, orig) in hooks {
        match unsafe { iat::hook("steam_api.dll", name, hook) } {
            Some(o) => orig.store(o, Ordering::Relaxed),
            None => log!("hotdo peer: steam_api.dll!{name} not imported"),
        }
    }
    log!("hotdo peer: player 2 as a fake network peer (Steam ID {PEER_ID})");
}
