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
//! The cabinet's two triggers of a player are its two guns (`PlayerGunL` / `PlayerGunR`, one
//! aim): with the lightguns plugged (`WAL_GUN_COUNT`) a player has two guns ([`guns_of`]).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use wal_payload_common::{log, mono};
use wal_protocol::button;

use crate::mib::{AIM, FRAME_COUNT, Key, P, SCREEN_HEIGHT, SCREEN_WIDTH, edge, pressed};

/// Every mission and stage selectable (`WAL_EUROPAR_UNLOCK`).
pub(crate) static UNLOCK: AtomicBool = AtomicBool::new(false);

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
pub(crate) fn unlock() {
    let frame_count = FRAME_COUNT.load(Ordering::Relaxed);
    let frame = if frame_count == 0 { 0 } else { unsafe { std::mem::transmute::<usize, unsafe extern "C" fn() -> i32>(frame_count)() } } as usize;
    if UNLOCKED_FRAME.swap(frame, Ordering::Relaxed) == frame {
        return;
    }
    // a trigger pressed at the stage select (the game reads no trigger there: no button under
    // a gun)
    static TRIGGERS: [AtomicBool; 2] = [const { AtomicBool::new(false) }; 2];
    for (player, held) in TRIGGERS.iter().enumerate() {
        let pressed = trigger(player, false) || trigger(player, true);
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
    let x = aim(player).map_or(f32::from_bits(AIM[player * 2].load(Ordering::Relaxed)), |(x, _)| x);
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

/// The lightguns plugged (`WAL_GUN_COUNT`: the launcher's players 1, 2... in device order).
pub(crate) static GUN_COUNT: AtomicUsize = AtomicUsize::new(0);

/// A player's guns in Mission: Impossible.
#[derive(Clone, Copy, PartialEq)]
enum Guns {
    /// Not a lightgun (a mouse, a pad): its first button the left trigger, its second the right.
    Own,
    /// One lightgun for both guns: its trigger fires both.
    Merged,
    /// Two lightguns (the launcher's players): the left gun's trigger, the right gun's.
    Dual(usize, usize),
}

/// A player's guns: with four lightguns two each (player 1 the guns of players 1 and 3, player
/// 2 those of 2 and 4: the launcher's players 1 and 2 stay the game's, their pads' start and
/// coin too); with two, one each while both play (merged), both player 1's while player 2 does
/// not play.
fn guns_of(player: usize) -> Guns {
    let count = GUN_COUNT.load(Ordering::Relaxed);
    if player + 2 < count {
        Guns::Dual(player, player + 2)
    } else if count == 2 && player == 0 && !player_2_plays() {
        Guns::Dual(0, 1)
    } else if player < count {
        Guns::Merged
    } else {
        Guns::Own
    }
}

/// A player's left or right trigger.
pub(crate) fn trigger(player: usize, right: bool) -> bool {
    let held = |p: usize, b: u32| wal_payload_common::input(p).pressed(b);
    match guns_of(player) {
        Guns::Own => held(player, if right { button::B2 } else { button::B1 }),
        Guns::Merged => held(player, button::B1) || right && held(player, button::B2),
        Guns::Dual(left, second) => {
            if right {
                held(second, button::B1) || held(left, button::B2)
            } else {
                held(left, button::B1)
            }
        }
    }
}

/// The triggers held at the last [`aim`] (the launcher's players), and each player's aiming gun.
static AIM_HELD: [AtomicBool; 4] = [const { AtomicBool::new(false) }; 4];
static AIM_GUN: [AtomicUsize; 2] = [AtomicUsize::new(0), AtomicUsize::new(1)];

/// A player's aim in the picture: its gun's, with two guns the one whose trigger was pressed
/// last (the game has one aim a player).
pub(crate) fn aim(player: usize) -> Option<(f32, f32)> {
    let gun = match guns_of(player) {
        Guns::Dual(left, second) => {
            let mut gun = AIM_GUN[player].load(Ordering::Relaxed);
            if gun != left && gun != second {
                gun = left;
            }
            for g in [left, second] {
                let held = wal_payload_common::input(g).pressed(button::B1);
                if held && !AIM_HELD[g].swap(held, Ordering::Relaxed) {
                    gun = g;
                }
                AIM_HELD[g].store(held, Ordering::Relaxed);
            }
            AIM_GUN[player].store(gun, Ordering::Relaxed);
            gun
        }
        _ => player,
    };
    wal_payload_common::gun::gun_of_player_in_picture(gun)
}

/// Whether player 2 plays, read once a frame: its start taken (`CreditManager.m_startFlag`,
/// the lobby) or its `Player` playing (`Player.IsPlayingGame(chkCont: true, chkWait: true)`:
/// `PlayerMode` 1-11 the game, 16-19 the continue, 0 with `m_bInWaitQueueFlag` waiting to join).
fn player_2_plays() -> bool {
    static FRAME: AtomicUsize = AtomicUsize::new(usize::MAX);
    static PLAYS: AtomicBool = AtomicBool::new(false);
    let frame_count = FRAME_COUNT.load(Ordering::Relaxed);
    let frame = if frame_count == 0 { 0 } else { unsafe { std::mem::transmute::<usize, unsafe extern "C" fn() -> i32>(frame_count)() } } as usize;
    if FRAME.swap(frame, Ordering::Relaxed) == frame {
        return PLAYS.load(Ordering::Relaxed);
    }
    let plays = (|| {
        let m = mono::mono()?;
        let image = m.image("Assembly-CSharp");
        // an instance's object (a class's `Instance` property's backing field)
        let instance = |class: P| {
            let at = m.static_addr(class, m.field(class, "<Instance>k__BackingField"));
            (!at.is_null()).then(|| unsafe { mono::read::<usize>(at, 0) } as *mut u8).filter(|o| !o.is_null())
        };
        let credits = m.class(image, "", "CreditManager");
        if let Some(credits_object) = instance(credits) {
            let flags = unsafe { mono::read::<usize>(credits_object, m.offset(m.field(credits, "m_startFlag"))) } as *mut u8;
            if !flags.is_null()
                && unsafe { mono::read::<usize>(flags, 3 * size_of::<usize>()) } >= 2
                && unsafe { mono::read::<u8>(flags, mono::ARRAY_DATA + 1) } != 0
            {
                return Some(true);
            }
        }
        let manager = m.class(image, "", "PlayerManager");
        let players = unsafe { mono::read::<usize>(instance(manager)?, m.offset(m.field(manager, "m_players"))) } as *mut u8;
        if players.is_null() || unsafe { mono::read::<usize>(players, 3 * size_of::<usize>()) } < 2 {
            return Some(false);
        }
        let player = unsafe { mono::read::<usize>(players, mono::ARRAY_DATA + size_of::<usize>()) } as *mut u8;
        if player.is_null() {
            return Some(false);
        }
        let class = m.class(image, "", "Player");
        let mode = unsafe { mono::read::<i32>(player, m.offset(m.field(class, "m_playerMode"))) };
        let waiting = unsafe { mono::read::<u8>(player, m.offset(m.field(class, "m_bInWaitQueueFlag"))) } != 0;
        Some((1..=11).contains(&mode) || (16..=19).contains(&mode) || mode == 0 && waiting)
    })()
    .unwrap_or(false);
    if PLAYS.swap(plays, Ordering::Relaxed) != plays {
        log!("mib: player 2 {}", if plays { "plays: one gun a player" } else { "out: player 1 has both guns" });
    }
    plays
}

/// The stage select's time, 1.2 s (`NONE_SELECT_TIME`: it shows the next stage, then plays it),
/// made 10 s, the mission select's (`FULL_SELECT_TIME`), to shoot another stage: the second
/// `ldc.r4 1.2` of `UISelectScreen.Update` (its stage fade-in; the first is the mission
/// select's without a choice), patched in its IL before the game first compiles it.
pub(crate) fn stage_select_time() {
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
    recoil(m, image);
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

/// `PlayerGunBase.Pid`'s field (its backing field).
static PID: AtomicUsize = AtomicUsize::new(0);

/// Mission: Impossible's recoil, to the network outputs: each `PlayerGunBase.Shoot` (a prefix,
/// the U-Shoot plugin's), its gun's player (`Pid`: 0 one, 1 two) `P1_GunRecoil` /
/// `P1_CtmRecoil` pulsed.
fn recoil(m: &'static mono::Mono, image: mono::P) {
    if !wal_payload_common::outputs::enabled() {
        return;
    }
    extern "C" fn shoot(gun: mono::P) {
        let pid = PID.load(Ordering::Relaxed);
        if gun.is_null() || pid == 0 {
            return;
        }
        match unsafe { mono::read::<i32>(gun as *const u8, pid) } {
            0 => {
                wal_payload_common::outputs::pulse("P1_GunRecoil", 50);
                wal_payload_common::outputs::pulse("P1_CtmRecoil", 50);
            }
            1 => {
                wal_payload_common::outputs::pulse("P2_GunRecoil", 50);
                wal_payload_common::outputs::pulse("P2_CtmRecoil", 50);
            }
            _ => {}
        }
    }
    let gun = m.class(image, "", "PlayerGunBase");
    PID.store(m.offset(m.field(gun, "<Pid>k__BackingField")), Ordering::Relaxed);
    #[cfg(target_arch = "x86_64")]
    let hooked = m.prefix(m.method(gun, "Shoot", 0), shoot);
    #[cfg(not(target_arch = "x86_64"))]
    let hooked = {
        let _ = shoot;
        false
    };
    log!("mib: PlayerGunBase.Shoot {} (Pid +{:#x})", if hooked { "hooked: the recoil" } else { "not hooked" }, PID.load(Ordering::Relaxed));
}

/// Mission: Impossible's shots (`CameraBufferManager.ViewportPointToRay`: the guns, the laser
/// sight, the mini-games) read the aim in 1920x1080 pixels (`x / 1920f`, `y / 1080f`), its sights
/// and `GunAdjust` in the screen's (`Screen.width` / `height`): in another window size the shots
/// missed the sights. Its two constants made the screen's, in its IL before the game compiles it
/// (the first `GetAxis`, on the game's thread, before a shot).
pub(crate) fn shots_in_screen() {
    let size = |call: &AtomicUsize| match call.load(Ordering::Relaxed) {
        0 => 0,
        f => unsafe { std::mem::transmute::<usize, unsafe extern "C" fn() -> i32>(f)() },
    };
    let (w, h) = (size(&SCREEN_WIDTH), size(&SCREEN_HEIGHT));
    if w <= 0 || h <= 0 || (w, h) == (1920, 1080) {
        log!("mib: screen {w}x{h}: the shots' 1920x1080 kept");
        return;
    }
    let Some(m) = mono::mono() else { return };
    let class = m.class(m.image("Assembly-CSharp"), "", "CameraBufferManager");
    let Some(il) = m.il_code(m.method(class, "ViewportPointToRay", 1)) else {
        log!("mib: CameraBufferManager.ViewportPointToRay not found");
        return;
    };
    for (from, to) in [(1920f32, w), (1080f32, h)] {
        let ldc = [[0x22u8].as_slice(), &from.to_le_bytes()].concat();
        match (0..il.len().saturating_sub(4)).find(|&i| il[i..i + 5] == ldc[..]) {
            Some(i) => il[i + 1..i + 5].copy_from_slice(&(to as f32).to_le_bytes()),
            None => log!("mib: ViewportPointToRay: no {from}"),
        }
    }
    log!("mib: the shots in the screen's {w}x{h}");
}

/// `bool Input.GetKeyInt(KeyCode)`, `GetKeyDownInt`, `GetKeyUpInt` (Mission: Impossible): the
/// joystick keys of the table, no keyboard key.
pub(crate) unsafe extern "C" fn get_key(code: i32) -> u8 {
    pressed(Key::Code(code)) as u8
}

pub(crate) unsafe extern "C" fn get_key_down(code: i32) -> u8 {
    edge(Key::Code(code), true)
}

pub(crate) unsafe extern "C" fn get_key_up(code: i32) -> u8 {
    edge(Key::Code(code), false)
}
