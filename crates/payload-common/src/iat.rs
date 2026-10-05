//! Import Address Table hooks on the game executable.
//!
//! Only calls made by the game module itself are redirected (system DLLs keep the
//! original functions), which is what a game-specific emulation needs and stays safe
//! with Wine builtin DLLs: no code is patched. `hook_module` patches another module the
//! same way (e.g. the C runtime the game does its file I/O through).

use crate::log;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
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

unsafe fn hook_import(base: usize, dll: &str, function: Import, replacement: usize) -> Option<usize> {
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
                let lookup_rva = match read::<u32>(desc, 0) {
                    0 => read::<u32>(desc, 16), // no OriginalFirstThunk: names are in the IAT
                    rva => rva,
                } as usize;
                let iat = base + read::<u32>(desc, 16) as usize;
                let mut i = 0;
                loop {
                    let entry = read::<usize>(base + lookup_rva, i * ptr_size);
                    if entry == 0 {
                        break;
                    }
                    let found = match function {
                        // IMAGE_IMPORT_BY_NAME: u16 hint, then the name
                        Import::Name(name) => entry & ORDINAL_FLAG == 0 && c_str_eq(base + entry + 2, name),
                        Import::Ordinal(o) => entry & ORDINAL_FLAG != 0 && entry & 0xFFFF == o as usize,
                    };
                    if found {
                        let slot = (iat + i * ptr_size) as *mut usize;
                        let mut old = 0;
                        VirtualProtect(slot.cast(), ptr_size, PAGE_READWRITE, &mut old);
                        let original = slot.read();
                        slot.write(replacement);
                        VirtualProtect(slot.cast(), ptr_size, old, &mut old);
                        log!("iat: hooked {dll}!{function}");
                        return Some(original);
                    }
                    i += 1;
                }
            }
            desc += 20; // IMAGE_IMPORT_DESCRIPTOR
        }
    }
}
