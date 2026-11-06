//! Big Foot Crush (UNIS's monster truck cabinet, Unity 2022, Mono, 32-bit):
//! `WAL_UNIS_BOARD=bigfootcrush`.
//!
//! An Android cabinet's game in a Windows build: its board bridge (`Framework_Call_JNI`) is empty
//! here, nothing ever comes from the MCU. The Windows build drives with Unity's `Input` keys
//! (`CarAll`: `W` the gas, `A` / `D` the wheel) and has the framework's keyboard test keys
//! (`Framework_Keyboard_Test`: `1` a coin, `M` test, the left / right arrows the menu's values):
//! the payload posts them from player 1's stick (`input_filter`: the real ones never reach it),
//! each as the virtual key at its place in the host's layout ([`layout_key`]).
//!
//! - **The operator menu.** The test keys are queued for the link between a cabinet's two PCs
//!   (`YuSocketUDPClient.MCUSendBtnMsg`), sent when the other MCU is ready (`GetFromOther2` = 1)
//!   and repeated until it echoes their number (`GetFromOther4` = `SendToOther4`): `GetData` and
//!   `SetData` of `Framework_UART_Manager` are replaced to answer as that link, its polling of
//!   the MCU ended (the board's IO ERROR).
//! - **Leaving the menu.** The game's scene loaded again, as in the editor ([`menu_exit`]): the
//!   Android player restarted there.
//! - **The arrows.** Out of the menu the left arrow is the operator's free coin
//!   (`Framework_Main`): the wheel is `A` / `D` there, the arrows only in the menu
//!   (`GlobalSetting.isInMenu`).
//! - **The settings.** The dump lacks the APK's `StreamingAssets/Setting`, read from
//!   `C:\mnt\sdcard\UNIS_GameData\Setting`: the profile's files. The common free play is set in
//!   the saved settings before the game reads them.
//!
//! Natives: `coin test service gas left right`.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicU64, Ordering};
use std::time::Duration;

use wal_payload_common::mapping::ButtonMap;
use wal_payload_common::mono::mono;
use wal_payload_common::{gun, input, input_filter, log};
use wal_protocol::Axis;

const VK_LEFT: u16 = 0x25;
const VK_RIGHT: u16 = 0x27;
const VK_1: u16 = b'1' as u16;
const VK_A: u16 = b'A' as u16;
const VK_D: u16 = b'D' as u16;
const VK_M: u16 = b'M' as u16;
const VK_W: u16 = b'W' as u16;

/// The keys the payload posts.
const KEYS: [u16; 7] = [VK_LEFT, VK_RIGHT, VK_1, VK_A, VK_D, VK_M, VK_W];

/// The saved settings the game reads.
const SETTINGS: &str = r"C:\mnt\sdcard\UNIS_GameData\Setting\UNIS_htGameSetting.ini";
const FREE_PLAY: &str = "Root.GameMode.Free Play";

#[derive(Clone, Copy, PartialEq)]
enum Native {
    Coin,
    Test,
    Service,
    Gas,
    Left,
    Right,
}

const NATIVES: &[(&str, Native)] = &[
    ("coin", Native::Coin),
    ("test", Native::Test),
    ("service", Native::Service),
    ("gas", Native::Gas),
    ("left", Native::Left),
    ("right", Native::Right),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("coin", "coin"),
    ("start", "gas"),
    ("b1", "gas"),
    ("left", "left"),
    ("right", "right"),
    // the operator menu: test opens it and moves down, service selects (the right arrow)
    ("test", "test"),
    ("service", "service"),
];

fn map() -> &'static ButtonMap<Native> {
    static MAP: OnceLock<ButtonMap<Native>> = OnceLock::new();
    MAP.get_or_init(|| ButtonMap::new(NATIVES, DEFAULTS))
}

/// `GlobalSetting.isInMenu`, once found.
static IN_MENU: AtomicPtr<i32> = AtomicPtr::new(std::ptr::null_mut());

fn in_menu() -> bool {
    let at = IN_MENU.load(Ordering::Relaxed);
    !at.is_null() && unsafe { *at } > 0
}

/// Whether a key is down for this state of player 1's stick.
fn wanted(vk: u16, held: impl Fn(Native) -> bool, x: i32, gas: i32, menu: bool) -> bool {
    let left = held(Native::Left) || x < -11000;
    let right = held(Native::Right) || x > 11000;
    match vk {
        VK_1 => held(Native::Coin),
        VK_M => held(Native::Test),
        VK_W => held(Native::Gas) || gas > 8000,
        VK_A => left && !menu,
        VK_D => right && !menu,
        VK_LEFT => left && menu,
        VK_RIGHT => (right && menu) || held(Native::Service),
        _ => false,
    }
}

/// The virtual key to post for a key of the game. Unity 2022's `KeyCode` is the place on a US
/// keyboard of the key a virtual key has in the host's layout (on AZERTY `VK_W` reads as `Z`): the
/// virtual key at that key's place.
fn layout_key(vk: u16) -> u16 {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MAPVK_VSC_TO_VK, MapVirtualKeyW};
    let scan = match vk {
        VK_1 => 0x02,
        VK_A => 0x1E,
        VK_D => 0x20,
        VK_M => 0x32,
        VK_W => 0x11,
        _ => return vk,
    };
    match unsafe { MapVirtualKeyW(scan, MAPVK_VSC_TO_VK) } as u16 {
        0 => vk,
        mapped => mapped,
    }
}

/// The virtual keys held (a bit each, 256 keys).
static HELD: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn bit(vk: u16) -> (usize, u64) {
    (vk as usize / 64 % 4, 1 << (vk % 64))
}

/// `input_filter`'s key state: the keys posted held.
pub fn key_held(vk: i32) -> bool {
    let (word, bit) = bit((vk & 0xFF) as u16);
    HELD[word].load(Ordering::Relaxed) & bit != 0
}

/// Posts the keys' changes to the game's window every 10 ms.
fn post_keys() {
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(Duration::from_millis(10));
            let window = gun::find_window();
            if window.is_null() {
                continue;
            }
            let stick = input(0);
            let mut pressed = [false; NATIVES.len()];
            map().for_each_pressed(&stick, |n| pressed[NATIVES.iter().position(|(_, m)| *m == n).unwrap_or(0)] = true);
            let held = |n: Native| NATIVES.iter().position(|(_, m)| *m == n).is_some_and(|i| pressed[i]);
            let menu = in_menu();
            let (x, gas) = (stick.axis(Axis::LeftX) as i32, stick.axis(Axis::Accel) as i32);
            for vk in KEYS {
                let is = wanted(vk, held, x, gas, menu);
                let vk = layout_key(vk);
                let was = key_held(vk as i32);
                if was == is {
                    continue;
                }
                // held first, so the game's key state polling agrees with the message
                let (word, bit) = bit(vk);
                if is {
                    HELD[word].fetch_or(bit, Ordering::Relaxed);
                }
                input_filter::post_key(window, vk, is);
                if !is {
                    HELD[word].fetch_and(!bit, Ordering::Relaxed);
                }
            }
        }
    });
}

/// Whether a managed string is `text` (ASCII).
unsafe fn string_is(string: *const u8, text: &str) -> bool {
    if string.is_null() {
        return false;
    }
    // MonoString: the object's header (vtable, synchronisation), length, UTF-16 characters
    let header = 2 * size_of::<usize>();
    let (len, chars) = unsafe { (*(string.add(header) as *const i32), string.add(header + 4) as *const u16) };
    len as usize == text.len() && text.bytes().enumerate().all(|(i, b)| unsafe { chars.add(i).read_unaligned() } == b as u16)
}

/// The number of the last key message sent over the link.
static SENT: AtomicI32 = AtomicI32::new(0);

/// `Framework_UART_Manager.GetData(name, board)`: nothing from the MCU (-1, as without a board)
/// but the link's state.
unsafe extern "C" fn get_data(_this: *const u8, name: *const u8, _board: i32) -> i32 {
    unsafe {
        if string_is(name, "GetFromOther2") {
            1
        } else if string_is(name, "GetFromOther4") {
            SENT.load(Ordering::Relaxed)
        } else {
            -1
        }
    }
}

/// `Framework_UART_Manager.SetData(name, value, board)`: the link's message number kept.
unsafe extern "C" fn set_data(_this: *const u8, name: *const u8, value: i32, _board: i32) {
    if unsafe { string_is(name, "SendToOther4") } {
        SENT.store(value, Ordering::Relaxed);
    }
}

/// The MCU's polling coroutine (`Framework_UART_Manager.IEUART_USB`) ended: no MCU answers here,
/// it would raise the board's error (the operator menu's IO ERROR).
unsafe extern "C" fn no_polling(_this: *const u8) -> i32 {
    0
}

/// `CallBackState.OnMenuEvent`'s test of `Application.platform` against `WindowsEditor`: the
/// value it compares with, in its IL.
const EDITOR_TEST: usize = 0x2E;
const LDC_I4_7: u8 = 0x1D;
/// `ldc.i4.2`: `WindowsPlayer`.
const LDC_I4_2: u8 = 0x18;

/// Leaving the operator menu: an Android player restarts (`ResetPlayer`, empty here: the menu
/// stayed on the screen), the editor loads the game's scene again: the Windows player does too.
/// Patched before the method's first call (the menu's).
fn menu_exit(m: &wal_payload_common::mono::Mono, image: wal_payload_common::mono::P) {
    let back = m.class(image, "", "CallBackState");
    match m.il_code(m.method(back, "OnMenuEvent", 2)).and_then(|il| il.get_mut(EDITOR_TEST)) {
        Some(op) if *op == LDC_I4_7 => {
            *op = LDC_I4_2;
            log!("bigfootcrush: the operator menu's exit loads the game's scene");
        }
        _ => log!("bigfootcrush: the operator menu's exit not found"),
    }
}

/// Replaced once the game's domain runs (the board manager created): the methods compiled before
/// are the root domain's.
fn answer_link() {
    for _ in 0..1200 {
        std::thread::sleep(Duration::from_millis(100));
        let Some(m) = mono() else { continue };
        if !m.attach() {
            continue;
        }
        let image = m.image("Assembly-CSharp");
        let manager = m.class(image, "", "Framework_UART_Manager");
        if manager.is_null() {
            continue;
        }
        let instance = m.static_addr(manager, m.field(manager, "instance"));
        if instance.is_null() || unsafe { *(instance as *const usize) } == 0 {
            continue;
        }
        let settings = m.class(image, "", "GlobalSetting");
        IN_MENU.store(m.static_addr(settings, m.field(settings, "isInMenu")).cast(), Ordering::Relaxed);
        let replaced = m.replace(m.method(manager, "GetData", 2), get_data as *const () as usize)
            && m.replace(m.method(manager, "SetData", 3), set_data as *const () as usize);
        let polling = m.class(image, "", "Framework_UART_Manager/<IEUART_USB>d__55");
        let replaced = replaced && m.replace(m.method(polling, "MoveNext", 0), no_polling as *const () as usize);
        log!("bigfootcrush: the cabinets' link answered ({replaced})");
        menu_exit(m, image);
        return;
    }
    log!("bigfootcrush: the board manager not found");
}

/// The saved settings with the free play line set.
fn with_free_play(data: &str, on: bool) -> String {
    let line = format!("{FREE_PLAY}={}", on as u8);
    let mut found = false;
    let mut lines: Vec<String> = data
        .lines()
        .map(|l| {
            if l.starts_with(FREE_PLAY) {
                found = true;
                line.clone()
            } else {
                l.to_string()
            }
        })
        .collect();
    if lines.is_empty() {
        lines.push("[MenuSetting]".to_string());
    }
    if !found {
        lines.push(line);
    }
    lines.join("\r\n") + "\r\n"
}

fn free_play() {
    let Some(on) = wal_payload_common::free_play("WAL_UNIS_FREEPLAY") else { return };
    let data = std::fs::read_to_string(SETTINGS).unwrap_or_default();
    match std::fs::write(SETTINGS, with_free_play(&data, on)) {
        Ok(()) => log!("bigfootcrush: free play {}", if on { "on" } else { "off" }),
        Err(e) => log!("bigfootcrush: {SETTINGS}: {e}"),
    }
}

pub fn init() {
    free_play();
    post_keys();
    std::thread::spawn(answer_link);
    log!("bigfootcrush: keys from player 1's stick");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_play_line() {
        assert_eq!(with_free_play("", true), "[MenuSetting]\r\nRoot.GameMode.Free Play=1\r\n");
        assert_eq!(with_free_play("[MenuSetting]\r\nRoot.GameMode.Free Play=1\r\nRoot.Language=0\r\n", false), "[MenuSetting]\r\nRoot.GameMode.Free Play=0\r\nRoot.Language=0\r\n");
    }

    #[test]
    fn wheel_keys_follow_the_menu() {
        let none = |_: Native| false;
        assert!(wanted(VK_A, none, -20000, 0, false));
        assert!(!wanted(VK_LEFT, none, -20000, 0, false));
        assert!(wanted(VK_LEFT, none, -20000, 0, true));
        assert!(!wanted(VK_A, none, -20000, 0, true));
        assert!(wanted(VK_W, none, 0, 20000, false));
        assert!(wanted(VK_RIGHT, |n| n == Native::Service, 0, 0, false));
    }
}
