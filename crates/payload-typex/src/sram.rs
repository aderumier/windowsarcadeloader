//! Backup SRAM board of the Taito medal games (TXE001, New Super Mario Bros. Wii Coin World).
//!
//! The game imports `TxedLap.dll` (C++ names, cdecl), a layer over the board's driver
//! (`txedctl.dll`, DeviceIoControl): `TXE001_Open` copies the SRAM into memory, `Read`/`Write`
//! copy bytes at an offset. Without the board `Open` fails and the game stays on "checking the
//! backup RAM" (its backup thread only starts when the board opened). The imports are
//! answered here, the SRAM kept in `txe001-sram.bin` of the game's data folder (32 KB, the
//! size of the game's backup buffer), written through on every write.
//!
//! `Read`/`Write(offset, count, buffer, width)`: `count` bytes, or with `count` 1 one item of
//! `width` 0/2/4 (1, 2, 4 bytes, aligned). Results: 0 ok, 4 not open, 5 bad parameter.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::Mutex;

use wal_payload_common::{drive, iat, log};

const SIZE: usize = 0x8000;
const OK: i32 = 0;
const NOT_OPEN: i32 = 4;
const BAD_PARAMETER: i32 = 5;

struct Sram {
    data: Vec<u8>,
    file: Option<File>,
}

static SRAM: Mutex<Option<Sram>> = Mutex::new(None);

fn path() -> String {
    format!("{}\\txe001-sram.bin", drive::data_dir())
}

unsafe extern "C" fn open(_flags: i32) -> i32 {
    let path = path();
    let mut data = vec![0u8; SIZE];
    let file = File::options().read(true).write(true).create(true).truncate(false).open(&path);
    let file = match file {
        Ok(mut f) => {
            let mut old = Vec::new();
            let _ = f.read_to_end(&mut old);
            let n = old.len().min(SIZE);
            data[..n].copy_from_slice(&old[..n]);
            if old.len() != SIZE {
                let _ = f.seek(SeekFrom::Start(0)).and_then(|_| f.write_all(&data)).and_then(|_| f.set_len(SIZE as u64));
            }
            Some(f)
        }
        Err(e) => {
            log!("sram: cannot open {path}: {e} (not kept)");
            None
        }
    };
    log!("sram: TXE001 opened, {SIZE} bytes in {path}");
    *SRAM.lock().unwrap() = Some(Sram { data, file });
    OK
}

unsafe extern "C" fn close() -> i32 {
    *SRAM.lock().unwrap() = None;
    OK
}

unsafe extern "C" fn get_size(size: *mut u32) -> i32 {
    if SRAM.lock().unwrap().is_none() {
        return NOT_OPEN;
    }
    unsafe { *size = SIZE as u32 };
    OK
}

/// Byte range of an access, `None` when the parameters are invalid.
fn range(offset: u32, count: u32, width: u32) -> Option<std::ops::Range<usize>> {
    let len = if count == 1 {
        let len = match width {
            0 => 1,
            2 => 2,
            4 => 4,
            _ => return None,
        };
        if offset % len != 0 {
            return None;
        }
        len
    } else {
        count
    } as usize;
    let start = offset as usize;
    (len > 0 && start.checked_add(len)? <= SIZE).then_some(start..start + len)
}

unsafe extern "C" fn read(offset: u32, count: u32, buffer: *mut u8, width: u32) -> i32 {
    let guard = SRAM.lock().unwrap();
    let Some(sram) = guard.as_ref() else { return NOT_OPEN };
    let Some(r) = range(offset, count, width) else { return BAD_PARAMETER };
    unsafe { std::ptr::copy_nonoverlapping(sram.data[r.clone()].as_ptr(), buffer, r.len()) };
    OK
}

unsafe extern "C" fn write(offset: u32, count: u32, buffer: *const u8, width: u32) -> i32 {
    let mut guard = SRAM.lock().unwrap();
    let Some(sram) = guard.as_mut() else { return NOT_OPEN };
    let Some(r) = range(offset, count, width) else { return BAD_PARAMETER };
    let bytes = unsafe { std::slice::from_raw_parts(buffer, r.len()) };
    sram.data[r.clone()].copy_from_slice(bytes);
    if let Some(f) = sram.file.as_mut()
        && let Err(e) = f.seek(SeekFrom::Start(r.start as u64)).and_then(|_| f.write_all(bytes))
    {
        log!("sram: write failed: {e}");
        sram.file = None;
    }
    OK
}

pub(crate) fn init() {
    let hooks: [(&str, usize); 5] = [
        ("?TXE001_Open@@YAHH@Z", open as *const () as usize),
        ("?TXE001_Close@@YAHXZ", close as *const () as usize),
        ("?TXE001_GetSRAMSize@@YAHPAK@Z", get_size as *const () as usize),
        ("?TXE001_Read@@YAHKKPAKK@Z", read as *const () as usize),
        ("?TXE001_Write@@YAHKKPAKK@Z", write as *const () as usize),
    ];
    let mut hooked = 0;
    for (name, f) in hooks {
        if unsafe { iat::hook("TxedLap.dll", name, f) }.is_some() {
            hooked += 1;
        }
    }
    if hooked > 0 {
        log!("sram: TxedLap.dll answered ({hooked} imports, TXE001 backup SRAM)");
    }
}
