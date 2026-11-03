//! `wal-loader <payload.dll> <game.exe> [args...]`
//!
//! Starts the game suspended and makes it load the payload DLL before running any of its own
//! code: the entry point is patched with a jump to a small stub (allocated in the game) that
//! calls `LoadLibraryW(payload)`, restores the entry point bytes and jumps back to it. No
//! remote thread is used (unreliable under Wine before the process is initialized).
//!
//! For games that do not import a driver DLL the payload could replace. The loader waits for
//! the game and exits with its exit code. Built for the game's bitness (i686, x86-64 as
//! `wal-loader64.exe`).
//!
//! The game is started with `SW_SHOWNORMAL` (as `start` and Explorer do; `CreateProcess`
//! without it gives `SW_SHOWDEFAULT`, 10): Aliens Extermination passes its `WinMain` show
//! command as `main`'s argc, glut32's `glutInit` then read 10 arguments and crashed.

#![cfg_attr(not(windows), allow(dead_code))]

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;

mod teratoma;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::System::Diagnostics::Debug::{FlushInstructionCache, ReadProcessMemory, WriteProcessMemory};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAllocEx, VirtualProtectEx};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, STARTF_USESHOWWINDOW, GetExitCodeProcess, INFINITE, PROCESS_INFORMATION, ResumeThread, STARTUPINFOW,
    TerminateProcess, WaitForSingleObject,
};

#[repr(C)]
struct ProcessBasicInformation {
    exit_status: i32,
    peb: usize,
    affinity: usize,
    base_priority: i32,
    pid: usize,
    parent_pid: usize,
}

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQueryInformationProcess(process: HANDLE, class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s).encode_wide().chain([0]).collect()
}

/// Windows command line quoting.
fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut out = String::from("\"");
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(backslashes * 2 + 1));
                out.push('"');
                backslashes = 0;
            }
            _ => {
                out.push_str(&"\\".repeat(backslashes));
                out.push(c);
                backslashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}

unsafe fn read<T: Copy + Default>(process: HANDLE, addr: usize) -> Result<T, String> {
    let mut value = T::default();
    let ok = unsafe { ReadProcessMemory(process, addr as *const c_void, (&mut value as *mut T).cast(), size_of::<T>(), std::ptr::null_mut()) };
    if ok == 0 { Err(format!("ReadProcessMemory({addr:#x}) failed: {}", unsafe { GetLastError() })) } else { Ok(value) }
}

unsafe fn write(process: HANDLE, addr: usize, data: &[u8]) -> Result<(), String> {
    let ok = unsafe { WriteProcessMemory(process, addr as *const c_void, data.as_ptr().cast(), data.len(), std::ptr::null_mut()) };
    if ok == 0 { Err(format!("WriteProcessMemory({addr:#x}) failed: {}", unsafe { GetLastError() })) } else { Ok(()) }
}

/// The stub's size in the game's memory.
const STUB: usize = 128;

/// The stub (`LoadLibraryW(path)`, the entry point's bytes restored from `saved`, a jump to it;
/// the registers kept) and the entry point's jump to it (at `mem`). i686: a 5-byte relative jump.
#[cfg(target_pointer_width = "32")]
fn stub(load_library: usize, path_at: usize, saved_at: usize, entry: usize, mem: usize) -> (Vec<u8>, Vec<u8>) {
    let mut stub: Vec<u8> = Vec::with_capacity(STUB);
    stub.extend_from_slice(&[0x60, 0x9C]); // pushad; pushfd
    stub.push(0x68); // push path
    stub.extend_from_slice(&(path_at as u32).to_le_bytes());
    stub.push(0xB8); // mov eax, LoadLibraryW
    stub.extend_from_slice(&(load_library as u32).to_le_bytes());
    stub.extend_from_slice(&[0xFF, 0xD0]); // call eax
    stub.push(0xBE); // mov esi, saved
    stub.extend_from_slice(&(saved_at as u32).to_le_bytes());
    stub.push(0xBF); // mov edi, entry
    stub.extend_from_slice(&(entry as u32).to_le_bytes());
    stub.extend_from_slice(&[0xB9, 5, 0, 0, 0]); // mov ecx, 5
    stub.extend_from_slice(&[0xFC, 0xF3, 0xA4]); // cld; rep movsb
    stub.extend_from_slice(&[0x9D, 0x61]); // popfd; popad
    stub.push(0x68); // push entry
    stub.extend_from_slice(&(entry as u32).to_le_bytes());
    stub.push(0xC3); // ret
    stub.resize(STUB, 0xCC);
    let mut jmp = vec![0xE9];
    jmp.extend_from_slice(&((mem as i64 - (entry as i64 + 5)) as i32).to_le_bytes());
    (stub, jmp)
}

/// x86-64 (Disney Crossy Road): the volatile registers and rsi / rdi kept, the stack aligned
/// for the call (rsp 8 mod 16 at the entry point: 11 pushes and 32 bytes of shadow space), a
/// 14-byte absolute jump (the stub may be more than 2 GB away).
#[cfg(target_pointer_width = "64")]
fn stub(load_library: usize, path_at: usize, saved_at: usize, entry: usize, mem: usize) -> (Vec<u8>, Vec<u8>) {
    let jmp_abs = |to: usize| {
        let mut j = vec![0xFF, 0x25, 0, 0, 0, 0]; // jmp [rip+0]
        j.extend_from_slice(&(to as u64).to_le_bytes());
        j
    };
    let mut stub: Vec<u8> = Vec::with_capacity(STUB);
    // pushfq; push rax, rcx, rdx, rbx, rsi, rdi, r8, r9, r10, r11
    stub.extend_from_slice(&[0x9C, 0x50, 0x51, 0x52, 0x53, 0x56, 0x57, 0x41, 0x50, 0x41, 0x51, 0x41, 0x52, 0x41, 0x53]);
    stub.extend_from_slice(&[0x48, 0x83, 0xEC, 0x20]); // sub rsp, 32
    stub.extend_from_slice(&[0x48, 0xB9]); // mov rcx, path
    stub.extend_from_slice(&(path_at as u64).to_le_bytes());
    stub.extend_from_slice(&[0x48, 0xB8]); // mov rax, LoadLibraryW
    stub.extend_from_slice(&(load_library as u64).to_le_bytes());
    stub.extend_from_slice(&[0xFF, 0xD0]); // call rax
    stub.extend_from_slice(&[0x48, 0x83, 0xC4, 0x20]); // add rsp, 32
    stub.extend_from_slice(&[0x48, 0xBE]); // mov rsi, saved
    stub.extend_from_slice(&(saved_at as u64).to_le_bytes());
    stub.extend_from_slice(&[0x48, 0xBF]); // mov rdi, entry
    stub.extend_from_slice(&(entry as u64).to_le_bytes());
    stub.extend_from_slice(&[0xB9, 14, 0, 0, 0]); // mov ecx, 14
    stub.extend_from_slice(&[0xFC, 0xF3, 0xA4]); // cld; rep movsb
    // pop r11, r10, r9, r8, rdi, rsi, rbx, rdx, rcx, rax; popfq
    stub.extend_from_slice(&[0x41, 0x5B, 0x41, 0x5A, 0x41, 0x59, 0x41, 0x58, 0x5F, 0x5E, 0x5B, 0x5A, 0x59, 0x58, 0x9D]);
    stub.extend(jmp_abs(entry));
    assert!(stub.len() <= STUB);
    stub.resize(STUB, 0xCC);
    (stub, jmp_abs(mem))
}

/// Patches the entry point of the suspended `process` to load `payload` first.
unsafe fn inject(process: HANDLE, payload: &str) -> Result<(), String> {
    unsafe {
        let mut info: ProcessBasicInformation = std::mem::zeroed();
        let status = NtQueryInformationProcess(process, 0, (&mut info as *mut ProcessBasicInformation).cast(), size_of::<ProcessBasicInformation>() as u32, std::ptr::null_mut());
        if status != 0 {
            return Err(format!("NtQueryInformationProcess failed: {status:#x}"));
        }
        // PEB.ImageBaseAddress (32-bit PEB: +0x08, 64-bit: +0x10)
        let image = if cfg!(target_pointer_width = "64") {
            read::<u64>(process, info.peb + 0x10)? as usize
        } else {
            read::<u32>(process, info.peb + 0x08)? as usize
        };
        let e_lfanew: u32 = read(process, image + 0x3C)?;
        let entry_rva: u32 = read(process, image + e_lfanew as usize + 0x28)?;
        let entry = image + entry_rva as usize;

        let load_library = GetProcAddress(GetModuleHandleA(c"kernel32.dll".as_ptr().cast()), c"LoadLibraryW".as_ptr().cast())
            .ok_or("LoadLibraryW not found")? as usize;

        // memory: [stub STUB bytes][saved entry bytes 16][payload path]
        let path = wide(payload);
        let size = STUB + 16 + path.len() * 2;
        let mem = VirtualAllocEx(process, std::ptr::null(), size, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as usize;
        if mem == 0 {
            return Err(format!("VirtualAllocEx failed: {}", GetLastError()));
        }
        let saved_at = mem + STUB;
        let path_at = mem + STUB + 16;
        let saved: [u8; 16] = read(process, entry)?;
        let (stub, jmp) = stub(load_library, path_at, saved_at, entry, mem);

        let mut block = stub;
        block.extend_from_slice(&saved);
        block.extend(path.iter().flat_map(|c| c.to_le_bytes()));
        write(process, mem, &block)?;

        // entry point: jmp stub (the page stays writable for the stub's restore)
        let mut old = 0;
        if VirtualProtectEx(process, entry as *const c_void, jmp.len(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return Err(format!("VirtualProtectEx failed: {}", GetLastError()));
        }
        write(process, entry, &jmp)?;
        FlushInstructionCache(process, entry as *const c_void, jmp.len());
        eprintln!("wal-loader: payload {payload} queued at entry point {entry:#x}");
        Ok(())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: wal-loader <payload.dll> <game.exe> [args...]");
        std::process::exit(2);
    }
    let payload = std::path::absolute(&args[0]).map(|p| p.display().to_string()).unwrap_or_else(|_| args[0].clone());
    let cmdline: Vec<String> = args[1..].iter().map(|a| quote(a)).collect();
    let mut cmdline = wide(&cmdline.join(" "));

    unsafe {
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = size_of::<STARTUPINFOW>() as u32;
        si.dwFlags = STARTF_USESHOWWINDOW;
        si.wShowWindow = 1; // SW_SHOWNORMAL
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        let ok = CreateProcessW(
            std::ptr::null(),
            cmdline.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_SUSPENDED,
            std::ptr::null(),
            std::ptr::null(),
            &si,
            &mut pi,
        );
        if ok == 0 {
            eprintln!("wal-loader: cannot start {}: error {}", args[1], GetLastError());
            std::process::exit(1);
        }
        let teratoma = std::env::var("WAL_LOADER_TERATOMA").is_ok_and(|s| s == "1");
        let result = if teratoma {
            teratoma::boot(&pi, &args[1], &payload)
        } else {
            inject(pi.hProcess, &payload)
        };
        if let Err(e) = result {
            eprintln!("wal-loader: {e}");
            TerminateProcess(pi.hProcess, 1);
            std::process::exit(1);
        }
        if !teratoma {
            ResumeThread(pi.hThread);
        }
        WaitForSingleObject(pi.hProcess, INFINITE);
        let mut code = 0u32;
        GetExitCodeProcess(pi.hProcess, &mut code);
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        std::process::exit(code as i32);
    }
}
