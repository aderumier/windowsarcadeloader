//! Groove Coaster 2: Heavenly Festival (Taito, NESiCAxLive, Type X3, 2015: `game.exe`, 32-bit,
//! Direct3D 9, portrait 720x1280), `WAL_NESICA_GAME=groovecoaster`.
//!
//! * The cabinets' LAN link (UDP 60100) names each cabinet by the last two bytes of the first
//!   adapter's address (`GetAdaptersInfo`, the address as hex text): the boot waits until that
//!   number is not 0. Under Wine the first adapter may be one without an address (a VPN or a
//!   bridge, MAC 00:00:00:00:00:00): the game sees one adapter, the first with an IP address,
//!   with the NESYS service's MAC (`DE:AD:BA:BE:CA:FE`).
//! * The two boosters (a stick with a button on top each, one player) are FastIO switches
//!   (0x4120, the game's FastIO board firmware 0x823C), as the test menu shows them: left booster
//!   up / down / left / right = player 1 up, player 2 up, player 1 down, player 2 down, its button
//!   player 1 button 1; right booster up / down / left / right = player 1 left, player 2 left,
//!   player 1 right, player 2 right, its button player 2 button 1; Select = player 1 start, Enter
//!   = player 2 start. All from player 1's virtual stick ([`switches`]): left booster = d-pad or
//!   left stick, b1; right booster = right stick or b5-b8 (up, down, left, right), b2; Enter =
//!   start, Select = b3.
//! * The common free play option: the game is in free play when the system settings' coins per
//!   credit are 0 (the test menu's FREE PLAY), asked of one function (+0x203450, its 4 callers:
//!   the credit display and coin handling, the start screens): it answers yes. Off: the game's
//!   own setting.
//! * NESiCAxLive's update downloader (`TtxUpdateDownloader.dll`, started by the game's NESYS
//!   thread once certified): its `TtxUDLInit` waits forever on a lock of the DLL under Wine (the
//!   NESYS thread stopped there, the boot never left the NOTICE screen). No update is delivered
//!   here: its functions are answered by the payload (started, status idle).

use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use wal_protocol::{Axis, button};
use windows_sys::Win32::NetworkManagement::IpHelper::IP_ADAPTER_INFO;

const MAC: [u8; 6] = [0xDE, 0xAD, 0xBA, 0xBE, 0xCA, 0xFE];
const ERROR_BUFFER_OVERFLOW: u32 = 111;

pub(crate) fn detected() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAL_NESICA_GAME").is_ok_and(|v| v.trim().eq_ignore_ascii_case("groovecoaster")))
}

/// FastIO 0x4120 for the cabinet's controls (see the module documentation): player 1's
/// boosters, test and service from any player.
pub(crate) fn switches() -> u32 {
    const HALF: i16 = 16384;
    let p = wal_payload_common::input(0);
    let (lx, ly, rx, ry) = (p.axis(Axis::LeftX), p.axis(Axis::LeftY), p.axis(Axis::RightX), p.axis(Axis::RightY));
    let held = |b: u32| p.pressed(b);
    let bits = [
        (held(button::UP) || ly < -HALF, 0x0100),
        (held(button::DOWN) || ly > HALF, 0x0200),
        (held(button::LEFT) || lx < -HALF, 0x0400),
        (held(button::RIGHT) || lx > HALF, 0x0800),
        (held(button::B5) || ry < -HALF, 0x1000),
        (held(button::B6) || ry > HALF, 0x2000),
        (held(button::B7) || rx < -HALF, 0x4000),
        (held(button::B8) || rx > HALF, 0x8000),
        (held(button::B1), 0x1_0000),
        (held(button::B2), 0x2_0000),
        (held(button::B3), 0x10),
        (held(button::START), 0x20),
    ];
    let mut v = bits.iter().filter(|(on, _)| *on).fold(0, |v, (_, bit)| v | bit);
    for player in 0..wal_protocol::MAX_PLAYERS {
        let s = wal_payload_common::input(player);
        if s.pressed(button::TEST) {
            v |= 0x40;
        }
        if s.pressed(button::SERVICE) {
            v |= 0x04;
        }
    }
    v
}

static ORIG_GET_ADAPTERS_INFO: AtomicUsize = AtomicUsize::new(0);

type GetAdaptersInfo = unsafe extern "system" fn(*mut IP_ADAPTER_INFO, *mut u32) -> u32;

/// `GetAdaptersInfo`: one adapter, the first with an IP address, with a fixed MAC address.
unsafe extern "system" fn get_adapters_info(out: *mut IP_ADAPTER_INFO, size: *mut u32) -> u32 {
    let original: GetAdaptersInfo = unsafe { std::mem::transmute(ORIG_GET_ADAPTERS_INFO.load(Ordering::Relaxed)) };
    if size.is_null() {
        return unsafe { original(out, size) };
    }
    let one = size_of::<IP_ADAPTER_INFO>() as u32;
    if out.is_null() || unsafe { *size } < one {
        unsafe { *size = one };
        return ERROR_BUFFER_OVERFLOW;
    }
    let mut all_size = 0u32;
    unsafe { original(std::ptr::null_mut(), &mut all_size) };
    let count = (all_size as usize).div_ceil(one as usize).max(1);
    let mut all: Vec<IP_ADAPTER_INFO> = vec![unsafe { std::mem::zeroed() }; count];
    all_size = count as u32 * one;
    let ret = unsafe { original(all.as_mut_ptr(), &mut all_size) };
    let mut chosen: Option<IP_ADAPTER_INFO> = None;
    if ret == 0 {
        let mut at: *const IP_ADAPTER_INFO = all.as_ptr();
        while let Some(a) = unsafe { at.as_ref() } {
            let ip = &a.IpAddressList.IpAddress.String;
            let text: Vec<u8> = ip.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
            if !text.is_empty() && text != b"0.0.0.0" {
                chosen = Some(*a);
                break;
            }
            at = a.Next;
        }
    }
    let mut adapter = chosen.unwrap_or_else(|| unsafe { std::mem::zeroed() });
    adapter.Next = std::ptr::null_mut();
    adapter.IpAddressList.Next = std::ptr::null_mut();
    adapter.GatewayList.Next = std::ptr::null_mut();
    adapter.DhcpServer.Next = std::ptr::null_mut();
    adapter.PrimaryWinsServer.Next = std::ptr::null_mut();
    adapter.SecondaryWinsServer.Next = std::ptr::null_mut();
    adapter.CurrentIpAddress = std::ptr::null_mut();
    adapter.AddressLength = 6;
    adapter.Address = [0; 8];
    adapter.Address[..6].copy_from_slice(&MAC);
    unsafe { out.write(adapter) };
    static LOGGED: std::sync::Once = std::sync::Once::new();
    LOGGED.call_once(|| {
        let ip = &adapter.IpAddressList.IpAddress.String;
        let text: String = ip.iter().take_while(|&&c| c != 0).map(|&c| c as u8 as char).collect();
        log!("groovecoaster: adapter {text} given MAC DE:AD:BA:BE:CA:FE (real list: {ret})");
    });
    0
}

/// `TtxUDLInit(...)`: started.
unsafe extern "C" fn udl_init(_a: u32, _b: u32, _c: u32, _d: u32) -> i32 {
    log!("groovecoaster: update downloader not started (no update delivery)");
    1
}

/// `TtxUDLSetInterval`, `TtxUDLDelete`, `TtxUDLGetStatus` (the caller's zeroed status: idle).
unsafe extern "C" fn udl_nothing(_a: usize, _b: usize) {}

/// `TtxUDLSuspend` / `TtxUDLResume`: done.
unsafe extern "C" fn udl_zero() -> u32 {
    0
}

/// The common free play option (see the module documentation).
fn free_play() {
    if wal_payload_common::free_play("WAL_NESICA_FREEPLAY") != Some(true) {
        return;
    }
    // push ebp; mov ebp, esp; push ecx; mov [ebp-4], ecx
    const ORIGINAL: [u8; 7] = [0x55, 0x8b, 0xec, 0x51, 0x89, 0x4d, 0xfc];
    if unsafe { (0x603450 as *const [u8; 7]).read_unaligned() } != ORIGINAL {
        log!("groovecoaster: unknown executable at +0x203450: free play not set");
        return;
    }
    // mov al, 1; ret
    wal_payload_common::patches::apply_list("203450:B001C3");
    log!("groovecoaster: free play");
}

pub(crate) fn init() {
    free_play();
    let udl: [(&str, usize); 6] = [
        ("?TtxUDLInit@@YAHKKKK@Z", udl_init as *const () as usize),
        ("?TtxUDLSetInterval@@YAXKK@Z", udl_nothing as *const () as usize),
        ("?TtxUDLGetStatus@@YAXPAU_TTXUDL_STATUS@@@Z", udl_nothing as *const () as usize),
        ("?TtxUDLDelete@@YAXXZ", udl_nothing as *const () as usize),
        ("?TtxUDLSuspend@@YAKXZ", udl_zero as *const () as usize),
        ("?TtxUDLResume@@YAKXZ", udl_zero as *const () as usize),
    ];
    for (name, ours) in udl {
        if unsafe { iat::hook("TtxUpdateDownloader.dll", name, ours) }.is_none() {
            log!("groovecoaster: {name} not hooked");
        }
    }
    let ours = get_adapters_info as *const () as usize;
    match unsafe { iat::hook("iphlpapi.dll", "GetAdaptersInfo", ours) } {
        Some(o) if o != ours => ORIG_GET_ADAPTERS_INFO.store(o, Ordering::Relaxed),
        _ => log!("groovecoaster: GetAdaptersInfo not hooked"),
    }
    log!("groovecoaster: initialized");
}
