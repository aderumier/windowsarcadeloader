//! The Konami gun games' outputs for the network outputs (`WAL_OUTPUTS`), read in the game as
//! DemulShooter does, at its addresses for the builds it knows (the dumps' executables have its
//! MD5s, checked by their link time); `P1_CtmRecoil` when a player's ammo goes down, `P1_Damaged` the life.
//!
//! * Castlevania: The Arcade (HCV.exe 2009-04-22): 12 lamps at +0x331F24, a dword each (on:
//!   the level at the address - 0x30): start, card reader red / green, whip red / green / blue,
//!   each player's; life (dword, 10000 when not playing) +0x326B48 / +0x326D08, ammo +0x326B58 /
//!   +0x326D18.
//! * Cooper's 9: output byte +0x1FE7F48 (start lamps bits 0 / 1, the guns' recoil bits 6 / 7:
//!   `P1_GunRecoil`); life +0x24FCD10 / +0x24FCDD8 and ammo +0x24FCDC2 / +0x24FCE8A while
//!   +0x1FE7F4E / +0x1FE7F4F is 1; credits +0x1FE8188.
//! * Wartran Troopers (4 players): start lamps +0x149E37C (bits 3..0); life +0x130600C (each
//!   player + 0x1FC); the weapon record +0x1475A90 (each player + 0x24): weapon +0xE, its ammo
//!   +0x10 + weapon; credits +0x1AC55C.
//! * Silent Hill: The Arcade (KSHG.exe): lamps (+0x2E38C4 start P1, +0x2E38CC P2, card reader
//!   +0x2E38D4..EC, left / right +0x2E38F4 / FC), credits +0x2F9008; the recoil is the game's
//!   `shaiolib_GUN_reaction(player)` (`shg.rs`).

use wal_payload_common::outputs::{Falling, mem, poll_build, set};

const P: [&str; 4] = ["P1", "P2", "P3", "P4"];
const RECOIL: [&str; 4] = ["P1_CtmRecoil", "P2_CtmRecoil", "P3_CtmRecoil", "P4_CtmRecoil"];
const DAMAGED: [&str; 4] = ["P1_Damaged", "P2_Damaged", "P3_Damaged", "P4_Damaged"];

/// A player's ammo and life this read (`None`: not playing), their outputs and pulses.
fn player(p: usize, ammo: Option<i64>, life: Option<i64>, recoil: &mut Falling, damage: &mut Falling) {
    recoil.update(ammo, RECOIL[p]);
    damage.update(life, DAMAGED[p]);
    set(&format!("{}_Ammo", P[p]), ammo.unwrap_or(0) as i32);
    set(&format!("{}_Clip", P[p]), (ammo.unwrap_or(0) > 0) as i32);
    set(&format!("{}_Life", P[p]), life.unwrap_or(0) as i32);
}

pub fn castlevania() {
    let mut recoil: [Falling; 2] = Default::default();
    let mut damage: [Falling; 2] = Default::default();
    poll_build("castlevania", 0x49C9AF26, 16, move || {
        let b = mem::base();
        let o = b + 0x331F24;
        let lamps = ["P1_LmpStart", "P2_LmpStart", "P1_LmpCard_R", "P1_LmpCard_G", "P2_LmpCard_R", "P2_LmpCard_G", "P1_Whip_R", "P1_Whip_G", "P1_Whip_B", "P2_Whip_R", "P2_Whip_G", "P2_Whip_B"];
        for (i, name) in lamps.iter().enumerate() {
            let at = o + 4 * i;
            set(name, if mem::u8(at) != 0 { mem::u8(at - 0x30) as i32 } else { 0 });
        }
        for (p, (life, ammo)) in [(0x326B48, 0x326B58), (0x326D08, 0x326D18)].into_iter().enumerate() {
            let l = mem::u32(b + life) as i32;
            let playing = l != 10000;
            let a = (playing && l > 0).then(|| mem::u8(b + ammo) as i64);
            player(p, a, playing.then_some(l as i64), &mut recoil[p], &mut damage[p]);
        }
    });
}

pub fn coopers9() {
    let mut recoil: [Falling; 2] = Default::default();
    let mut damage: [Falling; 2] = Default::default();
    poll_build("cooper's 9", 0x43B40DA3, 16, move || {
        let b = mem::base();
        let o = mem::u8(b + 0x1FE7F48);
        set("P1_LmpStart", (o & 1) as i32);
        set("P2_LmpStart", (o >> 1 & 1) as i32);
        set("P1_GunRecoil", (o >> 6 & 1) as i32);
        set("P2_GunRecoil", (o >> 7 & 1) as i32);
        for (p, (life, ammo)) in [(0x24FCD10, 0x24FCDC2), (0x24FCDD8, 0x24FCE8A)].into_iter().enumerate() {
            let playing = mem::u8(b + 0x1FE7F4E + p) == 1;
            let l = playing.then(|| mem::u8(b + life) as i64);
            let a = l.filter(|l| *l > 0).map(|_| mem::u8(b + ammo) as i64);
            player(p, a, l, &mut recoil[p], &mut damage[p]);
        }
        set("Credits", mem::u8(b + 0x1FE8188) as i32);
    });
}

pub fn wartran() {
    let mut recoil: [Falling; 4] = Default::default();
    let mut damage: [Falling; 4] = Default::default();
    let mut weapon = [0u8; 4];
    poll_build("wartran troopers", 0x4003CE1B, 16, move || {
        let b = mem::base();
        let lamps = mem::u8(b + 0x149E37C);
        for p in 0..4 {
            set(&format!("{}_LmpStart", P[p]), (lamps >> (3 - p) & 1) as i32);
            let l = mem::u8(b + 0x130600C + 0x1FC * p);
            let w_rec = b + 0x1475A90 + 0x24 * p;
            let w = mem::u8(w_rec + 0xE);
            // the ammo of the weapon held since the last read (a change of weapon is no shot)
            let a = (l > 0 && w == weapon[p]).then(|| mem::u8(w_rec + 0x10 + w as usize) as i64);
            weapon[p] = w;
            player(p, a, Some(l as i64), &mut recoil[p], &mut damage[p]);
        }
        set("Credits", mem::u8(b + 0x1AC55C) as i32);
    });
}

pub fn silent_hill() {
    poll_build("silent hill", 0x476A090A, 16, move || {
        let b = mem::base();
        for (name, at) in [
            ("P1_LmpStart", 0x2E38C4),
            ("P2_LmpStart", 0x2E38CC),
            ("P1_LmpCard_R", 0x2E38D4),
            ("P1_LmpCard_G", 0x2E38DC),
            ("P2_LmpCard_R", 0x2E38E4),
            ("P2_LmpCard_G", 0x2E38EC),
            ("LmpLeft", 0x2E38F4),
            ("LmpRight", 0x2E38FC),
        ] {
            set(name, mem::u8(b + at) as i32);
        }
        set("Credits", mem::u8(b + 0x2F9008) as i32);
    });
}
