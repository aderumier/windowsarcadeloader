//! NESiCA crypto server (port of WindowsLoader's CryptoPipe).
//!
//! Encrypted NESiCA games get their content key from the cabinet's crypto service over the
//! byte-mode pipe `\\.\pipe\TtxAppCtyptPipe` (sic). The service holds a per-game RSA private
//! key; the game sends:
//! - `FF FD <u32 size> <PUBLICKEYBLOB>`: its own RSA public key,
//! - `FF FE <u32 size> <SIMPLEBLOB>`: the content key, encrypted for the service key.
//!
//! In the size field `FF FF` stands for a literal `FF`. The service decrypts the content
//! key and returns it encrypted for the game's public key: `<u32 size><blob>`, `<u32 0>` on
//! error. Most games want a SIMPLEBLOB; KOF XIII Climax wants a PLAINTEXTKEYBLOB encrypted
//! with the game's key (`WAL_NESICA_CRYPT_REPLY=plaintext`).
//!
//! Wine fix: games import the reply with `CryptImportKey(SIMPLEBLOB, hPubKey = 0)`, which on
//! Windows decrypts with the container's key exchange key; Wine fails with
//! NTE_BAD_PUBLIC_KEY. The game's import is hooked to pass that key explicitly.
//!
//! Options: `WAL_NESICA_KEY` = built-in key name (see `keys.rs`) or key file (PRIVATEKEYBLOB,
//! relative to the game directory, e.g. `303002.key`); default `usf4` like WindowsLoader.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;

use wal_payload_common::{iat, log};
use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Cryptography::{
    AT_KEYEXCHANGE, CRYPT_EXPORTABLE, CRYPT_NEWKEYSET, CryptGetUserKey, CryptAcquireContextA, CryptDestroyKey, CryptEncrypt, CryptExportKey,
    CryptGetKeyParam, CryptImportKey, KP_KEYLEN, PLAINTEXTKEYBLOB, PROV_RSA_FULL, SIMPLEBLOB,
};
use windows_sys::Win32::Storage::FileSystem::{PIPE_ACCESS_DUPLEX, ReadFile, WriteFile};
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeA, DisconnectNamedPipe, PIPE_WAIT};

use crate::keys::KEYS;

const ERROR_BROKEN_PIPE: u32 = 109;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reply {
    SimpleBlob,
    /// KOF XIII Climax
    Plaintext,
}

struct Pipe(HANDLE);
unsafe impl Send for Pipe {}

/// A CryptoAPI key handle, destroyed on drop.
struct Key(usize);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { CryptDestroyKey(self.0) };
    }
}

fn load_key() -> Option<Vec<u8>> {
    let spec = std::env::var("WAL_NESICA_KEY").unwrap_or_else(|_| "usf4".into());
    if let Some((_, hex)) = KEYS.iter().find(|(name, _)| name.eq_ignore_ascii_case(&spec)) {
        log!("crypto: built-in key {spec}");
        return (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok()).collect();
    }
    match std::fs::read(&spec) {
        Ok(blob) => {
            log!("crypto: key file {spec} ({} bytes)", blob.len());
            Some(blob)
        }
        Err(e) => {
            log!("crypto: cannot read key '{spec}': {e}");
            None
        }
    }
}

pub(crate) fn start() {
    let reply = match std::env::var("WAL_NESICA_CRYPT_REPLY").as_deref() {
        Ok("plaintext") => Reply::Plaintext,
        _ => Reply::SimpleBlob,
    };
    let Some(blob) = load_key() else { return };
    if let Some(o) = unsafe { iat::hook("advapi32.dll", "CryptImportKey", game_import_key as *const () as usize) } {
        ORIG_IMPORT.store(o, Ordering::Relaxed);
    }
    thread::spawn(move || serve(&blob, reply));
}

static ORIG_IMPORT: AtomicUsize = AtomicUsize::new(0);

type ImportKeyFn = unsafe extern "system" fn(usize, *const u8, u32, usize, u32, *mut usize) -> i32;

/// The game's `CryptImportKey`: a SIMPLEBLOB without import key uses the container's key
/// exchange key (Wine does not).
unsafe extern "system" fn game_import_key(prov: usize, data: *const u8, len: u32, pubkey: usize, flags: u32, key: *mut usize) -> i32 {
    let original: ImportKeyFn = unsafe { std::mem::transmute(ORIG_IMPORT.load(Ordering::Relaxed)) };
    let simple_blob = !data.is_null() && len > 0 && unsafe { *data } as u32 == SIMPLEBLOB;
    if pubkey != 0 || !simple_blob {
        return unsafe { original(prov, data, len, pubkey, flags, key) };
    }
    let mut exchange = 0usize;
    if unsafe { CryptGetUserKey(prov, AT_KEYEXCHANGE, &mut exchange) } == 0 {
        log!("crypto: game has no key exchange key: {:#x}", unsafe { GetLastError() });
        return unsafe { original(prov, data, len, pubkey, flags, key) };
    }
    let ok = unsafe { original(prov, data, len, exchange, flags, key) };
    log!("crypto: game imported the content key: {}", if ok != 0 { "ok".into() } else { format!("failed {:#x}", unsafe { GetLastError() }) });
    unsafe { CryptDestroyKey(exchange) };
    ok
}

fn acquire() -> Option<usize> {
    let container = c"TypeXAppCrypt";
    let provider = c"Microsoft Base Cryptographic Provider v1.0";
    let mut prov = 0usize;
    for flags in [0, CRYPT_NEWKEYSET] {
        if unsafe { CryptAcquireContextA(&mut prov, container.as_ptr().cast(), provider.as_ptr().cast(), PROV_RSA_FULL, flags) } != 0 {
            return Some(prov);
        }
    }
    None
}

fn import(prov: usize, blob: &[u8], decrypt_with: usize, flags: u32) -> Option<Key> {
    let mut key = 0usize;
    let ok = unsafe { CryptImportKey(prov, blob.as_ptr(), blob.len() as u32, decrypt_with, flags, &mut key) };
    (ok != 0).then_some(Key(key))
}

fn serve(blob: &[u8], reply: Reply) {
    let Some(prov) = acquire() else {
        return log!("crypto: CryptAcquireContext failed: {}", unsafe { GetLastError() });
    };
    let Some(server_key) = import(prov, blob, 0, 0) else {
        return log!("crypto: cannot import the service key: {}", unsafe { GetLastError() });
    };
    let pipe = unsafe {
        CreateNamedPipeA(
            c"\\\\.\\pipe\\TtxAppCtyptPipe".as_ptr().cast(),
            PIPE_ACCESS_DUPLEX,
            PIPE_WAIT,
            1,
            0x400,
            0x400,
            1000,
            std::ptr::null(),
        )
    };
    if pipe == INVALID_HANDLE_VALUE {
        return log!("crypto: cannot create the pipe: {}", unsafe { GetLastError() });
    }
    let pipe = Pipe(pipe);
    log!("crypto: service ready");

    let mut client_key: Option<Key> = None;
    unsafe { ConnectNamedPipe(pipe.0, std::ptr::null_mut()) };
    loop {
        let Some((is_public_key, data)) = read_request(&pipe) else {
            // client gone: answer an error if it is still there, wait for the next one
            write(&pipe, &0u32.to_le_bytes());
            unsafe {
                DisconnectNamedPipe(pipe.0);
                ConnectNamedPipe(pipe.0, std::ptr::null_mut());
            }
            continue;
        };
        if is_public_key {
            client_key = import(prov, &data, 0, 0);
            log!("crypto: client public key {}", if client_key.is_some() { "imported" } else { "rejected" });
            continue;
        }
        let answer = match (&client_key, import(prov, &data, server_key.0, CRYPT_EXPORTABLE)) {
            (Some(client), Some(content)) => match reply {
                Reply::SimpleBlob => export(content.0, client.0, SIMPLEBLOB),
                Reply::Plaintext => plaintext_for(content.0, client.0),
            },
            _ => None,
        };
        log!("crypto: content key request -> {}", answer.as_ref().map_or("error".into(), |a| format!("{} bytes", a.len())));
        match answer {
            Some(a) => {
                write(&pipe, &(a.len() as u32).to_le_bytes());
                write(&pipe, &a);
            }
            None => write(&pipe, &0u32.to_le_bytes()),
        }
    }
}

fn export(key: usize, exp_key: usize, blob_type: u32) -> Option<Vec<u8>> {
    let mut len = 0u32;
    if unsafe { CryptExportKey(key, exp_key, blob_type, 0, std::ptr::null_mut(), &mut len) } == 0 {
        return None;
    }
    let mut out = vec![0u8; len as usize];
    if unsafe { CryptExportKey(key, exp_key, blob_type, 0, out.as_mut_ptr(), &mut len) } == 0 {
        return None;
    }
    out.truncate(len as usize);
    Some(out)
}

/// KOF XIII Climax: PLAINTEXTKEYBLOB encrypted directly with the client's RSA key.
fn plaintext_for(content: usize, client: usize) -> Option<Vec<u8>> {
    let plain = export(content, 0, PLAINTEXTKEYBLOB)?;
    let mut bits = 0u32;
    let mut bits_len = 4u32;
    let mut capacity = 256usize;
    if unsafe { CryptGetKeyParam(client, KP_KEYLEN, (&mut bits as *mut u32).cast(), &mut bits_len, 0) } != 0 && bits != 0 {
        capacity = bits.div_ceil(8) as usize;
    }
    let mut buf = plain.clone();
    buf.resize(capacity.max(plain.len()), 0);
    let mut len = plain.len() as u32;
    if unsafe { CryptEncrypt(client, 0, 1, 0, buf.as_mut_ptr(), &mut len, buf.len() as u32) } == 0 {
        return None;
    }
    buf.truncate(len as usize);
    Some(buf)
}

fn read_byte(pipe: &Pipe) -> Option<u8> {
    let mut b = 0u8;
    let mut read = 0u32;
    loop {
        if unsafe { ReadFile(pipe.0, &mut b, 1, &mut read, std::ptr::null_mut()) } != 0 && read == 1 {
            return Some(b);
        }
        if unsafe { GetLastError() } == ERROR_BROKEN_PIPE || read == 0 {
            return None;
        }
    }
}

/// Reads one `FF FD|FE <u32 size> <data>` request. Returns (is public key, data).
fn read_request(pipe: &Pipe) -> Option<(bool, Vec<u8>)> {
    // mode of a frame start found while reading the previous size
    let mut restart: Option<bool> = None;
    'frame: loop {
        let is_public_key = match restart.take() {
            Some(mode) => mode,
            // synchronize on FF FD / FF FE
            None => loop {
                if read_byte(pipe)? != 0xFF {
                    continue;
                }
                match read_byte(pipe)? {
                    0xFD => break true,
                    0xFE => break false,
                    _ => continue,
                }
            },
        };
        let mut size = 0u32;
        for i in 0..4 {
            let mut b = read_byte(pipe)?;
            if b == 0xFF {
                b = match read_byte(pipe)? {
                    0xFD => {
                        restart = Some(true);
                        continue 'frame;
                    }
                    0xFE => {
                        restart = Some(false);
                        continue 'frame;
                    }
                    // FF FF: escaped literal
                    other => other,
                };
            }
            size |= (b as u32) << (8 * i);
        }
        if size > 0x10000 {
            log!("crypto: request size {size} too large");
            return None;
        }
        let mut data = vec![0u8; size as usize];
        let mut got = 0usize;
        while got < data.len() {
            let mut read = 0u32;
            let ok = unsafe {
                ReadFile(pipe.0, data[got..].as_mut_ptr(), (data.len() - got) as u32, &mut read, std::ptr::null_mut())
            };
            if ok == 0 || read == 0 {
                return None;
            }
            got += read as usize;
        }
        return Some((is_public_key, data));
    }
}

fn write(pipe: &Pipe, data: &[u8]) {
    let mut written = 0u32;
    unsafe { WriteFile(pipe.0, data.as_ptr(), data.len() as u32, &mut written, std::ptr::null_mut()) };
}
