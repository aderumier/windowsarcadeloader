//! `GvrLinking.dll` of the NASCAR cabinets' frontend: the network between linked cabinets, for
//! its linking plug-in (`GvrLinkingPlugIn.dll`), which gives the scenes the link's state
//! (`Gvr.OnLine`, `Gvr.NumCabinetsOnLine`, `Gvr.Packet`, the addresses...). The original waits for
//! the cabinet network (the frontend never showed), and without the plug-in the scenes reading
//! these properties stop (a property no plug-in has ends the script's frame: NASCAR Team Racing's
//! driver selection never picked a driver). The payload, installed under its name, answers a
//! cabinet alone: only itself on line, no packet. Loaded as `GvrLinking.dll` the payload skips
//! its initialization (`DllMain`): the frontend's `GvrIO.dll` copy does it.
//!
//! The `Gvr::Zeus::GvrLinking` class (thiscall methods, the callee pops its arguments).

use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, Ordering};

/// The cabinet's number (its link id), as the frontend sets it.
static CABINET: AtomicI32 = AtomicI32::new(1);

/// `GvrLinking()`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn linking_new(this: *mut c_void) -> *mut c_void {
    this
}

/// `~GvrLinking()`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn linking_drop(_this: *mut c_void) {}

/// `void SetPacketDataBlock(unsigned long, void *)`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn linking_set_data(_this: *mut c_void, _size: u32, _data: *mut c_void) {}

/// `void SuspendPackets(bool)`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn linking_suspend(_this: *mut c_void, _suspend: u32) {}

/// `unsigned long GetPacketsReceived()`, `int GetCabIndex()`: none, the first.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn linking_zero(_this: *mut c_void) -> u32 {
    0
}

/// `int GetNumCabinetsOnline()`: this one.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn linking_online_count(_this: *mut c_void) -> i32 {
    1
}

/// Writes `text` into the caller's buffer of `size` bytes.
fn fill(buf: *mut u8, size: i32, text: &[u8]) -> bool {
    if buf.is_null() || size <= text.len() as i32 {
        return false;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr(), buf, text.len());
        *buf.add(text.len()) = 0;
    }
    true
}

/// `bool GetMacAddress(char *, int, int)`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn linking_mac(_this: *mut c_void, buf: *mut u8, size: i32, _index: i32) -> bool {
    fill(buf, size, b"00-00-00-00-00-00")
}

/// `bool GetIPAddress(char *, int, int)`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn linking_ip(_this: *mut c_void, buf: *mut u8, size: i32, _index: i32) -> bool {
    fill(buf, size, b"127.0.0.1")
}

/// `bool IsOnline(int)`: the cabinet itself only.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn linking_is_online(_this: *mut c_void, index: i32) -> bool {
    index == 0
}

/// `bool SetCabinetNumber(int)`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn linking_set_cabinet(_this: *mut c_void, number: i32) -> bool {
    CABINET.store(number, Ordering::Relaxed);
    true
}

/// `int GetCabinetNumber()`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn linking_cabinet(_this: *mut c_void) -> i32 {
    CABINET.load(Ordering::Relaxed)
}

/// `bool SetPacketReceiveCallback(void *(*)(int, sGvrPacket *))`: no packet ever comes.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn linking_set_callback(_this: *mut c_void, _callback: *const c_void) -> bool {
    true
}
