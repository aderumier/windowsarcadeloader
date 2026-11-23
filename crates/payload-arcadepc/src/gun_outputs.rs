//! The arcade PC gun games' outputs for the network outputs (`WAL_OUTPUTS`), read in the game as
//! DemulShooter does, at its addresses for the builds it knows (the dumps' executables have its
//! MD5s, checked by their link time); `P1_CtmRecoil` when a player's ammo goes down (or shot
//! count up), `P1_Damaged` the life.
//!
//! * Akuma Mortis Immortal (`ngun`): the players' record +0x190CE8: output byte +0x78 (start
//!   lamps bits 0 / 1, left / right 4 / 5, the guns' recoil 6 / 7: `P1_GunRecoil`), in game
//!   +0x7E / +0x7F; shot counts +0x5E3B48 / +0x5E3BDC, life +0x5E3B08 / +0x5E3B9C; credits
//!   +0x190F02.
//! * Bug Busters: start lamps +0x9553C / +0x95540, the air blowers +0x95418 / +0x9541C
//!   (`P1_AirFront`, `P1_GunMotor` while blowing); in a game (mode +0x60BF8 5) the players
//!   playing (+0x60C20 / 24) ammo and life (floats × 100) +0x5509C / +0x550A0, +0x55094 /
//!   +0x55098; credits the coins +0x60C1C / the cost +0x55090 (0: free play).
//! * The House of the Dead 3: the players' state +0x3B8038 / +0x3B8264 (5 playing; else the
//!   start lamp blinks: `P1_CtmLmpStart`), life +0x3B8044 / +0x3B8270, ammo +0x3B8090 /
//!   +0x3B82BC; credits +0x3B7DD0.
//! * Robin Hood: the arrows shot +0x9C616C (scene +0x9C6138 0: a recoil each); credits
//!   +0x9B32FC.
//! * Plants vs Zombies: The Last Stand (Shell.exe 1.0.0): output byte +0x118AC0 (the gun's red /
//!   green / blue bits 0..2, its recoil 3: `P1_GunRecoil`, the cabinet's red / green 4 / 5), +1
//!   bit 1 the ticket motor; credits +0x10AF80.
//! * Mad Bullets (1.0.2, 1 player): playing [+0x1D55E8] + 0x1AC, ammo [+0x1D2890] + 0xD7, life
//!   [+0x1D28C0] + 0x24; DemulShooter's code caves as player flags: the shot (+0x4FFA2, `cmp
//!   dword [edi + 0xF8], 0`) and the hit (+0xB1E92, `mov al, [ecx + 0x189]`).
//! * The House of the Dead: Overkill (HOTD_NG.exe, 1 player): playing +0x59C378, the player's
//!   record [[+0x599688] + 0x24]: life +0x20C, ammo +0x28C; the shot (+0x118C5F, `cmp [esi +
//!   0x7A0], bl`) and the hit (+0x18B399, `mov [esi + 0x20C], edi`); credits [[+0x5996C8] +
//!   0x228] + 0x17C.

use std::time::Instant;

use wal_payload_common::outputs::{Falling, mem, poll_build, set};

fn bit(v: u8, b: u8) -> i32 {
    (v >> b & 1) as i32
}

/// A player's ammo and life this read (`None`: not playing), their outputs and pulses.
fn player(p: usize, ammo: Option<i64>, life: Option<i64>, recoil: &mut Falling, damage: &mut Falling) {
    recoil.update(ammo, ["P1_CtmRecoil", "P2_CtmRecoil"][p]);
    damage.update(life, ["P1_Damaged", "P2_Damaged"][p]);
    set(["P1_Ammo", "P2_Ammo"][p], ammo.unwrap_or(0) as i32);
    set(["P1_Clip", "P2_Clip"][p], (ammo.unwrap_or(0) > 0) as i32);
    set(["P1_Life", "P2_Life"][p], life.unwrap_or(0) as i32);
}

pub(crate) fn init(board: &str) {
    if !wal_payload_common::outputs::enabled() {
        return;
    }
    match board {
        "ngun" => {
            // the shot count goes up: negated, a fall
            let mut shots: [Falling; 2] = Default::default();
            let mut damage: [Falling; 2] = Default::default();
            poll_build("akuma mortis immortal", 0x421F25EA, 16, move || {
                let b = mem::base();
                let info = b + 0x190CE8;
                let o = mem::u8(info + 0x78);
                set("P1_LmpStart", bit(o, 0));
                set("P2_LmpStart", bit(o, 1));
                set("LmpLeft", bit(o, 4));
                set("LmpRight", bit(o, 5));
                set("P1_GunRecoil", bit(o, 6));
                set("P2_GunRecoil", bit(o, 7));
                for (p, (count, life)) in [(0x5E3B48, 0x5E3B08), (0x5E3BDC, 0x5E3B9C)].into_iter().enumerate() {
                    let playing = mem::u8(info + 0x7E + p) == 1;
                    shots[p].update(playing.then(|| -(mem::u32(b + count) as i32 as i64)), ["P1_CtmRecoil", "P2_CtmRecoil"][p]);
                    let l = playing.then(|| mem::u32(b + life) as i32 as i64);
                    damage[p].update(l, ["P1_Damaged", "P2_Damaged"][p]);
                    set(["P1_Life", "P2_Life"][p], l.unwrap_or(0) as i32);
                }
                set("Credits", mem::u32(b + 0x190F02) as i32);
            });
        }
        "bugbusters" => {
            let mut recoil: [Falling; 2] = Default::default();
            let mut damage: [Falling; 2] = Default::default();
            poll_build("bug busters", 0x3D74A9DF, 16, move || {
                let b = mem::base();
                set("P1_LmpStart", mem::u8(b + 0x9553C) as i32);
                set("P2_LmpStart", mem::u8(b + 0x95540) as i32);
                for (p, air) in [0x95418, 0x9541C].into_iter().enumerate() {
                    let a = mem::u8(b + air);
                    set(["P1_AirFront", "P2_AirFront"][p], a as i32);
                    set(["P1_GunMotor", "P2_GunMotor"][p], (a != 0) as i32);
                }
                let in_game = mem::u8(b + 0x60BF8) == 5;
                for (p, (playing, ammo, life)) in [(0x60C20, 0x5509C, 0x55094), (0x60C24, 0x550A0, 0x55098)].into_iter().enumerate() {
                    let on = in_game && mem::u8(b + playing) == 1;
                    let a = on.then(|| (mem::f32(b + ammo) * 100.0) as i64);
                    let l = on.then(|| ((mem::f32(b + life) * 100.0) as i64).max(0));
                    player(p, a, l, &mut recoil[p], &mut damage[p]);
                }
                let cost = mem::u32(b + 0x55090);
                set("Credits", if cost == 0 { 0 } else { (mem::u32(b + 0x60C1C) / cost) as i32 });
            });
        }
        "hotd3" => {
            let mut recoil: [Falling; 2] = Default::default();
            let mut damage: [Falling; 2] = Default::default();
            let start = Instant::now();
            poll_build("the house of the dead 3", 0x41AA8B27, 16, move || {
                let b = mem::base();
                // a start lamp blinks (twice a second) while its player is not playing
                let blink = (start.elapsed().as_millis() / 500 % 2) as i32;
                for (p, (state, life, ammo)) in [(0x3B8038, 0x3B8044, 0x3B8090), (0x3B8264, 0x3B8270, 0x3B82BC)].into_iter().enumerate() {
                    let playing = mem::u8(b + state) == 5;
                    set(["P1_CtmLmpStart", "P2_CtmLmpStart"][p], if playing { 0 } else { blink });
                    let a = playing.then(|| mem::u8(b + ammo) as i64);
                    let l = playing.then(|| mem::u8(b + life) as i64);
                    player(p, a, l, &mut recoil[p], &mut damage[p]);
                }
                set("Credits", mem::u8(b + 0x3B7DD0) as i32);
            });
        }
        "robinhood" => {
            let mut arrows = Falling::default();
            poll_build("robin hood", 0x4DD6AD91, 16, move || {
                let b = mem::base();
                let shooting = mem::u8(b + 0x9C6138) == 0;
                arrows.update(shooting.then(|| -(mem::u32(b + 0x9C616C) as i32 as i64)), "P1_CtmRecoil");
                set("Credits", mem::u32(b + 0x9B32FC) as i32);
            });
        }
        "pvz" => {
            poll_build("plants vs zombies", 0x531EEC4E, 16, move || {
                let b = mem::base();
                let o = mem::u8(b + 0x118AC0);
                set("P1_LmpGun_R", bit(o, 0));
                set("P1_LmpGun_G", bit(o, 1));
                set("P1_LmpGun_B", bit(o, 2));
                set("P1_GunRecoil", bit(o, 3));
                set("P1_CtmRecoil", bit(o, 3));
                set("P1_Lmp_R", bit(o, 4));
                set("P1_Lmp_G", bit(o, 5));
                set("TicketDrive", bit(mem::u8(b + 0x118AC1), 1));
                set("Credits", mem::u8(b + 0x10AF80) as i32);
            });
        }
        #[cfg(target_arch = "x86")]
        "madbullets" => {
            let b = mem::base();
            let shot = cave("shot", b + 0x4FFA2, &[0x83, 0xBF, 0xF8, 0x00, 0x00, 0x00, 0x00]);
            let hit = cave("hit", b + 0xB1E92, &[0x8A, 0x81, 0x89, 0x01, 0x00, 0x00]);
            poll_build("mad bullets", 0x57762508, 16, move || {
                let playing = mem::u8(mem::u32(b + 0x1D55E8) as usize + 0x1AC) != 0;
                let ammo = if playing { mem::u8(mem::u32(b + 0x1D2890) as usize + 0xD7) as i32 } else { 0 };
                let life = if playing { mem::u8(mem::u32(b + 0x1D28C0) as usize + 0x24) as i32 } else { 0 };
                flags(shot, hit, playing);
                set("P1_Ammo", ammo);
                set("P1_Clip", (ammo > 0) as i32);
                set("P1_Life", life);
            });
        }
        #[cfg(target_arch = "x86")]
        "hotdo" => {
            let b = mem::base();
            let shot = cave("shot", b + 0x118C5F, &[0x38, 0x9E, 0xA0, 0x07, 0x00, 0x00]);
            let hit = cave("hit", b + 0x18B399, &[0x89, 0xBE, 0x0C, 0x02, 0x00, 0x00]);
            poll_build("house of the dead overkill", 0x5310A13D, 16, move || {
                let playing = mem::u8(b + 0x59C378) != 0;
                let record = mem::u32(b + 0x599688) as usize;
                let info = if record != 0 { mem::u32(record + 0x24) as usize } else { 0 };
                let (life, ammo) = if info != 0 { (mem::u8(info + 0x20C) as i32, mem::u8(info + 0x28C) as i32) } else { (0, 0) };
                flags(shot, hit, playing);
                set("P1_Ammo", ammo);
                set("P1_Clip", (playing && ammo > 0) as i32);
                set("P1_Life", life);
                let credits = mem::chain(b + 0x5996C8, &[0x228]) as usize;
                set("Credits", if credits != 0 { mem::u8(credits + 0x17C) as i32 } else { 0 });
            });
        }
        _ => {}
    }
}

/// A DemulShooter code cave as a player flag (player 1), logged when the build differs.
#[cfg(target_arch = "x86")]
fn cave(name: &str, site: usize, original: &[u8]) -> Option<wal_payload_common::outputs::hook::Flags> {
    let f = unsafe { wal_payload_common::outputs::hook::player_flag(site, original, wal_payload_common::outputs::hook::Player::Fixed, None) };
    if f.is_none() {
        wal_payload_common::log!("gun outputs: {name} at {site:#x}: another build, not hooked");
    }
    f
}

/// Player 1's shot and hit flags: `P1_CtmRecoil`, `P1_Damaged` while playing (taken anyway).
#[cfg(target_arch = "x86")]
fn flags(shot: Option<wal_payload_common::outputs::hook::Flags>, hit: Option<wal_payload_common::outputs::hook::Flags>, playing: bool) {
    if shot.is_some_and(|f| f.take(0)) && playing {
        wal_payload_common::outputs::pulse("P1_CtmRecoil", 50);
    }
    if hit.is_some_and(|f| f.take(0)) && playing {
        wal_payload_common::outputs::pulse("P1_Damaged", 50);
    }
}
