//! The Sinden lightguns' cameras hidden from a game's webcams ([`skip_sinden`]): a game taking
//! the first webcam for an optional feature (the players' photos) would open a gun's camera,
//! held by the Sinden software (no frames) or taken from it. Other webcams stay the game's.
//!
//! DirectShow's device list (Unity's `WebCamTexture.devices`): an IAT hook of
//! `ole32!CoCreateInstance` in the game executable wraps the system device enumerator's
//! `ICreateDevEnum::CreateClassEnumerator`, whose video input monikers' `IEnumMoniker::Next`
//! (their shared vtable) skips a device whose `FriendlyName` contains "sinden" (any case).

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
use windows_sys::core::GUID;

use crate::{iat, log};

type P = *mut c_void;

const S_OK: i32 = 0;
const S_FALSE: i32 = 1;

const CLSID_SYSTEM_DEVICE_ENUM: GUID = GUID::from_u128(0x62be5d10_60eb_11d0_bd3b_00a0c911ce86);
const CLSID_VIDEO_INPUT_DEVICE_CATEGORY: GUID = GUID::from_u128(0x860bb310_5d01_11d0_bd3b_00a0c911ce86);
const IID_IPROPERTY_BAG: GUID = GUID::from_u128(0x55272a00_42cb_11ce_8135_00aa004bb851);

// vtable slots
const RELEASE: usize = 2;
const CREATE_CLASS_ENUMERATOR: usize = 3;
const ENUM_NEXT: usize = 3;
const MONIKER_BIND_TO_STORAGE: usize = 9;
const BAG_READ: usize = 3;

static ORIG_CO_CREATE: AtomicUsize = AtomicUsize::new(0);
static ORIG_CREATE_CLASS_ENUMERATOR: AtomicUsize = AtomicUsize::new(0);
static ORIG_NEXT: AtomicUsize = AtomicUsize::new(0);

#[link(name = "oleaut32")]
unsafe extern "system" {
    fn VariantClear(var: *mut Variant) -> i32;
}

/// A `VARIANT` holding a `BSTR` (`VT_BSTR`).
#[repr(C)]
struct Variant {
    vt: u16,
    reserved: [u16; 3],
    value: *const u16,
    extra: usize,
}

const VT_BSTR: u16 = 8;

/// Hides the Sinden cameras from the game executable's DirectShow device lists.
pub fn skip_sinden() {
    match unsafe { iat::hook("ole32.dll", "CoCreateInstance", co_create_instance as *const () as usize) } {
        Some(orig) => {
            ORIG_CO_CREATE.store(orig, Ordering::Relaxed);
            log!("webcam: Sinden cameras hidden");
        }
        None => log!("webcam: CoCreateInstance import not found, Sinden cameras not hidden"),
    }
}

unsafe fn method(obj: P, slot: usize) -> usize {
    unsafe { *(*(obj as *const *const usize)).add(slot) }
}

unsafe fn release(obj: P) {
    let f: unsafe extern "system" fn(P) -> u32 = unsafe { std::mem::transmute(method(obj, RELEASE)) };
    unsafe { f(obj) };
}

fn guid_eq(a: *const GUID, b: &GUID) -> bool {
    !a.is_null() && {
        let a = unsafe { &*a };
        a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
    }
}

/// Wraps slot `slot` of the shared vtable of `obj`, once (`orig` keeps the original).
unsafe fn wrap(obj: P, slot: usize, replacement: usize, orig: &AtomicUsize) {
    if orig.load(Ordering::Relaxed) != 0 {
        return;
    }
    let entry = unsafe { (*(obj as *const *mut usize)).add(slot) };
    let mut old = 0;
    if unsafe { VirtualProtect(entry.cast(), size_of::<usize>(), PAGE_READWRITE, &mut old) } != 0 {
        orig.store(unsafe { *entry }, Ordering::Relaxed);
        unsafe { *entry = replacement };
        unsafe { VirtualProtect(entry.cast(), size_of::<usize>(), old, &mut old) };
    }
}

unsafe extern "system" fn co_create_instance(clsid: *const GUID, outer: P, ctx: u32, iid: *const GUID, out: *mut P) -> i32 {
    let orig: unsafe extern "system" fn(*const GUID, P, u32, *const GUID, *mut P) -> i32 =
        unsafe { std::mem::transmute(ORIG_CO_CREATE.load(Ordering::Relaxed)) };
    let hr = unsafe { orig(clsid, outer, ctx, iid, out) };
    if hr >= 0 && !out.is_null() && unsafe { !(*out).is_null() } && guid_eq(clsid, &CLSID_SYSTEM_DEVICE_ENUM) {
        // ICreateDevEnum is the enumerator's only interface
        unsafe { wrap(*out, CREATE_CLASS_ENUMERATOR, create_class_enumerator as *const () as usize, &ORIG_CREATE_CLASS_ENUMERATOR) };
    }
    hr
}

unsafe extern "system" fn create_class_enumerator(this: P, category: *const GUID, out: *mut P, flags: u32) -> i32 {
    let orig: unsafe extern "system" fn(P, *const GUID, *mut P, u32) -> i32 =
        unsafe { std::mem::transmute(ORIG_CREATE_CLASS_ENUMERATOR.load(Ordering::Relaxed)) };
    let hr = unsafe { orig(this, category, out, flags) };
    if hr == S_OK && !out.is_null() && unsafe { !(*out).is_null() } && guid_eq(category, &CLSID_VIDEO_INPUT_DEVICE_CATEGORY) {
        unsafe { wrap(*out, ENUM_NEXT, next as *const () as usize, &ORIG_NEXT) };
    }
    hr
}

/// `IEnumMoniker::Next`: the devices but the Sinden cameras.
unsafe extern "system" fn next(this: P, count: u32, monikers: *mut P, fetched: *mut u32) -> i32 {
    let orig: unsafe extern "system" fn(P, u32, *mut P, *mut u32) -> i32 =
        unsafe { std::mem::transmute(ORIG_NEXT.load(Ordering::Relaxed)) };
    let mut got = 0;
    while got < count {
        let mut moniker: P = std::ptr::null_mut();
        let mut one = 0;
        if unsafe { orig(this, 1, &mut moniker, &mut one) } != S_OK || one == 0 || moniker.is_null() {
            break;
        }
        let name = unsafe { friendly_name(moniker) };
        if name.to_ascii_lowercase().contains("sinden") {
            log!("webcam: {name} hidden");
            unsafe { release(moniker) };
            continue;
        }
        log!("webcam: {name}");
        unsafe { *monikers.add(got as usize) = moniker };
        got += 1;
    }
    if !fetched.is_null() {
        unsafe { *fetched = got };
    }
    if got == count { S_OK } else { S_FALSE }
}

/// The device's `FriendlyName` (empty if it has none).
unsafe fn friendly_name(moniker: P) -> String {
    let bind: unsafe extern "system" fn(P, P, P, *const GUID, *mut P) -> i32 =
        unsafe { std::mem::transmute(method(moniker, MONIKER_BIND_TO_STORAGE)) };
    let mut bag: P = std::ptr::null_mut();
    if unsafe { bind(moniker, std::ptr::null_mut(), std::ptr::null_mut(), &IID_IPROPERTY_BAG, &mut bag) } < 0 || bag.is_null() {
        return String::new();
    }
    let read: unsafe extern "system" fn(P, *const u16, *mut Variant, P) -> i32 = unsafe { std::mem::transmute(method(bag, BAG_READ)) };
    let key: Vec<u16> = "FriendlyName\0".encode_utf16().collect();
    let mut var = Variant { vt: 0, reserved: [0; 3], value: std::ptr::null(), extra: 0 };
    let mut name = String::new();
    if unsafe { read(bag, key.as_ptr(), &mut var, std::ptr::null_mut()) } >= 0 && var.vt == VT_BSTR && !var.value.is_null() {
        let len = (0..).take_while(|&i| unsafe { *var.value.add(i) } != 0).count();
        name = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(var.value, len) });
    }
    unsafe { VariantClear(&mut var) };
    unsafe { release(bag) };
    name
}
