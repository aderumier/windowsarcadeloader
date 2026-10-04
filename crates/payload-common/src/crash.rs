//! Crash reporter: logs the first access violations of the game process with registers and
//! a symbolized stack scan (`module+offset` of every stack value pointing into a module).
//! Wine's own traces only give the faulting address; this shows which game code led there.

use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
    GetModuleHandleExW,
};

use crate::log;

const EXCEPTION_ACCESS_VIOLATION: i32 = 0xC000_0005_u32 as i32;
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const MAX_REPORTS: u32 = 3;

static REPORTS: AtomicU32 = AtomicU32::new(0);

pub fn init() {
    unsafe { AddVectoredExceptionHandler(1, Some(handler)) };
}

/// `module+offset` for an address inside a loaded module.
fn symbolize(addr: usize) -> Option<String> {
    let mut module: HMODULE = std::ptr::null_mut();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    if unsafe { GetModuleHandleExW(flags, addr as *const u16, &mut module) } == 0 || module.is_null() {
        return None;
    }
    let mut name = [0u16; 260];
    let n = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&name[..n]);
    let file = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_string();
    Some(format!("{file}+{:#x}", addr - module as usize))
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    let record = unsafe { &*(*info).ExceptionRecord };
    if record.ExceptionCode != EXCEPTION_ACCESS_VIOLATION || REPORTS.fetch_add(1, Ordering::Relaxed) >= MAX_REPORTS {
        return EXCEPTION_CONTINUE_SEARCH;
    }
    let addr = record.ExceptionAddress as usize;
    let (op, target) = (record.ExceptionInformation[0], record.ExceptionInformation[1]);
    log!(
        "crash: access violation ({} {target:#x}) at {addr:#x} {}",
        if op == 1 { "writing" } else { "reading" },
        symbolize(addr).unwrap_or_default()
    );
    // write overrun: show what was being written just below the faulting address
    if op == 1 && target > 0x10000 {
        let start = target - 160;
        let bytes: Vec<u8> = (start..target).map(|a| unsafe { std::ptr::read_volatile(a as *const u8) }).collect();
        let text: String = bytes.iter().map(|b| if (0x20..0x7f).contains(b) { *b as char } else { '.' }).collect();
        log!("crash: bytes before {target:#x}: {text}");
    }
    #[cfg(target_arch = "x86")]
    unsafe {
        let c = &*(*info).ContextRecord;
        log!(
            "crash: eax={:08x} ebx={:08x} ecx={:08x} edx={:08x} esi={:08x} edi={:08x} ebp={:08x} esp={:08x}",
            c.Eax, c.Ebx, c.Ecx, c.Edx, c.Esi, c.Edi, c.Ebp, c.Esp
        );
        // EBP frame chain: [ebp] = caller ebp, [ebp+4] = return address
        let mut ebp = c.Ebp as usize;
        for depth in 0..16 {
            if ebp < 0x10000 || ebp % 4 != 0 || ebp < c.Esp as usize || ebp > c.Esp as usize + 0x100000 {
                break;
            }
            let frame = ebp as *const usize;
            let ret = std::ptr::read_volatile(frame.add(1));
            log!("crash:   frame {depth}: return {ret:#010x} {}", symbolize(ret).unwrap_or_default());
            ebp = std::ptr::read_volatile(frame);
        }
        // stack scan: return addresses into modules
        let esp = c.Esp as usize as *const usize;
        let mut found = 0;
        for i in 0..512 {
            let v = std::ptr::read_volatile(esp.add(i));
            if v < 0x10000 {
                continue;
            }
            if let Some(sym) = symbolize(v) {
                log!("crash:   [esp+{:#05x}] {v:#010x} {sym}", i * 4);
                found += 1;
                if found >= 24 {
                    break;
                }
            }
        }
    }
    EXCEPTION_CONTINUE_SEARCH
}
