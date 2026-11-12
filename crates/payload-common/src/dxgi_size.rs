//! DXGI swap chains on a game desktop other than the real screen (`WAL_DESKTOP_SIZE`): always
//! windowed, their back buffer stretched to the window when presented. A full screen swap chain
//! asks for the desktop's mode, which the real screen lacks: Luigi's Mansion Arcade's 1920x1080
//! one showed its picture unscaled on a 1600x1200 screen (cut on the right). One sized by its
//! window (width / height 0: `CreateSwapChain`, `ResizeBuffers`) gets the desktop's size: DXGI
//! measures the real window, not the game's desktop units.
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::log;
use windows_sys::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW, GetProcAddress};
use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};
use windows_sys::core::GUID;

type P = *mut c_void;
type HRESULT = i32;

/// IID_IDXGIFactory1
const IID_FACTORY1: GUID = GUID::from_u128(0x770aae78_f26f_4dba_a829_253c83d1b387);

static CREATE_SWAP_CHAIN: AtomicUsize = AtomicUsize::new(0);
static CREATE_SWAP_CHAIN_HWND: AtomicUsize = AtomicUsize::new(0);
static RESIZE_BUFFERS: AtomicUsize = AtomicUsize::new(0);
static SET_FULLSCREEN_STATE: AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    match crate::desktop::size() {
        Some(size) if size != crate::desktop::screen_size() => {}
        _ => return,
    }
    std::thread::spawn(hook_factory);
}

/// A vtable slot replaced, its previous function kept in `orig` (once).
fn hook_slot(obj: P, index: usize, hook: usize, orig: &AtomicUsize) {
    let slot = unsafe { (*(obj as *const *mut usize)).add(index) };
    if unsafe { *slot } == hook {
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(slot.cast(), 8, PAGE_READWRITE, &mut old);
        orig.store(*slot, Ordering::Relaxed);
        *slot = hook;
        VirtualProtect(slot.cast(), 8, old, &mut old);
    }
}

fn hook_factory() {
    // pinned once loaded: a game loading it only to look at the adapters (ReRave's Unity 3.5,
    // a Direct3D 9 game) unloaded it under the factory's creation here (a crash in its freed code)
    let dxgi = loop {
        let mut m = std::ptr::null_mut();
        if unsafe { GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_PIN, windows_sys::w!("dxgi.dll"), &mut m) } != 0 && !m.is_null() {
            break m;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let Some(create) = (unsafe { GetProcAddress(dxgi, c"CreateDXGIFactory1".as_ptr().cast()) }) else {
        log!("dxgi size: CreateDXGIFactory1 not found");
        return;
    };
    let create: unsafe extern "system" fn(*const GUID, *mut P) -> HRESULT = unsafe { std::mem::transmute(create) };
    let mut factory: P = std::ptr::null_mut();
    if unsafe { create(&IID_FACTORY1, &mut factory) } < 0 || factory.is_null() {
        log!("dxgi size: no factory");
        return;
    }
    // IDXGIFactory::CreateSwapChain (D3D11CreateDeviceAndSwapChain's too), IDXGIFactory2::
    // CreateSwapChainForHwnd (the same vtable in DXVK's factory)
    hook_slot(factory, 10, create_swap_chain as *const () as usize, &CREATE_SWAP_CHAIN);
    hook_slot(factory, 15, create_swap_chain_hwnd as *const () as usize, &CREATE_SWAP_CHAIN_HWND);
    let release: unsafe extern "system" fn(P) -> u32 = unsafe { std::mem::transmute(*(*(factory as *const *const usize)).add(2)) };
    unsafe { release(factory) };
    log!("dxgi size: swap chains windowed, the desktop's size {:?} for their window's", crate::desktop::size().unwrap());
}

/// A width / height of 0 (the window's real client size) -> the desktop's.
fn desktop_size(what: &str, width: &mut u32, height: &mut u32) {
    let (w, h) = crate::desktop::size().unwrap();
    if *width == 0 || *height == 0 {
        log!("dxgi size: {what} {}x{} -> {w}x{h}", *width, *height);
        *width = w as u32;
        *height = h as u32;
    } else {
        log!("dxgi size: {what} {}x{}", *width, *height);
    }
}

/// `IDXGIFactory::CreateSwapChain`: DXGI_SWAP_CHAIN_DESC (BufferDesc's Width, Height first).
unsafe extern "system" fn create_swap_chain(factory: P, device: P, desc: *mut u32, out: *mut P) -> HRESULT {
    let orig: unsafe extern "system" fn(P, P, *mut u32, *mut P) -> HRESULT = unsafe { std::mem::transmute(CREATE_SWAP_CHAIN.load(Ordering::Relaxed)) };
    if !desc.is_null() {
        unsafe { desktop_size("swap chain", &mut *desc, &mut *desc.add(1)) };
        // BOOL Windowed (dword 14)
        if unsafe { *desc.add(14) } == 0 {
            log!("dxgi size: the full screen swap chain made windowed");
            unsafe { *desc.add(14) = 1 };
        }
    }
    let hr = unsafe { orig(factory, device, desc, out) };
    if hr >= 0 && !out.is_null() && !unsafe { *out }.is_null() {
        hook_swap_chain(unsafe { *out });
    }
    hr
}

/// `IDXGIFactory2::CreateSwapChainForHwnd`: DXGI_SWAP_CHAIN_DESC1 (Width, Height first).
unsafe extern "system" fn create_swap_chain_hwnd(factory: P, device: P, hwnd: P, desc: *mut u32, _fullscreen: P, output: P, out: *mut P) -> HRESULT {
    let orig: unsafe extern "system" fn(P, P, P, *mut u32, P, P, *mut P) -> HRESULT = unsafe { std::mem::transmute(CREATE_SWAP_CHAIN_HWND.load(Ordering::Relaxed)) };
    if !desc.is_null() {
        unsafe { desktop_size("swap chain", &mut *desc, &mut *desc.add(1)) };
    }
    // DXGI_SWAP_CHAIN_FULLSCREEN_DESC: windowed
    let hr = unsafe { orig(factory, device, hwnd, desc, std::ptr::null_mut(), output, out) };
    if hr >= 0 && !out.is_null() && !unsafe { *out }.is_null() {
        hook_swap_chain(unsafe { *out });
    }
    hr
}

fn hook_swap_chain(swap: P) {
    hook_slot(swap, 10, set_fullscreen_state as *const () as usize, &SET_FULLSCREEN_STATE);
    hook_slot(swap, 13, resize_buffers as *const () as usize, &RESIZE_BUFFERS);
}

/// `IDXGISwapChain::SetFullscreenState`: always windowed.
unsafe extern "system" fn set_fullscreen_state(swap: P, fullscreen: i32, output: P) -> HRESULT {
    let orig: unsafe extern "system" fn(P, i32, P) -> HRESULT = unsafe { std::mem::transmute(SET_FULLSCREEN_STATE.load(Ordering::Relaxed)) };
    if fullscreen != 0 {
        log!("dxgi size: full screen state ignored");
        return 0;
    }
    unsafe { orig(swap, 0, output) }
}

/// `IDXGISwapChain::ResizeBuffers`.
unsafe extern "system" fn resize_buffers(swap: P, count: u32, mut width: u32, mut height: u32, format: u32, flags: u32) -> HRESULT {
    let orig: unsafe extern "system" fn(P, u32, u32, u32, u32, u32) -> HRESULT = unsafe { std::mem::transmute(RESIZE_BUFFERS.load(Ordering::Relaxed)) };
    desktop_size("resize buffers", &mut width, &mut height);
    unsafe { orig(swap, count, width, height, format, flags) }
}
