//! `gvrNetComm.dll` of NASCAR Team Racing's race: the network link between cabinets (a race of
//! linked cabinets). The dump has no such DLL: the payload, installed under its name, answers no
//! cabinet linked (no valid address, no packet). Loaded as `gvrNetComm.dll` the payload skips its
//! initialization (`DllMain`): the race's `GvrIO.dll` copy does it.
//!
//! The race's `Gvr::GvrNetComm::GvrNetComm` class (thiscall methods, the callee pops its
//! arguments: the signatures are the imports').

use std::ffi::c_void;

/// `GvrNetComm()`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn netcomm_new(this: *mut c_void) -> *mut c_void {
    this
}

/// `~GvrNetComm()`, `sendPackets()`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn netcomm_none(_this: *mut c_void) {}

/// `void init(char *, char *)`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn netcomm_init(_this: *mut c_void, _a: *const u8, _b: *const u8) {}

/// `void link(char *)`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn netcomm_link(_this: *mut c_void, _a: *const u8) {}

/// `bool isIPValid(char *)`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn netcomm_ip_valid(_this: *mut c_void, _ip: *const u8) -> bool {
    false
}

/// `gvrDataPacket *getIncomingPacket()`, `getNextIncomingPacket()`: no packet.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn netcomm_packet(_this: *mut c_void) -> *mut c_void {
    std::ptr::null_mut()
}

/// `int getNumValidIPs()`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn netcomm_ip_count(_this: *mut c_void) -> i32 {
    0
}

/// `char *getValidIP(int)`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn netcomm_ip(_this: *mut c_void, _index: i32) -> *const u8 {
    c"0.0.0.0".as_ptr().cast()
}

/// `void addOutgoingPacket(void *, unsigned long, gvrSendType, char *)`.
#[unsafe(no_mangle)]
pub unsafe extern "thiscall" fn netcomm_add_packet(_this: *mut c_void, _data: *const c_void, _size: u32, _kind: i32, _ip: *const u8) {}
