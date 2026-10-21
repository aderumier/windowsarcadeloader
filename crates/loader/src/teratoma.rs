//! Teratoma's packed TT.exe: keep the unpacker's checked code intact, update its two
//! pending VM operands, and load our payload from the game's startup call after unpacking.
//! The dump stays unchanged. Every write is guarded against the supported executable.

use std::ffi::c_void;
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAllocEx, VirtualProtectEx};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, PROCESS_INFORMATION, ResumeThread, WaitForSingleObject};

use super::{NtQueryInformationProcess, ProcessBasicInformation, read, wide, write};

const HASH: &str = "cb711483cfb1b64923bf5e966fa11e9a4745f7c930781f7d479bcb34f79365ba";

struct Game {
    process: HANDLE,
    base: usize,
}

impl Game {
    fn word(&self, rva: usize) -> Option<u32> {
        unsafe { read(self.process, self.base + rva).ok() }
    }

    fn bytes(&self, rva: usize, expected: &[u8]) -> bool {
        expected.iter().enumerate().all(|(i, b)| unsafe { read::<u8>(self.process, self.base + rva + i).ok() == Some(*b) })
    }

    fn patch(&self, rva: usize, before: &[u8], after: &[u8]) -> Result<(), String> {
        if before.len() != after.len() || !self.bytes(rva, before) {
            return Err(format!("Teratoma: unsupported bytes at +{rva:#x}"));
        }
        let at = self.base + rva;
        let mut old = 0;
        unsafe {
            if VirtualProtectEx(self.process, at as *const c_void, after.len(), PAGE_EXECUTE_READWRITE, &mut old) == 0 {
                return Err(format!("Teratoma: cannot write-enable +{rva:#x}"));
            }
            let result = write(self.process, at, after);
            let flushed = FlushInstructionCache(self.process, at as *const c_void, after.len());
            let mut ignored = 0;
            let restored = VirtualProtectEx(self.process, at as *const c_void, after.len(), old, &mut ignored);
            result?;
            if flushed == 0 || restored == 0 || !self.bytes(rva, after) {
                return Err(format!("Teratoma: patch verification failed at +{rva:#x}"));
            }
        }
        Ok(())
    }

    fn operand_phase(&self) -> bool {
        self.word(0x27f99f4) == Some(1)
            && self.word(0x27f717c) == Some(self.base as u32 + 0x27f5d10)
            && self.word(0x27f9bb4) == Some(self.base as u32 + 0x28089b6)
            && self.word(0x27f9bb8) == Some(0x28e0)
            && self.word(0x27f99ec) == Some(0)
            && self.word(0x27f884c) == Some(0)
            && self.word(0x27f8834) == Some(0)
            && self.word(0x27ee5c8).is_some_and(|v| v != 0)
            && self.word(0x27ee49c) == Some(6)
            && self.word(0x27f4934) == Some(0)
    }

    fn unpacked_phase(&self) -> bool {
        self.word(0x29fd1c4) == Some(1)
            && self.word(0x29fa3a4) == Some(self.base as u32 + 0x29fa660)
            && self.word(0x29fbd58) == Some(self.base as u32 + 0x2a024f3)
            && self.word(0x29fbb40) == Some(0)
            && self.word(0x29fbb90) == Some(0)
            && self.word(0x29fa250) == Some(self.base as u32 + 0x2a00d0b)
    }

    fn exited(&self) -> Option<u32> {
        unsafe {
            if WaitForSingleObject(self.process, 0) != 0 {
                return None;
            }
            let mut code = 0;
            GetExitCodeProcess(self.process, &mut code);
            Some(code)
        }
    }

    fn queue_payload(&self, payload: &str) -> Result<usize, String> {
        let path = wide(payload);
        let size = 64 + path.len() * 2;
        unsafe {
            let memory = VirtualAllocEx(self.process, std::ptr::null(), size, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) as usize;
            if memory == 0 {
                return Err("Teratoma: cannot allocate startup stub".into());
            }
            let load = GetProcAddress(GetModuleHandleA(c"kernel32.dll".as_ptr().cast()), c"LoadLibraryW".as_ptr().cast())
                .ok_or("Teratoma: LoadLibraryW unavailable")? as usize;
            let mut stub = vec![0x9C, 0x60, 0x68]; // pushfd; pushad; push path
            stub.extend_from_slice(&((memory + 64) as u32).to_le_bytes());
            stub.push(0xB8);
            stub.extend_from_slice(&(load as u32).to_le_bytes());
            stub.extend_from_slice(&[0xFF, 0xD0, 0xA3]); // call eax; mov [result],eax
            stub.extend_from_slice(&((memory + 40) as u32).to_le_bytes());
            stub.extend_from_slice(&[0xC7, 0x05]); // mov [finished],1
            stub.extend_from_slice(&((memory + 44) as u32).to_le_bytes());
            stub.extend_from_slice(&1u32.to_le_bytes());
            stub.extend_from_slice(&[0x61, 0x9D, 0xC3]); // popad; popfd; ret
            stub.resize(64, 0);
            stub.extend(path.iter().flat_map(|v| v.to_le_bytes()));
            write(self.process, memory, &stub)?;
            if FlushInstructionCache(self.process, memory as *const c_void, stub.len()) == 0 {
                return Err("Teratoma: startup stub instruction flush failed".into());
            }
            let mut call = vec![0xE8];
            call.extend_from_slice(&(memory.wrapping_sub(self.base + 0x18db0 + 5) as u32).to_le_bytes());
            self.patch(0x18db0, &[0xE8, 0xD2, 0xAF, 0x16, 0], &call)?;
            Ok(memory)
        }
    }
}

/// On success the primary thread is running and the payload has loaded on that thread.
/// Do not inject at the packed PE entry point: it runs before the unpacker finishes.
pub unsafe fn boot(pi: &PROCESS_INFORMATION, executable: &str, payload: &str) -> Result<(), String> {
    let data = std::fs::read(executable).map_err(|e| format!("Teratoma: cannot read executable: {e}"))?;
    if format!("{:x}", Sha256::digest(&data)) != HASH {
        return Err("Teratoma: unsupported TT.exe (SHA-256 differs from the supported dump)".into());
    }
    let mut info: ProcessBasicInformation = unsafe { std::mem::zeroed() };
    let status = unsafe {
        NtQueryInformationProcess(pi.hProcess, 0, (&mut info as *mut ProcessBasicInformation).cast(), size_of::<ProcessBasicInformation>() as u32, std::ptr::null_mut())
    };
    if status != 0 {
        return Err(format!("Teratoma: cannot locate image: {status:#x}"));
    }
    let base: u32 = unsafe { read(pi.hProcess, info.peb + 8)? };
    let game = Game { process: pi.hProcess, base: base as usize };
    eprintln!("wal-loader: Teratoma supported TT.exe at {base:#x}; starting unpacker");
    game.patch(0x271d4fd, &[0x83, 0x7D, 0xF8, 0x15, 0x75, 0x21], &[0x83, 0x7D, 0xF8, 0x15, 0xEB, 0x21])?;
    if unsafe { ResumeThread(pi.hThread) } == u32::MAX {
        return Err("Teratoma: cannot resume unpacker".into());
    }
    let start = Instant::now();
    let mut operands = false;
    let mut stub = None;
    while start.elapsed() < Duration::from_secs(20) {
        if let Some(code) = game.exited() {
            return Err(format!("Teratoma: exited during startup: {code:#x}"));
        }
        if !operands && game.operand_phase() {
            if !game.bytes(0x2818c2e, &[0x33, 0xC0, 0xC3]) {
                return Err("Teratoma: unpacker suffix identity mismatch".into());
            }
            // These aligned VM operands are outside the checked native-code span. The
            // interpreter has not consumed them in this phase. Leave the helpers intact.
            let suffix = (base + 0x2818c2e).to_le_bytes();
            game.patch(0x27f52c4, &(base + 0x28064c8).to_le_bytes(), &suffix)?;
            game.patch(0x27f52d4, &(base + 0x27fb180).to_le_bytes(), &suffix)?;
            operands = true;
            eprintln!("wal-loader: Teratoma startup operands installed");
        }
        if operands && stub.is_none() && game.unpacked_phase() {
            // This phase is still executing the unpacker, outside the game's startup
            // instructions. Queue the payload before allowing the native Windows gate.
            let memory = game.queue_payload(payload)?;
            game.patch(0x18b8d, &[0x76, 0x7C], &[0xEB, 0x7C])?;
            game.patch(0x18d84, &[0x0F, 0x84, 0xD7, 0, 0, 0], &[0x90; 6])?;
            stub = Some(memory);
            eprintln!("wal-loader: Teratoma unpacked; payload queued at native startup");
        }
        if let Some(memory) = stub {
            if unsafe { read::<u32>(game.process, memory + 44) }.unwrap_or(0) != 0 {
                let module = unsafe { read::<u32>(game.process, memory + 40)? };
                if module == 0 {
                    return Err(format!("Teratoma: could not load payload {payload}"));
                }
                eprintln!("wal-loader: Teratoma payload loaded at {module:#x}");
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    Err(format!("Teratoma: startup timed out (operands={operands}, native_startup={})", stub.is_some()))
}
