//! The Sixense STEM trackers of Rabbids Hollywood: `sxCore.dll`, the STEM base's driver,
//! replaced. Each player's gun is a STEM pack (an electromagnetic tracker: position and
//! rotation) the base tracks.
//!
//! - **Devices.** Device 0 is the base (`device_type` 14), devices 1-4 the guns (`device_type`
//!   4, a STEM pack; `device_type_extended` = the gun's number). `GunDetection` waits for the
//!   enabled guns plus the base (`sxCoreGetNumConnectedTrackedDevices`), flashes any device
//!   whose firmware is not the expected one (base MCU 01.10, packs MCU CA.18 and DSP DA.17:
//!   reported, no flashing), waits for each gun to be auto-synced (`tracker_id` 7) and maps
//!   `device_type_extended` to the players.
//! - **Data.** `sxCoreGetData` (every frame) and `sxCoreGetDataPrevious` (history 1): status
//!   (enabled, wired), buttons (`START`, bit 0: the trigger), position in millimetres (Unity
//!   flips z), rotation quaternion (Unity flips z and w).
//! - **Aim.** `InputCollisionMesh.GetViewportPosition` casts a ray from the gun (plus the
//!   receiver's place: `m_ReceiverOffsetY - m_SizeY / 2` below the screen's centre,
//!   `m_ReceiverOffsetZ` before it) along its forward axis onto the screen plane, `m_SizeX` by
//!   `m_SizeY` metres around its centre (`ControllerBaseConfig.json`). Each gun sits at the
//!   receiver (position 0: the scene's tracker scale does not matter, and the game never sees
//!   it docked: no pickup sound) and turns towards where the player's gun aims.
//! - **Gun mode.** `NewInputManager` picks its input at its first frame: the guns if
//!   `GunDetection`'s "GunReady" came first, else the mouse for good; the dump's assembly
//!   starts it with "gun not found" set (mouse at once, the guns never read). A second after
//!   `GunDetection` has set the guns up (`sxCoreSoftReset`), the payload clears it, sets "gun
//!   ready" and calls its `ChooseInput` (Mono's embedding API, from `sxCoreGetData`: the main
//!   thread), on any new instance still on the mouse.
//!
use std::ffi::c_char;
use std::sync::OnceLock;
use std::time::Instant;

use std::sync::Mutex;

use wal_payload_common::json::Json;
use wal_payload_common::mono::{self, mono};
use wal_payload_common::{gun, log};

use crate::{Native, PLAYERS, held};

const SUCCESS: i32 = 0;
const FAILURE: i32 = -1;

/// The base and the four guns.
const DEVICES: u32 = 1 + PLAYERS as u32;
const STEM_PACK: u8 = 4;
const STEM_BASE: u8 = 14;
const AUTO_SYNCED: u16 = 7;
const STATUS_ENABLED: u32 = 0x1;
const STATUS_EXTERNAL_POWER: u32 = 0x4;
const STATUS_WIRED: u32 = 0x20;
const BUTTON_START: u32 = 0x1;

#[repr(C)]
#[derive(Clone, Copy)]
struct TrackedDeviceData {
    sequence_number: u8,
    device_type: u8,
    dsp_status: u8,
    battery_percent: u8,
    tracker_id: u16,
    tracked_device_index: u16,
    status: u32,
    hardware_status: u32,
    buttons: u32,
    trigger: f32,
    joystick_x: f32,
    joystick_y: f32,
    pos: [f32; 3],
    rot_quat: [f32; 4],
    rot_mat: [f32; 9],
    rot_euler: [f32; 3],
    imu_acc: [i16; 3],
    imu_gyro: [i16; 3],
    imu_mag: [i16; 3],
    imu_sequence_number: u8,
    imu_gravity: [f32; 3],
    packet_time: f64,
    imu_time: f64,
    mcu_timestamp: u16,
    dsp_timestamp: u16,
}

#[repr(C)]
struct TrackedDeviceInfo {
    runtime_version_major: u8,
    runtime_version_minor: u8,
    hardware_revision: u8,
    device_type_board: u8,
    magnetic_frequency: u16,
    tracked_device_index: i16,
    device_type: u8,
    device_type_extended: u8,
    serial_number: u16,
    mcu_firmware_version_major: u8,
    mcu_firmware_version_minor: u8,
    dsp_firmware_version_major: u8,
    dsp_firmware_version_minor: u8,
    board_hardware_gpio: u8,
    battery_adc_level: u8,
    battery_voltage: f32,
    imu_calibration_status: u8,
    dsp_gain_mode: u8,
    battery_state_of_charge: u8,
    debug_info: u8,
    mcu_build: u8,
    product_string: [u8; 256],
    serial_number_string: [u8; 256],
    port_string: [u8; 256],
    coil_rotation_quat: [f32; 4],
    coil_offset_pos: [f32; 3],
}

#[repr(C)]
struct AutoSyncParameters {
    stable_imu_frames_required: u32,
    stable_sync_frames_required: u32,
    gyro_threshold: f32,
    imu_angle_tolerance: f32,
    imu_max_accel: f32,
}

/// The screen plane and the receiver (`ControllerBaseConfig.json`, the game's defaults).
struct Screen {
    size_x: f32,
    size_y: f32,
    receiver_y: f32,
    receiver_z: f32,
}

fn screen() -> &'static Screen {
    static SCREEN: OnceLock<Screen> = OnceLock::new();
    SCREEN.get_or_init(|| {
        let config = std::fs::read_to_string("ControllerBaseConfig.json").ok().and_then(|t| Json::parse(&t));
        let get = |key: &str, default: f32| {
            config.as_ref().and_then(|c| c.get(key)).and_then(Json::as_f64).map_or(default, |v| v as f32)
        };
        let screen = Screen { size_x: get("m_SizeX", 1.45 * 16.0 / 9.0), size_y: get("m_SizeY", 1.45), receiver_y: get("m_ReceiverOffsetY", 0.07), receiver_z: get("m_ReceiverOffsetZ", -0.825) };
        log!("sixense: screen {} x {} m, receiver at {} m, {} m", screen.size_x, screen.size_y, screen.receiver_y, screen.receiver_z);
        screen
    })
}

fn start_time() -> &'static Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now)
}

/// Each gun's last aim (viewport, v up).
static AIM: Mutex<[(f32, f32); PLAYERS]> = Mutex::new([(0.5, 0.5); PLAYERS]);

/// When `GunDetection` set the guns up (`sxCoreSoftReset`), and when the input mode was last
/// checked.
static SET_UP: Mutex<Option<Instant>> = Mutex::new(None);
static CHECKED: Mutex<Option<Instant>> = Mutex::new(None);

const INPUT_MODE_MOUSE: i32 = 0;

/// `NewInputManager` on the guns, if it chose the mouse (the main thread).
fn gun_mode() {
    let Some(set_up) = *SET_UP.lock().unwrap() else { return };
    if set_up.elapsed().as_secs_f32() < 1.0 {
        return;
    }
    {
        let mut checked = CHECKED.lock().unwrap();
        if checked.is_some_and(|t| t.elapsed().as_millis() < 500) {
            return;
        }
        *checked = Some(Instant::now());
    }
    let Some(m) = mono() else { return };
    let class = m.class(m.image("Assembly-CSharp"), "", "NewInputManager");
    let (mode, instance) = (m.static_addr(class, m.field(class, "m_InputMode")), m.static_addr(class, m.field(class, "_activeInstance")));
    if mode.is_null() || instance.is_null() {
        return;
    }
    let obj = unsafe { mono::read::<usize>(instance, 0) } as *mut u8;
    if obj.is_null() || unsafe { mono::read::<i32>(mode, 0) } != INPUT_MODE_MOUSE {
        return;
    }
    let (not_found, ready, choose) = (m.field(class, "m_GunNotFound"), m.field(class, "m_GunReady"), m.method(class, "ChooseInput", 0));
    if not_found.is_null() || ready.is_null() || choose.is_null() {
        log!("sixense: NewInputManager fields not found");
        return;
    }
    unsafe {
        mono::write::<u8>(obj, m.offset(not_found), 0);
        mono::write::<u8>(obj, m.offset(ready), 1);
    }
    let gun = m.invoke(choose, obj.cast(), &mut []).is_some();
    log!("sixense: NewInputManager on the guns ({})", if gun { "ok" } else { "ChooseInput threw" });
}

/// The tracker's rotation quaternion turning the forward axis (Unity's +z) from the receiver
/// towards the viewport point (u, v).
fn rotation(u: f32, v: f32) -> [f32; 4] {
    let s = screen();
    // receiver to the point on the screen plane
    let d = [(u - 0.5) * s.size_x, (v - 0.5) * s.size_y - (s.receiver_y - s.size_y * 0.5), -s.receiver_z];
    let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let d = [d[0] / n, d[1] / n, d[2] / n];
    // about forward x d (-dy, dx, 0) by the angle between them
    let axis = (d[0] * d[0] + d[1] * d[1]).sqrt();
    let half = d[2].clamp(-1.0, 1.0).acos() * 0.5;
    let (x, y) = if axis > 1e-6 { (-d[1] / axis * half.sin(), d[0] / axis * half.sin()) } else { (0.0, 0.0) };
    // Unity flips z and w
    [x, y, 0.0, -half.cos()]
}

/// Device `index`'s current data.
fn data(index: u32) -> TrackedDeviceData {
    let t = start_time().elapsed();
    let mut d = TrackedDeviceData {
        // a new sample every 1/240 s
        sequence_number: (t.as_micros() / 4167) as u8,
        device_type: STEM_BASE,
        dsp_status: 0,
        battery_percent: 100,
        tracker_id: AUTO_SYNCED,
        tracked_device_index: index as u16,
        status: STATUS_ENABLED | STATUS_EXTERNAL_POWER | STATUS_WIRED,
        hardware_status: 0,
        buttons: 0,
        trigger: 0.0,
        joystick_x: 0.0,
        joystick_y: 0.0,
        pos: [0.0; 3],
        rot_quat: [0.0, 0.0, 0.0, 1.0],
        rot_mat: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        rot_euler: [0.0; 3],
        imu_acc: [0; 3],
        imu_gyro: [0; 3],
        imu_mag: [0; 3],
        imu_sequence_number: 0,
        imu_gravity: [0.0, -9.81, 0.0],
        packet_time: t.as_secs_f64(),
        imu_time: t.as_secs_f64(),
        mcu_timestamp: 0,
        dsp_timestamp: 0,
    };
    if index == 0 || index >= DEVICES {
        return d;
    }
    let player = index as usize - 1;
    if player == 0 {
        gun_mode();
    }
    d.device_type = STEM_PACK;
    if held(player, Native::Trigger) {
        d.buttons = BUTTON_START;
        d.trigger = 1.0;
    }
    let (u, v) = {
        let mut aim = AIM.lock().unwrap();
        // off the picture: the last aim
        if let Some((x, y)) = gun::gun_of_player_in_picture(player) {
            aim[player] = (x, 1.0 - y);
        }
        aim[player]
    };
    d.rot_quat = rotation(u, v);
    d
}

unsafe fn write<T>(p: *mut T, v: T) {
    if !p.is_null() {
        unsafe { p.write(v) };
    }
}

/// The functions without results: success.
extern "C" fn ok() -> i32 {
    SUCCESS
}

extern "C" fn get_api_version() -> *const c_char {
    c"2.0.0".as_ptr()
}

extern "C" fn init() -> i32 {
    start_time();
    screen();
    log!("sixense: sxCoreInit: the base and {PLAYERS} guns");
    SUCCESS
}

extern "C" fn soft_reset(_index: u32) -> i32 {
    *SET_UP.lock().unwrap() = Some(Instant::now());
    SUCCESS
}

extern "C" fn one() -> u32 {
    1
}

extern "C" fn zero() -> u32 {
    0
}

extern "C" fn devices() -> u32 {
    DEVICES
}

/// `sxCoreGetSystemType`: STEM.
extern "C" fn system_type(_index: u32) -> u32 {
    0x10
}

extern "C" fn base_status(_index: u32) -> u32 {
    1
}

extern "C" fn current_time(time: *mut u64) -> i32 {
    unsafe { write(time, start_time().elapsed().as_micros() as u64) };
    SUCCESS
}

/// `sxCoreGetFilterEnabled(type, out on)`, `sxCoreGetNetworkBridgeEnabled(out on)`-like: off.
extern "C" fn get_filter_enabled(_filter: u32, on: *mut i32) -> i32 {
    unsafe { write(on, 0) };
    SUCCESS
}

extern "C" fn get_out_zero(out: *mut u32) -> i32 {
    unsafe { write(out, 0) };
    SUCCESS
}

extern "C" fn request_auto_sync_enabled(on: *mut i32) -> i32 {
    unsafe { write(on, 1) };
    SUCCESS
}

extern "C" fn get_hemisphere_vector(_id: u32, x: *mut f32, y: *mut f32, z: *mut f32) -> i32 {
    unsafe {
        write(x, 1.0);
        write(y, 0.0);
        write(z, 0.0);
    }
    SUCCESS
}

extern "C" fn is_device_enabled(index: u32) -> i32 {
    (index < DEVICES) as i32
}

extern "C" fn get_info(index: u32, info: *mut TrackedDeviceInfo) -> i32 {
    if index >= DEVICES || info.is_null() {
        return FAILURE;
    }
    let gun = index != 0;
    let mut i = TrackedDeviceInfo {
        runtime_version_major: 2,
        runtime_version_minor: 0,
        hardware_revision: 1,
        device_type_board: 0,
        magnetic_frequency: 0,
        tracked_device_index: index as i16,
        device_type: if gun { STEM_PACK } else { STEM_BASE },
        device_type_extended: index as u8,
        serial_number: 0x5741 + index as u16,
        // the firmware the game flashes: base 01.10, packs CA.18-DA.17
        mcu_firmware_version_major: if gun { 0xCA } else { 0x01 },
        mcu_firmware_version_minor: if gun { 0x18 } else { 0x10 },
        dsp_firmware_version_major: if gun { 0xDA } else { 0x00 },
        dsp_firmware_version_minor: if gun { 0x17 } else { 0x00 },
        board_hardware_gpio: 0,
        battery_adc_level: 0,
        battery_voltage: 4.2,
        imu_calibration_status: 0,
        dsp_gain_mode: 0,
        battery_state_of_charge: 100,
        debug_info: 0,
        mcu_build: 0,
        product_string: [0; 256],
        serial_number_string: [0; 256],
        port_string: [0; 256],
        coil_rotation_quat: [0.0, 0.0, 0.0, 1.0],
        coil_offset_pos: [0.0; 3],
    };
    let name: &[u8] = if gun { b"STEM Pack" } else { b"STEM Base" };
    i.product_string[..name.len()].copy_from_slice(name);
    let port = format!("COM{}", 20 + index);
    i.port_string[..port.len()].copy_from_slice(port.as_bytes());
    let serial = format!("WAL{index:05}");
    i.serial_number_string[..serial.len()].copy_from_slice(serial.as_bytes());
    unsafe { info.write(i) };
    SUCCESS
}

extern "C" fn get_data(index: u32, out: *mut TrackedDeviceData) -> i32 {
    if index >= DEVICES || out.is_null() {
        return FAILURE;
    }
    unsafe { out.write(data(index)) };
    SUCCESS
}

/// `sxCoreGetDataPrevious(index, back, out data)`: a history of one sample.
extern "C" fn get_data_previous(index: u32, back: u32, out: *mut TrackedDeviceData) -> i32 {
    if back != 0 {
        return FAILURE;
    }
    get_data(index, out)
}

extern "C" fn get_data_at_time(index: u32, _time: u64, out: *mut TrackedDeviceData) -> i32 {
    get_data(index, out)
}

extern "C" fn get_vibration(_index: u32, _motor: u32, magnitude: *mut f32) -> i32 {
    unsafe { write(magnitude, 0.0) };
    SUCCESS
}

extern "C" fn get_device_filter_enabled(_filter: u32, _index: u32, on: *mut i32) -> i32 {
    unsafe { write(on, 0) };
    SUCCESS
}

extern "C" fn last_packet_delta(_index: u32, time: *mut f32) -> i32 {
    unsafe { write(time, 1.0 / 240.0) };
    SUCCESS
}

extern "C" fn get_auto_sync_parameters(_index: u32, out: *mut AutoSyncParameters) -> i32 {
    unsafe {
        write(out, AutoSyncParameters { stable_imu_frames_required: 100, stable_sync_frames_required: 200, gyro_threshold: 0.0, imu_angle_tolerance: 0.0, imu_max_accel: 0.0 })
    };
    SUCCESS
}

/// `sxCoreGetDeviceAutoSyncStatus(index, out status)`: synced and stable.
extern "C" fn get_auto_sync_status(index: u32, out: *mut [i32; 5]) -> i32 {
    unsafe { write(out, [index as i32, 1, 1, 1, 0]) };
    SUCCESS
}

/// The driver's functions.
pub fn resolve(name: &str) -> Option<usize> {
    let f = match name {
        "sxCoreGetAPIVersion" => get_api_version as *const () as usize,
        "sxCoreInit" => init as *const () as usize,
        "sxCoreSoftReset" => soft_reset as *const () as usize,
        "sxCoreGetReferenceCount" | "sxCoreGetMaxSystems" | "sxCoreGetNumSystems" | "sxCoreGetMaxBases" | "sxCoreGetNumBases" | "sxCoreGetHistorySize" => one as *const () as usize,
        "sxCoreGetActiveSystemIndex" | "sxCoreGetActiveBaseIndex" => zero as *const () as usize,
        "sxCoreGetMaxTrackedDevices" | "sxCoreGetNumEnabledTrackedDevices" | "sxCoreGetNumConnectedTrackedDevices" => devices as *const () as usize,
        "sxCoreGetSystemType" => system_type as *const () as usize,
        "sxCoreGetBaseStatus" => base_status as *const () as usize,
        "sxCoreGetCurrentTime" => current_time as *const () as usize,
        "sxCoreGetFilterEnabled" => get_filter_enabled as *const () as usize,
        "sxCoreGetNetworkBridgeEnabled" | "sxCoreGetNetworkBridgeAddressIndexMaxAddresses" => get_out_zero as *const () as usize,
        "sxCoreRequestAutoSyncEnabled" => request_auto_sync_enabled as *const () as usize,
        "sxCoreGetHemisphereTrackingInitVector" => get_hemisphere_vector as *const () as usize,
        "sxCoreIsTrackedDeviceEnabled" | "sxCoreIsTrackedDeviceConnected" => is_device_enabled as *const () as usize,
        "sxCoreGetInfo" => get_info as *const () as usize,
        "sxCoreGetData" => get_data as *const () as usize,
        "sxCoreGetDataPrevious" => get_data_previous as *const () as usize,
        "sxCoreGetDataAtTime" => get_data_at_time as *const () as usize,
        "sxCoreGetVibration" => get_vibration as *const () as usize,
        "sxCoreGetDeviceFilterEnabled" => get_device_filter_enabled as *const () as usize,
        "sxCoreGetLastPacketReadDeltaTime" => last_packet_delta as *const () as usize,
        "sxCoreGetDeviceAutoSyncParameters" => get_auto_sync_parameters as *const () as usize,
        "sxCoreGetDeviceAutoSyncStatus" => get_auto_sync_status as *const () as usize,
        // the filter parameters (out structures left as they are), the settings and commands
        n if n.starts_with("sxCore") => ok as *const () as usize,
        _ => return None,
    };
    Some(f)
}
