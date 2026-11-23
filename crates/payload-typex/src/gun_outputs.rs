//! The gun games' outputs for the network outputs (`WAL_OUTPUTS`), read in the game as
//! DemulShooter does, at its addresses for the builds it knows (the dumps' executables have its
//! MD5s): the lamps and gun motors in the game's I/O output bytes, the players' life and ammo,
//! and its code caves as [`hook::player_flag`]s: `P1_CtmRecoil` when the game asks the gun
//! board for a shot's reaction, `P1_Damaged` for a hit's.
//!
//! * Haunted Museum (1.00): outputs at +0x1E27021/22, recoil and damage at the gun reaction
//!   call (`0x702C0`) of the shot (+0x6FDCA) and of the hit (+0x72798), for a player whose
//!   [+0x975E54 + 8 × player] is 0; life (float) +0x975F6C / +0x975F8C while +0x975EB0 /
//!   +0x975EF4 is 1.
//! * Haunted Museum 2 (1.00): outputs at +0x2B140B4 (DemulShooter's +0x29AE434 holds nothing
//!   in this build; found as the JVS output bytes, reversed: +0 the guns' recoil bits 5 / 4, +2
//!   their rumble 2 / 1): `P1_GunRecoil` the board's own (DemulShooter's life and ammo
//!   addresses hold nothing in this build either: not read).
//! * Gaia Attack 4 (1.02, 4 players): outputs at +0x1FE7200..02, recoil at the call of
//!   `0x74200` at +0x73BDA, damage at +0x76658 and +0x764A0 ([+0xB16E34 + 8 × player] 0);
//!   life +0xB16E60 + 8 × player while +0xB16A98 + 8 × player is 1.
//! * Elevator Action Death Parade: outputs at +0x204EBB/BC/BE, recoil at +0x3ED23 (the call of
//!   `0xC56E0`), damage at +0x6E29A, the player in edx; ammo and life in the players' records.
//! * Music GunGun! 2 (1.01): outputs at +0x2BC311/12.

use wal_payload_common::log;
use wal_payload_common::outputs::hook::{Flags, player_flag};
use wal_payload_common::outputs::{mem, poll, pulse, set};

/// Bit `bit` of the byte at `addr`.
fn bit(addr: usize, bit: u8) -> i32 {
    (mem::u8(addr) >> bit & 1) as i32
}

/// A hook of the game's code, logged when the build differs.
fn flag(name: &str, site: usize, original: &[u8; 5], reg: u8, skip_unless_zero: Option<usize>) -> Option<Flags> {
    let f = unsafe { player_flag(site, original, reg, skip_unless_zero) };
    if f.is_none() {
        log!("gun outputs: {name} at {site:#x}: another build, not hooked");
    }
    f
}

/// `P<n>_<what>` for players 1 to `players`, made once (static names).
fn names(what: &str, players: usize) -> Vec<&'static str> {
    (1..=players).map(|n| &*format!("P{n}_{what}").leak()).collect()
}

fn pulses(hooks: &[Option<Flags>], names: &[&'static str]) {
    for (p, name) in names.iter().enumerate() {
        if hooks.iter().flatten().fold(false, |any, f| f.take(p) || any) {
            pulse(name, 50);
        }
    }
}

pub(crate) fn init(game: &str) {
    if !wal_payload_common::outputs::enabled() {
        return;
    }
    let b = mem::base();
    match game {
        "haunted-museum" => {
            let recoil = [flag("recoil", b + 0x6FDCA, &[0xE8, 0xF1, 0x04, 0x00, 0x00], 1, Some(b + 0x975E54))];
            let damage = [flag("damage", b + 0x72798, &[0xE8, 0x23, 0xDB, 0xFF, 0xFF], 1, Some(b + 0x975E54))];
            let (r, d) = (names("CtmRecoil", 2), names("Damaged", 2));
            poll("haunted museum", 16, move || {
                let (o1, o2) = (b + 0x1E27021, b + 0x1E27022);
                rgb_start_motor(o1, o2);
                pulses(&recoil, &r);
                pulses(&damage, &d);
                for (p, (status, life)) in [(0x975EB0, 0x975F6C), (0x975EF4, 0x975F8C)].into_iter().enumerate() {
                    let l = if mem::u8(b + status) == 1 { (mem::f32(b + life) * 100.0).max(0.0) as i32 } else { 0 };
                    set(["P1_Life", "P2_Life"][p], l);
                }
                set("Credits", mem::u8(b + 0x98B3F8) as i32);
            });
        }
        "haunted-museum-2" => {
            poll("haunted museum 2", 16, move || {
                let o = b + 0x2B140B4;
                rgb_start_motor(o + 1, o + 2);
                for (p, recoil, rumble) in [(0, 5, 2), (1, 4, 1)] {
                    let n = p + 1;
                    set(&format!("P{n}_GunRecoil"), bit(o, recoil));
                    set(&format!("P{n}_CtmRecoil"), bit(o, recoil));
                    set(&format!("P{n}_GunMotor"), bit(o + 2, rumble));
                }
            });
        }
        "gaia-attack-4" => {
            let cond = Some(b + 0xB16E34);
            let recoil = [flag("recoil", b + 0x73BDA, &[0xE8, 0x21, 0x06, 0x00, 0x00], 1, cond)];
            let damage = [
                flag("damage", b + 0x76658, &[0xE8, 0xA3, 0xDB, 0xFF, 0xFF], 1, cond),
                flag("damage", b + 0x764A0, &[0xE8, 0x5B, 0xDD, 0xFF, 0xFF], 1, cond),
            ];
            let (r, d) = (names("CtmRecoil", 4), names("Damaged", 4));
            poll("gaia attack 4", 16, move || {
                let (o0, o1, o2) = (b + 0x1FE7200, b + 0x1FE7201, b + 0x1FE7202);
                rgb_start_motor(o1, o2);
                set("P3_LmpStart", bit(o0, 5));
                set("P4_LmpStart", bit(o0, 4));
                set("P3_GunMotor", bit(o0, 3));
                set("P4_GunMotor", bit(o0, 2));
                pulses(&recoil, &r);
                pulses(&damage, &d);
                for p in 0..4 {
                    let l = if mem::u8(b + 0xB16A98 + 8 * p) == 1 { mem::u8(b + 0xB16E60 + 8 * p) as i32 } else { 0 };
                    set(["P1_Life", "P2_Life", "P3_Life", "P4_Life"][p], l);
                }
                set("Credits", mem::u8(b + 0xB3B7C8) as i32);
            });
        }
        "elevator-action-death-parade" => {
            let recoil = [flag("recoil", b + 0x3ED23, &[0xE8, 0xB8, 0x69, 0x08, 0x00], 2, None)];
            let damage = [flag("damage", b + 0x6E29A, &[0x6A, 0x14, 0x6A, 0x01, 0x52], 2, None)];
            let (r, d) = (names("CtmRecoil", 2), names("Damaged", 2));
            poll("elevator action death parade", 16, move || {
                let (o1, o2, o3) = (b + 0x204EBC, b + 0x204EBE, b + 0x204EBB);
                set("LmpSide_R", bit(o2, 6));
                set("LmpSide_G", bit(o2, 5));
                set("LmpSide_B", bit(o2, 4));
                set("LmpUpBtn", bit(o1, 6));
                set("LmpDownBtn", bit(o1, 5));
                set("LmpCloseBtn", bit(o1, 4));
                set("P1_GunMotor", bit(o3, 4));
                set("P2_GunMotor", bit(o3, 3));
                pulses(&recoil, &r);
                pulses(&damage, &d);
                let records = mem::u32(b + 0x212CD4) as usize;
                let players = mem::u32(b + 0x210A80) as usize;
                for (p, (on, ammo, life)) in [(0x1794C, 0x54, 0x4C), (0x17A38, 0x58, 0x50)].into_iter().enumerate() {
                    let playing = records != 0 && players != 0 && mem::u8(players + on) != 0;
                    let read = |at: usize| if playing { mem::u8(mem::u32(records + at) as usize + 0x150) as i32 } else { 0 };
                    set(["P1_Ammo", "P2_Ammo"][p], read(ammo));
                    set(["P1_Life", "P2_Life"][p], read(life));
                }
            });
        }
        "music-gungun-2" => {
            poll("music gungun 2", 16, move || {
                let (o1, o2) = (b + 0x2BC311, b + 0x2BC312);
                set("P1_LmpStart", bit(o2, 7));
                set("P2_LmpStart", bit(o2, 6));
                set("P1_LmpGun", bit(o2, 5));
                set("P2_LmpGun", bit(o2, 4));
                set("LmpLBtn", bit(o1, 5));
                set("LmpMBtn", bit(o1, 6));
                set("LmpRBtn", bit(o1, 4));
                set("Credits", mem::u8(mem::u32(b + 0x2DBE08) as usize + 0x18) as i32);
            });
        }
        _ => {}
    }
}

/// The Haunted Museum engine's lamp bytes (Gaia Attack 4's too): `p2` P2's red / green / blue
/// (bits 7 / 6 / 5), `p1` P1's (7 / 4 / 3), the start lamps (6 / 5) and the gun motors (2 / 1).
fn rgb_start_motor(p2: usize, p1: usize) {
    set("P1_LmpStart", bit(p1, 6));
    set("P2_LmpStart", bit(p1, 5));
    set("P1_Lmp_R", bit(p1, 7));
    set("P1_Lmp_G", bit(p1, 4));
    set("P1_Lmp_B", bit(p1, 3));
    set("P2_Lmp_R", bit(p2, 7));
    set("P2_Lmp_G", bit(p2, 6));
    set("P2_Lmp_B", bit(p2, 5));
    set("P1_GunMotor", bit(p1, 2));
    set("P2_GunMotor", bit(p1, 1));
}
