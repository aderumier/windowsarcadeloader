//! `WAL_SEGANU_UNPACK=<rva>`: a packed 64-bit executable (Luigi's Mansion Arcade's Capcom
//! protector, `.capcom0` / `.capcom1` sections) started at its original entry point once it is
//! unpacked. The protector decrypts the game's sections and fills its imports, then checks its
//! environment and ends the process under Wine before jumping to the game. Its first look at
//! the process list (`NtQuerySystemInformation(SystemProcessInformation)`) comes after the
//! decryption: there the payload writes `WAL_SEGANU_UNPACK_PATCHES` (the `WAL_PATCHES` format:
//! the functions the protector replaced by jumps into its virtual machine, written back) and
//! calls the entry point at `<rva>`, on the protector's stack (the game never returns).
//!
//! ntdll's system call stub (`mov r10, rcx; mov eax, <n>; ...`, no relative operand) is copied
//! to a trampoline and replaced by a jump to the hook; put back before the game starts.

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use wal_payload_common::{log, patches};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect};

const SYSTEM_PROCESS_INFORMATION: u32 = 5;
const STUB: usize = 32;
const JUMP: usize = 12;

/// The trampoline (the stub's copy), the stub's address, the entry point's RVA.
static TRAMPOLINE: AtomicUsize = AtomicUsize::new(0);
static STUB_AT: AtomicUsize = AtomicUsize::new(0);
static ENTRY: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn init() {
    let Some(entry) = std::env::var("WAL_SEGANU_UNPACK").ok().and_then(|v| usize::from_str_radix(v.trim().trim_start_matches("0x"), 16).ok()) else {
        return;
    };
    unsafe {
        let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
        let Some(f) = GetProcAddress(ntdll, c"NtQuerySystemInformation".as_ptr().cast()) else { return };
        let f = f as usize;
        if std::slice::from_raw_parts(f as *const u8, 4) != [0x4c, 0x8b, 0xd1, 0xb8] {
            log!("unpack: NtQuerySystemInformation is not a system call stub, the game is not unpacked");
            return;
        }
        let tramp = VirtualAlloc(std::ptr::null(), STUB, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as *mut u8;
        if tramp.is_null() {
            return;
        }
        std::ptr::copy_nonoverlapping(f as *const u8, tramp, STUB);
        TRAMPOLINE.store(tramp as usize, Ordering::Relaxed);
        STUB_AT.store(f, Ordering::Relaxed);
        ENTRY.store(entry, Ordering::Relaxed);
        let mut jump = [0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xE0];
        jump[2..10].copy_from_slice(&(query as *const () as usize).to_le_bytes());
        write(f, &jump);
    }
    log!("unpack: the game starts at +{entry:#x} once unpacked");
}

unsafe fn write(at: usize, bytes: &[u8]) {
    let mut old = 0;
    unsafe {
        VirtualProtect(at as *const c_void, bytes.len(), PAGE_EXECUTE_READWRITE, &mut old);
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
        VirtualProtect(at as *const c_void, bytes.len(), old, &mut old);
    }
}

unsafe extern "system" fn query(class: u32, info: *mut c_void, size: u32, returned: *mut u32) -> i32 {
    let tramp = TRAMPOLINE.load(Ordering::Relaxed);
    if class == SYSTEM_PROCESS_INFORMATION {
        // the stub put back: the game's own calls go to ntdll again
        unsafe { write(STUB_AT.load(Ordering::Relaxed), std::slice::from_raw_parts(tramp as *const u8, JUMP)) };
        start_game();
    }
    let f: unsafe extern "system" fn(u32, *mut c_void, u32, *mut u32) -> i32 = unsafe { std::mem::transmute(tramp) };
    unsafe { f(class, info, size, returned) }
}

fn start_game() -> ! {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let entry = ENTRY.load(Ordering::Relaxed);
    if let Ok(list) = std::env::var("WAL_SEGANU_UNPACK_PATCHES") {
        patches::apply_list(&list);
    }
    log!("unpack: unpacked, the game starts at +{entry:#x}");
    let start: extern "system" fn() -> u32 = unsafe { std::mem::transmute(base + entry) };
    let code = start();
    std::process::exit(code as i32)
}
