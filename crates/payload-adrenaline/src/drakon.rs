//! Drakon: Realm Keepers (Unity 2021.3, IL2CPP, 64-bit): two turrets on a pneumatic ride.
//!
//! The game's own code (`GameAssembly.dll`) is driven through IL2CPP's embedding API
//! (`payload-common`'s `il2cpp.rs`), looked up by name once IL2CPP runs (the game's Matrix
//! dongle's first call, `matrix64.dll`'s `Init_MatrixAPI`):
//!
//! - **Serial ports** (`RS_232_DLL.dll`: the turrets' controllers, the pneumatic seats): no
//!   port opens (`RS232_OpenComport` 1); the game, a development build, plays without them.
//!   The payload also hides the machine's serial ports (`lib.rs`): the game opens none.
//! - **WiFi** (`wlanapi.dll`, the operator menu): no adapter.
//! - **Picture** (`WAL_DRAKON_RENDER_SCALE`, `WAL_DRAKON_MSAA`): the 3D picture's resolution
//!   (FSR upscaled) and multisampling.
//! - **Guns.** Each turret reports where it points (its viewport position, -1..1 from the
//!   centre, y up) and its two triggers to `InputsManager.SendPositionEvent` and
//!   `SendButtonEvent`; `InputsManager.Update` (the mouse's every frame) is replaced by the
//!   payload's: each player's gun and trigger, every frame.
//! - **Keys.** The cabinet's coin, start and operator inputs are keyboard keys
//!   (`SBK.InputMapping.KeyboardPressInput`: `Input.GetKey` / `GetKeyDown`):
//!   `UnityEngine.Input`'s key functions are the payload's, answered from the virtual sticks
//!   (a key's press and release seen once a frame, `Time.frameCount`).
//! - **Menus.** The level choice's turrets (`WindowTurret`) shoot at the development build's
//!   `Input.GetMouseButtonDown(player)` (the cabinet's: the turrets' buttons message): the
//!   mouse button functions are the payload's too, button n player n's trigger.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use wal_payload_common::il2cpp::{self, ARRAY_DATA, ARRAY_LENGTH};
use wal_payload_common::{gun, log};

use crate::{Native, held};

type P = *mut c_void;

/// The turrets.
const PLAYERS: usize = 2;

/// `InputsManager.playersData` (its players' `PlayerData[]`).
static PLAYERS_DATA: AtomicUsize = AtomicUsize::new(0);
static SEND_BUTTON: AtomicUsize = AtomicUsize::new(0);
static SEND_POSITION: AtomicUsize = AtomicUsize::new(0);
/// `UnityEngine.Time.frameCount`'s internal call.
static FRAME_COUNT: AtomicUsize = AtomicUsize::new(0);

/// `RS_232_DLL.dll`'s functions (cdecl).
pub fn resolve(name: &str) -> Option<usize> {
    Some(match name {
        "RS232_OpenComport" => rs232_open as *const () as usize,
        "RS232_GetPortIndexByName" => rs232_index as *const () as usize,
        "RS232_PollComport" | "RS232_SendBuf" => rs232_io as *const () as usize,
        "RS232_CloseComport" => rs232_close as *const () as usize,
        _ => return None,
    })
}

unsafe extern "C" fn rs232_open(_port: i32, _baud: i32, _mode: *const u8, _flow: i32) -> i32 {
    1
}

unsafe extern "C" fn rs232_index(_name: *const u8) -> i32 {
    -1
}

unsafe extern "C" fn rs232_io(_port: i32, _buf: *mut u8, _size: i32) -> i32 {
    0
}

unsafe extern "C" fn rs232_close(_port: i32) {}

/// `wlanapi.dll` (the operator menu's WiFi tab, `NativeWifi.WlanClient`): a machine without
/// WiFi adapter. Wine's `WlanOpenHandle` fails (`ERROR_CALL_NOT_IMPLEMENTED`: an exception, the
/// development console shown over the operator menu).
pub fn resolve_wlan(name: &str) -> Option<usize> {
    Some(match name {
        "WlanOpenHandle" => wlan_open as *const () as usize,
        "WlanEnumInterfaces" => wlan_enum_interfaces as *const () as usize,
        "WlanRegisterNotification" | "WlanCloseHandle" => wlan_ok as *const () as usize,
        "WlanFreeMemory" => wlan_free as *const () as usize,
        _ => return None,
    })
}

unsafe extern "system" fn wlan_open(_version: u32, _reserved: P, negotiated: *mut u32, handle: *mut usize) -> u32 {
    unsafe {
        if !negotiated.is_null() {
            *negotiated = 2;
        }
        if !handle.is_null() {
            *handle = 1;
        }
    }
    0
}

unsafe extern "system" fn wlan_enum_interfaces(_handle: usize, _reserved: P, list: *mut *const u32) -> u32 {
    /// `WLAN_INTERFACE_INFO_LIST`: no interface (its count, its index).
    static EMPTY: [u32; 2] = [0, 0];
    if !list.is_null() {
        unsafe { *list = EMPTY.as_ptr() };
    }
    0
}

unsafe extern "system" fn wlan_ok(_handle: usize, _a: usize, _b: usize, _c: usize, _d: usize, _e: usize) -> u32 {
    0
}

unsafe extern "system" fn wlan_free(_memory: P) {}

/// The game's methods replaced, once (nothing for another game: no IL2CPP or no Drakon's
/// `InputsManager`).
pub fn init() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(i) = il2cpp::il2cpp() else {
        return;
    };
    let game = i.image("Assembly-CSharp");
    let inputs = i.class(game, "", "InputsManager");
    if inputs.is_null() {
        return;
    }
    PLAYERS_DATA.store(i.offset(inputs, "playersData").unwrap_or(0), Ordering::Relaxed);
    SEND_BUTTON.store(i.method(inputs, "SendButtonEvent", 2), Ordering::Relaxed);
    SEND_POSITION.store(i.method(inputs, "SendPositionEvent", 2), Ordering::Relaxed);
    let update = i.method(inputs, "Update", 0);
    if PLAYERS_DATA.load(Ordering::Relaxed) != 0 && SEND_BUTTON.load(Ordering::Relaxed) != 0 && SEND_POSITION.load(Ordering::Relaxed) != 0 {
        log!("drakon: guns ({})", if i.replace(update, inputs_update as *const () as usize) { "ok" } else { "failed" });
    } else {
        log!("drakon: InputsManager not found, no guns");
    }
    FRAME_COUNT.store(i.icall("UnityEngine.Time::get_frameCount()"), Ordering::Relaxed);
    let input = i.class(i.image("UnityEngine.InputLegacyModule"), "UnityEngine", "Input");
    let keys = [
        ("GetKeyInt", get_key as *const () as usize),
        ("GetKeyDownInt", get_key_down as *const () as usize),
        ("GetKeyUpInt", get_key_up as *const () as usize),
        ("GetMouseButton", get_mouse_button as *const () as usize),
        ("GetMouseButtonDown", get_mouse_button_down as *const () as usize),
        ("GetMouseButtonUp", get_mouse_button_up as *const () as usize),
    ];
    for (name, target) in keys {
        let ok = i.replace(i.method(input, name, 1), target);
        log!("drakon: Input.{name} {}", if ok { "replaced" } else { "not found" });
    }
}

/// `Vector3`.
#[repr(C)]
struct Vector3 {
    x: f32,
    y: f32,
    z: f32,
}

/// The players' last gun positions on the picture (viewport, -1..1, y up).
static AIM: Mutex<[(f32, f32); PLAYERS]> = Mutex::new([(0.0, 0.0); PLAYERS]);

/// `InputsManager.Update`: each player's trigger and gun.
unsafe extern "C" fn inputs_update(this: P, _method: P) {
    type Button = unsafe extern "C" fn(P, i32, bool, P);
    type Position = unsafe extern "C" fn(P, i32, *const Vector3, P);
    let players = unsafe { *(this.cast::<u8>().add(PLAYERS_DATA.load(Ordering::Relaxed)) as *const usize) };
    if players == 0 {
        return;
    }
    let count = unsafe { *((players + ARRAY_LENGTH) as *const u32) } as usize;
    let button: Button = unsafe { std::mem::transmute(SEND_BUTTON.load(Ordering::Relaxed)) };
    let position: Position = unsafe { std::mem::transmute(SEND_POSITION.load(Ordering::Relaxed)) };
    fps_log();
    picture();
    let mut aim = AIM.lock().unwrap();
    for p in 0..count.min(PLAYERS) {
        // a player's data present (the array of the scene's players)
        if unsafe { *((players + ARRAY_DATA + p * 8) as *const usize) } == 0 {
            continue;
        }
        if let Some((x, y)) = gun::gun_of_player_in_picture(p) {
            aim[p] = (2.0 * x - 1.0, 1.0 - 2.0 * y);
        }
        unsafe {
            button(this, p as i32, held(p, Native::Trigger), std::ptr::null_mut());
            position(this, p as i32, &Vector3 { x: aim[p].0, y: aim[p].1, z: 0.0 }, std::ptr::null_mut());
        }
    }
}

// The keyboard: Unity's `KeyCode`s the game polls, from the virtual sticks

/// The current frame (`Time.frameCount`).
fn frame() -> i32 {
    let f = FRAME_COUNT.load(Ordering::Relaxed);
    if f == 0 {
        return 0;
    }
    unsafe { std::mem::transmute::<usize, unsafe extern "C" fn() -> i32>(f)() }
}

/// Unity's `KeyCode`s of the cabinet's inputs (the scene's `KeyboardPressInput`s): the coin
/// doors `1` `2` (`Alt` with one clears its credits), the players' buttons `q` `w` (joining,
/// continuing: the turret's trigger presses it too), the
/// operator's keypad `o` (operator) and keypad `1` `2` `3`. The game also polls its development
/// cheats' keys (function keys, the keypad's others, `Escape`...): never down.
const COIN: [i32; PLAYERS] = [49, 50];
const START: [i32; PLAYERS] = [113, 119];
const OPERATOR: i32 = 111;
const UP: i32 = 257;
const SELECT: i32 = 258;
const DOWN: i32 = 259;

/// The mouse buttons in [`edge`]'s keys: button n at `MOUSE - n`.
const MOUSE: i32 = -1;

/// Whether a key (or a mouse button) is held for the game.
fn key_held(key: i32) -> bool {
    if key <= MOUSE {
        // the development build's mouse button n: player n's trigger (the menus' turrets,
        // `WindowTurret`: the level choice)
        let player = (MOUSE - key) as usize;
        return player < PLAYERS && held(player, Native::Trigger);
    }
    let player = |keys: [i32; PLAYERS], native| keys.iter().position(|&k| k == key).is_some_and(|p| held(p, native));
    match key {
        OPERATOR => held(0, Native::Operator),
        UP => held(0, Native::Up),
        SELECT => held(0, Native::Select),
        DOWN => held(0, Native::Down),
        // the turret's trigger is also its player's button: "press trigger" to join or continue
        _ => player(COIN, Native::Coin) || player(START, Native::Start) || player(START, Native::Trigger),
    }
}

/// A key's state at the previous and the current frame.
struct Edge {
    key: i32,
    frame: i32,
    before: bool,
    now: bool,
}

/// A key's state at the previous frame and this one (sampled once a frame).
fn edge(key: i32) -> (bool, bool) {
    static KEYS: Mutex<Vec<Edge>> = Mutex::new(Vec::new());
    let f = frame();
    let mut keys = KEYS.lock().unwrap();
    let k = match keys.iter().position(|e| e.key == key) {
        Some(k) => k,
        None => {
            keys.push(Edge { key, frame: f, before: false, now: key_held(key) });
            keys.len() - 1
        }
    };
    let e = &mut keys[k];
    if e.frame != f {
        e.frame = f;
        e.before = e.now;
        e.now = key_held(key);
    }
    (e.before, e.now)
}

unsafe extern "C" fn get_key(key: i32, _method: P) -> bool {
    edge(key).1
}

unsafe extern "C" fn get_key_down(key: i32, _method: P) -> bool {
    let (before, now) = edge(key);
    now && !before
}

unsafe extern "C" fn get_key_up(key: i32, _method: P) -> bool {
    let (before, now) = edge(key);
    before && !now
}

unsafe extern "C" fn get_mouse_button(button: i32, _method: P) -> bool {
    edge(MOUSE - button).1
}

unsafe extern "C" fn get_mouse_button_down(button: i32, _method: P) -> bool {
    let (before, now) = edge(MOUSE - button);
    now && !before
}

unsafe extern "C" fn get_mouse_button_up(button: i32, _method: P) -> bool {
    let (before, now) = edge(MOUSE - button);
    before && !now
}

/// The game's frame rate, logged every 10 s (once a frame, `InputsManager.Update`).
fn fps_log() {
    static LAST: Mutex<Option<(std::time::Instant, i32)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    let f = frame();
    match *last {
        Some((t, f0)) if t.elapsed().as_secs() >= 10 => {
            let scale = f32::from_bits(RENDER_SCALE.load(Ordering::Relaxed));
            log!("drakon: {:.1} frames per second (render scale {scale})", (f - f0) as f64 / t.elapsed().as_secs_f64());
            *last = Some((std::time::Instant::now(), f));
        }
        None => *last = Some((std::time::Instant::now(), f)),
        _ => {}
    }
}

/// `WAL_DRAKON_RENDER_SCALE` (0.5..1; the profile's default 0.8): the 3D picture's resolution
/// in the screen's, upscaled with AMD FSR 1 (URP's `UniversalRenderPipelineAsset.renderScale`,
/// `upscalingFilter`), lighter on small GPUs; the menus and HUD stay at the screen's.
/// `WAL_DRAKON_MSAA` (1, 2, 4: the game's; the profile's default 1): the URP asset's
/// `msaaSampleCount`, the levels' heaviest cost on an integrated GPU. Checked
/// every second from the game's frames (the asset of the current quality level), set again if
/// the game changed it.
fn picture() {
    static LAST: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    {
        let mut last = LAST.lock().unwrap();
        if last.is_some_and(|t| t.elapsed().as_secs() < 1) {
            return;
        }
        *last = Some(std::time::Instant::now());
    }
    let Some(i) = il2cpp::il2cpp() else { return };
    let urp = i.image("Unity.RenderPipelines.Universal.Runtime");
    let ns = "UnityEngine.Rendering.Universal";
    let pipeline = i.class(urp, ns, "UniversalRenderPipeline");
    let class = i.class(urp, ns, "UniversalRenderPipelineAsset");
    let get_asset = i.method(pipeline, "get_asset", 0);
    let methods = [
        i.method(class, "get_renderScale", 0),
        i.method(class, "set_renderScale", 1),
        i.method(class, "get_upscalingFilter", 0),
        i.method(class, "set_upscalingFilter", 1),
        i.method(class, "get_msaaSampleCount", 0),
        i.method(class, "get_shadowDistance", 0),
        i.method(class, "set_msaaSampleCount", 1),
    ];
    if get_asset == 0 || methods.contains(&0) {
        return;
    }
    let [get_scale, set_scale, get_filter, set_filter, get_msaa, get_shadows, set_msaa] = methods;
    let msaa = std::env::var("WAL_DRAKON_MSAA").ok().and_then(|v| v.trim().parse::<i32>().ok()).unwrap_or(4);
    let msaa = [1, 2, 4, 8].into_iter().find(|&m| m >= msaa).unwrap_or(4);
    let scale = std::env::var("WAL_DRAKON_RENDER_SCALE").ok().and_then(|v| v.trim().parse::<f32>().ok()).unwrap_or(1.0).clamp(0.5, 1.0);
    unsafe {
        let asset: P = std::mem::transmute::<usize, unsafe extern "C" fn(P) -> P>(get_asset)(std::ptr::null_mut());
        if asset.is_null() {
            return;
        }
        let getf = |f| std::mem::transmute::<usize, unsafe extern "C" fn(P, P) -> f32>(f)(asset, std::ptr::null_mut());
        let geti = |f| std::mem::transmute::<usize, unsafe extern "C" fn(P, P) -> i32>(f)(asset, std::ptr::null_mut());
        RENDER_SCALE.store(getf(get_scale).to_bits(), Ordering::Relaxed);
        if (getf(get_scale) - scale).abs() < 0.01 && geti(get_msaa) == msaa {
            return;
        }
        log!(
            "drakon: URP asset {asset:?}: render scale {}, upscaling {}, MSAA {}, shadow distance {}",
            getf(get_scale),
            geti(get_filter),
            geti(get_msaa),
            getf(get_shadows)
        );
        std::mem::transmute::<usize, unsafe extern "C" fn(P, f32, P)>(set_scale)(asset, scale, std::ptr::null_mut());
        // UpscalingFilterSelection.FSR (Auto at the game's own scale)
        std::mem::transmute::<usize, unsafe extern "C" fn(P, i32, P)>(set_filter)(asset, if scale < 1.0 { 3 } else { 0 }, std::ptr::null_mut());
        std::mem::transmute::<usize, unsafe extern "C" fn(P, i32, P)>(set_msaa)(asset, msaa, std::ptr::null_mut());
        log!(
            "drakon: render scale {}{}, MSAA {}",
            getf(get_scale),
            if scale < 1.0 { " (FSR upscaling)" } else { "" },
            geti(get_msaa)
        );
        RENDER_SCALE.store(getf(get_scale).to_bits(), Ordering::Relaxed);
    }
}

/// The URP asset's render scale, last seen (`f32` bits).
static RENDER_SCALE: AtomicU32 = AtomicU32::new(0);
