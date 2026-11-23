//! The RingWide gun games' outputs for the network outputs (`WAL_OUTPUTS`), read in the game as
//! DemulShooter does, at its addresses for the builds it knows (the dumps' executables have its
//! MD5s; the game told by its executable's link time). Their output bytes are the JVS board's
//! (also sent as `lamp<n>`), here by name, with the guns' recoil / motor bits as `P1_CtmRecoil`.
//!
//! * Dream Raiders (prg\Game.exe): output bytes +0x308A0A4..A6 (start lamps, the players' and the
//!   rear RGB lamps, the blower level, the gun motors); life +0x118FECC / +0x118FEE0 while
//!   +0x118FF60 / +0x118FFFC is 1; credits +0x607FF0.
//! * Let's Go Island (LGI_RingW_F_safe.exe): output byte [+0x63BF5C] + 0x44 (start lamps, the
//!   guns' recoil and motor bits, the motor's as `P1_CtmRecoil`: it runs on each shot); the
//!   players' records [[[+0x820020] + 8 / 0xC] + 0x14]: playing
//!   +0x30, life (float) +0x18.
//! * Medal no Gunman (gunman_dxF.exe): the guns' cylinders [[+0x6C9B30] + 0x34 / 0x38] + 0x1C8
//!   (the ammo, while credits +0x6CBB40 are there).
//! * Transformers: Human Alliance (RingEdge, TF_Gun_R_Ring_dumped.exe): output bytes [+0x1A3CBA0]
//!   + 8 / 9 (start, gun and break lamps, the billboard, the gun motors); the players
//!   [[+0x1A3CC1C] + 0x34 / 0x38]: state +0x5C (3, 4 playing), life +0x60; credits +0x964E60;
//!   the shot code caves of each player (+0x2D518 / +0x2D548, `mov eax, [ecx + 0x34 / 0x38]; test
//!   eax, eax`) as player flags: `P1_CtmRecoil`.
//! * SEGA Golden Gun (RingGunR_RingWide.exe): output byte [+0x27034A8] + 8 (start lamps, the
//!   guns' recoil bits); in a game ([[+0x27034BC] + 0x34] 12) the players [[+0x27034E4] + 0x34 /
//!   0x38]: state +0x38 (3, 4 playing), life +0x3C, ammo +0x38C; credits +0x65C410.

use wal_payload_common::log;
use wal_payload_common::outputs::{Falling, mem, poll, set};

fn bit(v: u8, b: u8) -> i32 {
    (v >> b & 1) as i32
}

pub(crate) fn init() {
    if !wal_payload_common::outputs::enabled() {
        return;
    }
    let b = mem::base();
    match mem::timestamp() {
        0x50194FC2 => {
            log!("gun outputs: Dream Raiders");
            let mut damage: [Falling; 2] = Default::default();
            poll("dream raiders", 16, move || {
                let o = b + 0x308A0A4;
                let (o1, o2, o3) = (mem::u8(o), mem::u8(o + 1), mem::u8(o + 2));
                set("P1_LmpStart", bit(o1, 7));
                set("P2_LmpStart", bit(o1, 4));
                for (i, name) in ["P1_Lmp_R", "P1_Lmp_G", "P1_Lmp_B", "P2_Lmp_R", "P2_Lmp_G", "P2_Lmp_B", "LmpRear_R", "LmpRear_G"].iter().enumerate() {
                    set(name, bit(o2, 7 - i as u8));
                }
                set("LmpRear_B", bit(o3, 7));
                set("Blower_Level", ((o1 & 3) + (o3 >> 4 & 4)) as i32);
                for (p, motor) in [(0, 6), (1, 3)] {
                    set(["P1_GunMotor", "P2_GunMotor"][p], bit(o1, motor));
                    set(["P1_CtmRecoil", "P2_CtmRecoil"][p], bit(o1, motor));
                }
                for (p, (status, life)) in [(0x118FF60, 0x118FECC), (0x118FFFC, 0x118FEE0)].into_iter().enumerate() {
                    let l = (mem::u8(b + status) == 1).then(|| mem::u8(b + life) as i64);
                    damage[p].update(l, ["P1_Damaged", "P2_Damaged"][p]);
                    set(["P1_Life", "P2_Life"][p], l.unwrap_or(0) as i32);
                }
                set("Credits", mem::u8(b + 0x607FF0) as i32);
            });
        }
        0x4CADC1DB => {
            log!("gun outputs: Let's Go Island");
            let mut damage: [Falling; 2] = Default::default();
            poll("let's go island", 16, move || {
                let o = mem::u8(mem::u32(b + 0x63BF5C) as usize + 0x44);
                set("P1_LmpStart", bit(o, 7));
                set("P2_LmpStart", bit(o, 4));
                for (p, recoil, motor) in [(0, 5, 6), (1, 2, 3)] {
                    set(["P1_GunRecoil", "P2_GunRecoil"][p], bit(o, recoil));
                    // the gun motor runs on each shot (the recoil bit stays off in this build)
                    set(["P1_CtmRecoil", "P2_CtmRecoil"][p], bit(o, motor));
                    set(["P1_GunMotor", "P2_GunMotor"][p], bit(o, motor));
                }
                let players = mem::u32(b + 0x820020) as usize;
                for p in 0..2 {
                    let rec = if players != 0 { mem::chain(players + 8 + 4 * p, &[0x14]) as usize } else { 0 };
                    let l = (rec != 0 && mem::u8(rec + 0x30) == 1).then(|| (mem::f32(rec + 0x18) as i64).max(0));
                    damage[p].update(l, ["P1_Damaged", "P2_Damaged"][p]);
                    set(["P1_Life", "P2_Life"][p], l.unwrap_or(0) as i32);
                }
            });
        }
        0x4BE21ECF => {
            log!("gun outputs: Medal no Gunman");
            let mut recoil: [Falling; 2] = Default::default();
            poll("medal no gunman", 16, move || {
                let credits = mem::u32(b + 0x6CBB40) as i32;
                for p in 0..2 {
                    let cyl = mem::chain(b + 0x6C9B30, &[0x34 + 4 * p]) as usize;
                    let a = (credits > 0 && cyl != 0).then(|| mem::u8(cyl + 0x1C8) as i64);
                    recoil[p].update(a, ["P1_CtmRecoil", "P2_CtmRecoil"][p]);
                    set(["P1_Ammo", "P2_Ammo"][p], a.unwrap_or(0) as i32);
                    set(["P1_Clip", "P2_Clip"][p], (a.unwrap_or(0) > 0) as i32);
                }
                set("Credits", credits);
            });
        }
        0x4D01EEE2 => {
            log!("gun outputs: SEGA Golden Gun");
            let mut damage: [Falling; 2] = Default::default();
            poll("sega golden gun", 16, move || {
                let o = mem::u8(mem::u32(b + 0x27034A8) as usize + 8);
                set("P1_LmpStart", bit(o, 7));
                set("P2_LmpStart", bit(o, 4));
                for (p, recoil) in [(0, 6), (1, 3)] {
                    set(["P1_GunRecoil", "P2_GunRecoil"][p], bit(o, recoil));
                    set(["P1_CtmRecoil", "P2_CtmRecoil"][p], bit(o, recoil));
                }
                let in_game = mem::u8(mem::u32(b + 0x27034BC) as usize + 0x34) == 12;
                let players = mem::u32(b + 0x27034E4) as usize;
                for p in 0..2 {
                    let rec = if in_game && players != 0 { mem::u32(players + 0x34 + 4 * p) as usize } else { 0 };
                    let playing = rec != 0 && matches!(mem::u8(rec + 0x38), 3 | 4);
                    let l = playing.then(|| mem::u8(rec + 0x3C) as i64);
                    let a = if playing { mem::u8(rec + 0x38C) as i32 } else { 0 };
                    damage[p].update(l, ["P1_Damaged", "P2_Damaged"][p]);
                    set(["P1_Life", "P2_Life"][p], l.unwrap_or(0) as i32);
                    set(["P1_Ammo", "P2_Ammo"][p], a);
                    set(["P1_Clip", "P2_Clip"][p], (a > 0) as i32);
                }
                set("Credits", mem::u32(b + 0x65C410) as i32);
            });
        }
        #[cfg(target_arch = "x86")]
        0x53A7881E => {
            use wal_payload_common::outputs::hook::{Player, player_flag};
            log!("gun outputs: Transformers: Human Alliance");
            let shots = [
                unsafe { player_flag(b + 0x2D518, &[0x8B, 0x41, 0x34, 0x85, 0xC0], Player::Fixed, None) },
                unsafe { player_flag(b + 0x2D548, &[0x8B, 0x41, 0x38, 0x85, 0xC0], Player::Fixed, None) },
            ];
            if shots.iter().any(Option::is_none) {
                log!("gun outputs: Transformers shot sites: another build, not hooked");
            }
            let mut damage: [Falling; 2] = Default::default();
            poll("transformers human alliance", 16, move || {
                let o = mem::u32(b + 0x1A3CBA0) as usize + 8;
                let (o1, o2) = (mem::u8(o), mem::u8(o + 1));
                set("P1_LmpStart", bit(o1, 7));
                set("P2_LmpStart", bit(o1, 4));
                set("P1_LmpGun", bit(o1, 1));
                set("P2_LmpGun", bit(o1, 0));
                set("LmpBillboard", bit(o2, 2));
                set("P1_LmpBreak", bit(o2, 7));
                set("P2_LmpBreak", bit(o2, 6));
                set("P1_GunMotor", bit(o1, 6));
                set("P2_GunMotor", bit(o1, 3));
                let players = mem::u32(b + 0x1A3CC1C) as usize;
                for p in 0..2 {
                    if shots[p].is_some_and(|f| f.take(0)) {
                        wal_payload_common::outputs::pulse(["P1_CtmRecoil", "P2_CtmRecoil"][p], 50);
                    }
                    let rec = if players != 0 { mem::u32(players + 0x34 + 4 * p) as usize } else { 0 };
                    let l = (rec != 0 && matches!(mem::u8(rec + 0x5C), 3 | 4)).then(|| mem::u8(rec + 0x60) as i64);
                    damage[p].update(l, ["P1_Damaged", "P2_Damaged"][p]);
                    set(["P1_Life", "P2_Life"][p], l.unwrap_or(0) as i32);
                }
                set("Credits", mem::u8(b + 0x964E60) as i32);
            });
        }
        _ => {}
    }
}
