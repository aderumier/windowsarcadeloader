//! Castlevania: The Arcade (HCV, 2009): the cabinet I/O through `hcvACLib.dll`.
//!
//! HCV.exe loads hcvACLib (`LoadLibraryA` + `GetProcAddress` of its 86 functions): the I/O board
//! (buttons, coins, lamps) and the camera that follows the players' whips (blobs, calibrated to
//! the screen). Its `setting.csv` debug switches `emulateIO` and `emulateSensor` (both TRUE in
//! the dump) boot the library without hardware (`hcvACLibBoot(1)`) and leave the game without
//! input: the buttons and coins read nothing, the whips' history is only shifted. The game is
//! patched to read them again, from the virtual sticks:
//!
//! - Buttons: the game's button read (`hcvACLibIOGetButton`, 30 bytes, the game's bits: 10
//!   0x200, 11 SERVICE 0x1, 12 TEST 0x2, 13 0x4, 14/15 the players' START 0x8/0x10, 16/17 their
//!   whip's red button 0x80/0x100, 18/19 its yellow button 0x20/0x40) answered here.
//! - Coins: the coin thread's `hcvACLibIOCoinGetIncrease` / `CoinConsume`, answered here.
//! - The whips: each update (30 per second), after the history shift, the current sample of each
//!   player (x, y on the screen, speed, camera x, y, on screen, seen) is the player's pointer.
//!   The whip cracks on a stroke across the whole screen (`SWING_START`): the attack control
//!   makes a vertical one through the aim point. The yellow button (`subweapon`) throws the
//!   selected sub-weapon at the whip's position (when the player has the hearts it costs; menus:
//!   point and press it), the red one (`select`) cycles the player's sub-weapons.
//!
//! Lightgun controls: `attack` (the trigger) swings the whip, or throws the sub-weapon after
//! `switch` (toggles the trigger between the two); `whip`, `subweapon` and `select` are the
//! cabinet's own. `WAL_KONAMI_UNLIMITED_SUBWEAPON=1`: throwing a sub-weapon costs no hearts.
//! `WAL_KONAMI_WHIP_SHOT=0`: classic movement: dragging with the attack control held swings the
//! whip in the drag's direction.
//!
//! Native inputs for `native_map`: `test service coin start attack switch whip subweapon select`.

use std::ffi::{CStr, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::{iat, log};
use wal_protocol::Axis;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Test,
    Service,
    Coin,
    Start,
    Attack,
    Switch,
    Whip,
    Subweapon,
    Select,
}

const NATIVES: &[(&str, Native)] = &[
    ("test", Native::Test),
    ("service", Native::Service),
    ("coin", Native::Coin),
    ("start", Native::Start),
    ("attack", Native::Attack),
    ("switch", Native::Switch),
    ("whip", Native::Whip),
    ("subweapon", Native::Subweapon),
    ("select", Native::Select),
];

const DEFAULT_MAP: &[(&str, &str)] = &[
    ("test", "test"),
    ("service", "service"),
    ("coin", "coin"),
    ("start", "start"),
    ("b1", "attack"),
    ("b2", "switch"),
    ("b3", "subweapon"),
    ("b4", "select"),
];

/// The players (two whips).
const PLAYERS: usize = 2;

/// The game's screen size (`setting.csv` screenWidth / screenHeight), the whips' coordinates.
const SCREEN_WIDTH: usize = 0x72508c;
const SCREEN_HEIGHT: usize = 0x725090;
/// Player 1's current whip sample (the I/O object 0x7280e8 + 0xb8), player 2's 0x4e14 further:
/// x, y (f32, screen), speed (f32: the distance from the previous sample), camera x, y (i32),
/// on screen, seen (u8). The previous samples follow, 0x18 bytes each.
const SAMPLES: usize = 0x7281a0;
const PLAYER_STRIDE: usize = 0x4e14;
const SAMPLE_SIZE: usize = 0x18;
/// The speed of a sample without a seen one before it.
const NO_SPEED: usize = 0x69b2b4;
/// The emulated sensor's update (`this` the I/O object): the history shift.
const EMULATED_SENSOR: usize = 0x480ef0;
/// Its call in the I/O update: pointed at `sensor_update`.
const SENSOR_CALL: usize = 0x483965;

/// A whip swing. The game (30 updates per second) follows each whip with a spring-damped
/// cursor (stiffness 350, damping 25) and cracks it when, in the cursor's last 5 positions, it
/// moved more than 300 pixels in 2 updates and its stroke from 4 updates ago to 1 ago crosses
/// the screen, from beyond one edge to beyond the opposite one: the whip hits along that line.
/// The swing pulls the cursor `SWING_FAR` screen heights (widths) beyond the top (left) for
/// `SWING_START` updates, then beyond the bottom (right) for `SWING_END`, or the other way,
/// through the aim point. 0.5 is the least that cracks the whip from any aim (0.4 misses near
/// the edges). A pointer kept on the screen never cracks it by itself: the game wants the
/// stroke's ends beyond the edges, as the cabinet's camera sees them.
const SWING_START: u32 = 2;
const SWING_END: u32 = 3;
const SWING_FAR: f32 = 0.55;
/// Updates between two swings, or two throws, while the button is held.
const REPEAT_FRAMES: u32 = 12;
/// Mouse movement (`WAL_KONAMI_WHIP_SHOT=0`): with the trigger held, a drag longer than this part
/// of the screen (width or height) swings the whip in its direction (any angle),
/// then the drag goes on from there.
const DRAG: f32 = 0.05;

#[derive(Clone, Copy, Default)]
struct Player {
    /// The trigger throws the sub-weapon (`switch`), else it swings the whip.
    subweapon_mode: bool,
    switch_held: bool,
    /// Frames since the current swing started (`None`: no swing).
    swing: Option<u32>,
    /// The current swing's direction (pixels), and its lightgun shot alternates up and down.
    direction: (f32, f32),
    backhand: bool,
    /// Mouse movement: where the held trigger's drag started (`None`: not held).
    anchor: Option<(f32, f32)>,
    /// Frames until a held attack repeats.
    repeat: u32,
    attack_held: bool,
    /// The sub-weapon button sent to the game (it reacts to its press).
    throw: bool,
}

struct Io {
    maps: Vec<ButtonMap<Native>>,
    players: [Player; PLAYERS],
    coin_held: bool,
    coins: u32,
}

static IO: Mutex<Option<Io>> = Mutex::new(None);

fn with_io<R>(f: impl FnOnce(&mut Io) -> R) -> R {
    let mut guard = IO.lock().unwrap();
    let io = guard.get_or_insert_with(|| Io {
        maps: (0..PLAYERS)
            .map(|p| if p == 0 { ButtonMap::new(NATIVES, DEFAULT_MAP) } else { ButtonMap::with_defaults(NATIVES, DEFAULT_MAP) })
            .collect(),
        players: [Player::default(); PLAYERS],
        coin_held: false,
        coins: 0,
    });
    f(io)
}

fn natives(io: &Io, player: usize) -> Vec<Native> {
    let mut v = Vec::new();
    io.maps[player].for_each_pressed(&wal_payload_common::input(player), |n| v.push(n));
    v
}

// hcvACLib functions answered here (cdecl, 0 = success).

unsafe extern "C" fn get_button(out: *mut u8) -> i32 {
    if out.is_null() {
        return -1;
    }
    let mut buf = [0u8; 30];
    with_io(|io| {
        for player in 0..PLAYERS {
            for n in natives(io, player) {
                match n {
                    Native::Test => buf[12] = 1,
                    Native::Service => buf[11] = 1,
                    Native::Start => buf[14 + player] = 1,
                    Native::Select => buf[16 + player] = 1,
                    Native::Subweapon => buf[18 + player] = 1,
                    _ => {}
                }
            }
            if io.players[player].throw {
                buf[18 + player] = 1;
            }
        }
    });
    unsafe { std::ptr::copy_nonoverlapping(buf.as_ptr(), out, buf.len()) };
    0
}

unsafe extern "C" fn coin_get_increase(out: *mut u32) -> i32 {
    let coins = with_io(|io| {
        let coin = (0..PLAYERS).any(|p| natives(io, p).contains(&Native::Coin));
        if coin && !io.coin_held {
            io.coins += 1;
        }
        io.coin_held = coin;
        std::mem::take(&mut io.coins)
    });
    if !out.is_null() {
        unsafe { *out = coins };
    }
    0
}

unsafe extern "C" fn coin_consume(_count: u32) -> i32 {
    0
}

/// The hcvACLib functions answered here.
fn override_for(name: &[u8]) -> Option<usize> {
    Some(match name {
        b"hcvACLibIOGetButton" => get_button as *const () as usize,
        b"hcvACLibIOCoinGetIncrease" => coin_get_increase as *const () as usize,
        b"hcvACLibIOCoinConsume" => coin_consume as *const () as usize,
        _ => return None,
    })
}

static ORIG_GET_PROC: AtomicUsize = AtomicUsize::new(0);
/// `WAL_KONAMI_WHIP_SHOT`: the attack control swings the whip (default); off, dragging with it
/// held does.
static WHIP_SHOT: AtomicBool = AtomicBool::new(true);

unsafe extern "system" fn get_proc_address(module: *mut c_void, name: *const u8) -> *const c_void {
    let orig: unsafe extern "system" fn(*mut c_void, *const u8) -> *const c_void =
        unsafe { std::mem::transmute(ORIG_GET_PROC.load(Ordering::Relaxed)) };
    let proc = unsafe { orig(module, name) };
    if name as usize <= 0xffff {
        return proc;
    }
    let name = unsafe { CStr::from_ptr(name.cast()) }.to_bytes();
    match override_for(name) {
        Some(f) => f as *const c_void,
        None => proc,
    }
}

/// One update of a player's controls: the whip's position (and the sub-weapon button sent to
/// the game, `Player::throw`).
fn step(player: usize, width: f32, height: f32) -> (f32, f32) {
    let stick = wal_payload_common::input(player);
    let shot = WHIP_SHOT.load(Ordering::Relaxed);
    // on the screen: aiming beyond its edges never cracks the whip
    let aim_x = ((stick.axis(Axis::LeftX) as f32 + 32768.0) / 65535.0).clamp(0.0, 1.0) * width;
    let aim_y = ((stick.axis(Axis::LeftY) as f32 + 32768.0) / 65535.0).clamp(0.0, 1.0) * height;
    with_io(|io| {
        let pressed = natives(io, player);
        let p = &mut io.players[player];
        let switch = pressed.contains(&Native::Switch);
        if switch && !p.switch_held {
            p.subweapon_mode = !p.subweapon_mode;
            log!("hcv: player {} attacks with the {}", player + 1, if p.subweapon_mode { "sub-weapon" } else { "whip" });
        }
        p.switch_held = switch;
        let attack = pressed.contains(&Native::Attack);
        let whip = pressed.contains(&Native::Whip) || (shot && attack && !p.subweapon_mode);
        // mouse movement: the held trigger records the drag
        let drag = !shot && attack && !p.subweapon_mode;
        let throw = attack && p.subweapon_mode;
        // a held button repeats: released for one frame every REPEAT_FRAMES
        let held = whip || throw;
        if !held {
            p.repeat = 0;
        } else if p.attack_held {
            p.repeat += 1;
        }
        let again = held && (!p.attack_held || p.repeat >= REPEAT_FRAMES);
        if again {
            p.repeat = 0;
        }
        p.attack_held = held;
        p.throw = throw && p.repeat != REPEAT_FRAMES - 1;
        if !drag {
            p.anchor = None;
        }
        if p.swing.is_none() {
            if whip && again {
                // the shot: a vertical stroke, down then up
                p.swing = Some(0);
                p.backhand = !p.backhand;
                p.direction = (0.0, if p.backhand { -1.0 } else { 1.0 });
            } else if drag {
                // the drag since the trigger's press (or the last swing) swings the whip in its
                // direction once it is long enough
                let (ax, ay) = *p.anchor.get_or_insert((aim_x, aim_y));
                let (dx, dy) = (aim_x - ax, aim_y - ay);
                if (dx / width).abs().max((dy / height).abs()) > DRAG {
                    p.swing = Some(0);
                    p.direction = (dx, dy);
                    p.anchor = Some((aim_x, aim_y));
                }
            }
        }
        let Some(frame) = p.swing else { return (aim_x, aim_y) };
        p.swing = (frame + 1 < SWING_START + SWING_END).then_some(frame + 1);
        if p.swing.is_none() && p.anchor.is_some() {
            // the drag goes on from where the swing ended
            p.anchor = Some((aim_x, aim_y));
        }
        // along the swing's direction through the aim point, from beyond one edge of the screen
        // to beyond the opposite one (on the axis it moves most along)
        let (dx, dy) = p.direction;
        let vertical = (dy / height).abs() > (dx / width).abs();
        let (forward, aim, size, d) = if vertical { (dy > 0.0, aim_y, height, dy) } else { (dx > 0.0, aim_x, width, dx) };
        let (start, end) = if forward { (-SWING_FAR, 1.0 + SWING_FAR) } else { (1.0 + SWING_FAR, -SWING_FAR) };
        let t = ((if frame < SWING_START { start } else { end }) * size - aim) / d;
        (aim_x + t * dx, aim_y + t * dy)
    })
}

/// The I/O update's whip read: the emulated sensor's history shift, then each player's current
/// sample.
unsafe extern "thiscall" fn sensor_update(io: *mut c_void) {
    let shift: unsafe extern "thiscall" fn(*mut c_void) = unsafe { std::mem::transmute(EMULATED_SENSOR) };
    unsafe { shift(io) };
    let width = unsafe { *(SCREEN_WIDTH as *const i32) } as f32;
    let height = unsafe { *(SCREEN_HEIGHT as *const i32) } as f32;
    for player in 0..PLAYERS {
        let sample = (SAMPLES + player * PLAYER_STRIDE) as *mut u8;
        let (x, y) = step(player, width, height);
        unsafe {
            // the distance from the previous sample, when there is one
            let speed = if *sample.add(SAMPLE_SIZE + 0x15) != 0 {
                let px = *(sample.add(SAMPLE_SIZE) as *const f32);
                let py = *(sample.add(SAMPLE_SIZE + 4) as *const f32);
                ((x - px).powi(2) + (y - py).powi(2)).sqrt()
            } else {
                *(NO_SPEED as *const f32)
            };
            let on_screen = (0.0..width).contains(&x) && (0.0..height).contains(&y);
            (sample as *mut f32).write(x);
            (sample.add(4) as *mut f32).write(y);
            (sample.add(8) as *mut f32).write(speed);
            (sample.add(0xc) as *mut i32).write(x as i32);
            (sample.add(0x10) as *mut i32).write(y as i32);
            *sample.add(0x14) = on_screen as u8;
            *sample.add(0x15) = 1;
        }
    }
}

/// Writes `new` over `expected` at `at`, when the code is the expected one.
unsafe fn patch(what: &str, at: usize, expected: &[u8], new: &[u8]) {
    let current = unsafe { std::slice::from_raw_parts(at as *const u8, expected.len()) };
    if current != expected {
        log!("hcv: {what}: code differs ({current:02x?}), not patched");
        return;
    }
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, new.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(new.as_ptr(), at as *mut u8, new.len());
        VirtualProtect(at as *const c_void, new.len(), old, &mut old);
    }
    log!("hcv: {what}");
}

pub fn init() {
    WHIP_SHOT.store(std::env::var("WAL_KONAMI_WHIP_SHOT").map_or(true, |v| v != "0"), Ordering::Relaxed);
    log!("hcv: whip {}", if WHIP_SHOT.load(Ordering::Relaxed) { "swung by the attack control" } else { "swung by drags with the attack control held" });
    let call = (sensor_update as *const () as usize).wrapping_sub(SENSOR_CALL + 5) as u32;
    let mut sensor_call = [0xe8, 0, 0, 0, 0];
    sensor_call[1..].copy_from_slice(&call.to_le_bytes());
    unsafe {
        // the I/O update's emulateIO branch (no buttons): `je` -> `jmp` to the button read
        patch("buttons read with emulateIO", 0x483978, &[0x74, 0x15], &[0xeb, 0x15]);
        // the coin thread's emulateIO branch (no coins): `je` -> `jmp` to the coin read
        patch("coins read with emulateIO", 0x482e66, &[0x0f, 0x84, 0xa0, 0, 0, 0], &[0xe9, 0xa1, 0, 0, 0, 0x90]);
        patch("whips from the virtual sticks", SENSOR_CALL, &[0xe8, 0x86, 0xd5, 0xff, 0xff], &sensor_call);
        // setting.csv's useSecretChara (FALSE in the dump), the secret character (Little
        // Witch): its read (bool reader 0x64a6f0, `ret 8`) -> settings +0xd0 = TRUE
        let mut secret = [0x90; 21];
        secret[..7].copy_from_slice(&[0xc6, 0x86, 0xd0, 0, 0, 0, 1]);
        patch(
            "secret character unlocked (useSecretChara)",
            0x4059b7,
            &[0x68, 0x24, 0xb0, 0x69, 0x00, 0x8d, 0x8e, 0xd0, 0, 0, 0, 0x51, 0x8d, 0x4c, 0x24, 0x14, 0xe8, 0x24, 0x4d, 0x24, 0x00],
            &secret,
        );
        if std::env::var("WAL_KONAMI_UNLIMITED_SUBWEAPON").is_ok_and(|v| v == "1") {
            // the throw's hearts check (`jl`, not enough hearts) and their payment (`sub`)
            patch("sub-weapons without hearts: check", 0x444a9c, &[0x7c, 0x04], &[0x90, 0x90]);
            patch("sub-weapons without hearts: cost", 0x444eb4, &[0x29, 0x90, 0x98, 0, 0, 0], &[0x90; 6]);
        }
        match iat::hook("kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) {
            Some(orig) => ORIG_GET_PROC.store(orig, Ordering::Relaxed),
            None => log!("hcv: GetProcAddress not imported, hcvACLib not hooked"),
        }
    }
    log!("hcv: cabinet I/O and whips from the virtual sticks (hcvACLib)");
}
