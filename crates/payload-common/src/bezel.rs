//! A bezel around the game's picture: `WAL_BEZEL=<image>` (PNG, JPEG..., a path relative to the
//! game's directory or absolute), or `WAL_BEZEL=black`: plain black strips, no image (a 4:3
//! game centered on a wide screen whose sides nothing paints: PGA TOUR Team Challenge's Anark
//! frontend, `WAL_WINDOW_SIZE=picture`).
//!
//! For a game whose window does not fill the screen (a portrait game fitted with
//! `WAL_DESKTOP_SIZE`): the image, scaled to fit the screen (black around, its transparent
//! parts black), is shown by windows of their own around the game's window (left, right, top,
//! bottom strips, following it). None covers the game's picture: a full-screen window behind
//! it was kept above it by the window manager (GNOME: a full-screen window's layer), and
//! transparency needs a compositor. The image is decoded with GDI+, loaded at run time.
//!
//! The strips hide while a window of another program is in front (a game started by its
//! frontend: the frontend's strips covered the race's sides).
//!
//! `WAL_BEZEL_FIT=1`: the image's opening (the dark area at its center: black or transparent)
//! fitted to the game's window. The image keeps its scale (fitted to the screen's height); its
//! parts left and right of the opening are drawn against the window's left and right edges
//! (Spider-Man Homecoming: an opening wider than the game's picture). No stretching: the art
//! keeps its aspect, the screen's edges black. `WAL_BEZEL_FIT=stretch`: those parts stretched
//! across to the screen's edges, the height still the screen's (Disney Crossy Road: a 16:9
//! image whose parts left black edges on the screen).

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::log;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BITMAPINFO, BITMAPINFOHEADER, BLACK_BRUSH, BeginPaint, DIB_RGB_COLORS, GetDIBits, CreateCompatibleDC, DeleteDC, EndPaint, FillRect, GetStockObject, HALFTONE, HBITMAP, InvalidateRect,
    PAINTSTRUCT, SRCCOPY, SelectObject, SetStretchBltMode, StretchBlt,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, EnumWindows, GetClientRect, GetForegroundWindow, GetMessageW, GetSystemMetrics,
    GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, MSG, RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER, SetTimer, SetWindowPos, ShowWindow, TranslateMessage, WM_ERASEBKGND,
    WM_PAINT, WM_TIMER, WNDCLASSW, WS_EX_NOACTIVATE, WS_POPUP,
};

type P = *mut c_void;

/// The image (HBITMAP), its width and height.
static IMAGE: AtomicUsize = AtomicUsize::new(0);
static IMAGE_W: AtomicUsize = AtomicUsize::new(0);
static IMAGE_H: AtomicUsize = AtomicUsize::new(0);
/// `WAL_BEZEL_FIT`: the opening's left and right edges in the image (0, 0: not fitted).
static HOLE_L: AtomicUsize = AtomicUsize::new(0);
static HOLE_R: AtomicUsize = AtomicUsize::new(0);
/// `WAL_BEZEL_FIT=stretch`: the image's parts stretched to the screen's edges.
static STRETCH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The game's window left and right edges (screen), for the fitted drawing.
static GAME_L: AtomicUsize = AtomicUsize::new(0);
static GAME_R: AtomicUsize = AtomicUsize::new(0);

/// The opening of the image `hbm` (`w` x `h`): the dark columns around its center, on its
/// middle row. `None`: no dark center.
fn opening(hbm: HBITMAP, w: u32, h: u32) -> Option<(u32, u32)> {
    let mut info: BITMAPINFO = unsafe { std::mem::zeroed() };
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = w as i32;
    // top-down, 32-bit BGRX
    info.bmiHeader.biHeight = -(h as i32);
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    let mut pixels = vec![0u32; w as usize * h as usize];
    let dc = unsafe { CreateCompatibleDC(std::ptr::null_mut()) };
    let n = unsafe { GetDIBits(dc, hbm, 0, h, pixels.as_mut_ptr().cast(), &mut info, DIB_RGB_COLORS) };
    unsafe { DeleteDC(dc) };
    if n == 0 {
        return None;
    }
    let row = &pixels[(h / 2) as usize * w as usize..][..w as usize];
    let dark = |x: u32| {
        let p = row[x as usize];
        (p & 0xFF) < 24 && (p >> 8 & 0xFF) < 24 && (p >> 16 & 0xFF) < 24
    };
    let c = w / 2;
    if !dark(c) {
        return None;
    }
    let mut l = c;
    while l > 0 && dark(l - 1) {
        l -= 1;
    }
    let mut r = c;
    while r + 1 < w && dark(r + 1) {
        r += 1;
    }
    // the edges of the image: no frame around a dark center
    (l > 0 && r + 1 < w).then_some((l, r + 1))
}

/// Decodes `path` into a bitmap over black (GDI+), with its size.
fn load(path: &str) -> Option<(HBITMAP, u32, u32)> {
    /// GdiplusStartupInput: version, debug callback (a pointer: padded on 64-bit), background
    /// thread suppressed, external codecs suppressed.
    #[repr(C)]
    struct StartupInput {
        version: u32,
        debug: usize,
        no_thread: i32,
        no_codecs: i32,
    }
    type Startup = unsafe extern "system" fn(*mut usize, *const StartupInput, P) -> i32;
    type FromFile = unsafe extern "system" fn(*const u16, *mut P) -> i32;
    type ToHbitmap = unsafe extern "system" fn(P, *mut HBITMAP, u32) -> i32;
    type Size = unsafe extern "system" fn(P, *mut u32) -> i32;
    let gdiplus = unsafe { LoadLibraryW(windows_sys::w!("gdiplus.dll")) };
    if gdiplus.is_null() {
        return None;
    }
    let f = |name: &[u8]| unsafe { GetProcAddress(gdiplus, name.as_ptr()) };
    let (startup, from_file, to_hbitmap, width, height) = (
        f(b"GdiplusStartup\0")?,
        f(b"GdipCreateBitmapFromFile\0")?,
        f(b"GdipCreateHBITMAPFromBitmap\0")?,
        f(b"GdipGetImageWidth\0")?,
        f(b"GdipGetImageHeight\0")?,
    );
    unsafe {
        let mut token = 0usize;
        // version 1, no callback, background thread, codecs
        let input = StartupInput { version: 1, debug: 0, no_thread: 0, no_codecs: 0 };
        let status = std::mem::transmute::<_, Startup>(startup)(&mut token, &input, std::ptr::null_mut());
        if status != 0 {
            log!("bezel: GdiplusStartup status {status}");
            return None;
        }
        let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
        let mut bitmap: P = std::ptr::null_mut();
        let status = std::mem::transmute::<_, FromFile>(from_file)(wide.as_ptr(), &mut bitmap);
        if status != 0 {
            log!("bezel: {path}: GDI+ status {status}");
            return None;
        }
        let (mut w, mut h) = (0u32, 0u32);
        std::mem::transmute::<_, Size>(width)(bitmap, &mut w);
        std::mem::transmute::<_, Size>(height)(bitmap, &mut h);
        let mut hbm: HBITMAP = std::ptr::null_mut();
        // the transparent parts over opaque black
        if std::mem::transmute::<_, ToHbitmap>(to_hbitmap)(bitmap, &mut hbm, 0xFF00_0000) != 0 {
            return None;
        }
        Some((hbm, w, h))
    }
}

unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            unsafe {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                GetClientRect(hwnd, &mut client);
                FillRect(dc, &client, GetStockObject(BLACK_BRUSH) as _);
                // this strip's part of the image fitted on the screen
                let mut at = RECT { left: 0, top: 0, right: 0, bottom: 0 };
                GetWindowRect(hwnd, &mut at);
                let (iw, ih) = (IMAGE_W.load(Ordering::Relaxed) as f64, IMAGE_H.load(Ordering::Relaxed) as f64);
                let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
                if iw > 0.0 && ih > 0.0 {
                    let s = (sw as f64 / iw).min(sh as f64 / ih);
                    let (w, h) = ((iw * s).round() as i32, (ih * s).round() as i32);
                    let (x, y) = ((sw - w) / 2 - at.left, (sh - h) / 2 - at.top);
                    let mem = CreateCompatibleDC(dc);
                    let old = SelectObject(mem, IMAGE.load(Ordering::Relaxed) as _);
                    SetStretchBltMode(dc, HALFTONE);
                    let (hl, hr) = (HOLE_L.load(Ordering::Relaxed) as i32, HOLE_R.load(Ordering::Relaxed) as i32);
                    let (gl, gr) = (GAME_L.load(Ordering::Relaxed) as i32, GAME_R.load(Ordering::Relaxed) as i32);
                    if hr > hl && gr > gl {
                        // the parts left and right of the opening against the game's edges
                        // (the image's scale kept: its height fitted to the screen; `stretch`:
                        // each part stretched to the screen's edge)
                        let s = sh as f64 / ih;
                        let (h, y) = ((ih * s).round() as i32, -at.top);
                        let (mut lw, mut rw) = ((hl as f64 * s).round() as i32, ((iw as i32 - hr) as f64 * s).round() as i32);
                        if STRETCH.load(Ordering::Relaxed) {
                            (lw, rw) = (gl, sw - gr);
                        }
                        StretchBlt(dc, gl - lw - at.left, y, lw, h, mem, 0, 0, hl, ih as i32, SRCCOPY);
                        StretchBlt(dc, gr - at.left, y, rw, h, mem, hr, 0, iw as i32 - hr, ih as i32, SRCCOPY);
                    } else {
                        StretchBlt(dc, x, y, w, h, mem, 0, 0, iw as i32, ih as i32, SRCCOPY);
                    }
                    SelectObject(mem, old);
                    DeleteDC(mem);
                }
                EndPaint(hwnd, &ps);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// The strips' windows (left, right, top, bottom).
static STRIPS: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];

fn is_strip(hwnd: HWND) -> bool {
    STRIPS.iter().any(|s| s.load(Ordering::Relaxed) == hwnd as usize)
}

/// The game's window: a visible top-level window of this process, not a strip.
fn game_window() -> HWND {
    unsafe extern "system" fn each(hwnd: HWND, found: LPARAM) -> i32 {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        let ours = pid == unsafe { GetCurrentProcessId() };
        if ours && !is_strip(hwnd) && unsafe { IsWindowVisible(hwnd) } != 0 {
            unsafe { *(found as *mut HWND) = hwnd };
            return 0;
        }
        1
    }
    let mut found: HWND = std::ptr::null_mut();
    unsafe { EnumWindows(Some(each), &mut found as *mut HWND as LPARAM) };
    found
}

/// The strips around the game's window `game` (screen coordinates): left, right, top, bottom.
fn strips(game: RECT, sw: i32, sh: i32) -> [RECT; 4] {
    let (l, t, r, b) = (game.left.clamp(0, sw), game.top.clamp(0, sh), game.right.clamp(0, sw), game.bottom.clamp(0, sh));
    [
        RECT { left: 0, top: 0, right: l, bottom: sh },
        RECT { left: r, top: 0, right: sw, bottom: sh },
        RECT { left: l, top: 0, right: r, bottom: t },
        RECT { left: l, top: b, right: r, bottom: sh },
    ]
}

/// The strips' windows and their message loop, following the game's window.
fn run(image: HBITMAP) {
    // created once the game has a window of its own (PGA TOUR Team Challenge's frontend stayed
    // on its startup checks' "Please wait..." with the strips made first)
    while game_window().is_null() {
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
    let class = windows_sys::w!("WalBezel");
    let wc = WNDCLASSW { lpfnWndProc: Some(window_proc), hInstance: instance, lpszClassName: class, ..unsafe { std::mem::zeroed() } };
    unsafe { RegisterClassW(&wc) };
    IMAGE.store(image as usize, Ordering::Relaxed);
    for strip in &STRIPS {
        let hwnd = unsafe {
            CreateWindowExW(WS_EX_NOACTIVATE, class, windows_sys::w!("Bezel"), WS_POPUP, 0, 0, 1, 1, std::ptr::null_mut(), std::ptr::null_mut(), instance, std::ptr::null())
        };
        strip.store(hwnd as usize, Ordering::Relaxed);
    }
    let first = STRIPS[0].load(Ordering::Relaxed) as HWND;
    if first.is_null() {
        log!("bezel: no window");
        return;
    }
    let timer = unsafe { SetTimer(first, 1, 250, None) };
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let mut placed: Option<RECT> = None;
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if timer == 0 || msg.message != WM_TIMER {
            continue;
        }
        // another program in front (the race Need for Speed Underground's frontend starts, with a
        // bezel of its own): hidden, placed again when the program's window is back in front
        let front = unsafe { GetForegroundWindow() };
        let mut pid = 0;
        if !front.is_null() {
            unsafe { GetWindowThreadProcessId(front, &mut pid) };
        }
        if !front.is_null() && pid != unsafe { GetCurrentProcessId() } {
            if placed.take().is_some() {
                for strip in &STRIPS {
                    unsafe { ShowWindow(strip.load(Ordering::Relaxed) as HWND, SW_HIDE) };
                }
                log!("bezel: hidden (another program in front)");
            }
            continue;
        }
        let game = game_window();
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        if game.is_null() || unsafe { GetWindowRect(game, &mut rect) } == 0 {
            continue;
        }
        let same = placed.is_some_and(|p| (p.left, p.top, p.right, p.bottom) == (rect.left, rect.top, rect.right, rect.bottom));
        if same {
            continue;
        }
        GAME_L.store(rect.left.max(0) as usize, Ordering::Relaxed);
        GAME_R.store(rect.right.max(0) as usize, Ordering::Relaxed);
        for (strip, r) in STRIPS.iter().zip(strips(rect, sw, sh)) {
            let hwnd = strip.load(Ordering::Relaxed) as HWND;
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            unsafe {
                if w > 0 && h > 0 {
                    SetWindowPos(hwnd, std::ptr::null_mut(), r.left, r.top, w, h, SWP_NOACTIVATE | SWP_NOZORDER);
                    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                    InvalidateRect(hwnd, std::ptr::null(), 0);
                } else {
                    ShowWindow(hwnd, SW_HIDE);
                }
            }
        }
        log!("bezel: around the game's window ({},{})-({},{})", rect.left, rect.top, rect.right, rect.bottom);
        placed = Some(rect);
    }
}
pub fn init() {
    let Ok(path) = std::env::var("WAL_BEZEL") else { return };
    let path = path.trim().to_string();
    if path.is_empty() || path == "0" {
        return;
    }
    if path == "black" {
        // no image: the strips are only painted black
        log!("bezel: black");
        std::thread::spawn(|| run(std::ptr::null_mut()));
        return;
    }
    std::thread::spawn(move || match load(&path) {
        Some((image, w, h)) => {
            IMAGE_W.store(w as usize, Ordering::Relaxed);
            IMAGE_H.store(h as usize, Ordering::Relaxed);
            log!("bezel: {path} ({w}x{h})");
            let mode = std::env::var("WAL_BEZEL_FIT").unwrap_or_default();
            let fit = matches!(mode.trim(), "1" | "stretch");
            STRETCH.store(mode.trim() == "stretch", Ordering::Relaxed);
            match opening(image, w, h).filter(|_| fit) {
                Some((l, r)) => {
                    HOLE_L.store(l as usize, Ordering::Relaxed);
                    HOLE_R.store(r as usize, Ordering::Relaxed);
                    log!("bezel: opening x {l}..{r} fitted to the game's window");
                }
                None if fit => log!("bezel: no opening found, not fitted"),
                None => {}
            }
            run(image);
        }
        None => log!("bezel: cannot load {path}"),
    });
}
