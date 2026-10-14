//! Offline ShareK IPC COM interface. Method order and argument sizes come from
//! IPCNet.IManagedInterface in the game's ipccl.dll. It is a dual interface:
//! seven IUnknown/IDispatch entries precede the 54 application methods.
//! No remote ShareK service is connected; getters report empty/idle data.

use std::ffi::c_void;
type P = *mut c_void;
const NOT_IMPLEMENTED: i32 = 0x80004001u32 as i32;

#[repr(C)]
pub(crate) struct Object { vtable: *const *const () }
unsafe impl Sync for Object {}
struct Vtable([*const (); 61]);
unsafe impl Sync for Vtable {}

unsafe extern "system" fn query(this: P, _iid: P, out: *mut P) -> i32 {
    if out.is_null() { return 0x80004003u32 as i32; }
    unsafe { *out = this; }
    0
}
unsafe extern "system" fn refs(_this: P) -> u32 { 1 }
unsafe extern "system" fn get_u16(_this: P, out: *mut u16) -> i32 {
    if !out.is_null() { unsafe { *out = 0; } }
    0
}
unsafe extern "system" fn get_u32(_this: P, out: *mut u32) -> i32 {
    if !out.is_null() { unsafe { *out = 0; } }
    0
}
unsafe extern "system" fn set_value(_this: P, _value: u32) -> i32 { 0 }
unsafe extern "system" fn get_pointer(_this: P, out: *mut P) -> i32 {
    if !out.is_null() { unsafe { *out = std::ptr::null_mut(); } }
    0
}
unsafe extern "system" fn command(_this: P, _a: P, _b: P, _c: P, _d: P) -> i32 { 0 }
unsafe extern "system" fn type_info(_this: P, _index: u32, _locale: u32, _out: P) -> i32 { NOT_IMPLEMENTED }
unsafe extern "system" fn names(_this: P, _iid: P, _names: P, _count: u32, _locale: u32, _out: P) -> i32 { NOT_IMPLEMENTED }
unsafe extern "system" fn invoke(_this: P, _id: u32, _iid: P, _locale: u32, _flags: u32, _params: P, _result: P, _except: P, _argerr: P) -> i32 { NOT_IMPLEMENTED }

static VTABLE: Vtable = Vtable([
    query as *const (), refs as *const (), refs as *const (),
    get_u32 as *const (), type_info as *const (), names as *const (), invoke as *const (),
    get_u16 as *const (), // StartIpcClient: VARIANT_BOOL false (no service)
    get_u16 as *const (), // EndIpcClient
    get_u32 as *const (), set_value as *const (), // process state
    get_u32 as *const (), set_value as *const (), // command result
    command as *const (), command as *const (), command as *const (),
    command as *const (), command as *const (), command as *const (),
    command as *const (), command as *const (), command as *const (),
    command as *const (), command as *const (), command as *const (),
    get_pointer as *const (), get_pointer as *const (), // error log BSTR
    get_u32 as *const (), set_value as *const (), // total download size
    get_u32 as *const (), set_value as *const (), // intermediate download size
    get_u16 as *const (), set_value as *const (), // wait transfer
    get_u16 as *const (), set_value as *const (), // process reboot (slot 33)
    get_u16 as *const (), set_value as *const (), // distribution status
    get_u16 as *const (), set_value as *const (), // downloading version
    get_u16 as *const (), set_value as *const (), // download rate
    get_u16 as *const (), set_value as *const (), // master flag
    get_u16 as *const (), set_value as *const (), // master download rate
    get_u16 as *const (), set_value as *const (), // PCB id
    get_u16 as *const (), set_value as *const (), // missed master connections
    get_u16 as *const (), set_value as *const (), // abnormality flag
    get_u16 as *const (), set_value as *const (), // checksum complete
    get_u16 as *const (), set_value as *const (), // master checksum complete
    get_u16 as *const (), set_value as *const (), // master downloaded version
    get_u16 as *const (), set_value as *const (), // master enable file download
    set_value as *const (), get_pointer as *const (), // RemoteObj
]);
pub(crate) static OBJECT: Object = Object { vtable: VTABLE.0.as_ptr() };
