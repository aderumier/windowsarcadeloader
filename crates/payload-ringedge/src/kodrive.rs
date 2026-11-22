//! K.O. Drive's cabinets linked across PCs (`WAL_LINK`, payload-common `link.rs`). Its link is
//! Sega's link library (UDP 11882, "POWER-ON NETWORK TEST"): node i of TOTAL MACHINE n in GROUP g
//! is 192.168.99.(1 + i + 6 (g + 2 (3 n - 6))), each node sending its frames to every node's
//! address. It reads its LINK ID, TOTAL MACHINE and GROUP ID (its test menu's settings) by three
//! functions (`0x4885C0`, `0x4885D0`, `0x4885F0`): linked, they answer the link's cabinet number
//! less one (LINK ID 0 leads the race), `WAL_LINK_CABINETS` (2 to 6) and group A, so the test
//! menu has nothing to set; its 192.168.99 addresses are the cabinets' PCs (`link::virtual_net`)
//! and amNetwork gives it its own ([`address`]).

use std::sync::OnceLock;

use wal_payload_common::{link, log, patches};

/// The executable's three functions as the known build has them (`mov eax,[settings]`, its
/// byte 0x30 or 0x31).
const FUNCTIONS: [(usize, [u8; 14]); 3] = [
    (0x4885C0, [0xA1, 0x3C, 0x65, 0xBC, 0x02, 0x8B, 0x48, 0x0C, 0x8A, 0x41, 0x30, 0x24, 0x0F, 0xC3]),
    (0x4885D0, [0xA1, 0x3C, 0x65, 0xBC, 0x02, 0x8B, 0x48, 0x0C, 0x8A, 0x41, 0x30, 0xC0, 0xE8, 0x04]),
    (0x4885F0, [0xA1, 0x3C, 0x65, 0xBC, 0x02, 0x8B, 0x48, 0x0C, 0x8A, 0x41, 0x31, 0x24, 0x0F, 0xC3]),
];

static ADDRESS: OnceLock<std::net::Ipv4Addr> = OnceLock::new();

/// Linked, the cabinet's address on the game's network (amNetwork's answer).
pub(crate) fn address() -> Option<std::net::Ipv4Addr> {
    ADDRESS.get().copied()
}

pub(crate) fn init() {
    if !patches::exe_name().to_ascii_lowercase().starts_with("m-driver_ringwide") {
        return;
    }
    if link::init() == 0 {
        return;
    }
    let (n, k) = (link::cabinets().unwrap_or(2).min(6), link::cabinet());
    if k > n {
        log!("kodrive: cabinet {k} of {n} (WAL_LINK_CABINETS): its test menu's LINK ID left as it is");
        return;
    }
    if FUNCTIONS.iter().any(|(va, was)| !wal_payload_common::crash::readable(*va, was.len()) || unsafe { std::slice::from_raw_parts(*va as *const u8, was.len()) } != was) {
        log!("kodrive: not the known build: its test menu's LINK ID, TOTAL MACHINE and GROUP ID left as they are");
        return;
    }
    // mov al, <value>; ret (the group: xor al, al; ret)
    let base = wal_payload_common::iat::game_module();
    patches::apply_list(&format!(
        "{:#x}:B0{:02X}C3,{:#x}:B0{:02X}C3,{:#x}:30C0C3",
        FUNCTIONS[0].0 - base,
        k - 1,
        FUNCTIONS[1].0 - base,
        n,
        FUNCTIONS[2].0 - base
    ));
    let first = 1 + 12 * (3 * n - 6);
    link::virtual_net([192, 168, 99], first as u8);
    let _ = ADDRESS.set(std::net::Ipv4Addr::new(192, 168, 99, (first + k - 1) as u8));
    log!("kodrive: linked, LINK ID {k} of {n}, group A, 192.168.99.{}{}", first + k - 1, if k == 1 { " (it leads the race)" } else { "" });
}
