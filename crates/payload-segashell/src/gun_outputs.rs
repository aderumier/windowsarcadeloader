//! Transformers: Shadows Rising's outputs for the network outputs (`WAL_OUTPUTS`), DemulShooter's
//! code caves for the build it knows (Transformers2.exe v180605, told by its link time) as
//! `outputs::hook` stubs:
//!
//! * the start lamps: the values of edi / ebx at the lamps' call (+0x67159): `P1_LmpStart`,
//!   `P2_LmpStart`;
//! * the gun lamps: the two arguments of CCreditMgr::ExecServer's blank lamp call (+0xFC124):
//!   `P1_LmpGun`, `P2_LmpGun`;
//! * a bullet fired (+0x24AAFF, the player the byte at esi + 0x60): `P1_CtmRecoil`;
//! * a hit (+0x246C69, the player the byte at edi + 0x60): `P1_Damaged`.

#[cfg(target_arch = "x86")]
pub(crate) fn init() {
    use wal_payload_common::outputs::hook::{Player, player_flag, stub};
    use wal_payload_common::outputs::{mem, poll, pulse, set};
    use wal_payload_common::log;

    if !wal_payload_common::outputs::enabled() || mem::timestamp() != 0x5B15074C {
        return;
    }
    let b = mem::base();
    let hooked = |name: &str, h: Option<wal_payload_common::outputs::hook::Slots>| {
        if h.is_none() {
            log!("gun outputs: {name}: another build, not hooked");
        }
        h
    };
    let start = hooked("start lamps", unsafe {
        // mov [slots], edi; mov [slots + 4], ebx
        stub(b + 0x67159, &[0xE8, 0x62, 0xC3, 0x1D, 0x00], |s| {
            let mut c = vec![0x89, 0x3D];
            c.extend_from_slice(&(s as u32).to_le_bytes());
            c.extend_from_slice(&[0x89, 0x1D]);
            c.extend_from_slice(&(s as u32 + 4).to_le_bytes());
            c
        })
    });
    let gun = hooked("gun lamps", unsafe {
        // push eax; mov eax, [esp + 4]; mov [slots], eax; mov eax, [esp + 8]; mov [slots + 4], eax; pop eax
        stub(b + 0xFC124, &[0xE8, 0x67, 0x73, 0x14, 0x00], |s| {
            let mut c = vec![0x50, 0x8B, 0x44, 0x24, 0x04, 0xA3];
            c.extend_from_slice(&(s as u32).to_le_bytes());
            c.extend_from_slice(&[0x8B, 0x44, 0x24, 0x08, 0xA3]);
            c.extend_from_slice(&(s as u32 + 4).to_le_bytes());
            c.push(0x58);
            c
        })
    });
    let recoil = hooked("recoil", unsafe { player_flag(b + 0x24AAFF, &[0x0F, 0xBE, 0x46, 0x60, 0x6A, 0x00], Player::ByteAt(6, 0x60), None) });
    let damage = hooked("damage", unsafe { player_flag(b + 0x246C69, &[0x88, 0x45, 0x15, 0x8D, 0x45, 0x14], Player::ByteAt(7, 0x60), None) });
    log!("gun outputs: Transformers: Shadows Rising");
    poll("transformers shadows rising", 16, move || {
        if let Some(s) = start {
            set("P1_LmpStart", s.get(0) as u8 as i32);
            set("P2_LmpStart", s.get(1) as u8 as i32);
        }
        if let Some(s) = gun {
            set("P1_LmpGun", s.get(0) as u8 as i32);
            set("P2_LmpGun", s.get(1) as u8 as i32);
        }
        for p in 0..2 {
            if recoil.is_some_and(|f| f.take(p)) {
                pulse(["P1_CtmRecoil", "P2_CtmRecoil"][p], 50);
            }
            if damage.is_some_and(|f| f.take(p)) {
                pulse(["P1_Damaged", "P2_Damaged"][p], 50);
            }
        }
    });
}

#[cfg(not(target_arch = "x86"))]
pub(crate) fn init() {}
