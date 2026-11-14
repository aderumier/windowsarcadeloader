//! The machine's serial ports as the SetupDi device enumeration lists them (the COM port
//! interface class, `GUID_DEVINTERFACE_COMPORT` 86E0D1E0): `WAL_COMPORTS=<port>[=USB:
//! <vid>:<pid>],...`, the main board's ports (enumerator `ACPI`) and the USB serial devices
//! (enumerator `USB`, service `usbser`, hardware id `USB\VID_<vid>&PID_<pid>&REV_0100`) a game
//! finds its boards among (Project DIVA Arcade's lamp control board and touch panels, through
//! amPlatform's `amPlatformGetComInfo`; KanColle Arcade's gear pop LED board, the first port not
//! on the main board). Wine lists the host's ports, none of them. `WAL_RINGEDGE_COMPORTS`: the
//! same, its former name.
//!
//! A device's registry key (`SetupDiOpenDevRegKey`) holds its `PortName`. The serial layer
//! (`wal_payload_common::serial`) then answers the port itself.

use std::ffi::{CStr, c_char, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{iat, log};

type P = *mut c_void;

/// The fake device information set.
const H_SET: usize = 0x5245_2001;
/// A device's instance (`DevInst`) and registry key: the base plus its index.
const DEVINST: u32 = 0x5245_3000;
const H_KEY: usize = 0x5245_4000;
const COMPORT_GUID_DATA1: u32 = 0x86e0_d1e0;

struct Port {
    name: String,
    /// USB serial: (vid, pid); else a main board port.
    usb: Option<(String, String)>,
}

fn ports() -> &'static [Port] {
    static PORTS: OnceLock<Vec<Port>> = OnceLock::new();
    PORTS.get_or_init(|| {
        let spec = std::env::var("WAL_COMPORTS").or_else(|_| std::env::var("WAL_RINGEDGE_COMPORTS")).unwrap_or_default();
        spec.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|item| match item.split_once('=') {
                Some((name, usb)) => {
                    let mut f = usb.split(':').skip(1);
                    let (vid, pid) = (f.next().unwrap_or("0000"), f.next().unwrap_or("0000"));
                    Port { name: name.trim().to_ascii_uppercase(), usb: Some((vid.to_ascii_uppercase(), pid.to_ascii_uppercase())) }
                }
                None => Port { name: item.to_ascii_uppercase(), usb: None },
            })
            .collect()
    })
}

const CLASS_DEVS: usize = 0;
const CLASS_DEVS_EX: usize = 1;
const ENUM_INFO: usize = 2;
const ENUM_IF: usize = 3;
const IF_DETAIL: usize = 4;
const PROPERTY: usize = 5;
const OPEN_KEY: usize = 6;
const DESTROY: usize = 7;
const QUERY_VALUE: usize = 8;
const CLOSE_KEY: usize = 9;
const DEVICE_ID: usize = 10;
static ORIG: [AtomicUsize; 11] = [const { AtomicUsize::new(0) }; 11];

fn orig<F: Copy>(idx: usize) -> F {
    let f = ORIG[idx].load(Ordering::Relaxed);
    unsafe { std::mem::transmute_copy(&f) }
}

fn set_error(code: u32) {
    unsafe { windows_sys::Win32::Foundation::SetLastError(code) };
}

fn comport_class(guid: *const u32) -> bool {
    !guid.is_null() && unsafe { *guid } == COMPORT_GUID_DATA1
}

unsafe extern "system" fn class_devs(guid: *const u32, enumerator: P, parent: P, flags: u32) -> P {
    if comport_class(guid) {
        log!("comports: SetupDiGetClassDevsA: {} ports", ports().len());
        return H_SET as P;
    }
    unsafe { orig::<unsafe extern "system" fn(*const u32, P, P, u32) -> P>(CLASS_DEVS)(guid, enumerator, parent, flags) }
}

unsafe extern "system" fn class_devs_ex(guid: *const u32, enumerator: P, parent: P, flags: u32, set: P, machine: P, reserved: P) -> P {
    if comport_class(guid) {
        log!("comports: SetupDiGetClassDevsExA: {} ports", ports().len());
        return H_SET as P;
    }
    type F = unsafe extern "system" fn(*const u32, P, P, u32, P, P, P) -> P;
    unsafe { orig::<F>(CLASS_DEVS_EX)(guid, enumerator, parent, flags, set, machine, reserved) }
}

/// `SP_DEVINFO_DATA` { cbSize, ClassGuid, DevInst, Reserved } of port `index`.
unsafe fn fill_devinfo(info: *mut u32, index: usize) {
    if !info.is_null() {
        unsafe {
            *info.add(1) = COMPORT_GUID_DATA1;
            *info.add(5) = DEVINST + index as u32;
            *info.add(6) = index as u32;
        }
    }
}

/// The port of an `SP_DEVINFO_DATA`.
fn devinfo_port(info: *const u32) -> Option<usize> {
    if info.is_null() {
        return None;
    }
    let inst = unsafe { *info.add(5) };
    inst.checked_sub(DEVINST).map(|i| i as usize).filter(|&i| i < ports().len())
}

unsafe extern "system" fn enum_info(set: P, index: u32, info: *mut u32) -> i32 {
    if set as usize != H_SET {
        return unsafe { orig::<unsafe extern "system" fn(P, u32, *mut u32) -> i32>(ENUM_INFO)(set, index, info) };
    }
    if index as usize >= ports().len() {
        set_error(259); // ERROR_NO_MORE_ITEMS
        return 0;
    }
    unsafe { fill_devinfo(info, index as usize) };
    1
}

unsafe extern "system" fn enum_interfaces(set: P, info: *const u32, guid: P, index: u32, data: *mut u32) -> i32 {
    if set as usize != H_SET {
        return unsafe { orig::<unsafe extern "system" fn(P, *const u32, P, u32, *mut u32) -> i32>(ENUM_IF)(set, info, guid, index, data) };
    }
    // one interface per device, or every device's
    let port = match devinfo_port(info) {
        Some(p) if index == 0 => Some(p),
        Some(_) => None,
        None => Some(index as usize).filter(|&i| i < ports().len()),
    };
    let Some(port) = port else {
        set_error(259);
        return 0;
    };
    // SP_DEVICE_INTERFACE_DATA { cbSize, InterfaceClassGuid, Flags, Reserved }
    if !data.is_null() {
        unsafe {
            *data.add(1) = COMPORT_GUID_DATA1;
            *data.add(5) = 1; // SPINT_ACTIVE
            *data.add(6) = port as u32;
        }
    }
    1
}

unsafe extern "system" fn interface_detail(set: P, data: *const u32, detail: *mut u8, size: u32, required: *mut u32, info: *mut u32) -> i32 {
    if set as usize != H_SET {
        type F = unsafe extern "system" fn(P, *const u32, *mut u8, u32, *mut u32, *mut u32) -> i32;
        return unsafe { orig::<F>(IF_DETAIL)(set, data, detail, size, required, info) };
    }
    let index = if data.is_null() { 0 } else { (unsafe { *data.add(6) }) as usize };
    let path = format!("\\\\.\\{}", ports().get(index).map_or("", |p| p.name.as_str()));
    let needed = 4 + path.len() as u32 + 1;
    if !required.is_null() {
        unsafe { *required = needed };
    }
    unsafe { fill_devinfo(info, index) };
    if detail.is_null() || size < needed {
        set_error(122); // ERROR_INSUFFICIENT_BUFFER
        return 0;
    }
    // SP_DEVICE_INTERFACE_DETAIL_DATA_A { cbSize, DevicePath[] }
    unsafe {
        std::ptr::copy_nonoverlapping(path.as_ptr(), detail.add(4), path.len());
        *detail.add(4 + path.len()) = 0;
    }
    1
}

/// A device's instance id (`CM_Get_Device_ID`).
fn instance_id(port: &Port, index: usize) -> String {
    match &port.usb {
        Some((vid, pid)) => format!("USB\\VID_{vid}&PID_{pid}\\5&1A2B3C4D&0&{}", index + 1),
        None => format!("ACPI\\PNP0501\\{}", index + 1),
    }
}

/// `SetupDiGetDeviceRegistryPropertyA`: the hardware id (1, a multi-string), the service (4),
/// the friendly name (0xC), the enumerator (0x16).
unsafe extern "system" fn property(set: P, info: *const u32, prop: u32, kind: *mut u32, buf: *mut u8, size: u32, required: *mut u32) -> i32 {
    if set as usize != H_SET {
        type F = unsafe extern "system" fn(P, *const u32, u32, *mut u32, *mut u8, u32, *mut u32) -> i32;
        return unsafe { orig::<F>(PROPERTY)(set, info, prop, kind, buf, size, required) };
    }
    let Some(index) = devinfo_port(info) else {
        set_error(13); // ERROR_INVALID_DATA
        return 0;
    };
    let port = &ports()[index];
    let (value, reg_type): (Vec<u8>, u32) = match (prop, &port.usb) {
        (1, Some((vid, pid))) => (format!("USB\\VID_{vid}&PID_{pid}&REV_0100\0USB\\VID_{vid}&PID_{pid}\0\0").into_bytes(), 7),
        (1, None) => (b"ACPI\\PNP0501\0*PNP0501\0\0".to_vec(), 7),
        (4, Some(_)) => (b"usbser\0".to_vec(), 1),
        (4, None) => (b"Serial\0".to_vec(), 1),
        (0xC, _) => (format!("Communications Port ({})\0", port.name).into_bytes(), 1),
        (0x16, Some(_)) => (b"USB\0".to_vec(), 1),
        (0x16, None) => (b"ACPI\0".to_vec(), 1),
        _ => {
            log!("comports: {} property {prop:#x}: none", port.name);
            set_error(13);
            return 0;
        }
    };
    if !required.is_null() {
        unsafe { *required = value.len() as u32 };
    }
    if !kind.is_null() {
        unsafe { *kind = reg_type };
    }
    if buf.is_null() || (size as usize) < value.len() {
        set_error(122);
        return 0;
    }
    unsafe { std::ptr::copy_nonoverlapping(value.as_ptr(), buf, value.len()) };
    1
}

unsafe extern "system" fn open_key(set: P, info: *const u32, scope: u32, profile: u32, kind: u32, access: u32) -> P {
    if set as usize != H_SET {
        type F = unsafe extern "system" fn(P, *const u32, u32, u32, u32, u32) -> P;
        return unsafe { orig::<F>(OPEN_KEY)(set, info, scope, profile, kind, access) };
    }
    match devinfo_port(info) {
        Some(i) => (H_KEY + i) as P,
        None => -1isize as P, // INVALID_HANDLE_VALUE
    }
}

fn key_port(key: P) -> Option<&'static Port> {
    (key as usize).checked_sub(H_KEY).and_then(|i| ports().get(i))
}

/// `RegQueryValueExA`: a device key's `PortName`.
unsafe extern "system" fn query_value(key: P, name: *const c_char, reserved: P, kind: *mut u32, data: *mut u8, size: *mut u32) -> i32 {
    let Some(port) = key_port(key) else {
        type F = unsafe extern "system" fn(P, *const c_char, P, *mut u32, *mut u8, *mut u32) -> i32;
        return unsafe { orig::<F>(QUERY_VALUE)(key, name, reserved, kind, data, size) };
    };
    let value_name = if name.is_null() { String::new() } else { unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned() };
    if !value_name.eq_ignore_ascii_case("PortName") {
        return 2; // ERROR_FILE_NOT_FOUND
    }
    let value = format!("{}\0", port.name);
    if !kind.is_null() {
        unsafe { *kind = 1 }; // REG_SZ
    }
    if size.is_null() {
        return 87;
    }
    let have = unsafe { *size } as usize;
    unsafe { *size = value.len() as u32 };
    if data.is_null() {
        return 0;
    }
    if have < value.len() {
        return 234; // ERROR_MORE_DATA
    }
    unsafe { std::ptr::copy_nonoverlapping(value.as_ptr(), data, value.len()) };
    0
}

unsafe extern "system" fn close_key(key: P) -> i32 {
    if key_port(key).is_some() {
        return 0;
    }
    unsafe { orig::<unsafe extern "system" fn(P) -> i32>(CLOSE_KEY)(key) }
}

/// `CM_Get_Device_ID_ExA(devinst, buffer, length, flags, machine)`.
unsafe extern "system" fn device_id(inst: u32, buf: *mut u8, len: u32, flags: u32, machine: P) -> u32 {
    let index = inst.checked_sub(DEVINST).map(|i| i as usize).filter(|&i| i < ports().len());
    let Some(index) = index else {
        return unsafe { orig::<unsafe extern "system" fn(u32, *mut u8, u32, u32, P) -> u32>(DEVICE_ID)(inst, buf, len, flags, machine) };
    };
    let id = instance_id(&ports()[index], index);
    if buf.is_null() || (len as usize) <= id.len() {
        return 0x1A; // CR_BUFFER_SMALL
    }
    unsafe {
        std::ptr::copy_nonoverlapping(id.as_ptr(), buf, id.len());
        *buf.add(id.len()) = 0;
    }
    0
}

unsafe extern "system" fn destroy(set: P) -> i32 {
    if set as usize == H_SET {
        return 1;
    }
    unsafe { orig::<unsafe extern "system" fn(P) -> i32>(DESTROY)(set) }
}

/// After the base payload's SetupDi hooks (`devices.rs`): the other classes go on to them.
pub fn init() {
    if ports().is_empty() {
        return;
    }
    let hooks: [(&str, &str, usize); 11] = [
        ("setupapi.dll", "SetupDiGetClassDevsA", class_devs as *const () as usize),
        ("setupapi.dll", "SetupDiGetClassDevsExA", class_devs_ex as *const () as usize),
        ("setupapi.dll", "SetupDiEnumDeviceInfo", enum_info as *const () as usize),
        ("setupapi.dll", "SetupDiEnumDeviceInterfaces", enum_interfaces as *const () as usize),
        ("setupapi.dll", "SetupDiGetDeviceInterfaceDetailA", interface_detail as *const () as usize),
        ("setupapi.dll", "SetupDiGetDeviceRegistryPropertyA", property as *const () as usize),
        ("setupapi.dll", "SetupDiOpenDevRegKey", open_key as *const () as usize),
        ("setupapi.dll", "SetupDiDestroyDeviceInfoList", destroy as *const () as usize),
        ("advapi32.dll", "RegQueryValueExA", query_value as *const () as usize),
        ("advapi32.dll", "RegCloseKey", close_key as *const () as usize),
        ("setupapi.dll", "CM_Get_Device_ID_ExA", device_id as *const () as usize),
    ];
    for (i, (dll, name, f)) in hooks.into_iter().enumerate() {
        match unsafe { iat::hook(dll, name, f) } {
            Some(o) => ORIG[i].store(o, Ordering::Relaxed),
            None => log!("comports: {dll}!{name} not imported"),
        }
    }
    let list: Vec<String> = ports().iter().map(|p| match &p.usb {
        Some((v, d)) => format!("{} (USB {v}:{d})", p.name),
        None => p.name.clone(),
    }).collect();
    log!("comports: {}", list.join(", "));
}
