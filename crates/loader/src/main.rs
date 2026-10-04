//! `wal-loader <payload.dll> <game.exe> [args...]`
//!
//! Starts the game suspended and makes it load the payload DLL before running any of its own
//! code: the entry point is patched with a jump to a small stub (allocated in the game) that
//! calls `LoadLibraryW(payload)`, restores the entry point bytes and jumps back to it. No
//! remote thread is used (unreliable under Wine before the process is initialized).
//!
//! For games that do not import a driver DLL the payload could replace. The loader waits for
//! the game and exits with its exit code. Built for the game's bitness (i686 here).

#![cfg_attr(not(windows), allow(dead_code))]

use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::System::Diagnostics::Debug::{FlushInstructionCache, ReadProcessMemory, WriteProcessMemory};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAllocEx, VirtualProtectEx};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessW, GetExitCodeProcess, INFINITE, PROCESS_INFORMATION, ResumeThread, STARTUPINFOW,
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

/// Patches the entry point of the suspended `process` to load `payload` first.
unsafe fn inject(process: HANDLE, payload: &str) -> Result<(), String> {
    unsafe {
        let mut info: ProcessBasicInformation = std::mem::zeroed();
        let status = NtQueryInformationProcess(process, 0, (&mut info as *mut ProcessBasicInformation).cast(), size_of::<ProcessBasicInformation>() as u32, std::ptr::null_mut());
        if status != 0 {
            return Err(format!("NtQueryInformationProcess failed: {status:#x}"));
        }
        // PEB.ImageBaseAddress (32-bit PEB: +0x08)
        let image: u32 = read(process, info.peb + 0x08)?;
        let image = image as usize;
        let e_lfanew: u32 = read(process, image + 0x3C)?;
        let entry_rva: u32 = read(process, image + e_lfanew as usize + 0x28)?;
        let entry = image + entry_rva as usize;

        let load_library = GetProcAddress(GetModuleHandleA(c"kernel32.dll".as_ptr().cast()), c"LoadLibraryW".as_ptr().cast())
            .ok_or("LoadLibraryW not found")? as usize;

        // memory: [stub 64 bytes][saved entry bytes 8][payload path]
        let path = wide(payload);
        let size = 64 + 8 + path.len() * 2;
        let mem = VirtualAllocEx(process, std::ptr::null(), size, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as usize;
        if mem == 0 {
            return Err(format!("VirtualAllocEx failed: {}", GetLastError()));
        }
        let saved_at = mem + 64;
        let path_at = mem + 72;
        let saved: [u8; 8] = read(process, entry)?;

        let mut stub: Vec<u8> = Vec::with_capacity(64);
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
        stub.resize(64, 0xCC);

        let mut block = stub;
        block.extend_from_slice(&saved);
        block.extend(path.iter().flat_map(|c| c.to_le_bytes()));
        write(process, mem, &block)?;

        // entry point: jmp stub (the page stays writable for the stub's restore)
        let mut old = 0;
        if VirtualProtectEx(process, entry as *const c_void, 5, PAGE_EXECUTE_READWRITE, &mut old) == 0 {
            return Err(format!("VirtualProtectEx failed: {}", GetLastError()));
        }
        let rel = (mem as i64 - (entry as i64 + 5)) as i32;
        let mut jmp = vec![0xE9];
        jmp.extend_from_slice(&rel.to_le_bytes());
        write(process, entry, &jmp)?;
        FlushInstructionCache(process, entry as *const c_void, 5);
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
        if let Err(e) = inject(pi.hProcess, &payload) {
            eprintln!("wal-loader: {e}");
            TerminateProcess(pi.hProcess, 1);
            std::process::exit(1);
        }
        ResumeThread(pi.hThread);
        WaitForSingleObject(pi.hProcess, INFINITE);
        let mut code = 0u32;
        GetExitCodeProcess(pi.hProcess, &mut code);
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        std::process::exit(code as i32);
    }
}
