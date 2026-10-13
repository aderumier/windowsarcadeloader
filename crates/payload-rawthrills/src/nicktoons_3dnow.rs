//! Nicktoons' inline 3DNow vector math faults on CPUs without AMD 3DNow.
//! Translate only fingerprinted, straight-line FEMMS blocks to SSE2. Preserve all XMM
//! registers and flags; loads/stores remain 64 bits, including at allocation boundaries.

use crate::common::write;
use wal_payload_common::log;
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_READWRITE, VirtualAlloc, VirtualProtect};
#[path = "nicktoons_3dnow_sites.rs"]
mod sites;
const STACK: i32 = 144;

/// Copy one ModRM operand, adjusting references to the original stack after reserving
/// space for XMM registers. Reject unusual ESP addressing rather than silently moving it.
fn operand(code: &[u8]) -> Option<(Vec<u8>, usize)> {
    let m = *code.first()?;
    let mode = m >> 6;
    let rm = m & 7;
    let mut n = 1;
    let mut out = vec![m];
    let mut stack = false;
    let mut absolute = mode == 0 && rm == 5;
    if mode != 3 && rm == 4 {
        let sib = *code.get(n)?;
        n += 1;
        out.push(sib);
        stack = sib & 7 == 4;
        if stack && (sib >> 3) & 7 != 4 {
            return None;
        }
        absolute = mode == 0 && sib & 7 == 5;
    }
    let count = if mode == 1 {
        1
    } else if mode == 2 || absolute {
        4
    } else {
        0
    };
    let d = code.get(n..n + count)?;
    if stack {
        let disp = match count {
            0 => 0,
            1 => d[0] as i8 as i32,
            4 => i32::from_le_bytes(d.try_into().ok()?),
            _ => return None,
        };
        out[0] = (m & 0x3f) | 0x80;
        out.extend_from_slice(&(disp + STACK).to_le_bytes());
    } else {
        out.extend_from_slice(d);
    }
    Some((out, n + count))
}

fn xmm_stack(out: &mut Vec<u8>, reg: u8, offset: i32, load: bool) {
    out.extend_from_slice(&[0x0f, if load { 0x10 } else { 0x11 }, 0x84 | (reg << 3), 0x24]);
    out.extend_from_slice(&offset.to_le_bytes());
}

fn translate(code: &[u8]) -> Option<Vec<u8>> {
    if !code.starts_with(&[0x0f, 0x0e]) || !code.ends_with(&[0x0f, 0x0e]) {
        return None;
    }
    let mut out = vec![0x0f, 0x77, 0x8d, 0xa4, 0x24]; // EMMS; LEA esp,[esp-STACK]
    out.extend_from_slice(&(-STACK).to_le_bytes());
    for r in 0..8 {
        xmm_stack(&mut out, r, r as i32 * 16, false);
    }
    let mut p = 2;
    while p < code.len() - 2 {
        let op = code[p];
        p += 1;
        match op {
            0xb8..=0xbf => {
                // MOV general register, immediate
                out.push(op);
                out.extend_from_slice(code.get(p..p + 4)?);
                p += 4;
            }
            0x8b => {
                let (arg, n) = operand(&code[p..])?;
                out.push(op);
                out.extend(arg);
                p += n;
            }
            0x0f => {
                let op = *code.get(p)?;
                p += 1;
                let (arg, n) = operand(&code[p..])?;
                p += n;
                let reg = (arg[0] >> 3) & 7;
                match op {
                    0x0d => {} // AMD prefetch is only a hint.
                    0x6f => {
                        out.extend([0xf3, 0x0f, 0x7e]);
                        out.extend(arg);
                    }
                    0x7f => {
                        out.extend([0x66, 0x0f, 0xd6]);
                        out.extend(arg);
                    }
                    0x62 if arg[0] >> 6 == 3 => {
                        out.extend([0x66, 0x0f, 0x62]);
                        out.extend(arg);
                    }
                    0x6a if arg[0] >> 6 == 3 => {
                        // MMX high dword means lane 1, whereas SSE high starts at lane 2.
                        out.extend([0x0f, 0xc6, arg[0], 0x55]);
                        out.extend([0x66, 0x0f, 0x70, 0xc0 | (reg << 3) | reg, 0xd8]);
                    }
                    0x0f => {
                        let math = match *code.get(p)? {
                            0x9e => 0x58,
                            0x9a => 0x5c,
                            0xb4 => 0x59,
                            _ => return None,
                        };
                        p += 1;
                        if arg[0] >> 6 == 3 {
                            out.extend([0x0f, math]);
                            out.extend(arg);
                        } else {
                            // Packed SSE memory math would read 16 bytes. Use a saved temporary
                            // so the original 8-byte operand is the only memory read.
                            let temp = (reg + 1) & 7;
                            xmm_stack(&mut out, temp, 128, false);
                            out.extend([0xf3, 0x0f, 0x7e, (arg[0] & 0xc7) | (temp << 3)]);
                            out.extend_from_slice(&arg[1..]);
                            out.extend([0x0f, math, 0xc0 | (reg << 3) | temp]);
                            xmm_stack(&mut out, temp, 128, true);
                        }
                    }
                    _ => return None,
                }
            }
            _ => return None,
        }
    }
    if p != code.len() - 2 {
        return None;
    }
    for r in 0..8 {
        xmm_stack(&mut out, r, r as i32 * 16, true);
    }
    out.extend([0x8d, 0xa4, 0x24]);
    out.extend_from_slice(&STACK.to_le_bytes());
    out.extend([0x0f, 0x77]);
    Some(out)
}

pub(crate) fn init() -> bool {
    let mut blocks = Vec::new();
    for &(address, size, expected) in sites::SITES {
        let original = unsafe { std::slice::from_raw_parts(address as *const u8, size) };
        let hash = original.iter().fold(2166136261u32, |h, b| (h ^ *b as u32).wrapping_mul(16777619));
        let Some(code) = (hash == expected).then(|| translate(original)).flatten() else {
            log!("nicktoons: unrecognized 3DNow block at {address:#x}, math patches not installed");
            return false;
        };
        blocks.push((address, size, code));
    }
    let total: usize = blocks.iter().map(|(_, _, code)| code.len() + 5).sum();
    let memory = unsafe { VirtualAlloc(std::ptr::null(), total, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) } as usize;
    if memory == 0 {
        log!("nicktoons: cannot allocate SSE2 math trampolines");
        return false;
    }
    let mut offset = 0;
    let mut jumps = Vec::new();
    for (address, size, mut code) in blocks {
        let dest = memory + offset;
        code.push(0xe9);
        code.extend_from_slice(&((address + size).wrapping_sub(dest + code.len() + 4) as u32).to_le_bytes());
        unsafe {
            std::ptr::copy_nonoverlapping(code.as_ptr(), dest as *mut u8, code.len());
        }
        jumps.push((address, dest));
        offset += code.len();
    }
    let mut old = 0;
    if unsafe { VirtualProtect(memory as _, total, PAGE_EXECUTE_READ, &mut old) } == 0 {
        log!("nicktoons: cannot enable SSE2 math trampolines");
        return false;
    }
    for (address, dest) in jumps {
        let mut jump = vec![0xe9];
        jump.extend_from_slice(&(dest.wrapping_sub(address + 5) as u32).to_le_bytes());
        write(address, &jump);
    }
    log!("nicktoons: {} validated 3DNow math blocks replaced by SSE2", sites::SITES.len());
    true
}
