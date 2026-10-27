//! Need for Speed Carbon's cabinet frontend, not in the dump: a race selection screen before
//! the game (`WAL_GLOBALVR_CARBON_MENU=1`).
//!
//! The game plays one race per run, the race and the car on its command line (`-race ct.3.1
//! -car carreragt -numai 3 [-trans 1]`, what the cabinet frontend passed: the dump's debug
//! `Launcher.exe` builds the same), then exits, the result in the registry
//! (`HKLM\Software\GlobalVR\CARBON`, `Result_*`).
//!
//! The C runtime's `__getmainargs` (the game's arguments, called by its startup code before
//! `WinMain`) is hooked: the payload shows its own screen there (a window drawn with GDI, fed
//! by the virtual sticks), the player chooses the track, the car and the transmission, and the
//! game gets these arguments after its own. Each run asks the launcher to start the game again
//! when it exits (the exit key still ends it): the screen comes back after the race, with its
//! result.
//!
//! * `WAL_CARBON_AI`: the computer opponents, 0..=7 (default 3).

use std::ffi::{CString, c_char};
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use wal_protocol::{Axis, StickState, button};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateSolidBrush, DT_CENTER, DT_SINGLELINE,
    DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, FillRect, GetDC, HDC, ReleaseDC, SRCCOPY, SelectObject, SetBkMode,
    SetTextColor, TRANSPARENT,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueA};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetSystemMetrics, MSG, PM_REMOVE, PeekMessageW,
    RegisterClassW, SM_CXSCREEN, SM_CYSCREEN, SW_SHOW, SetForegroundWindow, ShowWindow, TranslateMessage, WNDCLASSW,
    WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

/// The cabinet's tracks (its frontend's choices): name, race.
const TRACKS: &[(&str, &str)] = &[
    ("Lucky Towers", "ct.3.1"),
    ("Highway 142", "ce.15.1"),
    ("Downtown", "cs.12.1"),
    ("Ocean View", "cs.8.2"),
    ("Silk Road", "tn.2.1"),
    ("Library Square", "4.war"),
    ("Campbell Tunnel", "ce.3.2"),
    ("Drift", "tn.5.3"),
];

/// The cabinet's cars: class, name, car.
const CARS: &[(&str, &str, &str)] = &[
    ("TUNER", "Nissan 350Z", "350z"),
    ("TUNER", "Mitsubishi Eclipse", "eclipsegt"),
    ("TUNER", "Subaru Impreza WRX", "imprezawrxsti"),
    ("EXOTIC", "Porsche Carrera GT", "carreragt"),
    ("EXOTIC", "Lamborghini Murcielago", "murcielago"),
    ("EXOTIC", "Aston Martin DB9", "db9"),
    ("MUSCLE", "Shelby GT500", "mustangshlbyo"),
    ("MUSCLE", "Dodge Challenger", "challenger71"),
    ("MUSCLE", "Chevrolet Camaro", "camaro"),
    ("POLICE", "Police Cruiser", "copmidsize"),
];

const TRANSMISSIONS: &[&str] = &["Automatic", "Manual"];

static ORIG_GETMAINARGS: AtomicUsize = AtomicUsize::new(0);

type GetMainArgs = unsafe extern "C" fn(*mut i32, *mut *mut *mut c_char, *mut *mut *mut c_char, i32, *mut u8) -> i32;

pub(crate) fn init() {
    if std::env::var("WAL_GLOBALVR_CARBON_MENU").as_deref() != Ok("1") {
        return;
    }
    match unsafe { iat::hook("msvcr71.dll", "__getmainargs", get_main_args as *const () as usize) } {
        Some(orig) => {
            ORIG_GETMAINARGS.store(orig, Ordering::Relaxed);
            log!("carbon menu: on");
        }
        None => log!("carbon menu: __getmainargs not imported, no menu"),
    }
}

/// `__getmainargs`: the game's arguments, then the menu's choice after them.
unsafe extern "C" fn get_main_args(
    argc: *mut i32,
    argv: *mut *mut *mut c_char,
    env: *mut *mut *mut c_char,
    wildcard: i32,
    info: *mut u8,
) -> i32 {
    let orig: GetMainArgs = unsafe { std::mem::transmute(ORIG_GETMAINARGS.load(Ordering::Relaxed)) };
    let ret = unsafe { orig(argc, argv, env, wildcard, info) };
    if ret != 0 || argc.is_null() || argv.is_null() {
        return ret;
    }
    let choice = run_menu();
    let args = choice.args();
    log!("carbon menu: {args}");
    // started again when the race is over, back to this screen
    wal_payload_common::request_relaunch("");
    let mut list: Vec<*mut c_char> = (0..unsafe { *argc } as usize).map(|i| unsafe { *(*argv).add(i) }).collect();
    list.extend(args.split(' ').map(|a| CString::new(a).unwrap().into_raw()));
    let count = list.len() as i32;
    list.push(std::ptr::null_mut());
    unsafe {
        *argc = count;
        *argv = Box::leak(list.into_boxed_slice()).as_mut_ptr();
    }
    ret
}

struct Choice {
    track: usize,
    car: usize,
    manual: bool,
}

impl Choice {
    fn args(&self) -> String {
        let ai = std::env::var("WAL_CARBON_AI").ok().and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(3).min(7);
        let mut args = format!("-race {} -car {} -numai {ai}", TRACKS[self.track].1, CARS[self.car].2);
        if self.manual {
            args.push_str(" -trans 1");
        }
        args
    }
}

/// The menu's steps.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Track,
    Car,
    Transmission,
}

impl Step {
    fn title(self) -> &'static str {
        match self {
            Step::Track => "CHOOSE YOUR TRACK",
            Step::Car => "CHOOSE YOUR CAR",
            Step::Transmission => "CHOOSE YOUR TRANSMISSION",
        }
    }

    fn items(self) -> Vec<String> {
        match self {
            Step::Track => TRACKS.iter().map(|t| t.0.to_string()).collect(),
            Step::Car => CARS.iter().map(|c| format!("{}  -  {}", c.0, c.1)).collect(),
            Step::Transmission => TRANSMISSIONS.iter().map(|t| t.to_string()).collect(),
        }
    }
}

/// The menu actions from the stick: edges, the wheel and d-pad repeating while held.
#[derive(Default)]
struct Controls {
    held: u32,
    repeat_at: u64,
}

const PREV: u32 = 1;
const NEXT: u32 = 2;
const OK: u32 = 4;
const BACK: u32 = 8;

impl Controls {
    fn poll(&mut self, now: u64) -> u32 {
        let s: StickState = wal_payload_common::input(0);
        let wheel = s.axis(Axis::LeftX);
        let mut held = 0;
        if wheel < -12000 || s.pressed(button::LEFT) || s.pressed(button::UP) {
            held |= PREV;
        }
        if wheel > 12000 || s.pressed(button::RIGHT) || s.pressed(button::DOWN) {
            held |= NEXT;
        }
        if s.axis_u8(Axis::Accel) > 160 || s.pressed(button::START) || s.pressed(button::B1) {
            held |= OK;
        }
        if s.axis_u8(Axis::Brake) > 160 || s.pressed(button::B2) {
            held |= BACK;
        }
        let mut actions = held & !self.held;
        if actions & (PREV | NEXT) != 0 {
            self.repeat_at = now + 400;
        } else if held & (PREV | NEXT) != 0 && now >= self.repeat_at {
            actions |= held & (PREV | NEXT);
            self.repeat_at = now + 150;
        }
        self.held = held;
        actions
    }
}

/// The last race's result line, from the game's registry values.
fn last_result() -> Option<String> {
    let value = |name: &str| -> Option<String> {
        let key = c"Software\\GlobalVR\\CARBON";
        let name = CString::new(name).ok()?;
        let mut buf = [0u8; 64];
        let mut size = buf.len() as u32;
        let status = unsafe {
            RegGetValueA(
                HKEY_LOCAL_MACHINE,
                key.as_ptr().cast(),
                name.as_ptr().cast(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buf.as_mut_ptr().cast(),
                &mut size,
            )
        };
        (status == 0).then(|| String::from_utf8_lossy(&buf[..size.saturating_sub(1) as usize]).into_owned())
    };
    if !wal_payload_common::relaunched() {
        return None;
    }
    let track = value("Result_TrackName")?;
    let name = TRACKS.iter().find(|t| t.1 == track).map_or(track.as_str(), |t| t.0);
    let place: u32 = value("Result_Place")?.parse().ok()?;
    let time: f32 = value("Result_FinishTime")?.parse().ok()?;
    let suffix = match place {
        1 => "st",
        2 => "nd",
        3 => "rd",
        _ => "th",
    };
    let (m, s) = ((time / 60.0) as u32, time % 60.0);
    Some(format!("LAST RACE: {name}  -  {place}{suffix}  -  {m}:{s:05.2}"))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, w, l) }
}

/// GDI colors (0x00BBGGRR).
const BACKGROUND: u32 = 0x00100c0a;
const ACCENT: u32 = 0x000090ff;
const TEXT: u32 = 0x00e0e0e0;
const DIM: u32 = 0x00808080;

struct Screen {
    hwnd: HWND,
    width: i32,
    height: i32,
}

impl Screen {
    fn new() -> Option<Screen> {
        let class = wide("WalCarbonMenu");
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..unsafe { std::mem::zeroed() }
        };
        unsafe { RegisterClassW(&wc) };
        let (width, height) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let title = wide("Need for Speed Carbon");
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP | WS_VISIBLE,
                0,
                0,
                width,
                height,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        if hwnd.is_null() {
            log!("carbon menu: no window");
            return None;
        }
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
        }
        Some(Screen { hwnd, width, height })
    }

    fn pump(&self) {
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    fn draw(&self, step: Step, selected: usize, result: Option<&str>) {
        let (w, h) = (self.width, self.height);
        unsafe {
            let screen = GetDC(self.hwnd);
            let dc = CreateCompatibleDC(screen);
            let bitmap = CreateCompatibleBitmap(screen, w, h);
            let old_bitmap = SelectObject(dc, bitmap);
            let brush = CreateSolidBrush(BACKGROUND);
            FillRect(dc, &RECT { left: 0, top: 0, right: w, bottom: h }, brush);
            DeleteObject(brush);
            SetBkMode(dc, TRANSPARENT as i32);
            let unit = h / 30;
            text(dc, "NEED FOR SPEED CARBON", unit * 2, ACCENT, RECT { left: 0, top: unit, right: w, bottom: unit * 4 });
            if let Some(result) = result {
                text(dc, result, unit, TEXT, RECT { left: 0, top: unit * 4, right: w, bottom: unit * 6 });
            }
            text(dc, step.title(), unit * 3 / 2, TEXT, RECT { left: 0, top: unit * 6, right: w, bottom: unit * 9 });
            let items = step.items();
            let row = (unit * 2).min(unit * 18 / items.len().max(1) as i32);
            let top = unit * 10 + (unit * 18 - row * items.len() as i32) / 2;
            for (i, item) in items.iter().enumerate() {
                let r = RECT { left: w / 6, top: top + row * i as i32, right: w * 5 / 6, bottom: top + row * (i as i32 + 1) };
                if i == selected {
                    let brush = CreateSolidBrush(ACCENT);
                    FillRect(dc, &RECT { top: r.top + row / 10, bottom: r.bottom - row / 10, ..r }, brush);
                    DeleteObject(brush);
                    text(dc, item, row * 2 / 3, BACKGROUND, r);
                } else {
                    text(dc, item, row * 2 / 3, TEXT, r);
                }
            }
            let hint = if step == Step::Track {
                "WHEEL: CHOOSE      GAS / START: OK"
            } else {
                "WHEEL: CHOOSE      GAS / START: OK      BRAKE: BACK"
            };
            text(dc, hint, unit, DIM, RECT { left: 0, top: h - unit * 2, right: w, bottom: h - unit / 2 });
            BitBlt(screen, 0, 0, w, h, dc, 0, 0, SRCCOPY);
            SelectObject(dc, old_bitmap);
            DeleteObject(bitmap);
            DeleteDC(dc);
            ReleaseDC(self.hwnd, screen);
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.hwnd) };
        self.pump();
    }
}

/// Draws `s` centered in `r`, a bold font `size` pixels high.
unsafe fn text(dc: HDC, s: &str, size: i32, color: u32, mut r: RECT) {
    let face = wide("Arial");
    unsafe {
        let font = CreateFontW(size, 0, 0, 0, 700, 0, 0, 0, 0, 0, 0, 4, 0, face.as_ptr());
        let old = SelectObject(dc, font);
        SetTextColor(dc, color);
        let mut w: Vec<u16> = s.encode_utf16().collect();
        DrawTextW(dc, w.as_mut_ptr(), w.len() as i32, &mut r, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
        SelectObject(dc, old);
        DeleteObject(font);
    }
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The selection screen: track, car, transmission.
fn run_menu() -> Choice {
    let mut choice = Choice { track: 0, car: 3, manual: false };
    let Some(screen) = Screen::new() else {
        return choice;
    };
    let result = last_result();
    let mut controls = Controls::default();
    let mut step = Step::Track;
    // a button still held from the race (or the launcher's start) must not choose at once
    controls.held = PREV | NEXT | OK | BACK;
    loop {
        let len = step.items().len();
        let mut current = current_of(step, &choice);
        let actions = controls.poll(now_ms());
        if actions & PREV != 0 {
            current = (current + len - 1) % len;
        }
        if actions & NEXT != 0 {
            current = (current + 1) % len;
        }
        match step {
            Step::Track => choice.track = current,
            Step::Car => choice.car = current,
            Step::Transmission => choice.manual = current == 1,
        }
        if actions & OK != 0 {
            step = match step {
                Step::Track => Step::Car,
                Step::Car => Step::Transmission,
                Step::Transmission => break,
            };
        } else if actions & BACK != 0 {
            step = match step {
                Step::Transmission => Step::Car,
                _ => Step::Track,
            };
        }
        screen.pump();
        screen.draw(step, current_of(step, &choice), result.as_deref());
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    choice
}

fn current_of(step: Step, choice: &Choice) -> usize {
    match step {
        Step::Track => choice.track,
        Step::Car => choice.car,
        Step::Transmission => choice.manual as usize,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choice_args() {
        let c = Choice { track: 7, car: 9, manual: true };
        assert_eq!(c.args(), "-race tn.5.3 -car copmidsize -numai 3 -trans 1");
    }
}
