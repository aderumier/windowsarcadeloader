//! Debugging options: the P/Invoke functions a Unity IL2CPP game resolves
//! (`WAL_APM3_TRACE_PINVOKE=1`, `GameAssembly.dll`'s `GetProcAddress`), bytes of the game's
//! memory logged when they change (`WAL_APM3_PEEK`).

use std::time::Duration;

use wal_payload_common::log;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;


static REAL_GET_PROC: std::sync::OnceLock<usize> = std::sync::OnceLock::new();

unsafe extern "system" fn get_proc_address(module: usize, name: *const u8) -> usize {
    let real: unsafe extern "system" fn(usize, *const u8) -> usize = unsafe { std::mem::transmute(*REAL_GET_PROC.get().unwrap()) };
    if name as usize > 0xffff {
        let n = unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_string_lossy();
        log!("pinvoke: {n}");
    }
    unsafe { real(module, name) }
}

pub(crate) fn trace_pinvoke() {
    let module = unsafe { GetModuleHandleW(windows_sys::w!("GameAssembly.dll")) } as usize;
    if module != 0
        && let Some(real) = unsafe { wal_payload_common::iat::hook_module(module, "kernel32.dll", "GetProcAddress", get_proc_address as *const () as usize) }
    {
        let _ = REAL_GET_PROC.set(real);
        log!("pinvoke: GameAssembly.dll's GetProcAddress traced");
    }
}

/// `WAL_APM3_PEEK=<module>+<rva>,<offset>,...[;...]`: the byte at the end of each
/// pointer chain (the module's address plus rva read as a pointer, plus each offset read as a
/// pointer but the last) logged when it changes.
pub(crate) fn peek() {
    let Some(spec) = crate::env("WAL_APM3_PEEK") else { return };
    std::thread::spawn(move || {
        let mut last: Vec<Option<u8>> = Vec::new();
        loop {
            std::thread::sleep(Duration::from_millis(50));
            for (i, chain) in spec.split(';').enumerate() {
                let value = (|| {
                    let (module, rest) = chain.split_once('+')?;
                    let mut parts = rest.split(',').map(|p| usize::from_str_radix(p.trim().trim_start_matches("0x"), 16).ok());
                    let wide: Vec<u16> = module.encode_utf16().chain(Some(0)).collect();
                    let base = unsafe { GetModuleHandleW(wide.as_ptr()) } as usize;
                    if base == 0 {
                        return None;
                    }
                    let mut address = base + parts.next()??;
                    for offset in parts {
                        let p = unsafe { *(address as *const usize) };
                        if p == 0 {
                            return None;
                        }
                        address = p + offset?;
                    }
                    Some(unsafe { *(address as *const u8) })
                })();
                if last.len() <= i {
                    last.push(None);
                }
                if last[i] != value {
                    log!("peek: {chain} = {value:?}");
                    last[i] = value;
                }
            }
        }
    });
}
