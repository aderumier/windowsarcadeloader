//! The picture over the whole screen, at the screen's resolution.
//!
//! gs-system's screen settings (`/config.xml` `/config/screen/`: frame, the window; mon, the
//! monitor; draw, the picture; draw_f) default to 1280x720 when the file is missing (the
//! cabinet's): the game draws a 1280x720 picture in the window's corner. Their defaults become
//! the screen size: the game renders its 1280x720 layout scaled to it, the guns' coordinates
//! scaled alike. ek-lib's window (`CreateWindowExA`, then `SetWindowLongA` styles) is made a
//! borderless popup at 0,0.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};

const WS_POPUP: u32 = 0x8000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const GWL_STYLE: i32 = -16;

static ORIG_CREATE: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_LONG: AtomicUsize = AtomicUsize::new(0);

fn module(name: &str) -> usize {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { GetModuleHandleW(wide.as_ptr()) as usize }
}

/// gs-system's screen settings constructor: `mov ecx, 1280` / `mov eax, 720` (rva 0x205f), stored
/// in every width / height.
unsafe fn screen_size(w: u32, h: u32) {
    let base = module("gs-system.dll");
    if base == 0 {
        return;
    }
    let at = (base + 0x205f) as *mut u8;
    let expected = [0xb9, 0x00, 0x05, 0x00, 0x00, 0xb8, 0xd0, 0x02, 0x00, 0x00];
    if unsafe { std::slice::from_raw_parts(at, 10) } != expected {
        log!("screen: gs-system screen settings signature mismatch, 1280x720 kept");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(at.cast(), 10, PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(w.to_le_bytes().as_ptr(), at.add(1), 4);
        std::ptr::copy_nonoverlapping(h.to_le_bytes().as_ptr(), at.add(6), 4);
        VirtualProtect(at.cast(), 10, old, &mut old);
    }
    log!("screen: picture {w}x{h}");
}

type CreateWindowExA = unsafe extern "system" fn(
    u32, *const u8, *const u8, u32, i32, i32, i32, i32, *mut c_void, *mut c_void, *mut c_void, *mut c_void,
) -> *mut c_void;

unsafe extern "system" fn create_window(
    ex: u32, class: *const u8, name: *const u8, style: u32, _x: i32, _y: i32, w: i32, h: i32,
    parent: *mut c_void, menu: *mut c_void, instance: *mut c_void, param: *mut c_void,
) -> *mut c_void {
    let orig: CreateWindowExA = unsafe { std::mem::transmute(ORIG_CREATE.load(Ordering::Relaxed)) };
    let style = if parent.is_null() { WS_POPUP | (style & WS_VISIBLE) } else { style };
    unsafe { orig(ex, class, name, style, 0, 0, w, h, parent, menu, instance, param) }
}

unsafe extern "system" fn set_window_long(hwnd: *mut c_void, index: i32, value: i32) -> i32 {
    let orig: unsafe extern "system" fn(*mut c_void, i32, i32) -> i32 =
        unsafe { std::mem::transmute(ORIG_SET_LONG.load(Ordering::Relaxed)) };
    let value = if index == GWL_STYLE { (WS_POPUP | (value as u32 & WS_VISIBLE)) as i32 } else { value };
    unsafe { orig(hwnd, index, value) }
}

pub fn init() {
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    if w > 0 && h > 0 {
        unsafe { screen_size(w as u32, h as u32) };
    }
    let ek = module("ek-lib.dll");
    if ek == 0 {
        return;
    }
    unsafe {
        if let Some(o) = iat::hook_module(ek, "user32.dll", "CreateWindowExA", create_window as *const () as usize) {
            ORIG_CREATE.store(o, Ordering::Relaxed);
        }
        if let Some(o) = iat::hook_module(ek, "user32.dll", "SetWindowLongA", set_window_long as *const () as usize) {
            ORIG_SET_LONG.store(o, Ordering::Relaxed);
        }
    }
}
