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
//!
//! `WAL_PACKED_IMPORTS=<dll>,...` (profile `env`): the executable's import table is a
//! packer's, its code calls these DLLs through the packer's stubs (Teratoma). Their hooks patch the
//! functions themselves: Wine's hot patch prologue (kernelbase's function for kernel32's,
//! where kernel32's ends up), every caller redirected; a function without one keeps its
//! callers, the image's pointers to it are replaced (an unpacked import table slot).

use std::sync::{Mutex, OnceLock};

use crate::log;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryA};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE, VirtualProtect, VirtualQuery};

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
        if packed(dll) && base == GetModuleHandleW(std::ptr::null()) as usize {
            return hook_packed(base, dll, function, replacement);
        }
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

/// `WAL_PACKED_IMPORTS` lists `dll`: see the module documentation.
fn packed(dll: &str) -> bool {
    static PACKED: OnceLock<Vec<String>> = OnceLock::new();
    let list = PACKED.get_or_init(|| std::env::var("WAL_PACKED_IMPORTS").map(|v| v.split(',').map(|d| d.trim().to_ascii_lowercase()).collect()).unwrap_or_default());
    list.iter().any(|d| dll.eq_ignore_ascii_case(d))
}

/// A hook of a packed executable: `dll!function` patched where every caller reaches it.
unsafe fn hook_packed(base: usize, dll: &str, function: Import, replacement: usize) -> Option<usize> {
    unsafe {
        let real = resolve(dll, function);
        if real == 0 {
            return None;
        }
        // kernel32's functions end in kernelbase's (Wine): the one all callers go through
        let target = match function {
            Import::Name(_) if dll.eq_ignore_ascii_case("kernel32.dll") => Some(resolve("kernelbase.dll", function)).filter(|&f| f != 0 && hot_patchable(f)).unwrap_or(real),
            _ => real,
        };
        if let Some(original) = hot_patch(target, replacement) {
            log!("iat: {dll}!{function} hot patched (packed executable)");
            return Some(original);
        }
        // the pointers the image holds to it (or to a previous hook's replacement of it)
        let previous = HOOKED.lock().unwrap().iter().rev().find(|(r, _)| *r == real).map(|(_, h)| *h).unwrap_or(real);
        let nt = base + read::<u32>(base, 0x3C) as usize;
        let size = read::<u32>(nt + 24, 56) as usize; // SizeOfImage
        let mut slots = 0;
        let mut at = base;
        while at < base + size {
            let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            if VirtualQuery(at as *const _, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) == 0 {
                break;
            }
            let end = (info.BaseAddress as usize + info.RegionSize).min(base + size);
            if info.State == MEM_COMMIT && info.Protect & (PAGE_NOACCESS | PAGE_GUARD) == 0 {
                let mut p = at.next_multiple_of(size_of::<usize>());
                while p + size_of::<usize>() <= end {
                    if (p as *const usize).read() == previous {
                        let mut old = 0;
                        VirtualProtect(p as *const _, size_of::<usize>(), PAGE_READWRITE, &mut old);
                        (p as *mut usize).write(replacement);
                        VirtualProtect(p as *const _, size_of::<usize>(), old, &mut old);
                        slots += 1;
                    }
                    p += size_of::<usize>();
                }
            }
            at = end;
        }
        if slots == 0 {
            return None;
        }
        HOOKED.lock().unwrap().push((real, replacement));
        log!("iat: {dll}!{function}: {slots} pointer(s) in the image replaced (packed executable)");
        Some(previous)
    }
}

/// Wine's hot patch prologue at `f`: 5 padding bytes, then `mov edi,edi` (or a 2 byte nop),
/// or one of our hot patches.
unsafe fn hot_patchable(f: usize) -> bool {
    let code = unsafe { std::slice::from_raw_parts((f - 5) as *const u8, 7) };
    code[..5].iter().all(|&b| b == 0xCC || b == 0x90) && (code[5..] == [0x8B, 0xFF] || code[5..] == [0x66, 0x90]) || code[0] == 0xE9 && code[5..] == [0xEB, 0xF9]
}

/// Hot patch of the function at `f`: a jump to `hook` in its padding, a short jump to it at
/// its entry. Returns the previous function: past its 2 byte prologue, or the hook already
/// installed there (chained, as import hooks are).
unsafe fn hot_patch(f: usize, hook: usize) -> Option<usize> {
    unsafe {
        if !hot_patchable(f) {
            return None;
        }
        let code = (f - 5) as *mut u8;
        let previous = if code.read() == 0xE9 { f.wrapping_add(((f - 4) as *const i32).read_unaligned() as isize as usize) } else { f + 2 };
        let mut old = 0;
        VirtualProtect(code.cast(), 7, PAGE_EXECUTE_READWRITE, &mut old);
        code.write(0xE9);
        ((f - 4) as *mut i32).write_unaligned(hook.wrapping_sub(f) as i32);
        // jmp $-5, written last: one 2 byte store
        (f as *mut u16).write_unaligned(0xF9EB);
        VirtualProtect(code.cast(), 7, old, &mut old);
        Some(previous)
    }
}
