//! The system's backup (`Backup_*`): game memory blocks the cabinet keeps (Blade Strangers: 2
//! records of 8 and 3 bytes it copies at boot). `Backup_setupRecords` takes `{address, size}` pairs; the
//! game then only reads `Backup_getRecordStatus` and changes the memory, the system saving it by
//! itself. Here: `backup/<index>.bin` in the game folder (a launcher data dir) loaded at setup
//! (status 0, valid; a size different from the file's: their common part), status 1 without a file (new: the game uses its defaults; status 0 with
//! memory it never wrote, Blade Strangers never starts a game); a record the game changed is
//! written about every 2 s from `Core_execute`, and at `Core_exitGame`.

use std::sync::Mutex;

use wal_payload_common::log;

use crate::first_call;

const DIR: &str = "backup";
/// `Core_execute` calls between two checks (~2 s at 60 fps).
const CHECK_EVERY: u32 = 120;

struct Record {
    address: usize,
    size: usize,
    status: u32,
    /// Hash of the last contents written (or loaded).
    saved: u64,
}

static RECORDS: Mutex<Vec<Record>> = Mutex::new(Vec::new());
static CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn path(index: usize) -> String {
    format!("{DIR}\\{index}.bin")
}

fn hash(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn memory(record: &Record) -> &'static mut [u8] {
    unsafe { std::slice::from_raw_parts_mut(record.address as *mut u8, record.size) }
}

/// Writes the records whose memory changed since the last write.
pub(crate) fn flush() {
    let mut records = RECORDS.lock().unwrap();
    for (i, record) in records.iter_mut().enumerate() {
        let data = memory(record);
        let h = hash(data);
        if h == record.saved {
            continue;
        }
        let _ = std::fs::create_dir_all(DIR);
        match std::fs::write(path(i), &data[..]) {
            Ok(()) => {
                record.saved = h;
                log!("backup: record {i} saved ({} bytes)", record.size);
            }
            Err(e) => log!("backup: record {i}: {e}"),
        }
    }
}

/// From `Core_execute`: a check every [`CHECK_EVERY`] calls.
pub(crate) fn tick() {
    if CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % CHECK_EVERY == 0 {
        flush();
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Backup_setupRecords(records: *const usize, count: u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        log!("apm3: Backup_setupRecords({count})");
        if records.is_null() {
            return false;
        }
        let mut list = RECORDS.lock().unwrap();
        list.clear();
        for i in 0..count as usize {
            // `{void *data; size_t size}`: pointer-sized fields (16 bytes a record in 64-bit programs)
            let (address, size) = unsafe { (*records.add(2 * i), *records.add(2 * i + 1)) };
            let mut record = Record { address, size, status: 1, saved: 0 };
            let data = memory(&record);
            // the contents at setup are not the game's data yet (Blade Strangers' are its allocator's
            // fill and it never writes them): saved only once the game changes them
            record.saved = hash(data);
            // the game and its test program may declare a record with different sizes (Blade
            // Strangers: 8 and 7 bytes): their common part
            match std::fs::read(path(i)) {
                Ok(saved) if !saved.is_empty() => {
                    let n = saved.len().min(size);
                    data[..n].copy_from_slice(&saved[..n]);
                    record.status = 0;
                    record.saved = hash(data);
                    log!("backup: record {i} loaded ({n} of {size} bytes)");
                }
                _ => log!("backup: record {i} ({size} bytes): new"),
            }
            list.push(record);
        }
        true
    })() as u32
}

/// 0: the record holds valid saved data; 1: new (no saved data: the game initializes it).
#[unsafe(no_mangle)]
pub extern "system" fn Backup_getRecordStatus(index: u32) -> u32 {
    first_call("Backup_getRecordStatus");
    RECORDS.lock().unwrap().get(index as usize).map_or(1, |r| r.status)
}

#[unsafe(no_mangle)]
pub extern "system" fn Backup_isSetupSucceeded() -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Backup_isSetupSucceeded");
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Backup_saveRecord(_index: u32) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Backup_saveRecord");
        flush();
        true
    })() as u32
}

#[unsafe(no_mangle)]
pub extern "system" fn Backup_saveRecordByAddress(_address: *const u8) -> u32 {
    // a 32-bit BOOL: 64-bit C# callers (P/Invoke) read the whole register
    (|| -> bool {
        first_call("Backup_saveRecordByAddress");
        flush();
        true
    })() as u32
}
