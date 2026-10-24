//! `WAL_WASAPI_SHARED=1`: a game's exclusive-mode WASAPI stream played in shared mode.
//! winepulse.drv has no exclusive mode (`IAudioClient::Initialize` fails, the game is silent:
//! Bishi Bashi Channel). As spice2x's `-wasapishared` (the open-source reference):
//!
//! * `IsFormatSupported` in exclusive mode: yes for PCM / float (the shared engine converts).
//! * `Initialize` exclusive: shared, periodicity 0, `AUTOCONVERTPCM | SRC_DEFAULT_QUALITY`.
//! * `GetBufferSize`: one device period (what an exclusive event-driven game fills per event).
//! * The game's `GetBuffer` / `ReleaseBuffer` write into a FIFO, given to the device as its
//!   shared buffer has room (a full-buffer write would fail with AUDCLNT_E_BUFFER_TOO_LARGE).
//!
//! Wine's IAudioClient and IAudioRenderClient vtables are shared by all its streams: patched
//! once, on the game's first device (`CoCreateInstance(MMDeviceEnumerator)` ->
//! `GetDefaultAudioEndpoint` -> `Activate(IAudioClient)` -> `GetService(IAudioRenderClient)`);
//! only the redirected streams are handled differently.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};

use crate::{iat, log};

type P = *mut c_void;
type Guid = windows_sys::core::GUID;

const CLSID_MM_DEVICE_ENUMERATOR: Guid = Guid::from_u128(0xbcde0395_e52f_467c_8e3d_c4579291692e);
const IID_IAUDIO_CLIENT: Guid = Guid::from_u128(0x1cb9ad4c_dbfa_4c32_b178_c2f568a703b2);
const IID_IAUDIO_RENDER_CLIENT: Guid = Guid::from_u128(0xf294acfc_3146_4483_a7bf_addca7c260e2);

const SHAREMODE_SHARED: i32 = 0;
const SHAREMODE_EXCLUSIVE: i32 = 1;
const STREAMFLAGS_AUTOCONVERTPCM: u32 = 0x8000_0000;
const STREAMFLAGS_SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
const BUFFERFLAGS_SILENT: u32 = 0x2;
const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;

fn same(a: &Guid, b: &Guid) -> bool {
    (a.data1, a.data2, a.data3, a.data4) == (b.data1, b.data2, b.data3, b.data4)
}

#[repr(C)]
struct WaveFormat {
    tag: u16,
    channels: u16,
    rate: u32,
    bytes_per_sec: u32,
    block_align: u16,
    bits: u16,
    extra: u16,
}

/// PCM or float (the shared engine's converter takes those, not a bitstream).
unsafe fn pcm_or_float(fmt: *const WaveFormat) -> bool {
    let Some(f) = (unsafe { fmt.as_ref() }) else { return false };
    match f.tag {
        WAVE_FORMAT_PCM | WAVE_FORMAT_IEEE_FLOAT => true,
        // WAVEFORMATEXTENSIBLE: the sub-format's first u32 (PCM 1, float 3) after 22 bytes
        WAVE_FORMAT_EXTENSIBLE if f.extra >= 22 => {
            let sub = unsafe { std::ptr::read_unaligned((fmt as *const u8).add(18 + 6) as *const u32) };
            sub == 1 || sub == 3
        }
        _ => false,
    }
}

/// A stream moved from exclusive to shared mode.
struct Stream {
    frame_bytes: usize,
    rate: u32,
    /// The buffer size reported to the game (one device period), 0 until asked.
    reported: u32,
    /// The device's real buffer size.
    device_frames: u32,
    fifo: Vec<u8>,
    /// Where the game's current `GetBuffer` writes in the FIFO.
    pending: usize,
}

/// Redirected streams by IAudioClient.
static STREAMS: Mutex<Option<HashMap<usize, Stream>>> = Mutex::new(None);
/// The IAudioClient of each IAudioRenderClient handed out for a redirected stream.
static RENDERS: Mutex<Option<HashMap<usize, usize>>> = Mutex::new(None);

fn with_stream<R>(client: usize, f: impl FnOnce(&mut Stream) -> R) -> Option<R> {
    STREAMS.lock().unwrap().as_mut()?.get_mut(&client).map(f)
}

fn client_of(render: usize) -> Option<usize> {
    RENDERS.lock().unwrap().as_ref()?.get(&render).copied()
}

/// Original functions: CoCreateInstance, GetDefaultAudioEndpoint, Activate, then the
/// IAudioClient methods at 16 + their vtable index (3..=14), the render client's GetBuffer and
/// ReleaseBuffer.
static ORIG: [AtomicUsize; 40] = [const { AtomicUsize::new(0) }; 40];
const CO_CREATE: usize = 0;
const GET_ENDPOINT: usize = 1;
const ACTIVATE: usize = 2;
const CLIENT: usize = 16;
const GET_BUFFER: usize = 32;
const RELEASE_BUFFER: usize = 33;

fn orig(i: usize) -> usize {
    ORIG[i].load(Ordering::Relaxed)
}

/// Points vtable entry `index` of `object` at `f`, the original kept in `ORIG[slot]` (once).
unsafe fn patch(object: P, index: usize, f: usize, slot: usize) {
    if object.is_null() || orig(slot) != 0 {
        return;
    }
    let vtable = unsafe { *(object as *const *mut usize) };
    let entry = unsafe { vtable.add(index) };
    let mut old = 0;
    unsafe {
        if VirtualProtect(entry.cast(), size_of::<usize>(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return;
        }
        ORIG[slot].store(*entry, Ordering::Relaxed);
        *entry = f;
        VirtualProtect(entry.cast(), size_of::<usize>(), old, &mut old);
    }
}

pub fn init() {
    if !std::env::var("WAL_WASAPI_SHARED").is_ok_and(|v| v.trim() == "1") {
        return;
    }
    *STREAMS.lock().unwrap() = Some(HashMap::new());
    *RENDERS.lock().unwrap() = Some(HashMap::new());
    match unsafe { iat::hook("ole32.dll", "CoCreateInstance", co_create_instance as *const () as usize) } {
        Some(o) => {
            ORIG[CO_CREATE].store(o, Ordering::Relaxed);
            log!("wasapi: exclusive mode streams played in shared mode");
        }
        None => log!("wasapi: CoCreateInstance import not found, exclusive mode kept"),
    }
}

unsafe extern "system" fn co_create_instance(clsid: *const Guid, outer: P, ctx: u32, iid: *const Guid, out: *mut P) -> i32 {
    let original: unsafe extern "system" fn(*const Guid, P, u32, *const Guid, *mut P) -> i32 = unsafe { std::mem::transmute(orig(CO_CREATE)) };
    let hr = unsafe { original(clsid, outer, ctx, iid, out) };
    if hr >= 0 && !clsid.is_null() && same(unsafe { &*clsid }, &CLSID_MM_DEVICE_ENUMERATOR) && !out.is_null() {
        // IMMDeviceEnumerator::GetDefaultAudioEndpoint
        unsafe { patch(*out, 4, get_default_audio_endpoint as *const () as usize, GET_ENDPOINT) };
    }
    hr
}

unsafe extern "system" fn get_default_audio_endpoint(this: P, flow: i32, role: i32, out: *mut P) -> i32 {
    let original: unsafe extern "system" fn(P, i32, i32, *mut P) -> i32 = unsafe { std::mem::transmute(orig(GET_ENDPOINT)) };
    let hr = unsafe { original(this, flow, role, out) };
    if hr >= 0 && !out.is_null() {
        // IMMDevice::Activate
        unsafe { patch(*out, 3, activate as *const () as usize, ACTIVATE) };
    }
    hr
}

unsafe extern "system" fn activate(this: P, iid: *const Guid, ctx: u32, params: P, out: *mut P) -> i32 {
    let original: unsafe extern "system" fn(P, *const Guid, u32, P, *mut P) -> i32 = unsafe { std::mem::transmute(orig(ACTIVATE)) };
    let hr = unsafe { original(this, iid, ctx, params, out) };
    if hr >= 0 && !iid.is_null() && same(unsafe { &*iid }, &IID_IAUDIO_CLIENT) && !out.is_null() {
        let client = unsafe { *out };
        unsafe {
            patch(client, 3, initialize as *const () as usize, CLIENT + 3);
            patch(client, 4, get_buffer_size as *const () as usize, CLIENT + 4);
            patch(client, 6, get_current_padding as *const () as usize, CLIENT + 6);
            patch(client, 7, is_format_supported as *const () as usize, CLIENT + 7);
            patch(client, 14, get_service as *const () as usize, CLIENT + 14);
        }
    }
    hr
}

unsafe extern "system" fn is_format_supported(this: P, mode: i32, fmt: *const WaveFormat, closest: *mut P) -> i32 {
    if mode == SHAREMODE_EXCLUSIVE && unsafe { pcm_or_float(fmt) } {
        if !closest.is_null() {
            unsafe { *closest = std::ptr::null_mut() };
        }
        return 0;
    }
    let original: unsafe extern "system" fn(P, i32, *const WaveFormat, *mut P) -> i32 = unsafe { std::mem::transmute(orig(CLIENT + 7)) };
    unsafe { original(this, mode, fmt, closest) }
}

unsafe extern "system" fn initialize(this: P, mode: i32, flags: u32, duration: i64, period: i64, fmt: *const WaveFormat, session: *const Guid) -> i32 {
    let original: unsafe extern "system" fn(P, i32, u32, i64, i64, *const WaveFormat, *const Guid) -> i32 =
        unsafe { std::mem::transmute(orig(CLIENT + 3)) };
    if mode != SHAREMODE_EXCLUSIVE || !unsafe { pcm_or_float(fmt) } {
        return unsafe { original(this, mode, flags, duration, period, fmt, session) };
    }
    let flags = flags | STREAMFLAGS_AUTOCONVERTPCM | STREAMFLAGS_SRC_DEFAULT_QUALITY;
    let hr = unsafe { original(this, SHAREMODE_SHARED, flags, duration, 0, fmt, session) };
    let f = unsafe { &*fmt };
    log!(
        "wasapi: exclusive stream ({} Hz, {} channels, {} bits, {} ms) opened shared: {hr:#x}",
        f.rate,
        f.channels,
        f.bits,
        duration / 10_000
    );
    if hr >= 0
        && let Some(streams) = STREAMS.lock().unwrap().as_mut()
    {
        streams.insert(
            this as usize,
            Stream { frame_bytes: f.block_align.max(1) as usize, rate: f.rate, reported: 0, device_frames: 0, fifo: Vec::new(), pending: 0 },
        );
    }
    hr
}

/// One device period, in the stream's frames.
unsafe fn period_frames(client: P, rate: u32) -> u32 {
    let get_period: unsafe extern "system" fn(P, *mut i64, *mut i64) -> i32 = unsafe { std::mem::transmute(*(*(client as *const *const usize)).add(9)) };
    let mut period = 0i64;
    if unsafe { get_period(client, &mut period, std::ptr::null_mut()) } < 0 || period <= 0 {
        return 0;
    }
    (period as u64 * rate as u64 / 10_000_000) as u32
}

unsafe extern "system" fn get_buffer_size(this: P, frames: *mut u32) -> i32 {
    let original: unsafe extern "system" fn(P, *mut u32) -> i32 = unsafe { std::mem::transmute(orig(CLIENT + 4)) };
    let hr = unsafe { original(this, frames) };
    if hr < 0 || frames.is_null() {
        return hr;
    }
    let device = unsafe { *frames };
    let rate = with_stream(this as usize, |s| s.rate);
    if let Some(rate) = rate {
        let period = unsafe { period_frames(this, rate) };
        let reported = if period > 0 && period < device { period } else { device };
        with_stream(this as usize, |s| {
            s.reported = reported;
            s.device_frames = device;
        });
        unsafe { *frames = reported };
    }
    hr
}

unsafe extern "system" fn get_current_padding(this: P, padding: *mut u32) -> i32 {
    let virtual_padding = with_stream(this as usize, |s| {
        let pending = (s.fifo.len() / s.frame_bytes) as u32;
        if s.reported > 0 { pending.min(s.reported) } else { pending }
    });
    if let Some(p) = virtual_padding
        && !padding.is_null()
    {
        unsafe { *padding = p };
        return 0;
    }
    let original: unsafe extern "system" fn(P, *mut u32) -> i32 = unsafe { std::mem::transmute(orig(CLIENT + 6)) };
    unsafe { original(this, padding) }
}

unsafe extern "system" fn get_service(this: P, iid: *const Guid, out: *mut P) -> i32 {
    let original: unsafe extern "system" fn(P, *const Guid, *mut P) -> i32 = unsafe { std::mem::transmute(orig(CLIENT + 14)) };
    let hr = unsafe { original(this, iid, out) };
    if hr >= 0 && !iid.is_null() && same(unsafe { &*iid }, &IID_IAUDIO_RENDER_CLIENT) && !out.is_null() {
        let render = unsafe { *out };
        unsafe {
            patch(render, 3, get_buffer as *const () as usize, GET_BUFFER);
            patch(render, 4, release_buffer as *const () as usize, RELEASE_BUFFER);
        }
        if with_stream(this as usize, |_| ()).is_some()
            && let Some(renders) = RENDERS.lock().unwrap().as_mut()
        {
            renders.insert(render as usize, this as usize);
        }
    }
    hr
}

/// Gives the device what its shared buffer has room for, from the FIFO.
unsafe fn drain(render: P, client: usize) {
    let padding_fn: unsafe extern "system" fn(P, *mut u32) -> i32 = unsafe { std::mem::transmute(orig(CLIENT + 6)) };
    let get: unsafe extern "system" fn(P, u32, *mut *mut u8) -> i32 = unsafe { std::mem::transmute(orig(GET_BUFFER)) };
    let release: unsafe extern "system" fn(P, u32, u32) -> i32 = unsafe { std::mem::transmute(orig(RELEASE_BUFFER)) };
    let mut guard = STREAMS.lock().unwrap();
    let Some(s) = guard.as_mut().and_then(|m| m.get_mut(&client)) else { return };
    if s.device_frames == 0 {
        let size_fn: unsafe extern "system" fn(P, *mut u32) -> i32 = unsafe { std::mem::transmute(orig(CLIENT + 4)) };
        let mut n = 0;
        if unsafe { size_fn(client as P, &mut n) } < 0 || n == 0 {
            return;
        }
        s.device_frames = n;
    }
    let pending = (s.fifo.len() / s.frame_bytes) as u32;
    let mut padding = 0;
    if pending == 0 || unsafe { padding_fn(client as P, &mut padding) } < 0 {
        return;
    }
    let free = s.device_frames.saturating_sub(padding);
    let n = pending.min(free);
    if n == 0 {
        // the game is ahead of the device: keep at most a few periods queued
        let keep = s.reported.max(1) as usize * 8 * s.frame_bytes;
        if s.fifo.len() > keep {
            let drop = s.fifo.len() - keep;
            s.fifo.drain(..drop - drop % s.frame_bytes);
        }
        return;
    }
    let mut dev = std::ptr::null_mut();
    if unsafe { get(render, n, &mut dev) } < 0 || dev.is_null() {
        return;
    }
    let bytes = n as usize * s.frame_bytes;
    unsafe { std::ptr::copy_nonoverlapping(s.fifo.as_ptr(), dev, bytes) };
    unsafe { release(render, n, 0) };
    s.fifo.drain(..bytes);
}

unsafe extern "system" fn get_buffer(this: P, frames: u32, data: *mut *mut u8) -> i32 {
    let Some(client) = client_of(this as usize) else {
        let original: unsafe extern "system" fn(P, u32, *mut *mut u8) -> i32 = unsafe { std::mem::transmute(orig(GET_BUFFER)) };
        return unsafe { original(this, frames, data) };
    };
    unsafe { drain(this, client) };
    let ptr = with_stream(client, |s| {
        s.pending = s.fifo.len();
        s.fifo.resize(s.pending + frames as usize * s.frame_bytes, 0);
        // the FIFO does not move until ReleaseBuffer (no other write in between)
        unsafe { s.fifo.as_mut_ptr().add(s.pending) }
    });
    match ptr {
        Some(p) if !data.is_null() => {
            unsafe { *data = p };
            0
        }
        _ => 0x8000_4003u32 as i32, // E_POINTER
    }
}

unsafe extern "system" fn release_buffer(this: P, frames: u32, flags: u32) -> i32 {
    let Some(client) = client_of(this as usize) else {
        let original: unsafe extern "system" fn(P, u32, u32) -> i32 = unsafe { std::mem::transmute(orig(RELEASE_BUFFER)) };
        return unsafe { original(this, frames, flags) };
    };
    with_stream(client, |s| {
        let end = (s.pending + frames as usize * s.frame_bytes).min(s.fifo.len());
        if flags & BUFFERFLAGS_SILENT != 0 {
            s.fifo[s.pending..end].fill(0);
        }
        s.fifo.truncate(end);
    });
    unsafe { drain(this, client) };
    0
}

