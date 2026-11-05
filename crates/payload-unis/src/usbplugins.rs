//! UNIS's `UsbPluginsDll` cabinets (Unreal Engine 4, 64-bit): `WAL_UNIS_BOARD=usbplugins`,
//! the game told by its executable: Raccoon Rampage 1.4 (`RSGame-Win64-Shipping.exe`, four
//! water guns), Crazy Ride 1.9 (`CrazyTaxiGame-Win64-Shipping.exe`, a driving cabinet),
//! Elevator Action Invasion (`ESGame-Win64-Shipping.exe`, two guns, the elevator doors), Fantasy
//! Soccer 2.4 (`FB-Win64-Shipping.exe`, four players, a stick and a ball each).
//!
//! The game (its PDB in the dump) loads `Plugins/UsbPluginsDll.dll` (Crazy Ride:
//! `UsbPluginsDll_R.dll`) and looks its `BIND_*` functions up by name
//! (`FWindowsPlatformProcess::GetDllExport`, the executable's `GetProcAddress`); the DLL talks to
//! the I/O board (USB HID), the guns or the wheel and the dongle. The payload hooks that
//! `GetProcAddress`: for the DLL's handle, the functions below are the payload's, every other
//! one (lights, recoil, settings files, time) the DLL's own.
//!
//! ## Raccoon Rampage
//!
//! - **I/O.** `BIND_InitUnisIo`, `BIND_IsIoWorking`: true. `BIND_GetIoData(FCoUnisIoData&)`
//!   fills the board's 49 bytes (`ABPGameState::ParseIOData` reads them): byte 2 the operator's
//!   buttons (bit 2 test: the operator menu, its move up; 1 service: its move down; 0 repair:
//!   its enter), byte 5 the guns' triggers (bit 7 - n gun n; a press starts the gun's game) and
//!   the water tank's level sensors (bit 3 low, 2 medium, 1 high: medium, the normal level),
//!   bytes 6..9 the ticket and card dispensers (all clear: none empty), 10..25 each gun's
//!   x, y (16-bit, big endian: unused, see the aim).
//! - **Coins.** The DLL counts them: `BIND_GetCoinNumByPlayer(player)`, `BIND_DecCoins(player,
//!   count)`, `BIND_AddKeyboardCoins(player, count)` are the payload's counters, a coin key
//!   press one coin (the game's pay setting, the operator menu's, is free play by default).
//! - **Dongle.** `BIND_InitDongle`, `BIND_IsDongleExist`, `BIND_IsDongleVerify`,
//!   `BIND_UnInitDongle`: true. `BIND_SetLocalTime` (the operator menu's clock) changes nothing.
//! - **Aim.** The game maps each gun's raw x, y through the operator menu's grid calibration
//!   (`Setting_Hardware.ini`, `ABPGameState::GunDataToScreen`) to its 1920x1080 picture
//!   (`FPACKET::iX`, `iY`), then `UBPGameInstance::SetMouseLocation` moves the gun's crosshair.
//!   `GunDataToScreen` is replaced: the gun's place on the picture times 1920x1080 (kept while
//!   it points off it), and `IsGunCalibrate` answers true (the calibration done; DemulShooter's
//!   Raccoon Rampage module patches the same RVAs). The game draws its own crosshairs.
//!
//! Natives: `trigger coin` (each gun), `test down enter` (the operator's).
//!
//! ## Crazy Ride
//!
//! - **I/O.** `BIND_GetIoData(FCoUnisIoData&)`, 33 bytes (`ACTGameState::ParseIOData`): byte 2
//!   the operator's buttons (as Raccoon Rampage's), byte 3 bit 0 the start button (in a race:
//!   the nitro), 1 the seat motion's pause (each press toggles it), 2 the seat belt (fastened:
//!   set), 7 a coin (unused: the coins are the DLL's counter); byte 6 the brake, 7 the gas
//!   (1..255).
//! - **Wheel.** `BIND_IsExistSteeringWheel`, `BIND_OpenSteeringWheelDevice(port)`: true;
//!   `BIND_GetSteeringWheelAngle()`: 255 full left .. 1 full right, 128 the centre (the
//!   cabinet's wheel: left the high values). The game maps the wheel and the pedals through the
//!   operator menu's input calibration (`Setting_Test_Input.ini`, CRLF lines: `IniFile.dll`'s
//!   parser): the profile's matches these ranges.
//! - **Force feedback.** `BIND_SetSteeringWheelData(force, centre, shake, 0)`: the motor's
//!   centring force level (0..10, the operator menu's, 3 by default) and its shaking level (set
//!   while the car hits or bumps): the launcher's centring spring and vibration, a tenth of the
//!   full force per level.
//! - **Coins.** `BIND_GetInternalCoins()`: the credits; `BIND_DecCoins(count)`,
//!   `BIND_AddKeyboardCoins(count)`. The dump's pay setting is free play.
//!
//! Natives: `start pause coin` (player 1), `test down enter` (the operator's); the wheel `lx`, the
//! gas `accel`, the brake `brake`.
//!
//! ## Elevator Action Invasion
//!
//! - **I/O.** `BIND_IsExistESIODevice`, `BIND_OpenESIODevice`: true; `BIND_GetESIoData`, 17
//!   bytes: byte 2 the operator's (bit 0 enter, 1 down, 2 test) and the start buttons (5 1P,
//!   4 2P), bytes 3, 4 the doors' sensors (`eai_doors.rs`), 5..12 each gun's raw aim, byte 13
//!   the triggers (bit 1 1P, 0 2P).
//! - **Guns.** `BIND_GetGunData(player, FPACKET*)`: the gun's place on the 1080x1920 picture,
//!   its trigger and buttons, `iUSB` 1, a calibration. `BIND_SetDoorData`: the doors.
//!
//! Natives: `trigger p1start coin gunmiddle gunright gunup gundown gunleft gunright2` (each gun),
//! `test down enter` (the operator's).
//!
//! ## Fantasy Soccer
//!
//! The executable is packed (its code unpacked at run time): it finds the DLL's functions past
//! the hooked `GetProcAddress`, so once the DLL is loaded the exports the payload answers are
//! patched to jump to the payload's (`patch_exports`).
//!
//! - **I/O.** `BIND_GetIoData`, 33 bytes: byte 3 the operator's (bit 7 test: opens the menu,
//!   moves down; 6 up; 5 enter), byte 4 bit 3 - n a button of player n's (not on the operator
//!   menu's input test), bytes 5, 6 the ticket dispensers (a nibble each player: clear), 11 + n
//!   player n's ball sensor (the kick's strength, taken when it rises from 0: one read per
//!   kick), byte 15 bits 7 - 2n the red button (accelerate; any button starts) and 6 - 2n the
//!   blue one, 16 + 2n / 17 + 2n the stick's x / y (255 full left / up .. 0, 128 the centre).
//!   The game maps the sticks and the balls through the operator menu's calibration
//!   (`Setting_Test_Input.ini`, CRLF lines): the profile's has every player's.
//! - **Coins.** `BIND_GetInternalCoins(player)`: the player's coins since the start (the game
//!   adds each increase to the player's credits; three a game by default).
//! - **One screen.** `bEnableSplitscreen` (the cabinet's second screen, a video beside the game:
//!   a 3840x1080 window) off in the user's saved `Manage_Menu.ini` (`WAL_UNIS_SPLITSCREEN=1`:
//!   both). The camera (the players' photos) off in the profile's `Setting_Webcam.ini`.
//!
//! Natives: `kick pass accelerate switch button2 coin` (each player; `kick` a strong kick,
//! `pass` a weaker one), `test down enter` (the operator's).

use std::ffi::{CStr, c_void};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{gun, iat, input, log};
use wal_protocol::Axis;
use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

const PLAYERS: usize = 4;

/// The game, told by its executable.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Game {
    RaccoonRampage,
    CrazyRide,
    ElevatorActionInvasion,
    FantasySoccer,
}

impl Game {
    /// Its `FCoUnisIoData`'s size.
    fn io_len(self) -> usize {
        match self {
            Game::RaccoonRampage => 49,
            Game::CrazyRide => 33,
            Game::ElevatorActionInvasion => 17,
            Game::FantasySoccer => 33,
        }
    }

    /// Its plugin DLL.
    fn dll(self) -> *const u16 {
        match self {
            Game::RaccoonRampage | Game::ElevatorActionInvasion => windows_sys::w!("UsbPluginsDll.dll"),
            Game::CrazyRide | Game::FantasySoccer => windows_sys::w!("UsbPluginsDll_R.dll"),
        }
    }
}

static GAME: OnceLock<Game> = OnceLock::new();

fn game() -> Game {
    *GAME.get_or_init(|| {
        let exe = wal_payload_common::patches::exe_name().to_ascii_lowercase();
        if exe.starts_with("crazytaxigame") {
            Game::CrazyRide
        } else if exe.starts_with("esgame") {
            Game::ElevatorActionInvasion
        } else if exe.starts_with("fb-") {
            Game::FantasySoccer
        } else {
            Game::RaccoonRampage
        }
    })
}
/// The picture the game's aim is in (`UBPGameInstance::DefaultScreenSizeX`, `Y`).
const PICTURE: (f32, f32) = (1920.0, 1080.0);

/// Raccoon Rampage 1.4's code (RVAs, the bytes there checked first).
const GUN_DATA_TO_SCREEN: usize = 0x9B_3110;
const GUN_DATA_TO_SCREEN_CODE: [u8; 5] = [0x44, 0x88, 0x4C, 0x24, 0x20];
/// `IsGunCalibrate`'s `xor al, al` (not calibrated) at its end.
const IS_GUN_CALIBRATE_FALSE: usize = 0x9B_7290;

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Trigger,
    Coin,
    /// An operator's button: its bit of byte 2.
    Operator(u8),
    /// Crazy Ride's cabinet buttons: their bit of byte 3.
    Cabinet(u8),
    /// Elevator Action Invasion's start buttons.
    Start,
    /// Elevator Action Invasion's gun buttons: their `FPACKET` offset (middle, right, the
    /// D-pad's up, down, left, right).
    GunButton(u8),
    /// Fantasy Soccer's ball kick: its strength (1..255).
    Kick(u8),
    /// Fantasy Soccer's buttons: 0 and 1 byte 15's (the red one: accelerate, start; the blue
    /// one), 2 byte 4's (not on the operator menu's input test).
    Button(u8),
}

const NATIVES: &[(&str, Native)] = &[
    ("trigger", Native::Trigger),
    ("coin", Native::Coin),
    ("test", Native::Operator(2)),
    ("down", Native::Operator(1)),
    ("enter", Native::Operator(0)),
    ("start", Native::Cabinet(0)),
    ("pause", Native::Cabinet(1)),
    ("p1start", Native::Start),
    ("gunmiddle", Native::GunButton(0x52)),
    ("gunright", Native::GunButton(0x53)),
    ("gunup", Native::GunButton(0x54)),
    ("gundown", Native::GunButton(0x55)),
    ("gunleft", Native::GunButton(0x56)),
    ("gunright2", Native::GunButton(0x57)),
    ("kick", Native::Kick(255)),
    ("pass", Native::Kick(110)),
    ("accelerate", Native::Button(0)),
    ("switch", Native::Button(1)),
    ("button2", Native::Button(2)),
];

/// Raccoon Rampage's defaults.
const RACCOON_RAMPAGE_DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    // the trigger starts a game (the cabinet has no start buttons)
    ("start", "trigger"),
    ("coin", "coin"),
    // the operator menu: test opens it and moves up, down moves down, service enters
    ("test", "test"),
    ("up", "test"),
    ("down", "down"),
    ("service", "enter"),
    ("b2", "enter"),
];

/// Crazy Ride's: the start button (in a race, while a mission runs, the nitro: the cabinet's
/// "wing", `IO_TurnOnWing`), also on b1; the operator's (test opens the menu and moves up; not
/// on up: the keyboard's up arrow is the gas).
const CRAZY_RIDE_DEFAULTS: &[(&str, &str)] = &[
    ("start", "start"),
    ("b1", "start"),
    ("coin", "coin"),
    ("test", "test"),
    ("down", "down"),
    ("service", "enter"),
];

/// Elevator Action Invasion's: the start buttons, the guns' buttons (their D-pad's up and
/// down), the operator's (test opens the menu and moves, service enters).
const ELEVATOR_ACTION_INVASION_DEFAULTS: &[(&str, &str)] = &[
    ("b1", "trigger"),
    ("start", "p1start"),
    ("coin", "coin"),
    ("b2", "gunright"),
    ("b3", "gunmiddle"),
    ("up", "gunup"),
    ("down", "gundown"),
    ("test", "test"),
    ("service", "enter"),
];

/// Fantasy Soccer's: the stick moves the player, the operator's buttons on their own keys.
const FANTASY_SOCCER_DEFAULTS: &[(&str, &str)] = &[
    // the ball (a strong kick, a pass), the red button (accelerate, start), the blue one
    ("b1", "kick"),
    ("b2", "pass"),
    ("start", "accelerate"),
    ("b3", "accelerate"),
    ("b4", "switch"),
    ("coin", "coin"),
    // the operator menu: test opens it and moves down, up moves up (byte 3 bit 6: nothing out
    // of the menu), service enters
    ("test", "test"),
    ("up", "down"),
    ("service", "enter"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    // one native per pad button (a button's last mapping is the one kept): each game its own
    let defaults = match game() {
        Game::RaccoonRampage => RACCOON_RAMPAGE_DEFAULTS,
        Game::CrazyRide => CRAZY_RIDE_DEFAULTS,
        Game::ElevatorActionInvasion => ELEVATOR_ACTION_INVASION_DEFAULTS,
        Game::FantasySoccer => FANTASY_SOCCER_DEFAULTS,
    };
    MAP.get_or_init(|| ButtonMap::new(NATIVES, defaults))
}

struct Coins {
    count: [i32; PLAYERS],
    /// Every coin since the start (Fantasy Soccer's counter).
    total: [i32; PLAYERS],
    held: [bool; PLAYERS],
}

static COINS: Mutex<Coins> = Mutex::new(Coins { count: [0; PLAYERS], total: [0; PLAYERS], held: [false; PLAYERS] });

/// Counts the coin keys' presses (each I/O read).
fn count_coins(coin: [bool; PLAYERS]) {
    let mut c = COINS.lock().unwrap();
    for p in 0..PLAYERS {
        if coin[p] && !c.held[p] {
            c.count[p] = (c.count[p] + 1).min(99);
            c.total[p] = c.total[p].wrapping_add(1);
        }
        c.held[p] = coin[p];
    }
}

unsafe extern "C" fn init_unis_io() -> bool {
    log!("unis: BIND_InitUnisIo");
    true
}

unsafe extern "C" fn is_io_working() -> bool {
    true
}

unsafe extern "C" fn get_io_data(io: *mut u8) -> bool {
    if io.is_null() {
        return false;
    }
    let game = game();
    let mut d = [0u8; 49];
    let mut coin = [false; PLAYERS];
    match game {
        Game::RaccoonRampage => {
            // the water tank at its normal level
            d[5] = 0x04;
            for (player, coin) in coin.iter_mut().enumerate() {
                map().for_each_pressed(&input(player), |n| match n {
                    Native::Trigger => d[5] |= 0x80 >> player,
                    Native::Coin => *coin = true,
                    Native::Operator(bit) if player == 0 => d[2] |= 1 << bit,
                    Native::Operator(_) | Native::Cabinet(_) | Native::Start | Native::GunButton(_) | Native::Kick(_) | Native::Button(_) => {}
                });
            }
        }
        Game::ElevatorActionInvasion => {
            for (player, coin) in coin.iter_mut().enumerate().take(2) {
                let stick = input(player);
                map().for_each_pressed(&stick, |n| match n {
                    Native::Trigger => d[13] |= 2 >> player,
                    Native::Start => d[2] |= 0x20 >> player,
                    Native::Coin => *coin = true,
                    Native::Operator(bit) if player == 0 => d[2] |= 1 << bit,
                    Native::Operator(_) | Native::Cabinet(_) | Native::GunButton(_) | Native::Kick(_) | Native::Button(_) => {}
                });
                if let Some((x, y)) = gun::gun_of_player_in_picture(player) {
                    EAI_AIM.lock().unwrap()[player] = ((x * 4095.0) as u16, (y * 4095.0) as u16);
                }
                let (x, y) = EAI_AIM.lock().unwrap()[player];
                let at = 5 + 4 * player;
                d[at..at + 2].copy_from_slice(&x.to_be_bytes());
                d[at + 2..at + 4].copy_from_slice(&y.to_be_bytes());
            }
            // the doors' sensors: their closed part (0 open .. 10 closed)
            let (left, right) = crate::eai_doors::closed();
            d[3] = left;
            d[4] = right;
        }
        Game::CrazyRide => {
            let stick = input(0);
            // the seat belt fastened
            d[3] = 0x04;
            map().for_each_pressed(&stick, |n| match n {
                Native::Coin => coin[0] = true,
                Native::Operator(bit) => d[2] |= 1 << bit,
                Native::Cabinet(bit) => d[3] |= 1 << bit,
                Native::Trigger | Native::Start | Native::GunButton(_) | Native::Kick(_) | Native::Button(_) => {}
            });
            let pedal = |a: Axis| (stick.axis(a).max(0) as u32 * 254 / 32767 + 1).min(255) as u8;
            d[6] = pedal(Axis::Brake);
            d[7] = pedal(Axis::Accel);
        }
        Game::FantasySoccer => {
            let mut kicked = KICKED.lock().unwrap();
            for (player, coin) in coin.iter_mut().enumerate() {
                let stick = input(player);
                let mut kick = 0u8;
                map().for_each_pressed(&stick, |n| match n {
                    Native::Coin => *coin = true,
                    // the operator's: byte 3 bits 7 test (down), 6 up (the `down` native), 5 enter
                    Native::Operator(bit) if player == 0 => d[3] |= 0x20 << bit,
                    Native::Kick(strength) => kick = kick.max(strength),
                    Native::Button(2) => d[4] |= 0x08 >> player,
                    Native::Button(b) => d[15] |= 0x80 >> (b as usize + 2 * player),
                    _ => {}
                });
                // the ball's sensor: the kick's strength for one read
                d[11 + player] = if kick != 0 && kicked[player] == 0 { kick } else { 0 };
                kicked[player] = kick;
                // the stick: 255 full left / up .. 0 full right / down, 128 the centre
                let axis = |a: Axis| 255 - ((stick.axis(a) as i32 + 32768) >> 8) as u8;
                d[16 + 2 * player] = axis(Axis::LeftX);
                d[17 + 2 * player] = axis(Axis::LeftY);
            }
        }
    }
    count_coins(coin);
    unsafe { std::ptr::copy_nonoverlapping(d.as_ptr(), io, game.io_len()) };
    true
}

/// Fantasy Soccer: each player's kick strength at the last read (a kick is one read's).
static KICKED: Mutex<[u8; PLAYERS]> = Mutex::new([0; PLAYERS]);

/// Elevator Action Invasion: each gun's last raw aim (0..4095 across the picture).
static EAI_AIM: Mutex<[(u16, u16); 2]> = Mutex::new([(2048, 2048); 2]);

/// Elevator Action Invasion's USB guns: `BIND_GetGunData(player, FPACKET*)`, the gun's place
/// on the 1080x1920 picture (`iX`, `iY`), its trigger (`bLeftBtn`) and buttons; `iUSB` 1 (the
/// game's `IsGun` wants it above 0, a negative one stops the gun); the gun's calibration
/// (`LeftTop`, `Middle`, `RightBotton`: the picture's corners and middle, else "enter Setting
/// Menu to calibrate").
unsafe extern "C" fn get_gun_data(player: i32, packet: *mut u8) -> i32 {
    let Some(p) = usize::try_from(player).ok().filter(|&p| p < 2) else { return 0 };
    if packet.is_null() {
        return 0;
    }
    let mut aim = AIM.lock().unwrap();
    if let Some((x, y)) = gun::gun_of_player_in_picture(p) {
        aim[p] = ((x * 1080.0) as i32, (y * 1920.0) as i32);
    }
    let (x, y) = aim[p];
    drop(aim);
    let mut buttons = [false; 0x58];
    let mut trigger = false;
    map().for_each_pressed(&input(p), |n| match n {
        Native::Trigger => trigger = true,
        Native::GunButton(at) => buttons[at as usize] = true,
        _ => {}
    });
    let write_i32 = |at: usize, v: i32| unsafe { packet.add(at).cast::<i32>().write_unaligned(v) };
    write_i32(0, player);
    write_i32(4, x);
    write_i32(8, y);
    write_i32(0x14, x);
    write_i32(0x18, y);
    write_i32(0x34, 1);
    for (at, v) in [(0x38, 0), (0x3c, 0), (0x40, 540), (0x44, 960), (0x48, 1080), (0x4c, 1920)] {
        write_i32(at, v);
    }
    unsafe {
        *packet.add(0x50) = 0;
        *packet.add(0x51) = trigger as u8;
        for at in 0x52..0x58 {
            *packet.add(at) = buttons[at] as u8;
        }
        *packet.add(0x59) = 1;
    }
    0
}

/// `BIND_SetDoorData(left, right)`: the doors' closed part (0 open .. 10 closed).
unsafe extern "C" fn set_door_data(left: i32, right: i32) -> bool {
    crate::eai_doors::set(left, right);
    true
}

unsafe extern "C" fn get_coin_num(player: i32) -> i32 {
    usize::try_from(player).ok().filter(|&p| p < PLAYERS).map_or(0, |p| COINS.lock().unwrap().count[p])
}

unsafe extern "C" fn dec_coins(player: i32, count: i32) -> i32 {
    let Some(p) = usize::try_from(player).ok().filter(|&p| p < PLAYERS) else { return 0 };
    let mut c = COINS.lock().unwrap();
    c.count[p] = (c.count[p] - count).max(0);
    c.count[p]
}

unsafe extern "C" fn add_keyboard_coins(player: i32, count: i32) -> i32 {
    let Some(p) = usize::try_from(player).ok().filter(|&p| p < PLAYERS) else { return 0 };
    let mut c = COINS.lock().unwrap();
    c.count[p] = (c.count[p] + count).clamp(0, 99);
    c.count[p]
}

// Crazy Ride: one credit counter

unsafe extern "C" fn internal_coins() -> i32 {
    COINS.lock().unwrap().count[0]
}

/// Fantasy Soccer's `BIND_GetInternalCoins(player)`: the player's coins since the start (the
/// game adds each increase to its credits).
unsafe extern "C" fn total_coins(player: i32) -> i32 {
    usize::try_from(player).ok().filter(|&p| p < PLAYERS).map_or(0, |p| COINS.lock().unwrap().total[p])
}

unsafe extern "C" fn dec_internal_coins(count: i32) -> bool {
    let mut c = COINS.lock().unwrap();
    c.count[0] = (c.count[0] - count).max(0);
    true
}

unsafe extern "C" fn add_internal_coins(count: i32) -> bool {
    let mut c = COINS.lock().unwrap();
    c.count[0] = (c.count[0] + count).clamp(0, 99);
    true
}

/// The wheel (player 1's `lx`): 255 full left .. 1 full right, 128 the centre.
/// Crazy Ride's `BIND_SetSteeringWheelData(force, centre, shake, 0)`: the wheel motor's centring
/// force (the operator menu's `iSteeringWheelForceFeedback`, 0..10, 3 by default; 0 while the
/// game starts), its centre (128) and its shaking (`iSteeringWheelShakingForce` while the car
/// hits or bumps, 0 otherwise): the player's force feedback, a centring spring and a vibration
/// (each level a tenth of the full force).
unsafe extern "C" fn set_steering_wheel_data(force: i32, centre: i32, shake: i32, _unused: i32) {
    use wal_protocol::output::{FFB_MAX, FFB_SPRING, FFB_VIBRATION};
    static LAST: Mutex<Option<(i32, i32)>> = Mutex::new(None);
    let level = |l: i32| l.clamp(0, 10) * FFB_MAX / 10;
    let (spring, vibration) = (level(force), level(shake));
    let mut last = LAST.lock().unwrap();
    let previous = *last;
    if previous.is_some_and(|p| p == (spring, vibration)) {
        return;
    }
    *last = Some((spring, vibration));
    if previous.is_none_or(|p| p.0 != spring) {
        log!("unis: wheel force {force} (centre {centre}): spring {spring}");
        wal_payload_common::send_output(0, FFB_SPRING, spring);
    }
    if previous.is_none_or(|p| p.1 != vibration) {
        wal_payload_common::send_output(0, FFB_VIBRATION, vibration);
    }
}

unsafe extern "C" fn steering_wheel_angle() -> i32 {
    (128 - input(0).axis(Axis::LeftX) as i32 * 127 / 32767).clamp(1, 255)
}

unsafe extern "C" fn open_device(_port: i32) -> bool {
    true
}

unsafe extern "C" fn yes() -> bool {
    true
}

unsafe extern "C" fn dongle_verify(_challenge: *const u8) -> bool {
    true
}

unsafe extern "C" fn set_local_time(_: i32, _: i32, _: i32, _: i32, _: i32, _: i32, _: i32, _: i32) -> bool {
    true
}

/// The DLL's functions the payload answers.
fn replacement(name: &[u8]) -> Option<usize> {
    let f = |f: usize| Some(f);
    let common = match name {
        b"BIND_InitUnisIo" => f(init_unis_io as *const () as usize),
        b"BIND_IsIoWorking" => f(is_io_working as *const () as usize),
        b"BIND_GetIoData" => f(get_io_data as *const () as usize),
        b"BIND_InitDongle" | b"BIND_IsDongleExist" | b"BIND_UnInitDongle" => f(yes as *const () as usize),
        b"BIND_IsDongleVerify" => f(dongle_verify as *const () as usize),
        b"BIND_SetLocalTime" => f(set_local_time as *const () as usize),
        _ => None,
    };
    common.or_else(|| match (game(), name) {
        (Game::RaccoonRampage, b"BIND_GetCoinNumByPlayer") => f(get_coin_num as *const () as usize),
        (Game::RaccoonRampage, b"BIND_DecCoins") => f(dec_coins as *const () as usize),
        (Game::RaccoonRampage, b"BIND_AddKeyboardCoins") => f(add_keyboard_coins as *const () as usize),
        (Game::ElevatorActionInvasion, b"BIND_GetCoinNumByPlayer") => f(get_coin_num as *const () as usize),
        (Game::ElevatorActionInvasion, b"BIND_DecCoins") => f(dec_coins as *const () as usize),
        (Game::ElevatorActionInvasion, b"BIND_AddKeyboardCoins") => f(add_keyboard_coins as *const () as usize),
        (Game::ElevatorActionInvasion, b"BIND_IsExistESIODevice" | b"BIND_OpenESIODevice") => f(open_device as *const () as usize),
        (Game::ElevatorActionInvasion, b"BIND_GetESIoData") => f(get_io_data as *const () as usize),
        (Game::ElevatorActionInvasion, b"BIND_GetGunData") => f(get_gun_data as *const () as usize),
        (Game::ElevatorActionInvasion, b"BIND_OpenGunDevice") => f(yes as *const () as usize),
        (Game::ElevatorActionInvasion, b"BIND_SetDoorData") => f(set_door_data as *const () as usize),
        (Game::CrazyRide, b"BIND_GetInternalCoins") => f(internal_coins as *const () as usize),
        (Game::CrazyRide, b"BIND_DecCoins") => f(dec_internal_coins as *const () as usize),
        (Game::CrazyRide, b"BIND_AddKeyboardCoins") => f(add_internal_coins as *const () as usize),
        (Game::FantasySoccer, b"BIND_GetInternalCoins") => f(total_coins as *const () as usize),
        (Game::CrazyRide, b"BIND_IsExistSteeringWheel") => f(yes as *const () as usize),
        (Game::CrazyRide, b"BIND_OpenSteeringWheelDevice") => f(open_device as *const () as usize),
        (Game::CrazyRide, b"BIND_SetSteeringWheelData") => f(set_steering_wheel_data as *const () as usize),
        (Game::CrazyRide, b"BIND_GetSteeringWheelAngle") => f(steering_wheel_angle as *const () as usize),
        _ => None,
    })
}

static REAL_GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn get_proc_address(module: HMODULE, name: *const u8) -> usize {
    let real: unsafe extern "system" fn(HMODULE, *const u8) -> usize =
        unsafe { std::mem::transmute(REAL_GET_PROC_ADDRESS.load(Ordering::Relaxed)) };
    // by name (not an ordinal), in the game's plugin DLL
    if name as usize > 0xFFFF && module == unsafe { GetModuleHandleW(game().dll()) } {
        let n = unsafe { CStr::from_ptr(name.cast()) }.to_bytes();
        let f = replacement(n);
        static SEEN: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
        let mut seen = SEEN.lock().unwrap();
        if !seen.iter().any(|s| s == n) {
            seen.push(n.to_vec());
            log!("unis: {} {}", String::from_utf8_lossy(n), if f.is_some() { "answered" } else { "the DLL's" });
        }
        drop(seen);
        if let Some(f) = f {
            return f;
        }
    }
    unsafe { real(module, name) }
}

/// Each gun's last place in the game's picture (pixels), kept while it points off it.
static AIM: Mutex<[(i32, i32); PLAYERS]> = Mutex::new([(960, 540); PLAYERS]);

/// `ABPGameState::GunDataToScreen(player, FPACKET&, smooth)`: the packet's `iX`, `iY`
/// (offsets 0x14, 0x18) where the gun points.
unsafe extern "system" fn gun_data_to_screen(_state: *mut c_void, player: i32, packet: *mut u8, _smooth: bool) {
    let Some(p) = usize::try_from(player).ok().filter(|&p| p < PLAYERS) else { return };
    if packet.is_null() {
        return;
    }
    let mut aim = AIM.lock().unwrap();
    if let Some((x, y)) = gun::gun_of_player_in_picture(p) {
        aim[p] = ((x * PICTURE.0) as i32, (y * PICTURE.1) as i32);
    }
    unsafe {
        packet.add(0x14).cast::<i32>().write_unaligned(aim[p].0);
        packet.add(0x18).cast::<i32>().write_unaligned(aim[p].1);
    }
}

unsafe fn write_code(at: usize, code: &[u8]) {
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, code.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, code.len());
        VirtualProtect(at as *const c_void, code.len(), old, &mut old);
    }
}

/// The aim's patches, on Raccoon Rampage 1.4's code only.
fn patch_aim() {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let at = base + GUN_DATA_TO_SCREEN;
    let calibrate = base + IS_GUN_CALIBRATE_FALSE;
    let found = unsafe { std::slice::from_raw_parts(at as *const u8, GUN_DATA_TO_SCREEN_CODE.len()) };
    let calibrate_found = unsafe { std::slice::from_raw_parts(calibrate as *const u8, 2) };
    if found != GUN_DATA_TO_SCREEN_CODE || calibrate_found != [0x32, 0xC0] {
        log!("unis: not Raccoon Rampage 1.4's code, guns not patched");
        return;
    }
    // mov rax, gun_data_to_screen; jmp rax
    let mut jump = [0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xE0];
    jump[2..10].copy_from_slice(&(gun_data_to_screen as *const () as usize as u64).to_le_bytes());
    unsafe {
        write_code(at, &jump);
        // mov al, 1
        write_code(calibrate, &[0xB0, 0x01]);
    }
    log!("unis: GunDataToScreen replaced, IsGunCalibrate true");
}

/// Elevator Action Invasion's `UGameData::GetLevelIsUnlock`: a level is unlocked when every
/// level of its list (`FSelectLevelData`) is passed; its `je` (RVA 0x7687CE: one not passed,
/// locked).
const LEVEL_LOCKED_JUMP: usize = 0x76_87CE;
const LEVEL_LOCKED_JUMP_CODE: [u8; 6] = [0x0F, 0x84, 0x5A, 0xFF, 0xFF, 0xFF];

/// Every level of the mission select unlocked (the game keeps no progress: only the levels
/// open from the start, those passed in the session added).
fn unlock_levels() {
    let at = unsafe { GetModuleHandleW(std::ptr::null()) } as usize + LEVEL_LOCKED_JUMP;
    let found = unsafe { std::slice::from_raw_parts(at as *const u8, LEVEL_LOCKED_JUMP_CODE.len()) };
    if found != LEVEL_LOCKED_JUMP_CODE {
        log!("unis: not Elevator Action Invasion's known code, levels not unlocked");
        return;
    }
    // a 6-byte nop
    unsafe { write_code(at, &[0x66, 0x0F, 0x1F, 0x44, 0x00, 0x00]) };
    log!("unis: every level unlocked");
}

/// A packed executable (Fantasy Soccer's) finds the plugin DLL's functions past the hooked
/// `GetProcAddress`: once the DLL is loaded, each export the payload answers jumps to the
/// payload's function (`mov rax, f; jmp rax` at its start: every export is 16 bytes or more).
fn patch_exports() {
    std::thread::spawn(|| {
        let module = loop {
            let m = unsafe { GetModuleHandleW(game().dll()) };
            if !m.is_null() {
                break m as usize;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        };
        let mut patched = 0;
        for name in unsafe { export_names(module) } {
            let Some(f) = replacement(name.as_bytes()) else { continue };
            let Ok(cname) = std::ffi::CString::new(name.as_str()) else { continue };
            let Some(at) = (unsafe { windows_sys::Win32::System::LibraryLoader::GetProcAddress(module as HMODULE, cname.as_ptr().cast()) }) else { continue };
            let mut jump = [0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xE0];
            jump[2..10].copy_from_slice(&(f as u64).to_le_bytes());
            unsafe { write_code(at as usize, &jump) };
            patched += 1;
            log!("unis: {name} answered (the DLL's function patched)");
        }
        log!("unis: {patched} functions of the plugin DLL patched");
    });
}

/// Fantasy Soccer's two screens (`AManage_Menu`'s `bEnableSplitscreen`, on in the game's
/// config: a 3840x1080 window, the cabinet's second screen's video beside the game): one, unless
/// `WAL_UNIS_SPLITSCREEN=1`. The user's saved config (read after the game's) sets it off.
fn one_screen() {
    if std::env::var("WAL_UNIS_SPLITSCREEN").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    let Ok(local) = std::env::var("LOCALAPPDATA") else { return };
    let dir = std::path::Path::new(&local).join("FB\\Saved\\Config\\WindowsNoEditor");
    let path = dir.join("Manage_Menu.ini");
    let ini = std::fs::read_to_string(&path).unwrap_or_default();
    if ini.contains("bEnableSplitscreen=False") {
        return;
    }
    let kept: Vec<&str> = ini.lines().filter(|l| !l.starts_with("bEnableSplitscreen=")).collect();
    let ini = format!("{}\r\n[/Script/FB.Manage_Menu]\r\nbEnableSplitscreen=False\r\n", kept.join("\r\n").trim_end());
    match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, ini.trim_start())) {
        Ok(()) => log!("unis: one screen ({})", path.display()),
        Err(e) => log!("unis: {}: {e}", path.display()),
    }
}

/// The names a DLL exports.
unsafe fn export_names(base: usize) -> Vec<String> {
    let read32 = |at: usize| unsafe { (at as *const u32).read_unaligned() } as usize;
    let nt = base + read32(base + 0x3C);
    let dir = read32(nt + 24 + 112);
    if dir == 0 {
        return Vec::new();
    }
    let count = read32(base + dir + 24);
    let names = base + read32(base + dir + 32);
    (0..count)
        .map(|i| unsafe { CStr::from_ptr((base + read32(names + 4 * i)) as *const std::ffi::c_char) }.to_string_lossy().into_owned())
        .collect()
}

pub fn init() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    match unsafe { iat::hook("kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) } {
        Some(real) => REAL_GET_PROC_ADDRESS.store(real, Ordering::Relaxed),
        None => log!("unis: the game imports no GetProcAddress"),
    }
    match game() {
        Game::RaccoonRampage => patch_aim(),
        Game::ElevatorActionInvasion => {
            unlock_levels();
            crate::eai_doors::init();
        }
        Game::FantasySoccer => {
            patch_exports();
            one_screen();
        }
        Game::CrazyRide => {}
    }
    log!("unis: usbplugins board, {:?}", game());
}
