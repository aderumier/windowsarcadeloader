//! UltraPin's frontend movies (the tables' attract and backglass movies, `fe/sel/*.avi`).
//!
//! `plfe.exe` reads them with Video for Windows (`AVIStreamGetFrameOpen` /
//! `AVIStreamGetFrame`, then `DrawDibDraw`): they are DivX 5 (`DX50`), the cabinet's DivX codec
//! decoded them. Without a Video for Windows codec for it the frame reader is not opened (the
//! frontend draws a placeholder: "movie WxH"): it is then made here, the stream's compressed
//! frames (`AVIStreamRead`) decoded by the runner's Media Foundation MPEG-4 part 2 decoder (DivX
//! 5 is its bitstream, given as `MP4V`) into a 32-bit DIB.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{iat, log};
use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
use windows_sys::core::GUID;

type P = *mut c_void;

static ORIG_OPEN: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET: AtomicUsize = AtomicUsize::new(0);
static ORIG_CLOSE: AtomicUsize = AtomicUsize::new(0);

/// The frame readers made here (their addresses are the frontend's `PGETFRAME`s).
static MOVIES: Mutex<Vec<usize>> = Mutex::new(Vec::new());

pub(crate) fn init() {
    let hooks: [(&str, &AtomicUsize, usize); 3] = [
        ("AVIStreamGetFrameOpen", &ORIG_OPEN, get_frame_open as *const () as usize),
        ("AVIStreamGetFrame", &ORIG_GET, get_frame as *const () as usize),
        ("AVIStreamGetFrameClose", &ORIG_CLOSE, get_frame_close as *const () as usize),
    ];
    for (name, orig, hook) in hooks {
        if let Some(o) = unsafe { iat::hook("avifil32.dll", name, hook) } {
            orig.store(o, Ordering::Relaxed);
        }
    }
}

const fn guid(data1: u32, data2: u16, data3: u16, data4: [u8; 8]) -> GUID {
    GUID { data1, data2, data3, data4 }
}

/// A media subtype from its FourCC (or D3DFORMAT).
const fn subtype(fourcc: u32) -> GUID {
    guid(fourcc, 0, 0x10, [0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71])
}

const MF_MT_MAJOR_TYPE: GUID = guid(0x48eba18e, 0xf8c9, 0x4687, [0xbf, 0x11, 0x0a, 0x74, 0xc9, 0xf9, 0x6a, 0x8f]);
const MF_MT_SUBTYPE: GUID = guid(0xf7e34c9a, 0x42e8, 0x4714, [0xb7, 0x4b, 0xcb, 0x29, 0xd7, 0x2c, 0x35, 0xe5]);
const MF_MT_FRAME_SIZE: GUID = guid(0x1652c33d, 0xd6b2, 0x4012, [0xb8, 0x34, 0x72, 0x03, 0x08, 0x49, 0xa3, 0x7d]);
const MF_MT_FRAME_RATE: GUID = guid(0xc459a2e8, 0x3d2c, 0x4e44, [0xb1, 0x32, 0xfe, 0xe5, 0x15, 0x6c, 0x7b, 0xb0]);
const MF_MEDIA_TYPE_VIDEO: GUID = subtype(0x73646976);
const MP4V: GUID = subtype(0x5634504d);
const RGB32: GUID = subtype(0x16);
/// The MPEG-4 part 2 decoder (CLSID_CMpeg4sDecMFT) and IMFTransform.
const MPEG4_DECODER: GUID = guid(0x2d709e52, 0x123f, 0x49b5, [0x9c, 0xbc, 0x9a, 0xf5, 0xcd, 0xe2, 0x8f, 0xb9]);
const IID_IMFTRANSFORM: GUID = guid(0xbf94c121, 0x5b05, 0x4e6f, [0x80, 0x00, 0xba, 0x59, 0x89, 0x61, 0x41, 0x4d]);

const MF_E_TRANSFORM_NEED_MORE_INPUT: i32 = 0xC00D6D72_u32 as i32;
const MF_E_TRANSFORM_STREAM_CHANGE: i32 = 0xC00D6D61_u32 as i32;
const MFT_MESSAGE_COMMAND_FLUSH: u32 = 0;
const MFT_MESSAGE_COMMAND_DRAIN: u32 = 1;
/// AVIStreamFindSample: the previous key frame.
const FIND_PREV_KEY: i32 = 0x0004 | 0x0010;

/// An object's method `index`.
unsafe fn method(object: P, index: usize) -> usize {
    unsafe { *(*(object as *const *const usize)).add(index) }
}

unsafe fn release(object: P) {
    if !object.is_null() {
        let f: unsafe extern "system" fn(P) -> u32 = unsafe { std::mem::transmute(method(object, 2)) };
        unsafe { f(object) };
    }
}

/// A function of a DLL, loaded.
fn function(dll: &str, name: &str) -> Option<usize> {
    let module = unsafe { LoadLibraryA(format!("{dll}\0").as_ptr()) };
    unsafe { GetProcAddress(module, format!("{name}\0").as_ptr()) }.map(|f| f as usize)
}

/// The Media Foundation and Video for Windows functions used.
struct Api {
    create_media_type: unsafe extern "system" fn(*mut P) -> i32,
    create_sample: unsafe extern "system" fn(*mut P) -> i32,
    create_buffer: unsafe extern "system" fn(u32, *mut P) -> i32,
    read_format: unsafe extern "system" fn(P, i32, *mut u8, *mut i32) -> i32,
    read: unsafe extern "system" fn(P, i32, i32, *mut u8, i32, *mut i32, *mut i32) -> i32,
    find_sample: unsafe extern "system" fn(P, i32, i32) -> i32,
    start: unsafe extern "system" fn(P) -> i32,
    length: unsafe extern "system" fn(P) -> i32,
    info: unsafe extern "system" fn(P, *mut u32, i32) -> i32,
}

fn api() -> Option<&'static Api> {
    static API: std::sync::OnceLock<Option<Api>> = std::sync::OnceLock::new();
    API.get_or_init(|| unsafe {
        let startup: unsafe extern "system" fn(u32, u32) -> i32 = std::mem::transmute(function("mfplat.dll", "MFStartup")?);
        // MF_VERSION, MFSTARTUP_FULL
        startup(0x0002_0070, 0);
        Some(Api {
            create_media_type: std::mem::transmute(function("mfplat.dll", "MFCreateMediaType")?),
            create_sample: std::mem::transmute(function("mfplat.dll", "MFCreateSample")?),
            create_buffer: std::mem::transmute(function("mfplat.dll", "MFCreateMemoryBuffer")?),
            read_format: std::mem::transmute(function("avifil32.dll", "AVIStreamReadFormat")?),
            read: std::mem::transmute(function("avifil32.dll", "AVIStreamRead")?),
            find_sample: std::mem::transmute(function("avifil32.dll", "AVIStreamFindSample")?),
            start: std::mem::transmute(function("avifil32.dll", "AVIStreamStart")?),
            length: std::mem::transmute(function("avifil32.dll", "AVIStreamLength")?),
            info: std::mem::transmute(function("avifil32.dll", "AVIStreamInfoW")?),
        })
    })
    .as_ref()
}

/// A movie's frame reader: its decoder fed the stream's frames in order, the last frame decoded
/// kept as a bottom-up 32-bit DIB.
struct Movie {
    api: &'static Api,
    stream: P,
    decoder: P,
    width: usize,
    height: usize,
    /// The stream's first frame and its end.
    start: i32,
    end: i32,
    /// 100 ns units per frame.
    frame_time: f64,
    /// The next frame to feed the decoder, and the last one it gave.
    next: i32,
    shown: Option<i32>,
    /// The decoder's output sample (its buffer the frame's size).
    output: P,
    /// A compressed frame read.
    packet: Vec<u8>,
    /// BITMAPINFOHEADER then the pixels.
    dib: Vec<u8>,
}

impl Movie {
    unsafe fn new(stream: P) -> Option<Box<Movie>> {
        let api = api()?;
        // the stream's BITMAPINFOHEADER: its size and codec
        let mut format = [0u8; 256];
        let mut size = format.len() as i32;
        if unsafe { (api.read_format)(stream, 0, format.as_mut_ptr(), &mut size) } != 0 {
            return None;
        }
        let word = |at: usize| u32::from_le_bytes(format[at..at + 4].try_into().unwrap());
        let (width, height, codec) = (word(4) as usize, (word(8) as i32).unsigned_abs() as usize, word(16));
        // AVISTREAMINFOW: dwScale, dwRate
        let mut info = [0u32; 64];
        unsafe { (api.info)(stream, info.as_mut_ptr(), 256) };
        let (scale, rate) = (info[5].max(1), info[6].max(1));
        unsafe { CoInitializeEx(std::ptr::null(), 0) };
        let mut decoder: P = std::ptr::null_mut();
        if unsafe { CoCreateInstance(&MPEG4_DECODER, std::ptr::null_mut(), 1, &IID_IMFTRANSFORM, &mut decoder) } != 0 {
            log!("ultrapin: no MPEG-4 decoder for the movies");
            return None;
        }
        let mut movie = Box::new(Movie {
            api,
            stream,
            decoder,
            width,
            height,
            start: unsafe { (api.start)(stream) },
            end: unsafe { (api.start)(stream) + (api.length)(stream) },
            frame_time: 1e7 * scale as f64 / rate as f64,
            next: 0,
            shown: None,
            output: std::ptr::null_mut(),
            packet: Vec::new(),
            dib: Vec::new(),
        });
        movie.next = movie.start;
        if !unsafe { movie.set_types(rate, scale) } {
            log!("ultrapin: a movie's decoder refused it ({:?} {width}x{height})", String::from_utf8_lossy(&codec.to_le_bytes()));
            return None;
        }
        let mut dib = vec![0u8; 40 + width * height * 4];
        for (at, v) in [(0, 40u32), (4, width as u32), (8, height as u32), (12, 1 | 32 << 16), (20, (width * height * 4) as u32)] {
            dib[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        movie.dib = dib;
        log!("ultrapin: a movie decoded ({:?} {width}x{height}, {} frames)", String::from_utf8_lossy(&codec.to_le_bytes()), movie.end - movie.start);
        Some(movie)
    }

    /// The decoder's input (MPEG-4 part 2 of the movie's size and rate) and output (RGB32)
    /// types, its output sample.
    unsafe fn set_types(&mut self, rate: u32, scale: u32) -> bool {
        let api = self.api;
        let mut input: P = std::ptr::null_mut();
        if unsafe { (api.create_media_type)(&mut input) } != 0 {
            return false;
        }
        unsafe {
            let set_guid: unsafe extern "system" fn(P, *const GUID, *const GUID) -> i32 = std::mem::transmute(method(input, 24));
            let set_u64: unsafe extern "system" fn(P, *const GUID, u64) -> i32 = std::mem::transmute(method(input, 22));
            set_guid(input, &MF_MT_MAJOR_TYPE, &MF_MEDIA_TYPE_VIDEO);
            set_guid(input, &MF_MT_SUBTYPE, &MP4V);
            set_u64(input, &MF_MT_FRAME_SIZE, (self.width as u64) << 32 | self.height as u64);
            set_u64(input, &MF_MT_FRAME_RATE, (rate as u64) << 32 | scale as u64);
        }
        let set_input: unsafe extern "system" fn(P, u32, P, u32) -> i32 = unsafe { std::mem::transmute(method(self.decoder, 15)) };
        let ok = unsafe { set_input(self.decoder, 0, input, 0) } == 0;
        unsafe { release(input) };
        ok && unsafe { self.set_output() }
    }

    /// The decoder's RGB32 output type and a sample its frame's size.
    unsafe fn set_output(&mut self) -> bool {
        let available: unsafe extern "system" fn(P, u32, u32, *mut P) -> i32 = unsafe { std::mem::transmute(method(self.decoder, 14)) };
        let set_output: unsafe extern "system" fn(P, u32, P, u32) -> i32 = unsafe { std::mem::transmute(method(self.decoder, 16)) };
        let mut set = false;
        for i in 0.. {
            let mut t: P = std::ptr::null_mut();
            if unsafe { available(self.decoder, 0, i, &mut t) } != 0 {
                break;
            }
            let get_guid: unsafe extern "system" fn(P, *const GUID, *mut GUID) -> i32 = unsafe { std::mem::transmute(method(t, 10)) };
            let mut sub: GUID = unsafe { std::mem::zeroed() };
            unsafe { get_guid(t, &MF_MT_SUBTYPE, &mut sub) };
            if sub.data1 == RGB32.data1 && sub.data2 == RGB32.data2 {
                set = unsafe { set_output(self.decoder, 0, t, 0) } == 0;
            }
            unsafe { release(t) };
            if set {
                break;
            }
        }
        if !set {
            return false;
        }
        unsafe { release(self.output) };
        self.output = std::ptr::null_mut();
        let mut buffer: P = std::ptr::null_mut();
        if unsafe { (self.api.create_sample)(&mut self.output) } != 0
            || unsafe { (self.api.create_buffer)((self.width * self.height * 4) as u32, &mut buffer) } != 0
        {
            return false;
        }
        let add: unsafe extern "system" fn(P, P) -> i32 = unsafe { std::mem::transmute(method(self.output, 42)) };
        unsafe { add(self.output, buffer) };
        unsafe { release(buffer) };
        true
    }

    /// The DIB of frame `pos`: decoded from the previous key frame when not next.
    unsafe fn frame(&mut self, pos: i32) -> *const u8 {
        let pos = pos.clamp(self.start, self.end - 1);
        let sequential = self.shown.is_some_and(|s| s <= pos) && pos < self.next + 60;
        if self.shown != Some(pos) && !sequential {
            let key = unsafe { (self.api.find_sample)(self.stream, pos, FIND_PREV_KEY) };
            unsafe { self.message(MFT_MESSAGE_COMMAND_FLUSH) };
            self.next = if key >= self.start { key } else { self.start };
            self.shown = None;
        }
        while self.shown.is_none_or(|s| s < pos) {
            if self.next >= self.end {
                unsafe { self.message(MFT_MESSAGE_COMMAND_DRAIN) };
                unsafe { self.drain() };
                break;
            }
            let frame = self.next;
            self.next += 1;
            if unsafe { self.feed(frame) } {
                unsafe { self.drain() };
            }
        }
        self.dib.as_ptr()
    }

    unsafe fn message(&self, message: u32) {
        let f: unsafe extern "system" fn(P, u32, usize) -> i32 = unsafe { std::mem::transmute(method(self.decoder, 23)) };
        unsafe { f(self.decoder, message, 0) };
    }

    /// The compressed frame `frame` given to the decoder (false: an empty frame, a repeat).
    unsafe fn feed(&mut self, frame: i32) -> bool {
        let api = self.api;
        let (mut bytes, mut samples) = (0, 0);
        unsafe { (api.read)(self.stream, frame, 1, std::ptr::null_mut(), 0, &mut bytes, &mut samples) };
        if bytes <= 0 {
            return false;
        }
        self.packet.resize(bytes as usize, 0);
        if unsafe { (api.read)(self.stream, frame, 1, self.packet.as_mut_ptr(), bytes, &mut bytes, &mut samples) } != 0 {
            return false;
        }
        let (mut sample, mut buffer): (P, P) = (std::ptr::null_mut(), std::ptr::null_mut());
        if unsafe { (api.create_sample)(&mut sample) } != 0 || unsafe { (api.create_buffer)(bytes as u32, &mut buffer) } != 0 {
            unsafe { release(sample) };
            return false;
        }
        unsafe {
            let lock: unsafe extern "system" fn(P, *mut *mut u8, *mut u32, *mut u32) -> i32 = std::mem::transmute(method(buffer, 3));
            let unlock: unsafe extern "system" fn(P) -> i32 = std::mem::transmute(method(buffer, 4));
            let set_length: unsafe extern "system" fn(P, u32) -> i32 = std::mem::transmute(method(buffer, 6));
            let mut bits = std::ptr::null_mut();
            if lock(buffer, &mut bits, std::ptr::null_mut(), std::ptr::null_mut()) == 0 {
                std::ptr::copy_nonoverlapping(self.packet.as_ptr(), bits, bytes as usize);
                unlock(buffer);
                set_length(buffer, bytes as u32);
            }
            let add: unsafe extern "system" fn(P, P) -> i32 = std::mem::transmute(method(sample, 42));
            let set_time: unsafe extern "system" fn(P, i64) -> i32 = std::mem::transmute(method(sample, 36));
            let set_duration: unsafe extern "system" fn(P, i64) -> i32 = std::mem::transmute(method(sample, 38));
            add(sample, buffer);
            set_time(sample, ((frame - self.start) as f64 * self.frame_time) as i64);
            set_duration(sample, self.frame_time as i64);
            let input: unsafe extern "system" fn(P, u32, P, u32) -> i32 = std::mem::transmute(method(self.decoder, 24));
            input(self.decoder, 0, sample, 0);
            release(buffer);
            release(sample);
        }
        true
    }

    /// The frames the decoder has ready, the last one kept.
    unsafe fn drain(&mut self) {
        let output: unsafe extern "system" fn(P, u32, u32, *mut [usize; 4], *mut u32) -> i32 = unsafe { std::mem::transmute(method(self.decoder, 25)) };
        loop {
            // MFT_OUTPUT_DATA_BUFFER: stream id, sample, status, events
            let mut data = [0usize, self.output as usize, 0, 0];
            let mut status = 0;
            match unsafe { output(self.decoder, 0, 1, &mut data, &mut status) } {
                0 => unsafe { self.keep() },
                MF_E_TRANSFORM_STREAM_CHANGE => {
                    if !unsafe { self.set_output() } {
                        return;
                    }
                }
                MF_E_TRANSFORM_NEED_MORE_INPUT => return,
                _ => return,
            }
            if data[3] != 0 {
                unsafe { release(data[3] as P) };
            }
        }
    }

    /// The decoder's output frame copied into the DIB (its rows turned bottom-up).
    unsafe fn keep(&mut self) {
        unsafe {
            let get_time: unsafe extern "system" fn(P, *mut i64) -> i32 = std::mem::transmute(method(self.output, 35));
            let mut time = 0;
            get_time(self.output, &mut time);
            self.shown = Some(self.start + (time as f64 / self.frame_time).round() as i32);
            let contiguous: unsafe extern "system" fn(P, *mut P) -> i32 = std::mem::transmute(method(self.output, 41));
            let mut buffer: P = std::ptr::null_mut();
            if contiguous(self.output, &mut buffer) != 0 {
                return;
            }
            let lock: unsafe extern "system" fn(P, *mut *mut u8, *mut u32, *mut u32) -> i32 = std::mem::transmute(method(buffer, 3));
            let unlock: unsafe extern "system" fn(P) -> i32 = std::mem::transmute(method(buffer, 4));
            let (mut bits, mut length) = (std::ptr::null_mut(), 0u32);
            if lock(buffer, &mut bits, std::ptr::null_mut(), &mut length) == 0 {
                let row = self.width * 4;
                if length as usize >= row * self.height {
                    for y in 0..self.height {
                        let from = bits.add(y * row);
                        let to = self.dib.as_mut_ptr().add(40 + (self.height - 1 - y) * row);
                        std::ptr::copy_nonoverlapping(from, to, row);
                    }
                }
                unlock(buffer);
            }
            release(buffer);
        }
    }
}

impl Drop for Movie {
    fn drop(&mut self) {
        unsafe {
            release(self.output);
            release(self.decoder);
        }
    }
}

/// `AVIStreamGetFrameOpen(pavi, lpbiWanted)`: Video for Windows' reader, else one decoding
/// the stream here.
unsafe extern "system" fn get_frame_open(stream: P, wanted: P) -> usize {
    let orig: unsafe extern "system" fn(P, P) -> usize = unsafe { std::mem::transmute(ORIG_OPEN.load(Ordering::Relaxed)) };
    let reader = unsafe { orig(stream, wanted) };
    if reader != 0 {
        return reader;
    }
    let Some(movie) = (unsafe { Movie::new(stream) }) else { return 0 };
    let reader = Box::into_raw(movie) as usize;
    MOVIES.lock().unwrap_or_else(|e| e.into_inner()).push(reader);
    reader
}

fn ours(reader: usize) -> bool {
    MOVIES.lock().unwrap_or_else(|e| e.into_inner()).contains(&reader)
}

/// `AVIStreamGetFrame(pgf, lPos)`: the packed DIB of a frame.
unsafe extern "system" fn get_frame(reader: usize, pos: i32) -> *const u8 {
    if !ours(reader) {
        let orig: unsafe extern "system" fn(usize, i32) -> *const u8 = unsafe { std::mem::transmute(ORIG_GET.load(Ordering::Relaxed)) };
        return unsafe { orig(reader, pos) };
    }
    unsafe { (*(reader as *mut Movie)).frame(pos) }
}

/// `AVIStreamGetFrameClose(pgf)`.
unsafe extern "system" fn get_frame_close(reader: usize) -> i32 {
    let mut movies = MOVIES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = movies.iter().position(|&m| m == reader) {
        movies.swap_remove(i);
        drop(unsafe { Box::from_raw(reader as *mut Movie) });
        return 0;
    }
    drop(movies);
    let orig: unsafe extern "system" fn(usize) -> i32 = unsafe { std::mem::transmute(ORIG_CLOSE.load(Ordering::Relaxed)) };
    unsafe { orig(reader) }
}
