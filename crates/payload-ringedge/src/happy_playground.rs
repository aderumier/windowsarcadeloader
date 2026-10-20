//! Sega RingWide's card dispenser used by Pleasant Goat and Big Big Wolf.
//!
//! The game speaks a small framed protocol directly to COM2. A successful response is
//! `55 status reserved length payload... xor`, where the xor of the complete frame is zero.
//! Its startup probe accepts status zero and up to eight payload bytes.

use wal_payload_common::{log, serial};
use std::sync::{Mutex, OnceLock};

static REQUEST: Mutex<Vec<u8>> = Mutex::new(Vec::new());

pub(crate) fn init() {
    if !std::env::var("WAL_RINGWIDE_HAPPY_PLAYGROUND_CARD")
        .is_ok_and(|v| v.trim() == "1")
    {
        return;
    }
    // The service menu is a separate executable with different code addresses.
    if !std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().eq_ignore_ascii_case("XYY_RingWide.exe"))).unwrap_or(false) {
        return;
    }
    serial::install("COM2", card_dispenser);
    log!("happy-playground: card dispenser emulated on COM2");
    if std::env::var_os("WAL_RINGWIDE_HAPPY_PLAYGROUND_TRACE").is_some() {
        unsafe {
            trace_call(0x12b31, counter_init as *const () as usize);
            trace_call(0x1aa51b, read_frame as *const () as usize);
            for at in [0x1a967c, 0x1a9b62, 0x1a9c7f, 0x1aa218] {
                trace_call(at, probe as *const () as usize);
            }
        }
    }
}

unsafe fn trace_call(rva: usize, target: usize) {
    use windows_sys::Win32::System::{LibraryLoader::GetModuleHandleW, Memory::{VirtualProtect, PAGE_EXECUTE_READWRITE}};
    let at = unsafe { GetModuleHandleW(std::ptr::null()) } as usize + rva;
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const _, 5, PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::write_unaligned((at + 1) as *mut i32, target.wrapping_sub(at + 5) as i32);
        VirtualProtect(at as *const _, 5, old, &mut old);
    }
}

unsafe extern "system" fn read_frame(handle: u32, out: *mut u8) -> i16 {
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    let original: unsafe extern "system" fn(u32, *mut u8) -> i16 = unsafe { std::mem::transmute(base + 0x1ab910) };
    let result = unsafe { original(handle, out) };
    log!("happy-playground: read frame h={handle:#x} result={result} header={:02X?}", unsafe { std::slice::from_raw_parts(out, 5) });
    result
}

unsafe extern "system" fn probe(handle: u32, length: u32, out: *mut u8) -> i16 {
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    let original: unsafe extern "system" fn(u32, u32, *mut u8) -> i16 = unsafe { std::mem::transmute(base + 0x1aa490) };
    let result = unsafe { original(handle, length, out) };
    log!("happy-playground: probe h={handle:#x} length={length} result={result} data={:02X?}", unsafe { std::slice::from_raw_parts(out, length.min(8) as usize) });
    result
}

fn card_dispenser(request: &[u8]) -> Vec<u8> {
    // The game writes its startup query one byte at a time. The length byte is at offset 3;
    // it counts the seven bytes following it, making a 12 byte frame in total.
    let mut frame = REQUEST.lock().unwrap();
    let mut replies = Vec::new();
    for &byte in request {
        if frame.is_empty() && byte != 0xaa {
            continue;
        }
        frame.push(byte);
        if frame.len() >= 4 && frame.len() == usize::from(frame[3]) + 5 {
            replies.extend(answer_frame(&frame));
            frame.clear();
        }
    }
    replies
}

fn answer_frame(frame: &[u8]) -> Vec<u8> {
    if frame.len() < 5 || frame.iter().fold(0u8, |sum, byte| sum ^ byte) != 0 {
        log!("happy-playground: rejecting invalid COM2 frame {:02X?}", frame);
        return Vec::new();
    }
    let command = frame[1];
    let supported = frame[0] == 0xaa && matches!(command, 0xb0 | 0xb2 | 0xb3 | 0xb4 | 0xb9 | 0xc0 | 0x67 | 0x61 | 0x64);
    log!("happy-playground: COM2 frame {:02X?}, supported={supported}", frame);
    let mut data = match command {
        0xb0 => b"WAL_1_1\0".to_vec(),
        0x67 if frame.len() == 12 => crypt(&[1], &frame[9..11]),
        0x61 if frame.len() == 14 => {
            let address = u16::from_be_bytes([frame[7], frame[8]]) as usize;
            let length = frame[12] as usize;
            let channel = frame[4];
            let memory = counter_memory(channel);
            let Some(data) = memory.get(address..address + length) else {
                return Vec::new();
            };
            log!("happy-playground: memory read channel={channel} address={address:#x} length={length}");
            crypt(data, &frame[9..11])
        },
        0x61 | 0x67 => return Vec::new(),
        _ => Vec::new(),
    };
    if supported {
        let mut reply = vec![0x55, if command == 0xb9 { 3 } else { 0 }, 0, data.len() as u8];
        reply.append(&mut data);
        reply.push(reply.iter().fold(0, |sum, b| sum ^ b));
        return reply;
    }
    Vec::new()
}

fn crypt(data: &[u8], key: &[u8]) -> Vec<u8> {
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    let original: unsafe extern "C" fn(u32, *const u8, *mut u8, *const u8) = unsafe { std::mem::transmute(base + 0x1af180) };
    let mut out = vec![0; data.len()];
    unsafe { original(data.len() as u32, data.as_ptr(), out.as_mut_ptr(), key.as_ptr()) };
    out
}

// Experimental virtual counter image. Use the executable's own record encoder until
// the record format is understood well enough to implement independently.
fn counter_memory(channel: u8) -> &'static [u8] {
    static IMAGES: [OnceLock<Vec<u8>>; 2] = [OnceLock::new(), OnceLock::new()];
    let Some(image) = channel.checked_sub(1).and_then(|index| IMAGES.get(index as usize)) else {
        return &[];
    };
    image.get_or_init(|| make_counter_memory(channel))
}

fn make_counter_memory(channel: u8) -> Vec<u8> {
    let mut memory = vec![0; 1024];
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    let mut record = [0u8; 51];
    record[8] = channel;
    record[0x13..0x15].copy_from_slice(&100u16.to_le_bytes());
    let mut auxiliary = [0u8; 3];
    let pack: unsafe extern "C" fn(*const u8, u32, *mut u8, *mut u8, u32) -> i32 =
        unsafe { std::mem::transmute(base + 0x1ad1b9) };
    let result = unsafe { pack(record.as_ptr(), 51, memory[0x40..].as_mut_ptr(), auxiliary.as_mut_ptr(), 2) };
    log!("happy-playground: packed experimental counter record result={result}");
    memory[0x20..0x23].copy_from_slice(b"BVG");
    // Region is selected by the game from its backup settings.
    unsafe {
        let settings = std::ptr::read_unaligned((base + 0x253b81c) as *const usize);
        if settings != 0 {
            let region = std::ptr::read((settings + 0xa8) as *const u8) as usize;
            if region < 4 {
                memory[0x23..0x26].copy_from_slice(std::slice::from_raw_parts((base + 0x2a3e44 + region * 3) as *const u8, 3));
            }
        }
    }
    memory[0x29] = channel;
    memory
}

unsafe extern "C" fn counter_init() -> i32 {
    let base = unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(std::ptr::null()) } as usize;
    let original: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(base + 0x125b0) };
    let result = unsafe { original() };
    unsafe {
        let object = std::ptr::read_unaligned((base + 0x253b83c) as *const usize);
        log!("happy-playground: counter init result={result} error={} object={:02X?}",
            std::ptr::read_unaligned((base + 0x2a3e50) as *const u32),
            std::slice::from_raw_parts(object as *const u8, 64));
    }
    std::thread::spawn(move || {
        for _ in 0..5 {
            std::thread::sleep(std::time::Duration::from_secs(1));
            log!("happy-playground: startup error={:#x} switches={:02X?}",
                unsafe { std::ptr::read_volatile((base + 0x2a3e50) as *const u32) },
                unsafe { std::slice::from_raw_parts((base + 0x2b023c + 0x140) as *const u8, 16) });
        }
    });
    result
}
