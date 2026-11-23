//! Far Cry Paradise Lost (`FarCry_r.exe`): its network outputs (`WAL_OUTPUTS`), DemulShooter's
//! code caves (`Game_GvrFarCry.cs`, the same build) as stubs (`outputs::hook`):
//!
//! * a player leaving / joining (+0xCBA22 / +0xCBDA3, the player in ebp): playing, the start
//!   lamp blinking (500 ms) until then (`P1_CtmLmpStart`);
//! * the life compared (+0x125D82: edx, esi 1 player 2): `P1_Life`, a hit when it goes down;
//! * the guns' kill LEDs written (+0x3AEBA4: the player at `[esp+0x1C]`, the digits at
//!   `[esp+5]` / `[esp+6]` past the code's two pops): `P1_LedKills1` / `2`;
//! * the guns' rumble written (+0x3AEA6D, the gun at `[esp+0x10]`), on each shot and grenade:
//!   `P1_GunMotor`, the custom recoil.

use wal_payload_common::log;

pub(crate) fn init() {
    if !wal_payload_common::outputs::enabled() || !wal_payload_common::patches::exe_name().eq_ignore_ascii_case("FarCry_r.exe") {
        return;
    }
    outputs();
}

fn outputs() {
    use wal_payload_common::outputs::hook::stub;
    use wal_payload_common::outputs::{Falling, mem, poll, set};
    let b = mem::base();
    let hook = |name: &str, rva: usize, original: &[u8], code: &dyn Fn(u32) -> Vec<u8>| {
        let s = unsafe { stub(b + rva, original, |slots| code(slots as u32)) };
        if s.is_none() {
            log!("farcry: outputs: {name} at {rva:#x}: another build, not hooked");
        }
        s
    };
    // pushfd; cmp ebp, 1; ja; mov byte [ebp+slots], on; popfd
    let playing_code = |on: u8| move |slots: u32| [&[0x9C, 0x83, 0xFD, 0x01, 0x77, 0x07, 0xC6, 0x85][..], &slots.to_le_bytes(), &[on, 0x9D]].concat();
    // both hooks write the same flags: the first one's slots
    let leave = hook("leave", 0xCBA22, &[0x8B, 0x44, 0x24, 0x1C, 0x80, 0x48, 0x18, 0x01], &playing_code(0));
    let Some(players) = leave else { return };
    let join_code = move |_: u32| playing_code(1)(players.0 as u32);
    hook("join", 0xCBDA3, &[0x8B, 0x44, 0x24, 0x1C, 0x80, 0x60, 0x18, 0xFE], &join_code);
    // pushfd; cmp esi, 1; je; mov [slots], edx; jmp; mov [slots+4], edx; popfd
    let life = hook("life", 0x125D82, &[0x3B, 0x94, 0x8F, 0x10, 0x06, 0x00, 0x00], &|slots| {
        [&[0x9C, 0x83, 0xFE, 0x01, 0x74, 0x08, 0x89, 0x15][..], &slots.to_le_bytes(), &[0xEB, 0x06, 0x89, 0x15], &(slots + 4).to_le_bytes(), &[0x9D]].concat()
    });
    // pushfd; push ebx (esp 16 lower than past the code's pops); cmp dword [esp+2C], 1; je; the
    // digits [esp+15] / [esp+16] to slots + 4 × player; pop ebx; popfd
    let kills = hook("kills", 0x3AEBA4, &[0x83, 0xF8, 0xFF, 0x5F, 0x5E], &|slots| {
        let digits = |at: u32| [&[0x8A, 0x5C, 0x24, 0x15, 0x88, 0x1D][..], &at.to_le_bytes(), &[0x8A, 0x5C, 0x24, 0x16, 0x88, 0x1D], &(at + 1).to_le_bytes()].concat();
        [&[0x9C, 0x53, 0x83, 0x7C, 0x24, 0x2C, 0x01, 0x74, 0x16][..], &digits(slots), &[0xEB, 0x14], &digits(slots + 4), &[0x5B, 0x9D]].concat()
    });
    // pushfd; cmp dword [esp+14], 1; je; mov dword [slots], 1; jmp; mov dword [slots+4], 1; popfd
    let rumble = hook("rumble", 0x3AEA6D, &[0x8B, 0x04, 0x95, 0x00, 0x95, 0xA4, 0x00], &|slots| {
        [&[0x9C, 0x83, 0x7C, 0x24, 0x14, 0x01, 0x74, 0x0C, 0xC7, 0x05][..], &slots.to_le_bytes(), &[1, 0, 0, 0, 0xEB, 0x0A, 0xC7, 0x05], &(slots + 4).to_le_bytes(), &[1, 0, 0, 0, 0x9D]].concat()
    });
    log!("farcry: outputs");
    let mut damage: [Falling; 2] = Default::default();
    poll("far cry paradise lost", 16, move || {
        let blink = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() / 500 % 2) as i32;
        for p in 0..2 {
            let n = |f: &str| format!("P{}_{f}", p + 1);
            let playing = mem::u8(players.0 + p) == 1;
            let l = playing.then(|| life.map_or(0, |s| (s.get(p) as i32).max(0)) as i64);
            damage[p].update(l, ["P1_Damaged", "P2_Damaged"][p]);
            set(&n("Life"), l.unwrap_or(0) as i32);
            set(&n("CtmLmpStart"), if playing { 0 } else { blink });
            if let Some(k) = kills {
                set(&n("LedKills1"), mem::u8(k.0 + 4 * p) as i32);
                set(&n("LedKills2"), mem::u8(k.0 + 4 * p + 1) as i32);
            }
            let shook = rumble.is_some_and(|r| r.take(p));
            set(&n("GunMotor"), shook as i32);
            if shook {
                set(&n("CtmRecoil"), 1);
            }
        }
    });
}
