//! Background music from a folder of MP3 tracks (`WAL_ARCADEPC_MUSIC=<folder>`, relative to the
//! game's folder): the tracks in name order, again from the first after the last, for the games
//! whose sound hardware is not emulated (Xtom 3D: the cabinet's YMZ280B sound board, its
//! soundtrack ripped in the dump's `music\`). Decoded by the runner's MP3 codec (`l3codeca.acm`,
//! Wine's mpg123, through `msacm32`), played with `waveOut` (the common volume, `WAL_VOLUME`,
//! applies: the process's audio session).

use std::time::Duration;

use wal_payload_common::log;
use windows_sys::Win32::Media::Audio::{
    acmStreamClose, acmStreamConvert, acmStreamOpen, acmStreamPrepareHeader, acmStreamUnprepareHeader, waveOutOpen,
    waveOutPrepareHeader, waveOutUnprepareHeader, waveOutWrite, ACMSTREAMHEADER, HACMSTREAM, HWAVEOUT, WAVEFORMATEX,
    WAVEHDR, WAVE_MAPPER, WHDR_DONE,
};

/// `MPEGLAYER3WAVEFORMAT` (mmreg.h).
#[repr(C, packed)]
struct Mp3Format {
    wfx: WAVEFORMATEX,
    id: u16,
    flags: u32,
    block_size: u16,
    frames_per_block: u16,
    codec_delay: u16,
}

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_MPEGLAYER3: u16 = 0x55;
const MPEGLAYER3_ID_MPEG: u16 = 1;
const MPEGLAYER3_FLAG_PADDING_OFF: u32 = 2;
/// The MP3 bytes given to the codec at a time, and the room for their PCM (a 32 kbit/s frame's
/// 44 times its size, the lowest bit rate's).
const SOURCE_CHUNK: usize = 4096;
const DEST_CHUNK: usize = SOURCE_CHUNK * 48;
/// `waveOut`'s queue: buffers of a quarter second each.
const BUFFERS: usize = 6;

/// A track's sample rate and channels, from its first frame's header (after an ID3v2 tag).
fn stream_format(data: &[u8]) -> Option<(u32, u16, u32)> {
    let mut at = 0;
    if data.len() > 10 && &data[..3] == b"ID3" {
        let size = data[6..10].iter().fold(0usize, |s, &b| (s << 7) | (b & 0x7f) as usize);
        at = 10 + size;
    }
    while at + 4 <= data.len() {
        let h = &data[at..at + 4];
        if h[0] == 0xff && h[1] & 0xe0 == 0xe0 && (h[1] >> 1) & 3 == 1 {
            let version = (h[1] >> 3) & 3; // 3: MPEG 1, 2: MPEG 2, 0: MPEG 2.5
            let rate_index = ((h[2] >> 2) & 3) as usize;
            let bitrate_index = (h[2] >> 4) as usize;
            if version != 1 && rate_index < 3 && bitrate_index != 0 && bitrate_index != 15 {
                let rate = [44100, 48000, 32000][rate_index] >> [2, 0, 1, 0][version as usize];
                let kbits = if version == 3 {
                    [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320][bitrate_index]
                } else {
                    [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160][bitrate_index]
                };
                let channels = if h[3] >> 6 == 3 { 1 } else { 2 };
                return Some((rate, channels, kbits));
            }
        }
        at += 1;
    }
    None
}

/// The output: `waveOut` and its buffers, written in turn.
struct Output {
    device: HWAVEOUT,
    format: (u32, u16),
    headers: Vec<Box<WAVEHDR>>,
    buffers: Vec<Vec<u8>>,
    next: usize,
}

impl Output {
    fn open(rate: u32, channels: u16) -> Option<Output> {
        let format = pcm_format(rate, channels);
        let mut device: HWAVEOUT = std::ptr::null_mut();
        let r = unsafe { waveOutOpen(&mut device, WAVE_MAPPER, &format, 0, 0, 0) };
        if r != 0 {
            log!("music: waveOutOpen {rate} Hz {channels} channels failed ({r})");
            return None;
        }
        let headers = (0..BUFFERS)
            .map(|_| {
                let mut h: Box<WAVEHDR> = Box::new(unsafe { std::mem::zeroed() });
                h.dwFlags = WHDR_DONE;
                h
            })
            .collect();
        Some(Output { device, format: (rate, channels), headers, buffers: vec![Vec::new(); BUFFERS], next: 0 })
    }

    /// The bytes of a quarter second.
    fn quarter(&self) -> usize {
        (self.format.0 as usize * self.format.1 as usize * 2 / 4) & !3
    }

    /// `pcm` queued once a buffer is free.
    fn play(&mut self, pcm: Vec<u8>) {
        let i = self.next;
        self.next = (i + 1) % BUFFERS;
        while unsafe { std::ptr::read_volatile(std::ptr::addr_of!(self.headers[i].dwFlags)) } & WHDR_DONE == 0 {
            std::thread::sleep(Duration::from_millis(20));
        }
        let size = std::mem::size_of::<WAVEHDR>() as u32;
        let header = &mut *self.headers[i];
        if !header.lpData.is_null() {
            unsafe { waveOutUnprepareHeader(self.device, header, size) };
        }
        self.buffers[i] = pcm;
        *header = unsafe { std::mem::zeroed() };
        header.lpData = self.buffers[i].as_mut_ptr();
        header.dwBufferLength = self.buffers[i].len() as u32;
        unsafe {
            waveOutPrepareHeader(self.device, header, size);
            waveOutWrite(self.device, header, size);
        }
    }
}

fn pcm_format(rate: u32, channels: u16) -> WAVEFORMATEX {
    WAVEFORMATEX {
        wFormatTag: WAVE_FORMAT_PCM,
        nChannels: channels,
        nSamplesPerSec: rate,
        nAvgBytesPerSec: rate * channels as u32 * 2,
        nBlockAlign: channels * 2,
        wBitsPerSample: 16,
        cbSize: 0,
    }
}

/// One track decoded and queued (`output` opened, or reopened, at its format).
fn play_track(path: &std::path::Path, output: &mut Option<Output>) -> Result<(), String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let (rate, channels, kbits) = stream_format(&data).ok_or("no MP3 frame")?;
    if output.as_ref().map(|o| o.format) != Some((rate, channels)) {
        *output = Output::open(rate, channels);
    }
    let output = output.as_mut().ok_or("no audio output")?;
    let mut source = Mp3Format {
        wfx: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_MPEGLAYER3,
            nChannels: channels,
            nSamplesPerSec: rate,
            nAvgBytesPerSec: kbits * 1000 / 8,
            nBlockAlign: 1,
            wBitsPerSample: 0,
            cbSize: 12,
        },
        id: MPEGLAYER3_ID_MPEG,
        flags: MPEGLAYER3_FLAG_PADDING_OFF,
        block_size: (144 * kbits * 1000 / rate) as u16,
        frames_per_block: 1,
        codec_delay: 1393,
    };
    let mut dest = pcm_format(rate, channels);
    let mut stream: HACMSTREAM = std::ptr::null_mut();
    let r = unsafe {
        acmStreamOpen(&mut stream, std::ptr::null_mut(), &mut source.wfx, &mut dest, std::ptr::null_mut(), 0, 0, 0)
    };
    if r != 0 {
        return Err(format!("acmStreamOpen failed ({r}): no MP3 codec"));
    }
    let mut src = vec![0u8; SOURCE_CHUNK];
    let mut dst = vec![0u8; DEST_CHUNK];
    let mut header: ACMSTREAMHEADER = unsafe { std::mem::zeroed() };
    header.cbStruct = std::mem::size_of::<ACMSTREAMHEADER>() as u32;
    header.pbSrc = src.as_mut_ptr();
    header.cbSrcLength = SOURCE_CHUNK as u32;
    header.pbDst = dst.as_mut_ptr();
    header.cbDstLength = DEST_CHUNK as u32;
    unsafe { acmStreamPrepareHeader(stream, &mut header, 0) };
    let quarter = output.quarter();
    let mut pending = Vec::with_capacity(quarter * 2);
    for chunk in data.chunks(SOURCE_CHUNK) {
        src[..chunk.len()].copy_from_slice(chunk);
        header.cbSrcLength = chunk.len() as u32;
        let r = unsafe { acmStreamConvert(stream, &mut header, 0) };
        if r != 0 {
            break;
        }
        pending.extend_from_slice(&dst[..header.cbDstLengthUsed as usize]);
        while pending.len() >= quarter {
            let rest = pending.split_off(quarter);
            output.play(std::mem::replace(&mut pending, rest));
        }
    }
    if !pending.is_empty() {
        output.play(pending);
    }
    header.cbSrcLength = SOURCE_CHUNK as u32;
    unsafe {
        acmStreamUnprepareHeader(stream, &mut header, 0);
        acmStreamClose(stream, 0);
    }
    Ok(())
}

/// The tracks of `WAL_ARCADEPC_MUSIC`'s folder played in a loop, on a thread of their own.
pub fn init() {
    let Some(folder) = std::env::var("WAL_ARCADEPC_MUSIC").ok().filter(|v| !v.trim().is_empty()) else { return };
    let mut tracks: Vec<_> = match std::fs::read_dir(folder.trim()) {
        Ok(dir) => dir
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp3")))
            .collect(),
        Err(e) => {
            log!("music: {folder}: {e}");
            return;
        }
    };
    tracks.sort();
    if tracks.is_empty() {
        log!("music: no MP3 track in {folder}");
        return;
    }
    log!("music: {} tracks from {folder}", tracks.len());
    std::thread::spawn(move || {
        let mut output = None;
        loop {
            let mut played = false;
            for track in &tracks {
                match play_track(track, &mut output) {
                    Ok(()) => played = true,
                    Err(e) => log!("music: {}: {e}", track.display()),
                }
            }
            if !played {
                return;
            }
        }
    });
}
