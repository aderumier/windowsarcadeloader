//! Shared RIO USB board for Cars, Batman and Injustice (VID 0x0c70, PID 0x0780).
//! Type 2 packets contain switch transition counters and four 12-bit analog channels.
//! Cars uses interrupt transfers; Batman uses bulk transfers with the same packet layout.

use crate::common::{FfbSender, set_motor};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::time::Duration;
use wal_payload_common::{log, mapping::ButtonMap};
use wal_protocol::{Axis, button};

const VID: u16 = 0x0c70;
const PID: u16 = 0x0780;
const PACKET: usize = 64;
/// Delay of a read (the board reports continuously).
const READ_PERIOD: Duration = Duration::from_millis(4);

/// (switch id, packet byte, high nibble) of each switch.
const SWITCH_NIBBLES: [(u8, usize, bool); 19] = [
    (0x00, 0, true),
    (0x01, 0, false),
    (0x02, 1, true),
    (0x12, 1, false),
    (0x04, 2, true),
    (0x03, 2, false),
    (0x05, 3, true),
    (0x06, 3, false),
    (0x07, 5, true),
    (0x08, 5, false),
    (0x09, 6, true),
    (0x0a, 6, false),
    (0x0b, 7, true),
    (0x0c, 7, false),
    (0x11, 8, true),
    (0x0d, 9, true),
    (0x0e, 9, false),
    (0x0f, 10, true),
    (0x10, 10, false),
];

static MAP: OnceLock<ButtonMap<u8>> = OnceLock::new();
/// Player 2's buttons (two-player cabinets), from the second virtual stick.
static MAP2: OnceLock<ButtonMap<u8>> = OnceLock::new();
static NAME: OnceLock<&'static str> = OnceLock::new();
static COUNTERS: [AtomicU8; 0x13] = [const { AtomicU8::new(0) }; 0x13];
static BULK: OnceLock<bool> = OnceLock::new();
static TRACE: OnceLock<bool> = OnceLock::new();
/// Interleave type 1 (status) reports with the inputs: Injustice waits for both.
static STATUS: AtomicBool = AtomicBool::new(false);
static READS: AtomicU32 = AtomicU32::new(0);

/// Status reports (type 1) every 25th read: the game waits for one before it accepts the board.
/// Byte 7 is the reset cause: 0x40 a power-on reset (0x80 the watchdog, 0 invalid).
pub(crate) fn enable_status_reports() {
    STATUS.store(true, Ordering::Relaxed);
}

/// A libusb-win32 `struct usb_bus` / `struct usb_device` (the fields the game reads).
#[repr(C)]
struct UsbBus {
    next: usize,
    prev: usize,
    dirname: [u8; 512],
    devices: *mut UsbDevice,
    location: u32,
    root_dev: *mut UsbDevice,
}

#[repr(C)]
struct UsbDevice {
    next: usize,
    prev: usize,
    filename: [u8; 512],
    bus: *mut UsbBus,
    /// `struct usb_device_descriptor` (18 bytes, packed) + padding.
    descriptor: [u8; 20],
    config: usize,
    dev: usize,
    devnum: u8,
    num_children: u8,
    children: usize,
}

struct Fake {
    bus: *mut UsbBus,
}
unsafe impl Send for Fake {}
unsafe impl Sync for Fake {}

static FAKE: OnceLock<Fake> = OnceLock::new();

fn fake_bus() -> *mut UsbBus {
    FAKE.get_or_init(|| {
        let mut dirname = [0u8; 512];
        dirname[..3].copy_from_slice(b"bus");
        let bus = Box::into_raw(Box::new(UsbBus {
            next: 0,
            prev: 0,
            dirname,
            devices: std::ptr::null_mut(),
            location: 0,
            root_dev: std::ptr::null_mut(),
        }));
        let mut descriptor = [0u8; 20];
        descriptor[0] = 18;
        descriptor[1] = 1;
        descriptor[8..10].copy_from_slice(&VID.to_le_bytes());
        descriptor[10..12].copy_from_slice(&PID.to_le_bytes());
        descriptor[17] = 1;
        let mut filename = [0u8; 512];
        filename[..7].copy_from_slice(b"\\\\.\\rio");
        let dev = Box::into_raw(Box::new(UsbDevice {
            next: 0,
            prev: 0,
            filename,
            bus,
            descriptor,
            config: 0,
            dev: 0,
            devnum: 1,
            num_children: 0,
            children: 0,
        }));
        unsafe { (*bus).devices = dev };
        Fake { bus }
    })
    .bus
}

/// Install the game's USB transport and feed its native switch ids from the virtual sticks.
pub(crate) fn init(
    name: &'static str,
    natives: &'static [(&'static str, u8)],
    default_map: &'static [(&'static str, &'static str)],
    p2_map: &'static [(&'static str, &'static str)],
    bulk: bool,
    force: fn() -> f32,
) -> bool {
    let hooks: [(&str, usize); 11] = [
        ("usb_init", usb_ok as *const () as usize),
        ("usb_find_busses", usb_one as *const () as usize),
        ("usb_find_devices", usb_one as *const () as usize),
        ("usb_get_busses", usb_get_busses as *const () as usize),
        ("usb_open", usb_open as *const () as usize),
        ("usb_close", usb_ok as *const () as usize),
        ("usb_set_configuration", usb_ok as *const () as usize),
        ("usb_claim_interface", usb_ok as *const () as usize),
        ("usb_release_interface", usb_ok as *const () as usize),
        (
            if bulk { "usb_bulk_write" } else { "usb_interrupt_write" },
            usb_interrupt_write as *const () as usize,
        ),
        (
            if bulk { "usb_bulk_read" } else { "usb_interrupt_read" },
            usb_interrupt_read as *const () as usize,
        ),
    ];
    let missing: Vec<&str> = hooks
        .iter()
        .filter(|(f, r)| unsafe { wal_payload_common::iat::hook("libusb0.dll", f, *r) }.is_none())
        .map(|(f, _)| *f)
        .collect();
    if !missing.is_empty() {
        log!("{name}: libusb0 imports not found ({}), board not installed", missing.join(", "));
        return false;
    }
    if !bulk {
        unsafe { wal_payload_common::iat::hook("libusb0.dll", "usb_control_msg", usb_control_msg as *const () as usize) };
    }
    let _ = NAME.set(name);
    let _ = BULK.set(bulk);
    let _ = TRACE.set(std::env::var("WAL_RIO_TRACE").is_ok_and(|v| v == "1"));
    MAP.get_or_init(|| ButtonMap::new(natives, default_map));
    if !p2_map.is_empty() {
        MAP2.get_or_init(|| ButtonMap::with_defaults(natives, p2_map));
    }
    std::thread::spawn(move || run(force));
    log!("{name}: RIO board emulated behind libusb0, inputs from the virtual sticks");
    true
}

unsafe extern "C" fn usb_ok() -> i32 {
    0
}

unsafe extern "C" fn usb_one() -> i32 {
    1
}

unsafe extern "C" fn usb_get_busses() -> *mut UsbBus {
    fake_bus()
}

unsafe extern "C" fn usb_open(_dev: usize) -> usize {
    log!("{}: RIO opened", NAME.get().unwrap_or(&"rio"));
    fake_bus() as usize
}

unsafe extern "C" fn usb_interrupt_write(_h: usize, _ep: i32, _bytes: *const u8, size: i32, _timeout: i32) -> i32 {
    if TRACE.get().copied().unwrap_or(false) && size > 0 && !_bytes.is_null() {
        let data = unsafe { std::slice::from_raw_parts(_bytes, (size as usize).min(PACKET)) };
        log!("{}: RIO write {:02x?}", NAME.get().unwrap_or(&"rio"), data);
    }
    size
}

unsafe extern "C" fn usb_control_msg(_h: usize, _rt: i32, _r: i32, _v: i32, _i: i32, _bytes: *mut u8, size: i32, _timeout: i32) -> i32 {
    size
}

/// A type 2 report: the switches and the analog channels.
unsafe extern "C" fn usb_interrupt_read(_h: usize, _ep: i32, bytes: *mut u8, size: i32, _timeout: i32) -> i32 {
    std::thread::sleep(READ_PERIOD);
    let mut packet = [0u8; PACKET];
    if STATUS.load(Ordering::Relaxed) && READS.fetch_add(1, Ordering::Relaxed) % 25 == 0 {
        packet[0] = 1;
        packet[7] = 0x40;
        let n = (size.max(0) as usize).min(PACKET);
        unsafe { std::ptr::copy_nonoverlapping(packet.as_ptr(), bytes, n) };
        return n as i32;
    }
    packet[0] = 2;
    let p = &mut packet[3..];
    for (id, byte, high) in SWITCH_NIBBLES {
        let c = COUNTERS[id as usize].load(Ordering::Relaxed) & 0x0f;
        p[byte] |= if high { c << 4 } else { c };
    }
    let stick = wal_payload_common::input(0);
    let to12 = |v: u8| ((v as u32) * 4095 / 255) as u16;
    let analogs = [
        ((stick.axis(Axis::LeftX) as i32 + 32768) as u32 * 4095 / 65535) as u16,
        to12(stick.axis_u8(Axis::Accel)),
        to12(stick.axis_u8(Axis::Brake)),
        0,
    ];
    for (i, v) in analogs.iter().enumerate() {
        let shift = if i % 2 == 0 { 4 } else { 0 };
        p[11 + i / 2] |= (((v >> 8) & 0x0f) as u8) << shift;
        p[13 + i] = *v as u8;
    }
    if BULK.get().copied().unwrap_or(false) && MAP2.get().is_none() {
        // Batman consumes two keypad nibbles after the analog bytes (0xf: released).
        let key = if std::env::var("WAL_RAWTHRILLS_KEYPAD").is_ok_and(|v| v == "0") {
            crate::common::KEYPAD_NONE
        } else {
            crate::common::keypad_key()
        };
        p[17] = (key << 4) | 0x0f;
    }
    if TRACE.get().copied().unwrap_or(false) {
        use std::sync::Mutex;
        static LAST: Mutex<Option<[u8; 17]>> = Mutex::new(None);
        let report: [u8; 17] = p[..17].try_into().unwrap();
        let mut last = LAST.lock().unwrap();
        if *last != Some(report) {
            log!(
                "{}: RIO report {:02x?}, analogs {:?}",
                NAME.get().unwrap_or(&"rio"),
                report,
                analogs
            );
            *last = Some(report);
        }
    }
    let n = (size.max(0) as usize).min(PACKET);
    unsafe { std::ptr::copy_nonoverlapping(packet.as_ptr(), bytes, n) };
    n as i32
}

fn run(force: fn() -> f32) {
    let map = MAP.get().expect("map set by init");
    let mut held = [false; 0x13];
    let mut ffb = FfbSender::default();
    loop {
        let stick = wal_payload_common::input(0);
        let mut down = [false; 0x13];
        map.for_each_pressed(&stick, |id| down[id as usize] = true);
        if let Some(map2) = MAP2.get() {
            map2.for_each_pressed(&wal_payload_common::input(1), |id| down[id as usize] = true);
        } else {
            // single-player cabinets: Batman's brake pedal, a second coin slot
            if BULK.get().copied().unwrap_or(false) && stick.axis_u8(Axis::Brake) > 85 {
                down[0x08] = true;
            }
            if wal_payload_common::input(1).pressed(button::COIN) {
                down[0x01] = true;
            }
        }
        for (id, (&d, h)) in down.iter().zip(held.iter_mut()).enumerate() {
            if d != *h {
                COUNTERS[id].fetch_add(1, Ordering::Relaxed);
                *h = d;
            }
        }
        set_motor(force());
        ffb.poll();
        std::thread::sleep(Duration::from_millis(4));
    }
}
