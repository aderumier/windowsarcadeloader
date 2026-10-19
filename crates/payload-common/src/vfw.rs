//! Video for Windows codecs shipped with a game (`WAL_VFW_CODECS`).
//!
//! `WAL_VFW_CODECS=vidc.wmv3=WMV9VCM.dll[,...]` registers each driver in
//! `HKLM\Software\Microsoft\Windows NT\CurrentVersion\Drivers32` (what a codec installer does),
//! where Wine's `ICOpen` looks it up; the DLL is loaded by name, from the game directory.
//! Gaia Attack 4 plays WMV9 (`WMV3`) AVIs through AVIFile and ships Microsoft's WMV9 VCM
//! codec installer: its `WMV9VCM.dll` is self-contained.
//!
//! `WAL_VFW_READ_FIX=1`: `AVIStreamRead(stream, pos, AVISTREAMREAD_CONVENIENT, NULL, 0, &bytes,
//! ..)` (the size of the next "convenient" read) answers a chunk's size. Wine's avifile computes
//! the samples from the buffer size there (0 / sample size = 0) and answers 0 bytes for a
//! fixed sample size stream (PCM audio); Windows answers the rest of the current chunk. Storm
//! Racer G sizes its movies' audio reads that way (0: an assert at each frame). The hook asks
//! for the stream's suggested buffer size worth of samples instead. The read itself (buffer,
//! `AVISTREAMREAD_CONVENIENT`, no `bytes` / `samples` outputs) is refused by Wine
//! (`AVIERR_BADPARAM`): the hook passes an output of its own. Both are Wine avifil32 bugs
//! (`IAVIStream_fnRead`), not GE-Proton's.

use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW,
    RegSetValueExW,
};

use crate::{iat, log};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Registers the codecs of `WAL_VFW_CODECS`, if any, and hooks `AVIStreamRead` with
/// `WAL_VFW_READ_FIX=1`.
pub fn init() {
    if std::env::var("WAL_VFW_READ_FIX").is_ok_and(|v| v.trim() == "1") {
        read_fix();
    }
    register_codecs();
}

type StreamRead = unsafe extern "system" fn(usize, i32, i32, *mut u8, i32, *mut i32, *mut i32) -> i32;
type StreamInfo = unsafe extern "system" fn(usize, *mut u8, i32) -> i32;

static READ: std::sync::OnceLock<(StreamRead, StreamInfo)> = std::sync::OnceLock::new();

fn read_fix() {
    // the game's import slot holds the function
    let info = unsafe { iat::hook_addr("avifil32.dll", "AVIStreamInfoW") }.map(|slot| unsafe { *(slot as *const usize) });
    let orig = unsafe { iat::hook("avifil32.dll", "AVIStreamRead", stream_read as *const () as usize) };
    match (orig, info) {
        (Some(read), Some(info)) => {
            let _ = READ.set(unsafe { (std::mem::transmute::<usize, StreamRead>(read), std::mem::transmute::<usize, StreamInfo>(info)) });
            log!("vfw: AVIStreamRead size queries fixed");
        }
        _ => log!("vfw: AVIStreamRead not imported, no read fix"),
    }
}

/// `AVISTREAMREAD_CONVENIENT`
const CONVENIENT: i32 = -1;

unsafe extern "system" fn stream_read(
    stream: usize,
    start: i32,
    mut samples: i32,
    buffer: *mut u8,
    size: i32,
    bytes: *mut i32,
    samples_read: *mut i32,
) -> i32 {
    let &(read, info) = READ.get().unwrap();
    if samples == CONVENIENT && buffer.is_null() && size <= 0 {
        // AVISTREAMINFOW: dwRate +24, dwScale +20, dwSuggestedBufferSize +40, dwSampleSize +48
        let mut si = [0u8; 204];
        if unsafe { info(stream, si.as_mut_ptr(), si.len() as i32) } == 0 {
            let field = |o: usize| u32::from_le_bytes(si[o..o + 4].try_into().unwrap());
            let (scale, rate, suggested, sample_size) = (field(20), field(24), field(40), field(48));
            if sample_size != 0 {
                let per_frame = if scale != 0 { rate / scale / 30 } else { 0 };
                samples = (suggested / sample_size).max(per_frame).max(1) as i32;
            }
        }
    }
    // Wine refuses a convenient read with neither output (AVIERR_BADPARAM), Windows reads
    let mut local = 0;
    let bytes = if samples == CONVENIENT && bytes.is_null() && samples_read.is_null() { &mut local } else { bytes };
    unsafe { read(stream, start, samples, buffer, size, bytes, samples_read) }
}

fn register_codecs() {
    let Ok(spec) = std::env::var("WAL_VFW_CODECS") else { return };
    let path = wide("Software\\Microsoft\\Windows NT\\CurrentVersion\\Drivers32");
    let mut key: HKEY = std::ptr::null_mut();
    let r = unsafe {
        RegCreateKeyExW(HKEY_LOCAL_MACHINE, path.as_ptr(), 0, std::ptr::null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, std::ptr::null(), &mut key, std::ptr::null_mut())
    };
    if r != 0 {
        log!("vfw: cannot open Drivers32: error {r}");
        return;
    }
    for entry in spec.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let Some((name, dll)) = entry.split_once('=') else {
            log!("vfw: bad entry {entry:?}");
            continue;
        };
        let (name, dll) = (wide(name.trim()), wide(dll.trim()));
        let r = unsafe { RegSetValueExW(key, name.as_ptr(), 0, REG_SZ, dll.as_ptr().cast(), (dll.len() * 2) as u32) };
        log!("vfw: {entry} {}", if r == 0 { "registered".to_string() } else { format!("error {r}") });
    }
    unsafe { RegCloseKey(key) };
}
