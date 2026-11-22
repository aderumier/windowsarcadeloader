//! Sonic & Sega All-Stars Racing's cabinets linked across PCs (`WAL_LINK`, payload-common
//! `link.rs`). Its link is UDP only (its sync on 47616 and 47872 broadcast to 192.168.88.255, its
//! match presence to 255.255.255.255:48879), from the address amNetwork gives it, else (a
//! loopback one, refused) its backup 192.168.88.<CABINET ID> (`0x416e48`, its test menu's setting:
//! each cabinet must have its own). Linked, the backup is the link's cabinet number (the setting
//! made it too) and 192.168.88.k is cabinet k's PC (`link::virtual_net`).

use wal_payload_common::{link, log, patches};

/// `mov ecx,[eax+0x10]; shl ecx,0x18; or ecx,0x58a8c0`: the backup address from the CABINET ID.
const BACKUP: usize = 0x416E48;
const WAS: [u8; 12] = [0x8B, 0x48, 0x10, 0xC1, 0xE1, 0x18, 0x81, 0xC9, 0xC0, 0xA8, 0x58, 0x00];

pub(crate) fn init() {
    if !patches::exe_name().eq_ignore_ascii_case("game.exe")
        || !wal_payload_common::crash::readable(BACKUP, WAS.len())
        || unsafe { std::slice::from_raw_parts(BACKUP as *const u8, WAS.len()) } != WAS
    {
        return;
    }
    if link::init() == 0 {
        return;
    }
    let k = link::cabinet() as u8;
    // mov dword [eax+0x10], k; mov ecx, 192.168.88.k
    let base = wal_payload_common::iat::game_module();
    patches::apply_list(&format!("{:#x}:C74010{k:02X}000000B9C0A858{k:02X}", BACKUP - base));
    link::virtual_net([192, 168, 88], 1);
    log!("ssasr: linked, CABINET ID {k}, its backup address 192.168.88.{k}");
}
