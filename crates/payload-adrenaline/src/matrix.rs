//! The Matrix dongle (`matrix32.dll`, `matrix64.dll`, stdcall) of Adrenaline's games.
//!
//! - Skylanders Cloud Patrol: `Matrix.GetUserId` (the online services' device id):
//!   `Init_MatrixAPI`, `Dongle_Count`, `Dongle_ReadSerNr`, `Release_MatrixAPI`: one dongle, its
//!   serial.
//! - Rabbids Hollywood: `MatrixManager` finds the dongle the same way, then derives its save
//!   files' AES key with `Dongle_DecryptData` (8-byte blocks, the dongle's own key) and checks
//!   it against an MD5: the dongle's key is unknown, so the game uses a random key each run (the
//!   saves are not read back). Encrypting and decrypting is XTEA with a fixed key here.
//! - Tomb Raider: the same (Sarbakan's `SBK.Encryption.DataTransform`: its `KEY` and `IV`
//!   decrypted by the dongle, checked against `m_KeyHash` and `m_VIHash`). At the first
//!   `Dongle_DecryptData` (the game's key thread, before its check) the payload writes the MD5s
//!   of our decryption of `KEY` and `IV` into those hashes: the check passes with a key of its
//!   own, the same every run, and the saves (operator settings, free play) are read back.

const DONGLE_SERIAL: i32 = 0x0057_414C;
const KEY: [u32; 4] = [0x5741_4C20, 0x4D61_7472, 0x6978_2044, 0x6F6E_676C];
const DELTA: u32 = 0x9E37_79B9;

extern "system" fn init_matrix_api() -> i16 {
    0
}

extern "system" fn release_matrix_api() -> i16 {
    0
}

extern "system" fn dongle_count(_port: i16) -> i16 {
    1
}

extern "system" fn dongle_read_ser_nr(_user_code: i32, _dongle: i16, _port: i16) -> i32 {
    DONGLE_SERIAL
}

/// `Dongle_ReadData(user code, out data, count, dongle, port)`: zeros, the count read.
extern "system" fn dongle_read_data(_user_code: i32, data: *mut i32, count: i16, _dongle: i16, _port: i16) -> i16 {
    if !data.is_null() && count > 0 {
        unsafe { std::ptr::write_bytes(data, 0, count as usize) };
    }
    count
}

extern "system" fn dongle_encrypt(_user_code: i32, block: *mut [u32; 2], _dongle: i16, _port: i16) -> i16 {
    if block.is_null() {
        return -1;
    }
    let [mut v0, mut v1] = unsafe { *block };
    let mut sum = 0u32;
    for _ in 0..32 {
        v0 = v0.wrapping_add((((v1 << 4) ^ (v1 >> 5)).wrapping_add(v1)) ^ sum.wrapping_add(KEY[(sum & 3) as usize]));
        sum = sum.wrapping_add(DELTA);
        v1 = v1.wrapping_add((((v0 << 4) ^ (v0 >> 5)).wrapping_add(v0)) ^ sum.wrapping_add(KEY[((sum >> 11) & 3) as usize]));
    }
    unsafe { *block = [v0, v1] };
    0
}

fn decrypt([mut v0, mut v1]: [u32; 2]) -> [u32; 2] {
    let mut sum = DELTA.wrapping_mul(32);
    for _ in 0..32 {
        v1 = v1.wrapping_sub((((v0 << 4) ^ (v0 >> 5)).wrapping_add(v0)) ^ sum.wrapping_add(KEY[((sum >> 11) & 3) as usize]));
        sum = sum.wrapping_sub(DELTA);
        v0 = v0.wrapping_sub((((v1 << 4) ^ (v1 >> 5)).wrapping_add(v1)) ^ sum.wrapping_add(KEY[(sum & 3) as usize]));
    }
    [v0, v1]
}

extern "system" fn dongle_decrypt(_user_code: i32, block: *mut [u32; 2], _dongle: i16, _port: i16) -> i16 {
    if block.is_null() {
        return -1;
    }
    save_key_hashes();
    unsafe { *block = decrypt(*block) };
    0
}

/// Tomb Raider's save key check made to pass (once, from the game's managed key thread):
/// `DataTransform.m_KeyHash` / `m_VIHash` set to the MD5s of `KEY` / `IV` as our dongle decrypts
/// them (`MatrixManager.DecryptWithDongle`: big-endian 8-byte blocks).
fn save_key_hashes() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use wal_payload_common::mono::{self, ARRAY_DATA, mono};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.load(Ordering::Relaxed) {
        return;
    }
    let Some(m) = mono() else { return };
    let class = m.class(m.image("Assembly-CSharp"), "SBK.Encryption", "DataTransform");
    if class.is_null() {
        DONE.store(true, Ordering::Relaxed);
        return;
    }
    // a `byte[]` static: its data and length
    let array = |name: &str| -> Option<(*mut u8, usize)> {
        let at = m.static_addr(class, m.field(class, name));
        let obj = if at.is_null() { 0 } else { unsafe { mono::read::<usize>(at, 0) } };
        (obj != 0).then(|| unsafe { ((obj as *mut u8).add(ARRAY_DATA), mono::read::<usize>(obj as *const u8, 3 * size_of::<usize>())) })
    };
    let mut patched = 0;
    for (data, hash) in [("KEY", "m_KeyHash"), ("IV", "m_VIHash")] {
        let (Some((data, len)), Some((hash, 16))) = (array(data), array(hash)) else { continue };
        let mut bytes = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
        for b in bytes.chunks_exact_mut(8) {
            let v = decrypt([u32::from_be_bytes(b[..4].try_into().unwrap()), u32::from_be_bytes(b[4..].try_into().unwrap())]);
            b[..4].copy_from_slice(&v[0].to_be_bytes());
            b[4..].copy_from_slice(&v[1].to_be_bytes());
        }
        let sum = wal_easrv::kbin::md5(&bytes);
        unsafe { std::ptr::copy_nonoverlapping(sum.as_ptr(), hash, 16) };
        patched += 1;
    }
    if patched == 2 {
        DONE.store(true, Ordering::Relaxed);
        wal_payload_common::log!("matrix: save key hashes set (saves kept across runs)");
    }
}

/// The dongle's functions.
pub fn resolve(name: &str) -> Option<usize> {
    let f = match name {
        "Init_MatrixAPI" => init_matrix_api as *const () as usize,
        "Release_MatrixAPI" | "Dongle_Exit" => release_matrix_api as *const () as usize,
        "Dongle_Count" => dongle_count as *const () as usize,
        "Dongle_ReadSerNr" => dongle_read_ser_nr as *const () as usize,
        "Dongle_ReadData" => dongle_read_data as *const () as usize,
        "Dongle_EncryptData" => dongle_encrypt as *const () as usize,
        "Dongle_DecryptData" => dongle_decrypt as *const () as usize,
        _ => return None,
    };
    Some(f)
}
