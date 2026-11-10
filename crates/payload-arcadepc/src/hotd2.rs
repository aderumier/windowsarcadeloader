//! The House of the Dead 2: Remake (MegaPixel Studio, Unity 2022.3 HDRP, Mono, 64-bit):
//! `WAL_ARCADEPC_BOARD=hotd2`.
//!
//! The PC game plays with the mouse (player 1) and pads (Unity's Input System; Windows.Gaming.Input
//! disabled by the profile: XInput, the payload's pads of `pads.rs`), the real keyboard and mouse
//! filtered. Player 1's gun is the mouse: the payload's cursor (`input_filter::own_mouse`, posted
//! moves and buttons: the trigger the left button, a shot off the screen the right one, reload),
//! so the PC menus point and click with it (the payload draws it there: the game's cursor is the
//! system's, at the real mouse); its b2 is Escape (back, the pause menu). Player 2 is the only
//! pad (`pads.rs`: the trigger its right trigger, a shot off the screen its left trigger, start
//! its A button: accept, join).
//!
//! Player 1's start in the main menu (its title and attract too) starts a cooperative campaign
//! from the first chapter, the PC menus skipped (mode, character, chapter, trunk); player 2
//! joins with its start at any time. Free play: the game's unlimited credits (continues); off,
//! coins are the credits.
//!
//! The crosshairs move by the mouse's motion or the sticks' (`CR_Player.handleMovingCrosshair`,
//! each frame, `aimInput` times the speed): Mono's compiled code jumps to the payload's instead,
//! which places the player's crosshair (a screen space overlay canvas's `RectTransform`, its
//! world position in screen pixels: `CR_Crosshair.getRay` shoots through it) at its gun, while
//! the original's conditions hold (`CR_ReadyCheckPanel.ArePlayersReady`, the player alive).
//!
//! The controllers: the game gives player 1 the mouse at its start, then each player the pad last
//! pressed. `MP_ControllerManager.Update` (only its debug display) is replaced with the payload's
//! frame: player 1 kept on the mouse, player 2 on the pad.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use wal_payload_common::input_filter::{own_mouse, post_key, post_mouse, post_raw_buttons};
use wal_payload_common::mono::{self, BOXED, Mono, P, mono};
use wal_payload_common::{gun, input, log};
use windows_sys::Win32::UI::WindowsAndMessaging::{WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_RBUTTONDOWN, WM_RBUTTONUP};

/// Field offsets and methods found once at the start.
struct Found {
    player_type: usize,
    player_crosshair: usize,
    ready: *const u8,
    get_transform: P,
    set_position: P,
    has_died: P,
    assigned: P,
    gamepads: *const u8,
    mouse: *const u8,
    force_assign: P,
    main_menu: P,
    find_all: P,
    campaign_menu: P,
    menu_fields: [usize; 3],
    prepare: P,
    level_aborted: P,
    last_campaign: P,
    set_chapter: P,
    load_level: P,
    set_multiplayer: P,
    add_tokens: P,
    gameplay: P,
    cursor_visible: P,
    set_scale: P,
    unlimited: *const u8,
    players: *const u8,
    revive: P,
    start_tokens: *mut u8,
    press_any_key: P,
    press_text: usize,
    set_text: P,
}

unsafe impl Send for Found {}
unsafe impl Sync for Found {}

static FOUND: std::sync::OnceLock<Found> = std::sync::OnceLock::new();

/// At the start: player 1's mouse; Mono's classes once the game's are loaded, the methods
/// replaced.
pub fn init(unity: usize) {
    own_mouse(unity);
    // the menus' pointer: the game's own is the system cursor, at the real mouse
    wal_payload_common::pointer_d3d11::init(wal_payload_common::pointer_d3d11::Dot {
        color: [1.0, 0.15, 0.15],
        size: 0.03,
        position: menu_pointer,
        crosshair: false,
    });
    std::thread::spawn(mouse_thread);
    std::thread::spawn(|| {
        let m = loop {
            if let Some(m) = mono()
                && !m.domain().is_null()
                && !m.image("Assembly-CSharp").is_null()
                && m.attach()
            {
                break m;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        setup(m);
    });
}

fn setup(m: &Mono) {
    let game = m.image("Assembly-CSharp");
    let core = m.image("UnityEngine.CoreModule");
    let player = m.class(game, "", "CR_Player");
    let ready_panel = m.class(game, "", "CR_ReadyCheckPanel");
    let manager = m.class(game, "MegaPixel.Input", "MP_ControllerManager");
    let loader = m.class(game, "", "CR_LevelLoader");
    let campaign = m.class(game, "", "CR_NewGampaignMenu");
    let data = m.class(game, "", "CR_Data");
    let press = m.class(game, "", "CR_SimplePressAnyKeyDisplay");
    if [player, ready_panel, manager, loader, campaign, data, press].iter().any(|c| c.is_null()) {
        log!("hotd2: classes not found: no guns");
        return;
    }
    let static_field = |class: P, name: &str| {
        let field = m.field(class, name);
        if field.is_null() { std::ptr::null() } else { m.static_addr(class, field) as *const u8 }
    };
    let found = Found {
        player_type: m.offset(m.field(player, "Type")),
        player_crosshair: m.offset(m.field(player, "crosshair")),
        ready: static_field(ready_panel, "ArePlayersReady"),
        get_transform: m.method(m.class(core, "UnityEngine", "Component"), "get_transform", 0),
        set_position: m.method(m.class(core, "UnityEngine", "Transform"), "set_position", 1),
        has_died: m.method(player, "get_HasDied", 0),
        assigned: m.method(manager, "get_PlayerAssignedControllers", 0),
        gamepads: static_field(manager, "Gamepads"),
        mouse: static_field(manager, "MouseController"),
        force_assign: m.method(manager, "ForceAssignControllerToPlayerAndHandleOthers", 2),
        main_menu: m.method(loader, "get_IsMainMenuLoaded", 0),
        // (Type, includeInactive): the one-argument one is also FindObjectsOfType<T>(bool)
        find_all: m.method(m.class(core, "UnityEngine", "Object"), "FindObjectsOfType", 2),
        campaign_menu: m.pin(m.type_object(campaign)),
        menu_fields: ["activeMode", "multiplayerMode", "difficulty"].map(|name| m.offset(m.field(campaign, name))),
        prepare: m.method(campaign, "prepareSaveDataForNewGame", 0),
        level_aborted: m.method(m.class(game, "", "MP_GameManager"), "LevelAborted", 0),
        last_campaign: m.method(m.class(game, "", "MP_SaveDataHandler"), "get_LastPlayedCampaign", 0),
        set_chapter: m.method(m.class(game, "", "CR_CampaignData"), "SetCurrentChapter", 1),
        load_level: m.method(loader, "LoadGameplayLevel", 1),
        set_multiplayer: m.method(m.class(game, "", "CR_CampaignData"), "SetMultiplayerActive", 1),
        add_tokens: m.method(m.class(game, "", "CR_CampaignData"), "AddPlayerTokens", 2),
        gameplay: m.method(loader, "get_IsGameplayChapterLoaded", 0),
        cursor_visible: m.method(m.class(core, "UnityEngine", "Cursor"), "get_visible", 0),
        set_scale: m.method(m.class(core, "UnityEngine", "Transform"), "set_localScale", 1),
        unlimited: static_field(data, "UNLIMITED_CREDITS_ENABLED_FROM_CONTROL_PANEL"),
        players: static_field(player, "Players"),
        revive: m.method(player, "Revive", 0),
        start_tokens: static_field(data, "DEFAULT_START_TOKEN_AMOUNT") as *mut u8,
        press_any_key: m.pin(m.type_object(press)),
        press_text: m.offset(m.field(press, "textField")),
        set_text: m.method(m.class(m.image("Unity.TextMeshPro"), "TMPro", "TMP_Text"), "set_text", 1),
    };
    let methods = [
        found.get_transform,
        found.set_position,
        found.has_died,
        found.assigned,
        found.force_assign,
        found.main_menu,
        found.find_all,
        found.campaign_menu,
        found.prepare,
        found.level_aborted,
        found.last_campaign,
        found.set_chapter,
        found.load_level,
        found.set_multiplayer,
        found.add_tokens,
        found.gameplay,
        found.cursor_visible,
        found.set_scale,
        found.revive,
        found.press_any_key,
        found.set_text,
    ];
    let statics = [found.ready, found.gamepads, found.mouse, found.unlimited, found.start_tokens, found.players];
    if methods.iter().any(|p| p.is_null()) || statics.iter().any(|p| p.is_null()) {
        log!("hotd2: methods not found: no guns");
        return;
    }
    // without free play: no credits (continues) of the game's own, the coins'
    if !free_play() {
        unsafe { mono::write::<i32>(found.start_tokens, 0, 0) };
    }
    let _ = FOUND.set(found);
    let crosshair = m.replace(m.method(player, "handleMovingCrosshair", 0), moving_crosshair as *const () as usize);
    let frame = m.replace(m.method(manager, "Update", 0), frame as *const () as usize);
    log!("hotd2: crosshairs {crosshair}, controllers {frame}");
    if !pc_menu() {
        let menu = m.replace(m.method(m.class(game, "", "CR_MainMenu"), "Open", 0), menu_open as *const () as usize);
        log!("hotd2: PC menu never opened (title screen, start): {menu}");
    }
}

/// `WAL_HOTD2_PC_MENU=1`: the PC menu opened by any input on the title screen (its options,
/// modes, chapters), as the game. Off (the default): the title screen and its attract stay, its
/// "press any key" reading "PRESS START" ("INSERT COIN" without a credit), player 1's start or
/// trigger the quick start.
fn pc_menu() -> bool {
    std::env::var("WAL_HOTD2_PC_MENU").is_ok_and(|v| matches!(v.trim(), "1" | "true" | "on"))
}

/// `CR_MainMenu.Open` (the title screen's any input, a return from a mode's menu): nothing.
unsafe extern "system" fn menu_open(_this: P) {}

/// The title screen's text (`CR_SimplePressAnyKeyDisplay.textField`): the payload's.
fn title_text(m: &Mono, f: &Found) {
    let text = if free_play() || CREDITS.load(Ordering::Relaxed) > 0 { "PRESS START" } else { "INSERT COIN" };
    let mut inactive = true;
    let Some(array) = m.invoke(f.find_all, std::ptr::null_mut(), &mut [f.press_any_key, (&raw mut inactive).cast()]).filter(|a| !a.is_null()) else { return };
    let count = unsafe { mono::read::<usize>(array as *const u8, mono::ARRAY_DATA - size_of::<usize>()) };
    for i in 0..count {
        let display = unsafe { mono::read::<usize>(array as *const u8, mono::ARRAY_DATA + i * size_of::<usize>()) };
        let field = if display == 0 { 0 } else { unsafe { mono::read::<usize>(display as *const u8, f.press_text) } };
        if field != 0 {
            m.invoke(f.set_text, field as P, &mut [m.string(text)]);
        }
    }
}

/// `player`'s gun on the game window's picture (fractions from its top left, clamped), the
/// picture's size, and whether the gun is on it.
fn gun_on_picture(player: usize) -> Option<(f32, f32, f32, f32, bool)> {
    let (left, top, w, h) = gun::client_area(gun::find_window())?;
    let (sx, sy) = gun::on_screen(&input(player), gun::screen_size());
    let (fx, fy) = ((sx - left) / w, (sy - top) / h);
    let on = (0.0..1.0).contains(&fx) && (0.0..1.0).contains(&fy) && !crate::pads::offscreen_held(player);
    Some((fx.clamp(0.0, 0.9999), fy.clamp(0.0, 0.9999), w, h, on))
}

/// Player 1's keys: (native, virtual key): Escape (back, the pause menu).
const KEYS: [(&str, u16); 1] = [("button_b", 0x1B)];

/// The keys the game sees held (`input_filter`'s polls): player 1's.
pub fn key_held(vk: i32) -> bool {
    KEYS.iter().any(|(native, key)| *key as i32 == vk && crate::pads::held(0, native))
}

/// The game's cursor shown (a menu: `Cursor.visible`).
static MENU: AtomicBool = AtomicBool::new(false);

/// Player 1's gun on the picture while the game shows its cursor.
fn menu_pointer() -> Option<(f32, f32)> {
    if !MENU.load(Ordering::Relaxed) {
        return None;
    }
    gun_on_picture(0).filter(|g| g.4).map(|(x, y, ..)| (x, y))
}

/// Free play (`WAL_FREEPLAY`, on by default): the game's unlimited credits (continues) of its
/// control panel (`CR_Data.UNLIMITED_CREDITS_ENABLED_FROM_CONTROL_PANEL`); off: a coin is a
/// credit, a campaign starts with one, each one more is a continue (`AddPlayerTokens`).
fn free_play() -> bool {
    wal_payload_common::free_play("WAL_HOTD2_FREEPLAY").unwrap_or(true)
}

/// Mouse buttons: (bit, button down and up messages, raw input's down and up transitions).
const BUTTONS: [(u32, u32, u32, u16, u16); 2] = [(1, WM_LBUTTONDOWN, WM_LBUTTONUP, 0x1, 0x2), (2, WM_RBUTTONDOWN, WM_RBUTTONUP, 0x4, 0x8)];

/// Player 1's mouse: the cursor on the gun, the trigger the left button (the right one off the
/// screen or with the reload button: reload).
fn mouse_thread() {
    let mut last = (-1, -1);
    let mut held = 0u32;
    let mut keys = [false; KEYS.len()];
    let mut tick = 0u32;
    // MK_LBUTTON 1, MK_RBUTTON 2: the same bits
    let mk = |b: u32| b as usize;
    loop {
        std::thread::sleep(Duration::from_millis(4));
        let hwnd = gun::find_window();
        for (i, (native, vk)) in KEYS.iter().enumerate() {
            let down = crate::pads::held(0, native);
            if down != keys[i] && !hwnd.is_null() {
                post_key(hwnd, *vk, down);
                keys[i] = down;
            }
        }
        let Some((fx, fy, w, h, on)) = gun_on_picture(0) else { continue };
        let at = ((fx * w) as i32, (fy * h) as i32);
        let (trigger, reload) = (crate::pads::trigger_held(0), crate::pads::reload_held(0));
        let want = if reload || (trigger && !on) { 2 } else if trigger { 1 } else { 0 };
        // again every quarter second: a pointer that did not move still reaches a new menu
        tick = (tick + 1) % 64;
        if at != last || tick == 0 {
            post_mouse(hwnd, WM_MOUSEMOVE, at.0, at.1, mk(held));
            last = at;
        }
        if want == held {
            continue;
        }
        for (bit, _, up, _, raw_up) in BUTTONS {
            if held & bit != 0 && want & bit == 0 {
                held &= !bit;
                post_mouse(hwnd, up, at.0, at.1, mk(held));
                post_raw_buttons(hwnd, raw_up);
            }
        }
        for (bit, down, _, raw_down, _) in BUTTONS {
            if held & bit == 0 && want & bit != 0 {
                held |= bit;
                post_mouse(hwnd, down, at.0, at.1, mk(held));
                post_raw_buttons(hwnd, raw_down);
            }
        }
    }
}

/// `CR_Player.handleMovingCrosshair` (each frame, each player): the crosshair on the gun.
unsafe extern "system" fn moving_crosshair(this: P) {
    let (Some(m), Some(f)) = (mono(), FOUND.get()) else { return };
    let this_ = this as *const u8;
    let (kind, crosshair) = unsafe { (mono::read::<i32>(this_, f.player_type), mono::read::<usize>(this_, f.player_crosshair)) };
    if crosshair == 0 || unsafe { mono::read::<u8>(f.ready, 0) } == 0 {
        return;
    }
    match m.invoke(f.has_died, this, &mut []) {
        Some(r) if !r.is_null() && unsafe { mono::read::<u8>(r as *const u8, BOXED) } != 0 => return,
        _ => {}
    }
    let Some((fx, fy, w, h, _)) = gun_on_picture(kind.clamp(0, 1) as usize) else { return };
    let Some(transform) = m.invoke(f.get_transform, crosshair as P, &mut []).filter(|t| !t.is_null()) else { return };
    // Unity's screen pixels: from the bottom left
    let mut at = [fx * w, (1.0 - fy) * h, 0.0f32];
    m.invoke(f.set_position, transform, &mut [(&raw mut at).cast()]);
    // the common crosshair option off: the game's crosshair not drawn (its shots still there)
    if !wal_payload_common::crosshairs(true) {
        let mut none = [0.0f32; 3];
        m.invoke(f.set_scale, transform, &mut [(&raw mut none).cast()]);
    }
}

/// Frames counted (`MP_ControllerManager.Update`).
static FRAMES: AtomicUsize = AtomicUsize::new(0);

/// `MP_ControllerManager.Update` (each frame; the original only fills its debug display): the
/// players' controllers, twice a second.
unsafe extern "system" fn frame(_this: P) {
    let (Some(m), Some(f)) = (mono(), FOUND.get()) else { return };
    let flag = |r: Option<P>| r.is_some_and(|r| !r.is_null() && unsafe { mono::read::<u8>(r as *const u8, BOXED) } != 0);
    if FRAMES.fetch_add(1, Ordering::Relaxed) % 30 == 0 {
        assign(m, f);
        if !pc_menu() && flag(m.invoke(f.main_menu, std::ptr::null_mut(), &mut [])) {
            title_text(m, f);
        }
        MENU.store(flag(m.invoke(f.cursor_visible, std::ptr::null_mut(), &mut [])), Ordering::Relaxed);
        let unlimited = unsafe { mono::read::<usize>(f.unlimited, 0) } as *mut u8;
        if !unlimited.is_null() {
            for player in 0..2 {
                unsafe { mono::write::<u8>(unlimited, mono::ARRAY_DATA + player, free_play() as u8) };
            }
        }
    }
    for player in 0..2 {
        let start = crate::pads::held(player, "button_a");
        if start == START[player].swap(start, Ordering::Relaxed) || !start {
            continue;
        }
        if flag(m.invoke(f.gameplay, std::ptr::null_mut(), &mut [])) {
            // a continue (its own checks: dead, a credit)
            let players = unsafe { mono::read::<usize>(f.players, 0) } as *const u8;
            let p = if players.is_null() { 0 } else { unsafe { mono::read::<usize>(players, mono::ARRAY_DATA + player * size_of::<usize>()) } };
            if p != 0 {
                m.invoke(f.revive, p as P, &mut []);
            }
        } else if player == 0 {
            quick_start(m, f);
        }
    }
    // the title screen (no PC menu): player 1's trigger is its start too
    let trigger = crate::pads::trigger_held(0);
    if trigger != TRIGGER.swap(trigger, Ordering::Relaxed) && trigger && !pc_menu() && !flag(m.invoke(f.gameplay, std::ptr::null_mut(), &mut [])) {
        quick_start(m, f);
    }
    // the menus' coins beyond the campaign's: player 1's continues
    if CREDITS.load(Ordering::Relaxed) > 0 && flag(m.invoke(f.gameplay, std::ptr::null_mut(), &mut [])) {
        if let Some(campaign) = m.invoke(f.last_campaign, std::ptr::null_mut(), &mut []).filter(|c| !c.is_null()) {
            let (mut kind, mut n) = (0i32, CREDITS.swap(0, Ordering::Relaxed) as i32);
            m.invoke(f.add_tokens, campaign, &mut [(&raw mut kind).cast(), (&raw mut n).cast()]);
            log!("hotd2: player 1: {n} continue(s) from the menus' coins");
        }
    }
    for player in 0..2 {
        let coin = crate::pads::held(player, "coin");
        if coin == COINS[player].swap(coin, Ordering::Relaxed) || !coin {
            continue;
        }
        if flag(m.invoke(f.gameplay, std::ptr::null_mut(), &mut [])) {
            if let Some(campaign) = m.invoke(f.last_campaign, std::ptr::null_mut(), &mut []).filter(|c| !c.is_null()) {
                let (mut kind, mut one) = (player as i32, 1i32);
                m.invoke(f.add_tokens, campaign, &mut [(&raw mut kind).cast(), (&raw mut one).cast()]);
                log!("hotd2: coin: player {} continue", player + 1);
            }
        } else {
            let n = CREDITS.fetch_add(1, Ordering::Relaxed) + 1;
            log!("hotd2: coin: {n} credit(s)");
        }
    }
}

/// The players' coin buttons held.
static COINS: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];
/// Coins inserted in the menus, without free play (a campaign takes one, player 1 continues
/// with the others).
static CREDITS: AtomicUsize = AtomicUsize::new(0);

/// The players' start held: player 1's press in the menus starts a campaign, in a chapter it
/// continues (`CR_Player.Revive`), as player 2's (which also joins: the game's own).
static START: [AtomicBool; 2] = [AtomicBool::new(false), AtomicBool::new(false)];

/// Player 1's trigger held (its start on the title screen).
static TRIGGER: AtomicBool = AtomicBool::new(false);

/// When the last campaign started: presses while its chapter loads start no other.
static STARTED: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// `WAL_HOTD2_MODE`: the campaign mode (`CampaignMode`): `arcade` (0, the default) or `original`
/// (1: the trunk's power-ups, the PC game's extras).
fn mode() -> i32 {
    match std::env::var("WAL_HOTD2_MODE").map(|v| v.trim().to_ascii_lowercase()) {
        Ok(v) if v == "original" => 1,
        _ => 0,
    }
}

/// `WAL_HOTD2_DIFFICULTY`: `Difficulty` 0 (very easy) ..4 (very hard), 2 (normal) by default.
fn difficulty() -> i32 {
    std::env::var("WAL_HOTD2_DIFFICULTY").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(2).clamp(0, 4)
}

/// Player 1's start in the main menu (its title, attract and menus): a new campaign from its
/// first chapter, as the New Campaign menu's Start New Game and the chapter selection's Play do
/// (`CR_ChapterSelectionMenu.startNewGame`), skipping the PC menus (mode, character, chapter,
/// trunk). A cooperative campaign: player 2 joins with its start at any time
/// (`MP_InputManager.onPlayerJoined`), its player inactive until then.
fn quick_start(m: &Mono, f: &Found) {
    if STARTED.lock().unwrap().is_some_and(|t| t.elapsed() < Duration::from_secs(10)) {
        return;
    }
    let flag = |r: Option<P>| r.is_some_and(|r| !r.is_null() && unsafe { mono::read::<u8>(r as *const u8, BOXED) } != 0);
    if !flag(m.invoke(f.main_menu, std::ptr::null_mut(), &mut [])) {
        log!("hotd2: start outside the main menu");
        return;
    }
    if !free_play() && CREDITS.try_update(Ordering::Relaxed, Ordering::Relaxed, |c| c.checked_sub(1)).is_err() {
        log!("hotd2: start without a credit");
        return;
    }
    let mut inactive = true;
    let Some(array) = m.invoke(f.find_all, std::ptr::null_mut(), &mut [f.campaign_menu, (&raw mut inactive).cast()]).filter(|a| !a.is_null()) else { return };
    if unsafe { mono::read::<usize>(array as *const u8, mono::ARRAY_DATA - size_of::<usize>()) } == 0 {
        log!("hotd2: no New Campaign menu");
        return;
    }
    let menu = unsafe { mono::read::<usize>(array as *const u8, mono::ARRAY_DATA) } as *mut u8;
    // CampaignMode, MultiplayerMode.Cooperative, Difficulty
    for (offset, value) in f.menu_fields.iter().zip([mode(), 1, difficulty()]) {
        unsafe { mono::write::<i32>(menu, *offset, value) };
    }
    m.invoke(f.prepare, menu.cast(), &mut []);
    m.invoke(f.level_aborted, std::ptr::null_mut(), &mut []);
    if let Some(campaign) = m.invoke(f.last_campaign, std::ptr::null_mut(), &mut []).filter(|c| !c.is_null()) {
        let mut chapter: i32 = 1; // Ch1_APrelude
        m.invoke(f.set_chapter, campaign, &mut [(&raw mut chapter).cast()]);
        // player 2 not playing until it joins
        let mut active = false;
        m.invoke(f.set_multiplayer, campaign, &mut [(&raw mut active).cast()]);
    }
    let mut chapter: i32 = 1;
    let loaded = flag(m.invoke(f.load_level, std::ptr::null_mut(), &mut [(&raw mut chapter).cast()]));
    log!("hotd2: new campaign (mode {}, difficulty {}): {loaded}", mode(), difficulty());
    if loaded {
        *STARTED.lock().unwrap() = Some(std::time::Instant::now());
    }
}

/// Player 1 on the mouse (`MP_ControllerManager.MouseController`), player 2 on the pad
/// (`MP_ControllerManager.Gamepads`'s first).
fn assign(m: &Mono, f: &Found) {
    let Some(assigned) = m.invoke(f.assigned, std::ptr::null_mut(), &mut []).filter(|a| !a.is_null()) else { return };
    let players = unsafe { mono::read::<usize>(assigned as *const u8, mono::ARRAY_DATA - size_of::<usize>()) };
    let current = |player: usize| unsafe { mono::read::<usize>(assigned as *const u8, mono::ARRAY_DATA + player * size_of::<usize>()) };
    let mouse = unsafe { mono::read::<usize>(f.mouse, 0) };
    let list = unsafe { mono::read::<usize>(f.gamepads, 0) } as *const u8;
    let mut pad = 0;
    if !list.is_null() {
        // List<T>: _items, _size
        let class = m.class_of(list as P);
        let (items, size) = (m.field(class, "_items"), m.field(class, "_size"));
        if !items.is_null() && !size.is_null() {
            let (items, size) = unsafe { (mono::read::<usize>(list, m.offset(items)) as *const u8, mono::read::<i32>(list, m.offset(size))) };
            if size >= 1 {
                pad = unsafe { mono::read::<usize>(items, mono::ARRAY_DATA) };
            }
        }
    }
    // the same device (the controllers' objects are made again with each level)
    let device = |controller: usize| {
        if controller == 0 {
            return 0;
        }
        let field = m.field(m.class_of(controller as P), "Device");
        if field.is_null() { controller } else { unsafe { mono::read::<usize>(controller as *const u8, m.offset(field)) } }
    };
    for (player, controller) in [(0usize, mouse), (1, pad)] {
        if player < players && controller != 0 && device(current(player)) != device(controller) {
            let mut kind = player as i32;
            m.invoke(f.force_assign, std::ptr::null_mut(), &mut [(&raw mut kind).cast(), controller as P]);
            log!("hotd2: player {} on the {} (was {:#x}; mouse {mouse:#x}, pad {pad:#x}, other {:#x})", player + 1, if player == 0 { "mouse" } else { "pad" }, current(player), current(1 - player));
        }
    }
}
