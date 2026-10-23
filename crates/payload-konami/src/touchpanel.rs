//! The touch screen (Otomedius): `libtouchpanel.dll`, a serial touch controller, answered here.
//!
//! The game creates the panel with its picture size (`ac_touchpanel_new(1280, 1024)`) and reads
//! the touch in it (`ac_touchpanel_read_on`: x, y, pressure, touched). The touch is player 1's
//! right stick position over the screen (the profile's mouse or gun drives `rx`/`ry`) and its
//! virtual button 4 (the mouse button), as spice2x answers its Winning Eleven touch panel. The
//! pointer spans the screen, the game's 1280x1024 picture is fitted in it with its aspect
//! (pillarboxed on a wide screen): the touch is the pointer's place in the picture.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use wal_payload_common::log;
use wal_protocol::{Axis, button};

/// The virtual button touching the screen (none: never touched).
static TOUCH_BUTTON: AtomicU32 = AtomicU32::new(button::B4);

/// Sets the button touching the screen (0: none, the screen is not played).
pub fn set_touch_button(mask: u32) {
    TOUCH_BUTTON.store(mask, Ordering::Relaxed);
}

/// The panel spans the whole screen, whatever its coordinates' aspect (Winning Eleven 2012's
/// 640x480 panel over its 16:9 picture).
static FULL_SCREEN: AtomicBool = AtomicBool::new(false);

pub fn set_full_screen(full: bool) {
    FULL_SCREEN.store(full, Ordering::Relaxed);
}

/// A panel: the size of the coordinates it reports, when a diagnostic answer is due.
pub struct Panel {
    width: i32,
    height: i32,
    answer_at: Instant,
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_touchpanel_new(width: i32, height: i32) -> *mut Panel {
    log!("touchpanel: {width}x{height}");
    Box::into_raw(Box::new(Panel { width, height, answer_at: Instant::now() }))
}

/// Winning Eleven 2008's constructor: the serial port's number too.
#[unsafe(no_mangle)]
pub extern "C" fn ac_touchpanel_new2(width: i32, height: i32, _port: u8) -> *mut Panel {
    ac_touchpanel_new(width, height)
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_touchpanel_delete(panel: *mut Panel) -> bool {
    if panel.is_null() {
        return false;
    }
    drop(unsafe { Box::from_raw(panel) });
    true
}

/// The coordinates' range (raw 0..0xfff onto the picture size, which is all this panel has).
#[unsafe(no_mangle)]
pub extern "C" fn ac_touchpanel_caribrate_set(
    panel: *mut Panel, _x0: i32, _y0: i32, _x1: i32, _y1: i32, width: i32, height: i32, _a: i32, _b: i32,
) -> bool {
    let Some(panel) = (unsafe { panel.as_mut() }) else { return false };
    if width > 0 && height > 0 {
        panel.width = width;
        panel.height = height;
    }
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_touchpanel_polling(_panel: *mut Panel) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_touchpanel_renewal(_panel: *mut Panel) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_touchpanel_set_keepalive_mode_on(_panel: *mut Panel) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_touchpanel_set_keepalive_mode_off(_panel: *mut Panel) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_touchpanel_set_keepalive_mode(_panel: *mut Panel, _on: u8, _a: i32, _b: i32) -> bool {
    true
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_touchpanel_get_keepalive_error(_panel: *mut Panel) -> bool {
    false
}

// The boot's diagnostic: sent, answered after a moment, passed.

#[unsafe(no_mangle)]
pub extern "C" fn ac_touchpanel_send_check_diag(panel: *mut Panel) -> bool {
    let Some(panel) = (unsafe { panel.as_mut() }) else { return false };
    panel.answer_at = Instant::now() + Duration::from_millis(128);
    true
}

/// Still waiting for the panel's answer (Otomedius reads it once this is false).
#[unsafe(no_mangle)]
pub extern "C" fn ac_touchpanel_check_wait_receive(panel: *mut Panel) -> bool {
    unsafe { panel.as_ref() }.is_some_and(|p| Instant::now() < p.answer_at)
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_touchpanel_receive_check_diag(_panel: *mut Panel, ok: *mut u8) -> bool {
    if !ok.is_null() {
        unsafe { *ok = 1 };
    }
    true
}

/// The touch: player 1's right stick over the panel, touching while button 4 is held.
#[unsafe(no_mangle)]
pub extern "C" fn ac_touchpanel_read_on(panel: *mut Panel, x: *mut i32, y: *mut i32, z: *mut i32, down: *mut u32) -> bool {
    let Some(panel) = (unsafe { panel.as_ref() }) else { return false };
    let stick = wal_payload_common::input(0);
    let sx = (stick.axis(Axis::RightX) as f32 + 32768.0) / 65535.0;
    let sy = (stick.axis(Axis::RightY) as f32 + 32768.0) / 65535.0;
    let (x0, y0, w, h) = picture(panel);
    let px = (sx - x0) / w * panel.width as f32;
    let py = (sy - y0) / h * panel.height as f32;
    let mask = TOUCH_BUTTON.load(Ordering::Relaxed);
    let touched = mask != 0 && stick.pressed(mask);
    unsafe {
        if !x.is_null() {
            *x = (px as i32).clamp(0, panel.width - 1);
        }
        if !y.is_null() {
            *y = (py as i32).clamp(0, panel.height - 1);
        }
        if !z.is_null() {
            *z = if touched { 0xff } else { 0 };
        }
        if !down.is_null() {
            *down = touched as u32;
        }
    }
    true
}

/// The picture's rectangle on the screen, in fractions of it: the panel's aspect fitted.
fn picture(panel: &Panel) -> (f32, f32, f32, f32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN) as f32, GetSystemMetrics(SM_CYSCREEN) as f32) };
    if FULL_SCREEN.load(Ordering::Relaxed) || sw <= 0.0 || sh <= 0.0 || panel.height <= 0 {
        return (0.0, 0.0, 1.0, 1.0);
    }
    let aspect = panel.width as f32 / panel.height as f32;
    if sw / sh > aspect {
        let w = sh * aspect / sw;
        ((1.0 - w) / 2.0, 0.0, w, 1.0)
    } else {
        let h = sw / aspect / sh;
        (0.0, (1.0 - h) / 2.0, 1.0, h)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_touchpanel_read_on_direct(panel: *mut Panel, x: *mut i32, y: *mut i32, z: *mut i32, down: *mut u32) -> bool {
    ac_touchpanel_read_on(panel, x, y, z, down)
}
