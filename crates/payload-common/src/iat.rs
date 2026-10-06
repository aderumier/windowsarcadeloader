//! Import Address Table hooks on the game executable.
//!
//! Only calls made by the game module itself are redirected (system DLLs keep the
//! original functions), which is what a game-specific emulation needs and stays safe
//! with Wine builtin DLLs: no code is patched. `hook_module` patches another module the
//! same way (e.g. the C runtime the game does its file I/O through).
//!
//! Imports without a lookup table (no OriginalFirstThunk, e.g. a rebuilt executable such as
//! Haunted Museum II's) have no names once loaded, only the resolved addresses: the slot is
//! found by its address, the function's (`GetProcAddress`) or a previous hook's replacement.

use std::sync::Mutex;

use crate::log;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryA};
use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};

const IMAGE_DIRECTORY_ENTRY_IMPORT: usize = 1;
const ORDINAL_FLAG: usize = 1 << (usize::BITS - 1);

unsafe fn read<T: Copy>(base: usize, offset: usize) -> T {
    unsafe { ((base + offset) as *const T).read_unaligned() }
}

unsafe fn c_str_eq(ptr: usize, name: &str) -> bool {
    let bytes = unsafe { std::ffi::CStr::from_ptr(ptr as *const std::ffi::c_char) }.to_bytes();
    bytes.eq_ignore_ascii_case(name.as_bytes())
}

/// Replaces the import `dll!function` of the game executable by `replacement`.
/// Returns the original function, or `None` when the game does not import it.
///
/// # Safety
/// `replacement` must be a function with the exact signature and calling convention of
/// the replaced import.
pub unsafe fn hook(dll: &str, function: &str, replacement: usize) -> Option<usize> {
    unsafe { hook_module(GetModuleHandleW(std::ptr::null()) as usize, dll, function, replacement) }
}

/// Same as [`hook`], on the module loaded at `base`.
///
/// # Safety
/// See [`hook`]; `base` must be a loaded module.
pub unsafe fn hook_module(base: usize, dll: &str, function: &str, replacement: usize) -> Option<usize> {
    unsafe { hook_import(base, dll, Import::Name(function), replacement) }
}

/// Same as [`hook_module`], for a function imported by ordinal (e.g. `xinput1_3.dll` #2).
///
/// # Safety
/// See [`hook`]; `base` must be a loaded module.
pub unsafe fn hook_ordinal(base: usize, dll: &str, ordinal: u16, replacement: usize) -> Option<usize> {
    unsafe { hook_import(base, dll, Import::Ordinal(ordinal), replacement) }
}

#[derive(Clone, Copy)]
enum Import<'a> {
    Name(&'a str),
    Ordinal(u16),
}

impl std::fmt::Display for Import<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Import::Name(n) => write!(f, "{n}"),
            Import::Ordinal(o) => write!(f, "#{o}"),
        }
    }
}

/// (real function, replacement) of the hooks made: a slot without a name holds one of them.
static HOOKED: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

/// Address of `dll!function` (the DLL is loaded: the game imports it).
unsafe fn resolve(dll: &str, function: Import) -> usize {
    let Ok(cdll) = std::ffi::CString::new(dll) else { return 0 };
    let module = unsafe { LoadLibraryA(cdll.as_ptr().cast()) };
    if module.is_null() {
        return 0;
    }
    let f = match function {
        Import::Name(name) => {
            let Ok(cname) = std::ffi::CString::new(name) else { return 0 };
            unsafe { GetProcAddress(module, cname.as_ptr().cast()) }
        }
        Import::Ordinal(o) => unsafe { GetProcAddress(module, o as usize as *const u8) },
    };
    f.map_or(0, |f| f as usize)
}

unsafe fn hook_import(base: usize, dll: &str, function: Import, replacement: usize) -> Option<usize> {
    unsafe {
        let (slot, recorded) = find_slot(base, dll, function)?;
        let ptr_size = size_of::<usize>();
        let mut old = 0;
        VirtualProtect(slot.cast(), ptr_size, PAGE_READWRITE, &mut old);
        let original = slot.read();
        slot.write(replacement);
        VirtualProtect(slot.cast(), ptr_size, old, &mut old);
        HOOKED.lock().unwrap().push((recorded.unwrap_or(original), replacement));
        log!("iat: hooked {dll}!{function}");
        Some(original)
    }
}

/// Address of the game's IAT slot of `dll!function`, without patching it.
///
/// # Safety
/// The game must import `dll!function`.
pub unsafe fn hook_addr(dll: &str, function: &str) -> Option<usize> {
    unsafe {
        find_slot(GetModuleHandleW(std::ptr::null()) as usize, dll, Import::Name(function)).map(|(s, _)| s as usize)
    }
}

/// The IAT slot of `dll!function` in the module loaded at `base`, with what to record in
/// `HOOKED` when patching it: the resolved address for imports without a lookup table (a slot
/// without a name once loaded), the original slot value otherwise.
unsafe fn find_slot(base: usize, dll: &str, function: Import) -> Option<(*mut usize, Option<usize>)> {
    unsafe {
        let nt = base + read::<u32>(base, 0x3C) as usize;
        let optional = nt + 24;
        let data_dirs = match read::<u16>(optional, 0) {
            0x10b => optional + 96,  // PE32
            0x20b => optional + 112, // PE32+
            _ => return None,
        };
        let import_rva = read::<u32>(data_dirs, IMAGE_DIRECTORY_ENTRY_IMPORT * 8) as usize;
        if import_rva == 0 {
            return None;
        }
        let ptr_size = size_of::<usize>();
        let mut desc = base + import_rva;
        loop {
            let name_rva = read::<u32>(desc, 12) as usize;
            if name_rva == 0 {
                return None;
            }
            if c_str_eq(base + name_rva, dll) {
                let lookup_rva = read::<u32>(desc, 0) as usize;
                let iat = base + read::<u32>(desc, 16) as usize;
                // no lookup table: match the resolved address (or a hook's replacement of it)
                let addresses: Vec<usize> = if lookup_rva == 0 {
                    let real = resolve(dll, function);
                    if real == 0 {
                        desc += 20;
                        continue;
                    }
                    let hooked = HOOKED.lock().unwrap();
                    std::iter::once(real).chain(hooked.iter().filter(|(r, _)| *r == real).map(|(_, h)| *h)).collect()
                } else {
                    Vec::new()
                };
                let mut i = 0;
                loop {
                    let entry = if lookup_rva == 0 { read::<usize>(iat, i * ptr_size) } else { read::<usize>(base + lookup_rva, i * ptr_size) };
                    if entry == 0 {
                        break;
                    }
                    let found = if lookup_rva == 0 {
                        addresses.contains(&entry)
                    } else {
                        match function {
                            // IMAGE_IMPORT_BY_NAME: u16 hint, then the name
                            Import::Name(name) => entry & ORDINAL_FLAG == 0 && c_str_eq(base + entry + 2, name),
                            Import::Ordinal(o) => entry & ORDINAL_FLAG != 0 && entry & 0xFFFF == o as usize,
                        }
                    };
                    if found {
                        let recorded = (lookup_rva == 0).then(|| resolve(dll, function)).filter(|r| *r != 0);
                        return Some(((iat + i * ptr_size) as *mut usize, recorded));
                    }
                    i += 1;
                }
            }
            desc += 20; // IMAGE_IMPORT_DESCRIPTOR
        }
    }
}
