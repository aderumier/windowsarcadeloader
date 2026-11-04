//! Fruit Ninja FX 2 (Halfbrick's native engine, 32-bit, Direct3D 9): its figurine portal, its
//! image scaler's reads past its buffers.
//!
//! The executable imports `NfcReader.dll` (cdecl): at start it opens each port from COM19 down
//! to COM1 (`NfcOpen`, non-zero: opened) and asks a figurine's id (`NfcGetId`, negative: no
//! reader there). The library opens the machine's real serial ports (Batocera: COM1, where
//! nothing answers): no reader is found here, without a port opened. The figurines are an
//! option of the power choice (a power picked on the screen instead).

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};

extern "C" fn nfc_open(_port: i32) -> i32 {
    0
}

extern "C" fn nfc_close(_port: i32) {}

extern "C" fn nfc_get_id(_port: i32, _id: *mut u8, _size: i32) -> i32 {
    -1
}

extern "C" fn nfc_set_callback(_callback: *const c_void) {}

extern "C" fn nfc_read(_port: i32, _data: *mut u8, _size: i32) -> i32 {
    -1
}

extern "C" fn nfc_write(_port: i32, _data: *mut u8, _size: i32) -> i32 {
    -1
}

/// Large blocks' slack, and the size from which blocks get it.
const SLACK: usize = 0x10000;

static HEAP_ALLOC: AtomicUsize = AtomicUsize::new(0);
static HEAP_REALLOC: AtomicUsize = AtomicUsize::new(0);

fn with_slack(size: usize) -> usize {
    if size >= SLACK { size + SLACK } else { size }
}

/// The game's bilinear image scaler (at load, `+0x5b0a10`) reads the source's next row at its
/// last one: a row past the image's buffer, unseen on Windows' heap; Wine's large blocks can end
/// at a page's end (an access violation at start, about one run in four). The executable's
/// blocks of 64 KB and more get 64 KB more.
unsafe extern "system" fn heap_alloc(heap: *mut c_void, flags: u32, size: usize) -> *mut c_void {
    let orig: unsafe extern "system" fn(*mut c_void, u32, usize) -> *mut c_void = unsafe { std::mem::transmute(HEAP_ALLOC.load(Ordering::Relaxed)) };
    unsafe { orig(heap, flags, with_slack(size)) }
}

unsafe extern "system" fn heap_realloc(heap: *mut c_void, flags: u32, block: *mut c_void, size: usize) -> *mut c_void {
    let orig: unsafe extern "system" fn(*mut c_void, u32, *mut c_void, usize) -> *mut c_void = unsafe { std::mem::transmute(HEAP_REALLOC.load(Ordering::Relaxed)) };
    unsafe { orig(heap, flags, block, with_slack(size)) }
}

/// The executable's figurine portal imports pointed at ours, its heap blocks with slack.
pub fn init() {
    let functions: [(&str, usize); 6] = [
        ("?NfcOpen@@YAHH@Z", nfc_open as *const () as usize),
        ("?NfcClose@@YAXH@Z", nfc_close as *const () as usize),
        ("?NfcGetId@@YAHHPAEH@Z", nfc_get_id as *const () as usize),
        ("?NfcSetCallback@@YAXP6AXHPAEH@Z@Z", nfc_set_callback as *const () as usize),
        ("?NfcRead@@YAHHPAEH@Z", nfc_read as *const () as usize),
        ("?NfcWrite@@YAHHPAEH@Z", nfc_write as *const () as usize),
    ];
    let hooked = functions.iter().filter(|(name, f)| unsafe { iat::hook("NfcReader.dll", name, *f) }.is_some()).count();
    if hooked == 0 {
        return;
    }
    log!("fruitninja: figurine portal: no reader ({hooked} imports hooked)");
    if let Some(f) = unsafe { iat::hook("kernel32.dll", "HeapAlloc", heap_alloc as *const () as usize) } {
        HEAP_ALLOC.store(f, Ordering::Relaxed);
    }
    if let Some(f) = unsafe { iat::hook("kernel32.dll", "HeapReAlloc", heap_realloc as *const () as usize) } {
        HEAP_REALLOC.store(f, Ordering::Relaxed);
    }
}
