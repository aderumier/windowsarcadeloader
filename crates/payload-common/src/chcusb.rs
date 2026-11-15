//! The Sinfonia CHC-C310 card printer (KanColle Arcade), driven by the game through its USB
//! libraries `C310Ausb.dll` (`chcusb_*`) and `C310FWDLusb.dll` (`fwdlusb_*`, the firmware
//! updater): the game's `GetProcAddress` for those libraries hands it this module's functions
//! (segatools' `printer_chc.c`, public domain: c310emu, segatools-kancolle).
//!
//! `WAL_CHC_PRINTER=1`. The printer's firmware versions are those of the game's own firmware
//! files (`WAL_CHC_MAIN_FW`, `WAL_CHC_PARAM_FW`: their headers), so no update is wanted. A
//! printed card is written to `WAL_CHC_OUT` (default `printer\` in the working directory):
//! its picture (`<time>_<side>.bmp`) and its RFID chip (`<tid>.bin`: the 12-byte id, then the
//! 32 bytes the game wrote through the printer's RFID reader/writer, 837-15347).

use std::ffi::{CStr, c_char, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use crate::{iat, log};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleA;

const ID: u8 = 1;
const SERIAL: u64 = 0x0033_3231_412D_4135;

struct Printer {
    width: u16,
    height: u16,
    /// 0 idle, 1 standby, 2 printing
    status: u8,
    awaiting_exit: bool,
    side: u8,
    tid: [u8; 12],
    card: [u8; 32],
    main_fw: [u8; 0x40],
    param_fw: [u8; 0x40],
}

static PRINTER: Mutex<Printer> = Mutex::new(Printer { width: 0, height: 0, status: 0, awaiting_exit: false, side: 0, tid: [0; 12], card: [0; 32], main_fw: [0; 0x40], param_fw: [0; 0x40] });
static ORIG_GPA: AtomicUsize = AtomicUsize::new(0);

fn printer() -> std::sync::MutexGuard<'static, Printer> {
    PRINTER.lock().unwrap_or_else(|e| e.into_inner())
}

/// A hex number in ASCII (`buffer`), as the firmware headers write versions.
fn hex_ascii(b: &[u8]) -> u8 {
    b.iter().try_fold(0u32, |v, &c| (c as char).to_digit(16).map(|d| v * 16 + d)).unwrap_or(0) as u8
}

/// The firmware information of a firmware file's header (`main`: its version in ASCII hex).
fn firmware_info(header: &[u8], main: bool) -> Option<[u8; 0x40]> {
    if header.len() <= 0x24 {
        return None;
    }
    let mut r = [0u8; 0x40];
    r[0..8].copy_from_slice(&header[2..10]);
    r[8..0x18].copy_from_slice(&header[0xA..0x1A]);
    r[0x18..0x22].copy_from_slice(&header[0x20..0x2A]);
    if main {
        r[0x22] = hex_ascii(&header[0x1A..0x1C]);
        r[0x23] = hex_ascii(&header[0x1C..0x1E]);
    } else {
        r[0x22] = header[0x1A];
        r[0x23] = header[0x1C];
    }
    r[0x24..0x26].copy_from_slice(&header[0x2A..0x2C]);
    Some(r)
}

fn file_firmware(path: &str, main: bool) -> Option<[u8; 0x40]> {
    let data = std::fs::read(path).ok()?;
    firmware_info(&data[..data.len().min(0x40)], main)
}

fn new_tid() -> [u8; 12] {
    let mut tid = [0u8; 12];
    let mut x = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1;
    for b in tid.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = x as u8;
    }
    // the chip's maker prefix (E0 04: NXP ICODE)
    tid[0] = 0xE0;
    tid[1] = 0x04;
    tid
}

fn out_dir() -> String {
    std::env::var("WAL_CHC_OUT").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).unwrap_or_else(|| "printer".into())
}

pub fn init() {
    if !std::env::var("WAL_CHC_PRINTER").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    {
        let mut p = printer();
        let var = |k: &str, d: &str| std::env::var(k).ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()).unwrap_or_else(|| d.to_string());
        if let Some(f) = file_firmware(&var("WAL_CHC_MAIN_FW", "rom\\hw\\printer\\E0225500-0116-C310-MAINAPP.BIN"), true) {
            p.main_fw = f;
        }
        if let Some(f) = file_firmware(&var("WAL_CHC_PARAM_FW", "rom\\hw\\printer\\D0485300-0103-C310-PARAM.BIN"), false) {
            p.param_fw = f;
        }
        p.tid = new_tid();
    }
    match unsafe { iat::hook("kernel32.dll", "GetProcAddress", get_proc as *const () as usize) } {
        Some(o) => ORIG_GPA.store(o, Relaxed),
        None => log!("chcusb: no GetProcAddress import"),
    }
    log!("chcusb: CHC-C310 printer emulated, cards in {}", out_dir());
}

unsafe extern "system" fn get_proc(module: *mut c_void, name: *const c_char) -> usize {
    let orig: unsafe extern "system" fn(*mut c_void, *const c_char) -> usize = unsafe { std::mem::transmute(ORIG_GPA.load(Relaxed)) };
    if name as usize > 0xffff && !module.is_null() {
        let usb = unsafe { GetModuleHandleA(c"C310Ausb.dll".as_ptr().cast()) };
        let fwdl = unsafe { GetModuleHandleA(c"C310FWDLusb.dll".as_ptr().cast()) };
        if module == usb || module == fwdl {
            let n = unsafe { CStr::from_ptr(name) }.to_bytes();
            if let Some(f) = replacement(n) {
                return f;
            }
            log!("chcusb: {} not emulated", String::from_utf8_lossy(n));
        }
    }
    unsafe { orig(module, name) }
}

fn replacement(name: &[u8]) -> Option<usize> {
    let f: usize = match name {
        b"chcusb_MakeThread" | b"fwdlusb_MakeThread" => one_u16 as *const () as usize,
        b"chcusb_open" | b"fwdlusb_open" => open as *const () as usize,
        b"chcusb_close" | b"fwdlusb_close" => close as *const () as usize,
        b"chcusb_ReleaseThread" | b"fwdlusb_ReleaseThread" => ok_ptr as *const () as usize,
        b"chcusb_listupPrinter" | b"fwdlusb_listupPrinter" => listup as *const () as usize,
        b"chcusb_listupPrinterSN" | b"fwdlusb_listupPrinterSN" => listup_sn as *const () as usize,
        b"chcusb_selectPrinter" | b"fwdlusb_selectPrinter" | b"chcusb_selectPrinterSN" | b"fwdlusb_selectPrinterSN" => ok_arg_ptr as *const () as usize,
        b"chcusb_getPrinterInfo" | b"fwdlusb_getPrinterInfo" => get_printer_info as *const () as usize,
        b"chcusb_imageformat" => image_format as *const () as usize,
        b"chcusb_setmtf" => one_ptr as *const () as usize,
        b"chcusb_makeGamma" => make_gamma as *const () as usize,
        b"chcusb_setIcctable" => set_icc_table as *const () as usize,
        b"chcusb_copies" | b"chcusb_cancelCopies" => ok_arg_ptr as *const () as usize,
        b"chcusb_status" | b"fwdlusb_status" | b"chcusb_blinkLED" | b"chcusb_resetPrinter" | b"fwdlusb_resetPrinter" => ok_ptr as *const () as usize,
        b"chcusb_statusAll" | b"fwdlusb_statusAll" => status_all as *const () as usize,
        b"chcusb_startpage" => start_page as *const () as usize,
        b"chcusb_endpage" => end_page as *const () as usize,
        b"chcusb_write" => write as *const () as usize,
        b"chcusb_writeLaminate" | b"chcusb_writeHolo" => write_layer as *const () as usize,
        b"chcusb_setPrinterInfo" => set_printer_info as *const () as usize,
        b"chcusb_getGamma" => get_gamma as *const () as usize,
        b"chcusb_getMtf" => get_mtf as *const () as usize,
        b"chcusb_setPrinterToneCurve" | b"chcusb_getPrinterToneCurve" => tone_curve as *const () as usize,
        b"chcusb_AttachThreadCount" | b"fwdlusb_AttachThreadCount" => attach_thread_count as *const () as usize,
        b"chcusb_getPrintIDStatus" => print_id_status as *const () as usize,
        b"chcusb_setPrintStandby" => print_standby as *const () as usize,
        b"chcusb_testCardFeed" => ok_2args_ptr as *const () as usize,
        b"chcusb_exitCard" => exit_card as *const () as usize,
        b"chcusb_getCardRfidTID" => card_tid as *const () as usize,
        b"chcusb_commCardRfidReader" => comm_rfid as *const () as usize,
        b"chcusb_updateCardRfidReader" => ok_2args_ptr as *const () as usize,
        b"chcusb_getErrorLog" | b"fwdlusb_getErrorLog" => ok_2args_ptr as *const () as usize,
        b"chcusb_getErrorStatus" => error_status as *const () as usize,
        b"chcusb_setCutList" => ok_arg_ptr as *const () as usize,
        b"chcusb_setLaminatePattern" => ok_2args_ptr as *const () as usize,
        b"chcusb_color_adjustment" | b"chcusb_color_adjustmentEx" => color_adjustment as *const () as usize,
        b"chcusb_getEEPROM" | b"chcusb_setParameter" | b"chcusb_getParameter" => ok_2args_ptr as *const () as usize,
        b"fwdlusb_updateFirmware" => update_firmware as *const () as usize,
        b"fwdlusb_getFirmwareInfo" => get_firmware_info as *const () as usize,
        _ => return None,
    };
    Some(f)
}

unsafe fn set_u16(p: *mut u16, v: u16) {
    if !p.is_null() {
        unsafe { *p = v };
    }
}

unsafe extern "system" fn one_u16(_count: u16) -> i32 {
    1
}

unsafe extern "system" fn one_ptr(_p: *mut c_void) -> i32 {
    1
}

unsafe extern "system" fn ok_ptr(result: *mut u16) -> i32 {
    unsafe { set_u16(result, 0) };
    1
}

unsafe extern "system" fn ok_arg_ptr(_a: usize, result: *mut u16) -> i32 {
    unsafe { set_u16(result, 0) };
    1
}

unsafe extern "system" fn ok_2args_ptr(_a: usize, _b: usize, result: *mut u16) -> i32 {
    unsafe { set_u16(result, 0) };
    1
}

unsafe extern "system" fn open(result: *mut u16) -> i32 {
    log!("chcusb: open");
    unsafe { set_u16(result, 0) };
    1
}

unsafe extern "system" fn close() {}

unsafe extern "system" fn listup(ids: *mut u8) -> i32 {
    if !ids.is_null() {
        unsafe {
            std::ptr::write_bytes(ids, 0xFF, 0x80);
            *ids = ID;
        }
    }
    1
}

unsafe extern "system" fn listup_sn(serials: *mut u64) -> i32 {
    if !serials.is_null() {
        unsafe {
            std::ptr::write_bytes(serials.cast::<u8>(), 0xFF, 0x400);
            *serials = SERIAL;
        }
    }
    1
}

/// `getPrinterInfo(tag, buffer, len)`: the printer's information by tag.
unsafe extern "system" fn get_printer_info(tag: u16, buffer: *mut u8, len: *mut u32) -> i32 {
    if len.is_null() {
        return 1;
    }
    let p = printer();
    let fill = |size: usize, data: &[u8]| unsafe {
        *len = size as u32;
        if !buffer.is_null() {
            std::ptr::write_bytes(buffer, 0, size);
            std::ptr::copy_nonoverlapping(data.as_ptr(), buffer, data.len().min(size));
        }
    };
    match tag {
        // paper info
        0 => fill(0x67, &[]),
        2 => fill(0x17, &[]),
        // firmware versions: count, boot, main, parameter table, dsp (0x26 each)
        3 => {
            let mut b = vec![0u8; 0x99];
            b[0] = 4;
            b[1..0x27].copy_from_slice(&p.main_fw[..0x26]);
            b[0x27..0x4D].copy_from_slice(&p.main_fw[..0x26]);
            b[0x4D..0x73].copy_from_slice(&p.param_fw[..0x26]);
            fill(0x99, &b);
        }
        // print counters (C310A: printCounter0/1, feed roller, cutter, head, ribbon remain,
        // holo head, paper, printCounter2, holo print counter)
        4 | 5 => {
            let full = if tag == 4 { 0x20 } else { 0x28 };
            if buffer.is_null() {
                unsafe { *len = if tag == 4 { 0x1C } else { 0x28 } };
                return 1;
            }
            let values: [i32; 10] = [22, 23, 33, 55, 88, 999, if tag == 5 { 99 } else { 0 }, if tag == 5 { 69 } else { 0 }, 21, 20];
            let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
            let n = ((unsafe { *len }) as usize).min(full);
            fill(n, &bytes[..n]);
        }
        // page status: holo remain
        6 => fill(32, &[88]),
        // svc: main / sub error
        7 => fill(2, &[0, 0]),
        // print standby: a printed card to take out, else 0 (C310A and later; 1 a C300's standby,
        // a printer error for KanColle Arcade after a session with a card in standby)
        8 => fill(1, &[if p.awaiting_exit { 0xF0 } else { 0 }]),
        16 => fill(18, &[]),
        26 => fill(8, &SERIAL.to_le_bytes()),
        31 => fill(1, &[0]),
        // temperatures
        40 => fill(10, &[1, 2, 3]),
        50 => fill(61, &[]),
        60 => fill(6, &[]),
        _ => log!("chcusb: getPrinterInfo tag {tag}"),
    }
    1
}

unsafe extern "system" fn image_format(_format: u16, _ncomp: u16, _depth: u16, width: u16, height: u16, _image: *mut u8, result: *mut u16) -> i32 {
    let mut p = printer();
    p.width = width;
    p.height = height;
    unsafe { set_u16(result, 0) };
    1
}

unsafe extern "system" fn make_gamma(k: u16, r: *mut u8, g: *mut u8, b: *mut u8) -> i32 {
    let factor = k as f64 / 100.0;
    for i in 0..256usize {
        let tone = ((i as f64).powf(factor) / 255f64.powf(factor) * 255.0).clamp(0.0, 255.0) as u8;
        for t in [r, g, b] {
            if !t.is_null() {
                unsafe { *t.add(i) = tone };
            }
        }
    }
    1
}

/// `setIcctable(icc1, icc2, intents, in r g b, out r g b, result)`: identity tables.
#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn set_icc_table(_icc1: usize, _icc2: usize, _intents: u16, ir: *mut u8, ig: *mut u8, ib: *mut u8, or: *mut u8, og: *mut u8, ob: *mut u8, result: *mut u16) -> i32 {
    for t in [ir, ig, ib, or, og, ob] {
        if !t.is_null() {
            for i in 0..256 {
                unsafe { *t.add(i) = i as u8 };
            }
        }
    }
    unsafe { set_u16(result, 0) };
    1
}

unsafe extern "system" fn status_all(ids: *const u8, results: *mut u16) -> i32 {
    if !ids.is_null() && !results.is_null() {
        for i in 0..128 {
            if unsafe { *ids.add(i) } == 0xFF {
                break;
            }
            unsafe { *results.add(i) = 0 };
        }
    }
    1
}

unsafe extern "system" fn start_page(_post_card_state: u16, page_id: *mut u16, result: *mut u16) -> i32 {
    printer().status = 2;
    unsafe {
        set_u16(page_id, 1);
        set_u16(result, 0);
    }
    1
}

unsafe extern "system" fn end_page(result: *mut u16) -> i32 {
    let mut p = printer();
    p.awaiting_exit = true;
    p.side = 0;
    log!("chcusb: card printed (chip {})", hex(&p.tid));
    unsafe { set_u16(result, 0) };
    1
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

/// A 24-bit picture (the printer's rows, bottom-up as a bitmap's) to a BMP file.
fn write_bmp24(path: &str, w: u32, h: u32, data: &[u8]) -> std::io::Result<()> {
    let row = (w * 3) as usize;
    let stride = row.div_ceil(4) * 4;
    let mut out = Vec::with_capacity(54 + stride * h as usize);
    let size = (54 + stride * h as usize) as u32;
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0; 24]);
    for y in 0..h as usize {
        let line = data.get(y * row..(y + 1) * row).unwrap_or(&[]);
        out.extend_from_slice(line);
        out.resize(out.len() + stride - line.len(), 0);
    }
    std::fs::write(path, out)
}

unsafe extern "system" fn write(data: *const u8, size: *mut u32, result: *mut u16) -> i32 {
    let (w, h, side) = {
        let mut p = printer();
        let side = p.side;
        p.side += 1;
        (p.width as u32, p.height as u32, side)
    };
    let len = if size.is_null() { 0 } else { (unsafe { *size }) as usize };
    if !data.is_null() && len > 0 && w > 0 && h > 0 {
        let dir = out_dir();
        let _ = std::fs::create_dir_all(&dir);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let path = format!("{dir}\\{t}_{side}.bmp");
        let r = write_bmp24(&path, w, h, unsafe { std::slice::from_raw_parts(data, len) });
        log!("chcusb: {path} {w}x{h} ({len} bytes) {r:?}");
    }
    unsafe { set_u16(result, 0) };
    1
}

unsafe extern "system" fn write_layer(_data: *const u8, _size: *mut u32, result: *mut u16) -> i32 {
    unsafe { set_u16(result, 0) };
    1
}

unsafe extern "system" fn set_printer_info(_tag: u16, _buffer: *mut u8, _len: *mut u32, result: *mut u16) -> i32 {
    unsafe { set_u16(result, 0) };
    1
}

unsafe extern "system" fn get_gamma(_file: usize, r: *mut u8, g: *mut u8, b: *mut u8, result: *mut u16) -> i32 {
    for t in [r, g, b] {
        if !t.is_null() {
            for i in 0..256 {
                unsafe { *t.add(i) = i as u8 };
            }
        }
    }
    unsafe { set_u16(result, 0) };
    1
}

/// `getMtf(file, mtf, result)`: the sharpness table, numbers in the text file.
unsafe extern "system" fn get_mtf(file: *const c_char, mtf: *mut i32, result: *mut u16) -> i32 {
    if file.is_null() || mtf.is_null() {
        return 0;
    }
    let path = unsafe { CStr::from_ptr(file) }.to_string_lossy().into_owned();
    let Ok(text) = std::fs::read_to_string(&path) else { return 0 };
    let values: Vec<i32> = text.split_whitespace().filter_map(|v| v.parse().ok()).take(9).collect();
    for i in 0..9 {
        unsafe { *mtf.add(i) = values.get(i).copied().unwrap_or(0) };
    }
    unsafe { set_u16(result, 0) };
    1
}

unsafe extern "system" fn tone_curve(_kind: u16, _number: u16, _data: *mut u16, result: *mut u16) -> i32 {
    unsafe { set_u16(result, 0) };
    1
}

unsafe extern "system" fn attach_thread_count(count: *mut u16, max: *mut u16) -> i32 {
    unsafe {
        set_u16(count, 0);
        set_u16(max, 1);
    }
    1
}

/// `getPrintIDStatus(page, buffer, result)`: 2212 printed (once after a page), 2300 idle.
unsafe extern "system" fn print_id_status(_page: u16, buffer: *mut u8, result: *mut u16) -> i32 {
    let mut p = printer();
    let code: u16 = if p.status > 1 {
        p.status = 0;
        2212
    } else {
        2300
    };
    if !buffer.is_null() {
        unsafe {
            std::ptr::write_bytes(buffer, 0, 8);
            std::ptr::copy_nonoverlapping(code.to_le_bytes().as_ptr(), buffer.add(6), 2);
        }
    }
    unsafe { set_u16(result, 0) };
    1
}

unsafe extern "system" fn print_standby(_position: u16, result: *mut u16) -> i32 {
    let mut p = printer();
    let r = if p.status == 0 {
        p.status = 1;
        2100
    } else {
        0
    };
    log!("chcusb: print standby {r}");
    unsafe { set_u16(result, r) };
    1
}

/// The printed card leaves: its chip saved, a new blank card's chip next (a card in standby
/// not printed: out, nothing saved).
unsafe extern "system" fn exit_card(result: *mut u16) -> i32 {
    let mut p = printer();
    p.status = 0;
    if !std::mem::take(&mut p.awaiting_exit) {
        log!("chcusb: card out, not printed");
        unsafe { set_u16(result, 0) };
        return 1;
    }
    let dir = out_dir();
    let _ = std::fs::create_dir_all(&dir);
    let mut chip = p.tid.to_vec();
    chip.extend_from_slice(&p.card);
    let path = format!("{dir}\\{}.bin", hex(&p.tid));
    let r = std::fs::write(&path, chip);
    log!("chcusb: card out, chip {path} {r:?}");
    p.tid = new_tid();
    p.card = [0; 32];
    unsafe { set_u16(result, 0) };
    1
}

unsafe extern "system" fn card_tid(tid: *mut u8, result: *mut u16) -> i32 {
    let p = printer();
    if !tid.is_null() {
        unsafe { std::ptr::copy_nonoverlapping(p.tid.as_ptr(), tid, 12) };
    }
    unsafe { set_u16(result, 2405) };
    1
}

/// The printer's RFID reader/writer (837-15347): read (02) / write (03) the card's 32 bytes,
/// its versions (84 boot, 42 application: 0x91, the game's firmware file) and board (85).
unsafe extern "system" fn comm_rfid(send: *const u8, recv: *mut u8, send_size: u32, recv_size: *mut u32, result: *mut u16) -> i32 {
    unsafe { set_u16(result, 0) };
    if send.is_null() || recv.is_null() || send_size == 0 {
        return 1;
    }
    let req = unsafe { std::slice::from_raw_parts(send, send_size as usize) };
    let mut p = printer();
    let reply: Vec<u8> = match req[0] {
        0x02 => {
            let mut r = vec![2, 0, 32];
            r.extend_from_slice(&p.card);
            r
        }
        0x03 if req.len() >= 7 => {
            let off = (req[4] as usize).saturating_sub(1) * 2;
            if off + 1 < 32 {
                p.card[off] = req[5];
                p.card[off + 1] = req[6];
            }
            vec![3, 0, 0]
        }
        0x84 => vec![0x84, 0, 1, 0x90],
        0x85 => {
            let mut r = vec![0x85, 0, 9];
            r.extend_from_slice(b"837-15347");
            r
        }
        0x42 => vec![0x42, 0, 1, 0x91],
        c => vec![c, 0, 0],
    };
    unsafe {
        std::ptr::copy_nonoverlapping(reply.as_ptr(), recv, reply.len());
        if !recv_size.is_null() {
            *recv_size = reply.len() as u32;
        }
    }
    1
}

unsafe extern "system" fn error_status(buffer: *mut u16) -> i32 {
    if !buffer.is_null() {
        unsafe { std::ptr::write_bytes(buffer.cast::<u8>(), 0, 0x80) };
    }
    1
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn color_adjustment(_a: usize, _b: i32, _c: i32, _d: i16, _e: i16, _f: i64, _g: i64, result: *mut u16) -> i32 {
    unsafe { set_u16(result, 0) };
    1
}

/// `updateFirmware(kind, file, result)`: the printer now reports that file's version.
unsafe extern "system" fn update_firmware(kind: u8, file: *const c_char, result: *mut u16) -> i32 {
    let path = if file.is_null() { String::new() } else { unsafe { CStr::from_ptr(file) }.to_string_lossy().into_owned() };
    log!("chcusb: firmware update {kind} from {path}");
    let Some(info) = file_firmware(&path, kind == 1) else {
        unsafe { set_u16(result, 1005) };
        return 0;
    };
    let mut p = printer();
    match kind {
        1 => p.main_fw = info,
        3 => p.param_fw = info,
        _ => {}
    }
    unsafe { set_u16(result, 0) };
    1
}

/// `getFirmwareInfo(kind, file, buffer, len, result)`: a firmware file's information.
unsafe extern "system" fn get_firmware_info(kind: u8, file: *const c_char, buffer: *mut u8, len: *mut u32, result: *mut u16) -> i32 {
    if buffer.is_null() {
        if !len.is_null() {
            unsafe { *len = 38 };
        }
        return 1;
    }
    if !len.is_null() && unsafe { *len } > 38 {
        unsafe { *len = 38 };
    }
    if !(1..=3).contains(&kind) {
        unsafe { set_u16(result, 0) };
        return 1;
    }
    if file.is_null() {
        unsafe { set_u16(result, 1006) };
        return 0;
    }
    let path = unsafe { CStr::from_ptr(file) }.to_string_lossy().into_owned();
    match file_firmware(&path, kind == 1) {
        Some(info) => {
            unsafe {
                std::ptr::copy_nonoverlapping(info.as_ptr(), buffer, 38);
                set_u16(result, 0);
            }
            1
        }
        None => {
            unsafe { set_u16(result, 1005) };
            0
        }
    }
}
