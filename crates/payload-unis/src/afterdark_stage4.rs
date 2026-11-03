//! After Dark's fourth stage open from the start (`WAL_UNIS_AFTERDARK_STAGE4`, default on).
//!
//! The stage select (`Module/UI.nmo`, a Virtools composition) offers the fourth stage once the
//! first three are cleared in the same game: its script's local boolean `开启第四关` ("open the
//! fourth stage"), set by the stage elements' script (`关卡元素`) when the three stage flags are
//! true, reset with them to false at each game's start (an Identity of `Int Data`), and tested
//! by the cursor's selection (`光标选择` / `关卡判断`).
//!
//! When the game opens the composition, the payload gives it a copy where that boolean is saved
//! true and no reset writes it (its place in the reset's destinations takes another of them):
//! the fourth stage selectable with the others.
//!
//! The file: `Nemo Fi` header (64 bytes: an Adler-32 from 0 of the header with its own field
//! zeroed then the rest, the packed and unpacked sizes of the object table and of the data,
//! zlib), the object table (per object: id, class, its data's offset in the unpacked file, its
//! name), then each object's state chunk (size, header, dword count, dwords).

/// The header's fields (byte offsets).
const CRC: usize = 8;
const HDR_PACKED: usize = 28;
const DATA_PACKED: usize = 32;
const DATA_UNPACKED: usize = 36;
const OBJECTS: usize = 44;
const HDR_UNPACKED: usize = 60;
const HEADER: usize = 64;

/// Virtools classes: a parameter output, a local parameter.
const CLASS_PARAMETER_OUT: u32 = 3;
const CLASS_PARAMETER_LOCAL: u32 = 45;

/// The boolean parameter type's GUID.
const BOOL_GUID: [u32; 2] = [0x1ad5_2a8e, 0x5e74_1920];

/// `开启第四关` in GBK (the composition's names).
const OPEN_STAGE4: &[u8] = &[0xbf, 0xaa, 0xc6, 0xf4, 0xb5, 0xda, 0xcb, 0xc4, 0xb9, 0xd8];

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn put_u32(b: &mut [u8], at: usize, v: u32) {
    b[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// The section `packed` bytes long at `at`, unpacked to `unpacked` bytes (stored as is when
/// both sizes match).
fn section(file: &[u8], at: usize, packed: usize, unpacked: usize) -> Option<Vec<u8>> {
    let raw = file.get(at..at + packed)?;
    if packed == unpacked {
        return Some(raw.to_vec());
    }
    miniz_oxide::inflate::decompress_to_vec_zlib(raw).ok().filter(|v| v.len() == unpacked)
}

/// The Adler-32 from 0 of the header (its own field zeroed) and the rest of the file.
fn checksum(file: &[u8]) -> u32 {
    let (mut a, mut b) = (0u32, 0u32);
    let zeroed = (0..file.len()).map(|i| if (CRC..CRC + 4).contains(&i) { 0 } else { file[i] });
    for byte in zeroed {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// The composition with the fourth stage open, or None (not the expected file).
pub fn open_stage4(file: &[u8]) -> Option<Vec<u8>> {
    if !file.starts_with(b"Nemo Fi\0") {
        return None;
    }
    let field = |at| u32_at(file, at).map(|v| v as usize);
    let (hdr_packed, hdr_unpacked) = (field(HDR_PACKED)?, field(HDR_UNPACKED)?);
    let (data_packed, data_unpacked, objects) = (field(DATA_PACKED)?, field(DATA_UNPACKED)?, field(OBJECTS)?);
    let table = section(file, HEADER, hdr_packed, hdr_unpacked)?;
    let mut data = section(file, HEADER + hdr_packed, data_packed, data_unpacked)?;
    // the object table: (class, the dwords' offset in the data) by index, the boolean's index
    let base = HEADER + hdr_unpacked;
    let mut chunks = Vec::with_capacity(objects);
    let mut target = None;
    let mut p = 0;
    for index in 0..objects {
        let (class, offset, len) = (u32_at(&table, p + 4)?, u32_at(&table, p + 8)? as usize, u32_at(&table, p + 12)? as usize);
        let name = table.get(p + 16..p + 16 + len)?;
        p += 16 + len;
        // the chunk: size, header, dword count, dwords
        let words = offset.checked_sub(base)? + 12;
        chunks.push((class, words));
        if class == CLASS_PARAMETER_LOCAL && name.strip_suffix(&[0]).unwrap_or(name) == OPEN_STAGE4 {
            target = Some(index);
        }
    }
    let target = target?;
    let word = |data: &[u8], (_, at): (u32, usize), i: usize| u32_at(data, at + 4 * i);
    // the boolean saved true: dwords 0x40, 0, the type GUID, 1, its size 4, its value
    let local = chunks[target];
    if [word(&data, local, 2)?, word(&data, local, 3)?] != BOOL_GUID || word(&data, local, 5)? != 4 {
        return None;
    }
    put_u32(&mut data, local.1 + 4 * 6, 1);
    // the resets: outputs of value false whose destinations (dwords 9: count, 10..) include it
    let mut resets = 0;
    for &chunk in chunks.iter().filter(|c| c.0 == CLASS_PARAMETER_OUT) {
        let is_bool = word(&data, chunk, 2) == Some(BOOL_GUID[0]) && word(&data, chunk, 3) == Some(BOOL_GUID[1]);
        if !is_bool || word(&data, chunk, 6) != Some(0) {
            continue;
        }
        let Some(count) = word(&data, chunk, 9).map(|c| c as usize).filter(|&c| c < 64) else { continue };
        let dests: Vec<u32> = (0..count).filter_map(|i| word(&data, chunk, 10 + i)).collect();
        let Some(other) = dests.iter().copied().find(|&d| d != target as u32) else { continue };
        for (i, &d) in dests.iter().enumerate() {
            if d == target as u32 {
                put_u32(&mut data, chunk.1 + 4 * (10 + i), other);
                resets += 1;
            }
        }
    }
    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&data, 6);
    let mut out = file[..HEADER + hdr_packed].to_vec();
    out.extend_from_slice(&packed);
    put_u32(&mut out, DATA_PACKED, packed.len() as u32);
    let crc = checksum(&out);
    put_u32(&mut out, CRC, crc);
    wal_payload_common::log!("afterdark: UI.nmo: the fourth stage open (object {target}, {resets} reset(s) dropped)");
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adler() {
        // Adler-32 of "Wikipedia" (from 1) is 0x11E60398; from 0 the low half is one less
        let mut file = vec![0u8; 12];
        file.extend_from_slice(b"Wikipedia");
        let c = checksum(&file);
        assert_eq!(c & 0xffff, 0x0398 - 1);
    }
}
