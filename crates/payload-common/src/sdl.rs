//! SDL 1.2 games: `WAL_SDL_FULLSCREEN=1` adds `SDL_FULLSCREEN` to the game's
//! `SDL.dll!SDL_SetVideoMode` calls (Exception's config runs it in a window).

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{iat, log};

const SDL_FULLSCREEN: u32 = 0x8000_0000;

static ORIG_SET_VIDEO_MODE: AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    if std::env::var("WAL_SDL_FULLSCREEN").map_or(true, |v| v != "1") {
        return;
    }
    if let Some(o) = unsafe { iat::hook("SDL.dll", "SDL_SetVideoMode", set_video_mode as *const () as usize) } {
        ORIG_SET_VIDEO_MODE.store(o, Ordering::Relaxed);
        log!("sdl: video modes made fullscreen");
    }
}

/// SDLCALL is cdecl on win32.
unsafe extern "C" fn set_video_mode(width: i32, height: i32, bpp: i32, flags: u32) -> *mut c_void {
    let orig: unsafe extern "C" fn(i32, i32, i32, u32) -> *mut c_void =
        unsafe { std::mem::transmute(ORIG_SET_VIDEO_MODE.load(Ordering::Relaxed)) };
    log!("sdl: SetVideoMode {width}x{height}x{bpp} flags {flags:#x} -> {:#x}", flags | SDL_FULLSCREEN);
    unsafe { orig(width, height, bpp, flags | SDL_FULLSCREEN) }
}
