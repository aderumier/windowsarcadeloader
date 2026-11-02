//! The Matrix dongle (`matrix32.dll`, `matrix64.dll`, stdcall) of Adrenaline's games.
//!
//! - Skylanders Cloud Patrol: `Matrix.GetUserId` (the online services' device id):
//!   `Init_MatrixAPI`, `Dongle_Count`, `Dongle_ReadSerNr`, `Release_MatrixAPI`: one dongle, its
//!   serial.
//! - Rabbids Hollywood: `MatrixManager` finds the dongle the same way, then derives its save
//!   files' AES key with `Dongle_DecryptData` (8-byte blocks, the dongle's own key) and checks
//!   it against an MD5: the dongle's key is unknown, so the game uses a random key each run (the
//!   saves are not read back). Encrypting and decrypting is XTEA with a fixed key here.

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

extern "system" fn dongle_decrypt(_user_code: i32, block: *mut [u32; 2], _dongle: i16, _port: i16) -> i16 {
    if block.is_null() {
        return -1;
    }
    let [mut v0, mut v1] = unsafe { *block };
    let mut sum = DELTA.wrapping_mul(32);
    for _ in 0..32 {
        v1 = v1.wrapping_sub((((v0 << 4) ^ (v0 >> 5)).wrapping_add(v0)) ^ sum.wrapping_add(KEY[((sum >> 11) & 3) as usize]));
        sum = sum.wrapping_sub(DELTA);
        v0 = v0.wrapping_sub((((v1 << 4) ^ (v1 >> 5)).wrapping_add(v1)) ^ sum.wrapping_add(KEY[(sum & 3) as usize]));
    }
    unsafe { *block = [v0, v1] };
    0
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
