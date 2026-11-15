//! Men In Black (Sega Amusements Europe / SEGA Amusements, 2022, Unity 2020.3 Mono, 64-bit):
//! two guns, the cabinet's shell (`shell_mib.rs`) and its lamp board. `WAL_EUROPAR_BOARD=mib`.
//!
//! * **Guns.** The game reads its gun board as one Unity joystick (`Input.GetAxis("X Axis0")`...:
//!   axes 0 / 1 player 1's gun, 3 / 2 player 2's, inverted; `Input.GetButton("Player1Trigger1")`
//!   button 7, player 2's 9; `PlayerStart1` / `2` buttons 5 / 6), maps the axes onto the screen
//!   with the calibration of `ShellData.ini` (`[Player1Gun]`..., `MIBCore.SetCalibration`), and
//!   takes no input while a joystick has a name (`MIBCore.Update`: the board has none). Unity
//!   2020 reads joysticks with raw input and HID: instead its input functions, registered by
//!   UnityPlayer through Mono's `mono_add_internal_call` (found with `GetProcAddress`), are the
//!   payload's: the axes of those names are the players' guns (the profile's calibration, 0 to
//!   4096 unclamped, maps -1..1 onto the whole screen), the buttons their triggers and starts,
//!   `GetJoystickNames` gives the joysticks no name; the keyboard and mouse (debug keys: Escape
//!   skips the boss) see nothing pressed.
//! * **Lamps.** `RS232` opens the first serial port `SerialPort.GetPortNames` lists (the
//!   registry's `HARDWARE\DEVICEMAP\SERIALCOMM`, read through Mono's `RegOpenKeyExW` import):
//!   none listed, no lamp board (Batocera's real ports and Sinden guns left alone).
//! * **Free play.** `WAL_EUROPAR_FREEPLAY` (else the common `WAL_FREEPLAY`) written to
//!   `C:\Sega\ShellData\ShellData.ini`'s `[Credit] Freeplay` (the profile's copy of the file)
//!   before the game reads it. A player's start sets `CreditsUI.TakingCredit` until the shell
//!   sends a lower credit count, which free play ignores: the second player could not join. In
//!   free play each start pressed clears it first ([`free_play_start`]).
//!
//! Mission: Impossible (SEGA Amusements, 2020, Unity 2018.4 Mono, 64-bit): `WAL_EUROPAR_BOARD=mig`,
//! its shell's protocol `"MIG"`. Each player's gun has two triggers, read as joystick keys
//! (`InputGun1` / `InputGun2`: `Input.GetKey(KeyCode.JoystickButton7)`..., left trigger 7 / 9,
//! right 8 / 10, starts 5 / 6), its aim as axes `p1x` / `p1y` / `p2x` / `p2y` (`GunAdjust`: y
//! the screen's x, x its y from the top, -1..1 to 0..4096 then the calibration of
//! `..\ShellData\ShellData.ini` beside the game folder, unclamped). The keyboard keys are
//! nothing; free play needs no help, no lamp board on a serial port. The lobby's player photo
//! (OpenCV 3.3's `VideoCapture(0)`, DirectShow, opened whatever `config.ini`'s `ENABLE_WEBCAM`)
//! takes the first camera: the Sinden guns' are hidden from it (`webcam::skip_sinden`).
//! `WAL_EUROPAR_UNLOCK` (on unless 0) unlocks every mission and stage ([`unlock`]).

use std::ffi::{CStr, c_char, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use wal_payload_common::{iat, log, mono, mono_imports};
use wal_protocol::button;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

type P = *mut c_void;

#[derive(Clone, Copy, PartialEq)]
enum Board {
    Mib,
    Mig,
}

static BOARD: std::sync::OnceLock<Board> = std::sync::OnceLock::new();

fn board() -> Board {
    *BOARD.get().unwrap_or(&Board::Mib)
}

fn shell_data() -> &'static str {
    match board() {
        Board::Mib => "C:/Sega/ShellData/ShellData.ini",
        // the game's folder's parent (the current directory)
        Board::Mig => "../ShellData/ShellData.ini",
    }
}

static UNITY_GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);
static ADD_INTERNAL_CALL: AtomicUsize = AtomicUsize::new(0);
static FRAME_COUNT: AtomicUsize = AtomicUsize::new(0);
static JOYSTICK_NAMES: AtomicUsize = AtomicUsize::new(0);
static FREE_PLAY: AtomicBool = AtomicBool::new(false);
static UNLOCK: AtomicBool = AtomicBool::new(false);

/// A joystick button as the game reads it: by its input name (`GetButton`) or key code
/// (`GetKey`).
#[derive(Clone, Copy, PartialEq)]
enum Key {
    Name(&'static str),
    Code(i32),
}

/// `KeyCode.JoystickButton0`
const JOYSTICK_BUTTON0: i32 = 330;

/// The game's joystick buttons: (key, player, virtual button).
const MIB_BUTTONS: [(Key, usize, u32); 6] = [
    (Key::Name("PlayerStart1"), 0, button::START),
    (Key::Name("PlayerStart2"), 1, button::START),
    (Key::Name("Player1Trigger1"), 0, button::B1),
    (Key::Name("Player1Trigger2"), 0, button::B1),
    (Key::Name("Player2Trigger1"), 1, button::B1),
    (Key::Name("Player2Trigger2"), 1, button::B1),
];
const MIG_BUTTONS: [(Key, usize, u32); 6] = [
    (Key::Code(JOYSTICK_BUTTON0 + 5), 0, button::START),
    (Key::Code(JOYSTICK_BUTTON0 + 6), 1, button::START),
    (Key::Code(JOYSTICK_BUTTON0 + 7), 0, button::B1),
    (Key::Code(JOYSTICK_BUTTON0 + 8), 0, button::B2),
    (Key::Code(JOYSTICK_BUTTON0 + 9), 1, button::B1),
    (Key::Code(JOYSTICK_BUTTON0 + 10), 1, button::B2),
];

fn buttons() -> &'static [(Key, usize, u32); 6] {
    match board() {
        Board::Mib => &MIB_BUTTONS,
        Board::Mig => &MIG_BUTTONS,
    }
}

/// The text of a managed string (its length after the object header, then UTF-16).
fn managed_string(s: P) -> String {
    if s.is_null() {
        return String::new();
    }
    let s = s as *const u8;
    let len = unsafe { mono::read::<i32>(s, 2 * size_of::<usize>()) }.max(0) as usize;
    let chars: Vec<u16> = (0..len).map(|i| unsafe { mono::read::<u16>(s, 2 * size_of::<usize>() + 4 + 2 * i) }).collect();
    String::from_utf16_lossy(&chars)
}

fn pressed(key: Key) -> bool {
    buttons().iter().any(|&(k, player, b)| k == key && wal_payload_common::input(player).pressed(b))
}

/// The last gun positions in the picture (f32 bits: player 1 x, y, player 2 x, y, 0..1 from the
/// top left), kept while a gun points off the screen; at the centre first.
static AIM: [AtomicU32; 4] = [const { AtomicU32::new(0x3f00_0000) }; 4];

/// `float Input.GetAxis(string)`, `GetAxisRaw`: the guns, -1..1. Men In Black's inverted (1 at
/// the left / top), Mission: Impossible's `pNy` the x, `pNx` the y (-1 at the left / top).
unsafe extern "C" fn get_axis(name: P) -> f32 {
    let name = managed_string(name);
    let (names, sign) = match board() {
        Board::Mib => (["X Axis0", "Y Axis0", "X Axis1", "Y Axis1"], -1.0),
        Board::Mig => (["p1y", "p1x", "p2y", "p2x"], 1.0),
    };
    let Some(axis) = names.iter().position(|&a| a == name) else { return 0.0 };
    if UNLOCK.load(Ordering::Relaxed) {
        unlock();
    }
    let player = axis / 2;
    if let Some((x, y)) = wal_payload_common::gun::gun_of_player_in_picture(player) {
        AIM[player * 2].store(x.to_bits(), Ordering::Relaxed);
        AIM[player * 2 + 1].store(y.to_bits(), Ordering::Relaxed);
    }
    sign * (2.0 * f32::from_bits(AIM[axis].load(Ordering::Relaxed)) - 1.0)
}

/// `bool Input.GetButton(string)`
unsafe extern "C" fn get_button(name: P) -> u8 {
    pressed(named(name)) as u8
}

/// The button of a managed name (a key code of none for names not in the table).
fn named(name: P) -> Key {
    let name = managed_string(name);
    buttons().iter().map(|b| b.0).find(|k| matches!(k, Key::Name(n) if *n == name)).unwrap_or(Key::Code(-1))
}

/// Each button's state at the frames it was read: (frame, previous, current).
static EDGES: Mutex<[(i32, bool, bool); 6]> = Mutex::new([(-1, false, false); 6]);

/// Whether the button went down (`down`) or up since the previous frame.
fn edge(key: Key, down: bool) -> u8 {
    let Some(i) = buttons().iter().position(|&(k, ..)| k == key) else { return 0 };
    let frame_count = FRAME_COUNT.load(Ordering::Relaxed);
    let frame = if frame_count == 0 { 0 } else { unsafe { std::mem::transmute::<usize, unsafe extern "C" fn() -> i32>(frame_count)() } };
    let mut edges = EDGES.lock().unwrap_or_else(|e| e.into_inner());
    let (last, previous, current) = &mut edges[i];
    if *last != frame {
        *last = frame;
        *previous = *current;
        *current = pressed(key);
    }
    let went = if down { *current && !*previous } else { !*current && *previous };
    if went && down && matches!(key, Key::Name(n) if n.starts_with("PlayerStart")) && FREE_PLAY.load(Ordering::Relaxed) {
        free_play_start();
    }
    went as u8
}

/// `MIBCore.Instance.CreditsController.TakingCredit = false` (the game's main thread: its
/// `CreditsUI.CheckCreditTaken` reads the start button).
fn free_play_start() {
    let Some(m) = mono::mono() else { return };
    let image = m.image("Assembly-CSharp");
    let core = m.class(image, "MIB", "MIBCore");
    let instance = m.static_addr(core, m.field(core, "InstanceVariable"));
    if instance.is_null() {
        return;
    }
    let core_object = unsafe { mono::read::<usize>(instance, 0) } as *mut u8;
    let controller = m.field(core, "CreditsController");
    let taking = m.field(m.class(image, "", "CreditsUI"), "TakingCredit");
    if core_object.is_null() || controller.is_null() || taking.is_null() {
        return;
    }
    let credits_ui = unsafe { mono::read::<usize>(core_object, m.offset(controller)) } as *mut u8;
    if !credits_ui.is_null() {
        unsafe { mono::write::<u8>(credits_ui, m.offset(taking), 0) };
    }
}

/// The last frame [`unlock`] ran in; `UISelectScreen.MissionsState`'s and `StagesState`'s
/// addresses.
static UNLOCKED_FRAME: AtomicUsize = AtomicUsize::new(usize::MAX);
static MISSIONS_STATE: AtomicUsize = AtomicUsize::new(0);
static STAGES_STATE: AtomicUsize = AtomicUsize::new(0);

/// Mission: Impossible's missions and stages unlocked, once a frame (the game's main thread: its
/// guns' axes are read each frame, from the lobby on). `UISelectScreen.MissionsState` and
/// `StagesState` (0 locked, 1 open, 2 played; copied to the select screen's buttons when it
/// fades in): mission 3 locked until mission 1 or 2 is cleared (`ConfigManager
/// .Mission3Unlocked` is never set: its `config.ini` key is read nowhere), the stage select
/// opens the mission's next stage only. The locked ones are opened: a shot at one plays it
/// (the stage select's time made 10 s by [`stage_select_time`]).
fn unlock() {
    let frame_count = FRAME_COUNT.load(Ordering::Relaxed);
    let frame = if frame_count == 0 { 0 } else { unsafe { std::mem::transmute::<usize, unsafe extern "C" fn() -> i32>(frame_count)() } } as usize;
    if UNLOCKED_FRAME.swap(frame, Ordering::Relaxed) == frame {
        return;
    }
    // a trigger pressed at the stage select (the game reads no trigger there: no button under
    // a gun)
    static TRIGGERS: [AtomicBool; 2] = [const { AtomicBool::new(false) }; 2];
    for (player, held) in TRIGGERS.iter().enumerate() {
        let input = wal_payload_common::input(player);
        let pressed = input.pressed(button::B1) || input.pressed(button::B2);
        if pressed && !held.swap(pressed, Ordering::Relaxed) {
            choose_stage(player);
        }
        held.store(pressed, Ordering::Relaxed);
    }
    if MISSIONS_STATE.load(Ordering::Relaxed) == 0 || STAGES_STATE.load(Ordering::Relaxed) == 0 {
        let Some(m) = mono::mono() else { return };
        let select = m.class(m.image("Assembly-CSharp"), "", "UISelectScreen");
        MISSIONS_STATE.store(m.static_addr(select, m.field(select, "MissionsState")) as usize, Ordering::Relaxed);
        STAGES_STATE.store(m.static_addr(select, m.field(select, "StagesState")) as usize, Ordering::Relaxed);
    }
    for states in [&MISSIONS_STATE, &STAGES_STATE] {
        let states = states.load(Ordering::Relaxed) as *const u8;
        let array = if states.is_null() { 0 } else { unsafe { mono::read::<usize>(states, 0) } } as *mut u8;
        if array.is_null() {
            continue;
        }
        let len = unsafe { mono::read::<usize>(array, 3 * size_of::<usize>()) };
        for i in 0..len.min(8) {
            if unsafe { mono::read::<i32>(array, mono::ARRAY_DATA + 4 * i) } == 0 {
                unsafe { mono::write::<i32>(array, mono::ARRAY_DATA + 4 * i, 1) };
            }
        }

    }
}

/// A trigger pressed at the stage select (shown for 1 s at least): the stage under the player's
/// gun, from its x (the left, middle or right third). The game's buttons never take the shot:
/// the mission select's, behind them at the same places, take its raycast
/// (`UISelectButton.CheckEntry2D`: the name of the button hit compared). That stage made
/// `GameManager.SelectedStage` (the stage played when the time ends, and the one whose click
/// animation `UISelectScreen` waits for) and clicked (`UIStageSelectInteract.PlayerClickN`:
/// its animation, `UISelectScreen.StageClicked`; ignored before the screen's fade-in ends).
fn choose_stage(player: usize) {
    let entered = crate::shell_mib::STAGE_SELECT.load(Ordering::Relaxed);
    if entered == 0 || crate::shell_mib::now_ms() < entered + 1000 {
        return;
    }
    let x = wal_payload_common::gun::gun_of_player_in_picture(player).map_or(f32::from_bits(AIM[player * 2].load(Ordering::Relaxed)), |(x, _)| x);
    let stage = ((x * 3.0) as i32).clamp(0, 2);
    let Some(m) = mono::mono() else { return };
    let image = m.image("Assembly-CSharp");
    let game = m.class(image, "", "GameManager");
    let instance = m.static_addr(game, m.field(game, "<Instance>k__BackingField"));
    let game_object = if instance.is_null() { 0 } else { unsafe { mono::read::<usize>(instance, 0) } } as P;
    if game_object.is_null() {
        log!("mib: no GameManager: stage {} not chosen", stage + 1);
        return;
    }
    let mut value = stage;
    m.invoke(m.method(game, "set_SelectedStage", 1), game_object, &mut [(&raw mut value).cast()]);
    let interact = m.class(image, "", "UIStageSelectInteract");
    let unity = m.image("UnityEngine.CoreModule");
    let find = m.method(m.class(unity, "UnityEngine", "Object"), "FindObjectOfType", 1);
    let select = m.invoke(find, std::ptr::null_mut(), &mut [m.type_object(interact)]).unwrap_or(std::ptr::null_mut());
    if !select.is_null() {
        m.invoke(m.method(interact, &format!("PlayerClick{}", stage + 1), 0), select, &mut []);
    }
    log!("mib: player {} chose stage {}", player + 1, stage + 1);
}

/// The stage select's time, 1.2 s (`NONE_SELECT_TIME`: it shows the next stage, then plays it),
/// made 10 s, the mission select's (`FULL_SELECT_TIME`), to shoot another stage: the second
/// `ldc.r4 1.2` of `UISelectScreen.Update` (its stage fade-in; the first is the mission
/// select's without a choice), patched in its IL before the game first compiles it.
fn stage_select_time() {
    let start = std::time::Instant::now();
    let (m, image) = loop {
        // Mono's loader only once its root domain exists, from a thread of Mono's
        if let Some(m) = mono::mono()
            && !m.domain().is_null()
            && m.attach()
        {
            let image = m.image("Assembly-CSharp");
            if !image.is_null() {
                break (m, image);
            }
        }
        if start.elapsed() > std::time::Duration::from_secs(60) {
            log!("mib: Assembly-CSharp not loaded: the stage select's time unchanged");
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    };
    let Some(il) = m.il_code(m.method(m.class(image, "", "UISelectScreen"), "Update", 0)) else {
        log!("mib: UISelectScreen.Update not found");
        return;
    };
    const SHORT: [u8; 5] = [0x22, 0x9a, 0x99, 0x99, 0x3f];
    let at: Vec<usize> = (0..il.len().saturating_sub(4)).filter(|&i| il[i..i + 5] == SHORT).collect();
    match at[..] {
        [_, stage] => {
            il[stage + 1..stage + 5].copy_from_slice(&10f32.to_le_bytes());
            log!("mib: the stage select's time 10 s");
        }
        _ => log!("mib: UISelectScreen.Update: {} times of 1.2 s, unchanged", at.len()),
    }
}

unsafe extern "C" fn get_button_down(name: P) -> u8 {
    edge(named(name), true)
}

unsafe extern "C" fn get_button_up(name: P) -> u8 {
    edge(named(name), false)
}

/// `bool Input.GetKeyInt(KeyCode)`, `GetKeyDownInt`, `GetKeyUpInt` (Mission: Impossible): the
/// joystick keys of the table, no keyboard key.
unsafe extern "C" fn get_key(code: i32) -> u8 {
    pressed(Key::Code(code)) as u8
}

unsafe extern "C" fn get_key_down(code: i32) -> u8 {
    edge(Key::Code(code), true)
}

unsafe extern "C" fn get_key_up(code: i32) -> u8 {
    edge(Key::Code(code), false)
}

/// The keyboard's keys and the mouse's buttons (`GetKey*`, `GetMouseButton*`): none pressed.
unsafe extern "C" fn nothing_pressed(_key: usize) -> u8 {
    0
}

/// `string[] Input.GetJoystickNames()`: the joysticks without names.
unsafe extern "C" fn get_joystick_names() -> P {
    let f: unsafe extern "C" fn() -> P = unsafe { std::mem::transmute(JOYSTICK_NAMES.load(Ordering::Relaxed)) };
    let names = unsafe { f() };
    static EMPTY: AtomicUsize = AtomicUsize::new(0);
    if !names.is_null() && let Some(m) = mono::mono() {
        if EMPTY.load(Ordering::Relaxed) == 0 {
            EMPTY.store(m.pin(m.string("")) as usize, Ordering::Relaxed);
        }
        let len = unsafe { mono::read::<usize>(names as *const u8, 3 * size_of::<usize>()) };
        for i in 0..len {
            unsafe { mono::write::<usize>(names as *mut u8, mono::ARRAY_DATA + i * size_of::<usize>(), EMPTY.load(Ordering::Relaxed)) };
        }
    }
    names
}

unsafe extern "C" fn add_internal_call(name: *const c_char, method: P) {
    let orig: unsafe extern "C" fn(*const c_char, P) = unsafe { std::mem::transmute(ADD_INTERNAL_CALL.load(Ordering::Relaxed)) };
    let call = if name.is_null() { &[][..] } else { unsafe { CStr::from_ptr(name) }.to_bytes() };
    // Mono is up (its loader used before crashes): the stage select's IL patched once loaded
    static STARTED: AtomicBool = AtomicBool::new(false);
    if UNLOCK.load(Ordering::Relaxed) && !STARTED.swap(true, Ordering::Relaxed) {
        std::thread::spawn(stage_select_time);
    }
    let ours: Option<usize> = match call.strip_prefix(b"UnityEngine.Input::") {
        Some(b"GetAxis" | b"GetAxisRaw") => Some(get_axis as *const () as usize),
        Some(b"GetButton") => Some(get_button as *const () as usize),
        Some(b"GetButtonDown") => Some(get_button_down as *const () as usize),
        Some(b"GetButtonUp") => Some(get_button_up as *const () as usize),
        Some(b"GetKeyInt") if board() == Board::Mig => Some(get_key as *const () as usize),
        Some(b"GetKeyDownInt") if board() == Board::Mig => Some(get_key_down as *const () as usize),
        Some(b"GetKeyUpInt") if board() == Board::Mig => Some(get_key_up as *const () as usize),
        Some(
            b"GetKeyInt" | b"GetKeyString" | b"GetKeyDownInt" | b"GetKeyDownString" | b"GetKeyUpInt" | b"GetKeyUpString"
            | b"GetMouseButton" | b"GetMouseButtonDown" | b"GetMouseButtonUp",
        ) => Some(nothing_pressed as *const () as usize),
        Some(b"GetJoystickNames") => {
            JOYSTICK_NAMES.store(method as usize, Ordering::Relaxed);
            Some(get_joystick_names as *const () as usize)
        }
        _ => {
            if call == b"UnityEngine.Time::get_frameCount" {
                FRAME_COUNT.store(method as usize, Ordering::Relaxed);
            }
            None
        }
    };
    match ours {
        Some(f) => {
            log!("mib: {} is the payload's", String::from_utf8_lossy(call));
            unsafe { orig(name, f as P) }
        }
        None => unsafe { orig(name, method) },
    }
}

unsafe extern "system" fn unity_get_proc_address(module: P, name: *const u8) -> P {
    let orig: unsafe extern "system" fn(P, *const u8) -> P = unsafe { std::mem::transmute(UNITY_GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    let f = unsafe { orig(module, name) };
    if !f.is_null() && (name as usize) >> 16 != 0 && unsafe { CStr::from_ptr(name.cast()) }.to_bytes() == b"mono_add_internal_call" {
        ADD_INTERNAL_CALL.store(f as usize, Ordering::Relaxed);
        return add_internal_call as *const () as P;
    }
    f
}

/// `RegOpenKeyExW` (Mono's `Microsoft.Win32.Registry`): the serial port list absent.
unsafe extern "system" fn reg_open_key_ex_w(key: P, sub: *const u16, options: u32, access: u32, result: *mut P) -> i32 {
    type F = unsafe extern "system" fn(P, *const u16, u32, u32, *mut P) -> i32;
    static ORIG: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    if !sub.is_null() {
        let len = (0..).take_while(|&i| unsafe { *sub.add(i) } != 0).count();
        let name = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(sub, len) });
        if name.trim_matches('\\').eq_ignore_ascii_case("HARDWARE\\DEVICEMAP\\SERIALCOMM") {
            static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
            if !LOGGED.swap(true, Ordering::Relaxed) {
                log!("mib: serial port list hidden (no lamp board)");
            }
            return 2; // ERROR_FILE_NOT_FOUND
        }
    }
    let orig = *ORIG.get_or_init(|| unsafe {
        let advapi32 = windows_sys::Win32::System::LibraryLoader::LoadLibraryW(windows_sys::w!("advapi32.dll"));
        windows_sys::Win32::System::LibraryLoader::GetProcAddress(advapi32, c"RegOpenKeyExW".as_ptr().cast()).map_or(0, |f| f as usize)
    });
    unsafe { std::mem::transmute::<usize, F>(orig)(key, sub, options, access, result) }
}

fn resolve(name: &str, module: &str) -> Option<usize> {
    (module == "advapi32.dll" && name == "RegOpenKeyExW").then_some(reg_open_key_ex_w as *const () as usize)
}

/// `[Credit] Freeplay` of the shell's settings, set to `on`.
fn write_free_play(on: bool) {
    let shell_data = shell_data();
    let Ok(text) = std::fs::read_to_string(shell_data) else {
        log!("mib: {shell_data} missing");
        return;
    };
    let mut section = String::new();
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            let t = line.trim();
            if t.starts_with('[') {
                section = t.to_ascii_lowercase();
            } else if section == "[credit]" && t.split('=').next().is_some_and(|k| k.trim().eq_ignore_ascii_case("Freeplay")) {
                return format!("Freeplay={}", on as u8);
            }
            line.to_string()
        })
        .collect();
    let new = lines.join("\r\n") + "\r\n";
    if new != text && let Err(e) = std::fs::write(shell_data, new) {
        log!("mib: writing {shell_data}: {e}");
    }
}

/// `board`: `mib` or `mig`.
pub(crate) fn init(board: &str) {
    let board = if board == "mig" { Board::Mig } else { Board::Mib };
    let _ = BOARD.set(board);
    let free_play = wal_payload_common::free_play("WAL_EUROPAR_FREEPLAY").unwrap_or(false);
    write_free_play(free_play);
    // Mission: Impossible's free play needs no help
    FREE_PLAY.store(free_play && board == Board::Mib, Ordering::Relaxed);
    let unlock = board == Board::Mig && std::env::var("WAL_EUROPAR_UNLOCK").map_or(true, |v| v.trim() != "0");
    UNLOCK.store(unlock, Ordering::Relaxed);

    crate::shell_mib::init(if board == Board::Mig { &crate::shell_mib::MIG } else { &crate::shell_mib::MIB }, free_play);
    match board {
        Board::Mib => mono_imports::hook(resolve),
        Board::Mig => wal_payload_common::webcam::skip_sinden(),
    }
    let unity = unsafe { GetModuleHandleW(windows_sys::w!("UnityPlayer.dll")) } as usize;
    match unsafe { iat::hook_module(unity, "kernel32.dll", "GetProcAddress", unity_get_proc_address as *const () as usize) } {
        Some(o) => UNITY_GET_PROC_ADDRESS.store(o, Ordering::Relaxed),
        None => log!("mib: UnityPlayer does not import GetProcAddress: no gun input"),
    }
    log!("mib: shell, guns, free play {free_play}, unlock {unlock}");
}
