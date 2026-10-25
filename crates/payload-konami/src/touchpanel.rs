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
/// A touch made by the payload (a choice taken for the player): x, y in 1/65535 of the panel,
/// down, and whether it is on.
static AUTO_TOUCH: AtomicU32 = AtomicU32::new(0);
static AUTO_DOWN: AtomicBool = AtomicBool::new(false);
static AUTO_ON: AtomicBool = AtomicBool::new(false);

/// Touches the panel at (x, y), fractions of it, `down` or not, in place of the player's
/// touch; `None` gives it back.
pub fn set_auto_touch(touch: Option<(f32, f32, bool)>) {
    match touch {
        Some((x, y, down)) => {
            let fx = (x.clamp(0.0, 1.0) * 65535.0) as u32;
            let fy = (y.clamp(0.0, 1.0) * 65535.0) as u32;
            AUTO_TOUCH.store(fx << 16 | fy, Ordering::Relaxed);
            AUTO_DOWN.store(down, Ordering::Relaxed);
            AUTO_ON.store(true, Ordering::Relaxed);
        }
        None => AUTO_ON.store(false, Ordering::Relaxed),
    }
}

static FULL_SCREEN: AtomicBool = AtomicBool::new(false);

pub fn set_full_screen(full: bool) {
    FULL_SCREEN.store(full, Ordering::Relaxed);
}

/// The game calibrates the panel in screen pixels (Winning Eleven 2012): the panel's raw point is
/// the pointer's place in screen pixels (`read_on_direct`, what the test menu's calibration
/// reads), its point the raw one through the game's calibration (`ac_touchpanel_caribrate_set`:
/// two raw points and the screen points they are, identity until calibrated).
static SCREEN_CALIBRATION: AtomicBool = AtomicBool::new(false);

pub fn set_screen_calibration(on: bool) {
    SCREEN_CALIBRATION.store(on, Ordering::Relaxed);
}

/// A panel: the size of the coordinates it reports, when a diagnostic answer is due.
pub struct Panel {
    width: i32,
    height: i32,
    answer_at: Instant,
    /// The game's calibration (screen calibration): raw x0, y0, screen x0, y0, raw x1, y1,
    /// screen x1, y1.
    calibration: Option<[f32; 8]>,
}

#[unsafe(no_mangle)]
pub extern "C" fn ac_touchpanel_new(width: i32, height: i32) -> *mut Panel {
    log!("touchpanel: {width}x{height}");
    Box::into_raw(Box::new(Panel { width, height, answer_at: Instant::now(), calibration: None }))
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
    panel: *mut Panel, x0: i32, y0: i32, x1: i32, y1: i32, width: i32, height: i32, a: i32, b: i32,
) -> bool {
    let Some(panel) = (unsafe { panel.as_mut() }) else { return false };
    if SCREEN_CALIBRATION.load(Ordering::Relaxed) {
        // raw (x0, y0) is screen (x1, y1), raw (width, height) is screen (a, b)
        let c = [x0, y0, x1, y1, width, height, a, b].map(|v| v as f32);
        if c[4] != c[0] && c[5] != c[1] {
            panel.calibration = Some(c);
        }
        return true;
    }
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
    let mask = TOUCH_BUTTON.load(Ordering::Relaxed);
    // the point, in fractions of the screen (the pointer), or of the panel (the payload's touch)
    let auto = AUTO_ON.load(Ordering::Relaxed);
    let (sx, sy, touched) = if auto {
        let t = AUTO_TOUCH.load(Ordering::Relaxed);
        ((t >> 16) as f32 / 65535.0, (t & 0xffff) as f32 / 65535.0, AUTO_DOWN.load(Ordering::Relaxed))
    } else {
        let sx = (stick.axis(Axis::RightX) as f32 + 32768.0) / 65535.0;
        let sy = (stick.axis(Axis::RightY) as f32 + 32768.0) / 65535.0;
        (sx, sy, mask != 0 && stick.pressed(mask))
    };
    let (px, py, max_x, max_y) = if SCREEN_CALIBRATION.load(Ordering::Relaxed) {
        let (raw_x, raw_y, sw, sh) = screen_raw(sx, sy);
        let (px, py) = match panel.calibration {
            Some(c) => (c[2] + (raw_x - c[0]) * (c[6] - c[2]) / (c[4] - c[0]), c[3] + (raw_y - c[1]) * (c[7] - c[3]) / (c[5] - c[1])),
            None => (raw_x, raw_y),
        };
        (px, py, sw as i32 - 1, sh as i32 - 1)
    } else if auto {
        (sx * panel.width as f32, sy * panel.height as f32, panel.width - 1, panel.height - 1)
    } else {
        let (x0, y0, w, h) = picture(panel);
        ((sx - x0) / w * panel.width as f32, (sy - y0) / h * panel.height as f32, panel.width - 1, panel.height - 1)
    };
    unsafe {
        if !x.is_null() {
            *x = (px as i32).clamp(0, max_x);
        }
        if !y.is_null() {
            *y = (py as i32).clamp(0, max_y);
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
    if !SCREEN_CALIBRATION.load(Ordering::Relaxed) || AUTO_ON.load(Ordering::Relaxed) {
        return ac_touchpanel_read_on(panel, x, y, z, down);
    }
    // the raw point: the pointer in screen pixels
    let stick = wal_payload_common::input(0);
    let sx = (stick.axis(Axis::RightX) as f32 + 32768.0) / 65535.0;
    let sy = (stick.axis(Axis::RightY) as f32 + 32768.0) / 65535.0;
    let (raw_x, raw_y, sw, sh) = screen_raw(sx, sy);
    let mask = TOUCH_BUTTON.load(Ordering::Relaxed);
    let touched = mask != 0 && stick.pressed(mask);
    unsafe {
        if !x.is_null() {
            *x = (raw_x as i32).clamp(0, sw as i32 - 1);
        }
        if !y.is_null() {
            *y = (raw_y as i32).clamp(0, sh as i32 - 1);
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

/// The pointer (fractions of the screen) in screen pixels, and the screen's size.
fn screen_raw(sx: f32, sy: f32) -> (f32, f32, f32, f32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN) as f32, GetSystemMetrics(SM_CYSCREEN) as f32) };
    (sx * sw, sy * sh, sw.max(1.0), sh.max(1.0))
}
