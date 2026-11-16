//! Kemono Friends 3's cameras without webcams: the card slot's camera reads the printed cards'
//! QR codes (`QrCodeDecoder`: a webcam texture's frames decoded by ZXing on a thread, then
//! decrypted), the player's camera takes photos.
//!
//! - Two webcams listed (`WebCamTexture.devices`, Unity's internal call replaced when the
//!   player registers it): the game's camera setup (`CardReaderTool.SetupCameraDevice`) and
//!   `QrCodeDecoder.Open` find them. Their textures never play (no such device): the decoder's
//!   thread gets no frame; `QrCodeDecoder.IsCameraPlaying` answers true.
//! - The printed cards: the game's `QrCodeEncoder` checksums the card's whole QR content
//!   (header, encrypted data) with `QrCodeUtil.CalcCRC7` before drawing it: that function is
//!   the payload's (the same CRC), which keeps the content (`Y:\kemono\cards\<n>.bin`).
//! - The card slot: when the card camera's decoder opens (`SetDeviceNameFromPos`), it holds
//!   the next card printed in this session and not read yet, else the empty slot's code
//!   (`SDFL_POS_CARD`, by which the boot's camera check finds the card camera), as a code its
//!   thread decoded (`foundResult`, `m_IsFound`): the game decrypts and checks it as a camera's.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::log;
use wal_payload_common::mono::{self, ARRAY_DATA, P};

const CARD_DIR: &str = "Y:\\kemono\\cards";
/// The longest code the decoder's thread hands over (`resultBuffer = new byte[254]`).
const RESULT_MAX: usize = 254;

/// The cards printed this session, not read yet; every card's content known.
static PENDING: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
static KNOWN: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());
/// The webcam list (a pinned `WebCamDevice[]`).
static DEVICES: AtomicUsize = AtomicUsize::new(0);

/// `QrCodeUtil.CalcCRC7`'s checksum.
fn crc7(data: &[u8]) -> u8 {
    let mut b: u8 = 0;
    for &x in data {
        b ^= x.wrapping_add(1);
        for _ in 0..8 {
            if b & 0x80 != 0 {
                b ^= 0x89;
            }
            b <<= 1;
        }
    }
    b
}

/// A managed `byte[]`'s bytes.
unsafe fn bytes<'a>(array: P) -> &'a mut [u8] {
    if array.is_null() {
        return &mut [];
    }
    let len = unsafe { mono::read::<usize>(array.cast(), ARRAY_DATA - size_of::<usize>()) };
    unsafe { std::slice::from_raw_parts_mut(array.cast::<u8>().add(ARRAY_DATA), len) }
}

/// The printed cards kept from earlier sessions.
pub(super) fn init() {
    let mut known = KNOWN.lock().unwrap();
    if let Ok(entries) = std::fs::read_dir(CARD_DIR) {
        known.extend(entries.flatten().filter_map(|e| std::fs::read(e.path()).ok()));
    }
}

/// `static byte QrCodeUtil.CalcCRC7(byte[] buffer, int startPos, int endPos)`: the checksum;
/// a whole QR content checksummed before its last byte (the encoder's) kept as a printed card.
pub(super) unsafe extern "C" fn calc_crc7(buffer: P, start: i32, end: i32) -> u8 {
    let data = unsafe { bytes(buffer) };
    let end = if end <= 0 { data.len() } else { (end as usize).min(data.len()) };
    let start = (start.max(0) as usize).min(end);
    let crc = crc7(&data[start..end]);
    if start == 0 && end + 1 == data.len() && end > 16 && end < RESULT_MAX {
        let mut card = data[..end].to_vec();
        card.push(crc);
        let mut known = KNOWN.lock().unwrap();
        if !known.contains(&card) {
            let _ = std::fs::create_dir_all(CARD_DIR);
            let path = format!("{CARD_DIR}\\{}.bin", known.len() + 1);
            let _ = std::fs::write(&path, &card);
            log!("kemono: card printed ({} bytes), kept as {path}", card.len());
            known.push(card.clone());
            PENDING.lock().unwrap().push(card);
        }
    }
    crc
}

/// `WebCamTexture.devices`: the card camera and the player's camera.
pub(super) unsafe extern "C" fn webcam_devices() -> P {
    let cached = DEVICES.load(Ordering::Relaxed);
    if cached != 0 {
        return cached as P;
    }
    let Some(m) = mono::mono() else { return std::ptr::null_mut() };
    let image = m.image("UnityEngine.AudioModule");
    let class = m.class(image, "UnityEngine", "WebCamDevice");
    let name = m.field(class, "m_Name");
    let depth = m.field(class, "m_DepthCameraName");
    let res = m.field(class, "m_Resolutions");
    if name.is_null() || depth.is_null() || res.is_null() {
        log!("kemono: WebCamDevice not found: no webcams listed");
        return m.array(class, 0);
    }
    // a value type's field offsets count an object header
    let header = 2 * size_of::<usize>();
    let element = m.offset(res) - header + size_of::<usize>();
    let array = m.pin(m.array(class, 2));
    for (i, label) in ["WAL Card Camera", "WAL Player Camera"].iter().enumerate() {
        let elem = unsafe { array.cast::<u8>().add(ARRAY_DATA + i * element) };
        let text = m.pin(m.string(label));
        let empty = m.pin(m.string(""));
        unsafe {
            mono::write(elem, m.offset(name) - header, text);
            mono::write(elem, m.offset(depth) - header, empty);
        }
    }
    DEVICES.store(array as usize, Ordering::Relaxed);
    log!("kemono: two webcams listed (the card slot's, the player's)");
    array
}

/// `QrCodeDecoder`'s fields set when it opens: m_DeviceName, m_DevicePos, foundResult, m_IsFound.
static FIELDS_SET: Mutex<Option<[usize; 4]>> = Mutex::new(None);

/// The webcams' names, by position.
const NAMES: [&str; 2] = ["WAL Card Camera", "WAL Player Camera"];
/// The code inside the empty card slot (`QrCodeDecoder.OnFoundResult`'s `RecognitionCode`): the
/// boot's camera check takes the webcam reading it for the card camera.
const SLOT_CODE: &[u8] = b"SDFL_POS_CARD";

/// `void QrCodeDecoder.SetDeviceNameFromPos(int pos)` (from `QrCodeDecoder.Open`): the webcam's
/// name and position, and what the card camera sees in the slot, as a code its thread decoded
/// (Awake cleared the result; Start leaves it): the next printed card not read yet, else the
/// empty slot's code.
pub(super) unsafe extern "C" fn set_device_name_from_pos(this: P, pos: i32) {
    let Some(m) = mono::mono() else { return };
    let fields = {
        let mut cached = FIELDS_SET.lock().unwrap();
        if cached.is_none() {
            let class = m.class_of(this);
            let f = ["m_DeviceName", "m_DevicePos", "foundResult", "m_IsFound"].map(|n| m.field(class, n));
            if f.iter().any(|p| p.is_null()) {
                log!("kemono: QrCodeDecoder's fields not found: the cameras see nothing");
                return;
            }
            *cached = Some(f.map(|p| p as usize));
        }
        cached.unwrap().map(|p| p as P)
    };
    let Some(name) = NAMES.get(pos as usize) else { return };
    m.set(this, fields[0], m.string(name));
    unsafe { mono::write(this.cast(), m.offset(fields[1]), pos) };
    if pos != 0 {
        return;
    }
    let card = {
        let mut pending = PENDING.lock().unwrap();
        (!pending.is_empty()).then(|| pending.remove(0))
    };
    let content = card.as_deref().unwrap_or(SLOT_CODE);
    let array = m.byte_array(content.len());
    unsafe { bytes(array) }.copy_from_slice(content);
    m.set(this, fields[2], array);
    unsafe { mono::write(this.cast(), m.offset(fields[3]), 1u8) };
    match card {
        Some(c) => log!("kemono: card camera opened, a printed card in the slot ({} bytes)", c.len()),
        None => log!("kemono: card camera opened, the slot empty"),
    }
}

/// `bool QrCodeDecoder.IsCameraPlaying`: true (the webcams' textures never play).
pub(super) unsafe extern "C" fn is_camera_playing(_this: P) -> i32 {
    1
}
