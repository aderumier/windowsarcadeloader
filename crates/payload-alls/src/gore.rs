//! House of the Dead: Scarlet Dawn's cut gore (`WAL_HOTD_GORE=1`): the burned-flesh body seen
//! through the bullet holes, the skeleton's bones ejected at death. Port of Michel Denizob's
//! gore restoration patch (MIT, https://github.com/MichelDenizob/hotdsd_gorepatch), which
//! appends the edited assets to the game's 7.8 GB pak: here they go to a small patch pak beside
//! it (`Hodzero-WindowsNoEditor_Gore_P.pak`: Unreal reads a `_P` pak before the others), built
//! at the game's start from the untouched pak and removed when the option is off.
//!
//! The edits (offsets in the decompressed assets; every value checked against the stock
//! asset, another build is left alone):
//! 1. `BPC_ClothDamage.uexp` @5582, `BP_CrashBones.uexp` @2103, `BPC_BurnBodyDamage.uexp`
//!    @2195 (no visible effect, kept as the original): `bGoreFlag = false` (Kismet `EX_False`
//!    0x28) made `EX_True` (0x27). The cloth damage clips the skin (holes), the crash bones
//!    eject at death.
//! 2. `BP_EnemyNormal`: the orphaned `BurnedBody` gets its mesh (`SK_Burned_Middle`: 3 names,
//!    2 imports, a `SkeletalMesh` object property on its template, a preload dependency, the
//!    offsets after them moved), is made visible (its construction script's
//!    `SetVisibility(False)` made `True`: occluded by the skin, seen through the holes) and
//!    hidden in `StartDieMotion` (the corpse turns into the skeleton).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use wal_payload_common::log;

type Result<T> = std::result::Result<T, String>;

const PAK: &str = "Hodzero-WindowsNoEditor.pak";
const GORE_PAK: &str = "Hodzero-WindowsNoEditor_Gore_P.pak";
const PAK_MAGIC: u32 = 0x5A6F_12E1;
/// The on-disk header of a stored (uncompressed) entry, before its data.
const RECORD: usize = 53;

fn rd32(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn rd64(b: &[u8], o: usize) -> i64 {
    i64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}
fn wr32(b: &mut [u8], o: usize, v: i32) {
    b[o..o + 4].copy_from_slice(&v.to_le_bytes());
}
fn wr64(b: &mut [u8], o: usize, v: i64) {
    b[o..o + 8].copy_from_slice(&v.to_le_bytes());
}

fn check(ok: bool, what: &str) -> Result<()> {
    if ok { Ok(()) } else { Err(what.to_string()) }
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// The FName hashes of a new name: non case preserving (upper case, CRC-32 0x04C11DB7 table,
/// MSB first) and case preserving (CRC-32 of the name as 4-byte characters).
fn name_hashes(name: &str) -> (u16, u16) {
    let mut table = [0u32; 256];
    for (n, t) in table.iter_mut().enumerate() {
        let mut c = (n as u32) << 24;
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
        }
        *t = c;
    }
    let mut h = 0u32;
    for c in name.bytes().map(|c| c.to_ascii_uppercase()) {
        h = ((h >> 8) & 0x00FF_FFFF) ^ table[((h ^ c as u32) & 0xFF) as usize];
    }
    let wide: Vec<u8> = name.bytes().flat_map(|c| [c, 0, 0, 0]).collect();
    (h as u16, crc32(&wide) as u16)
}

// The pak (version 4): its index entries

struct Entry {
    /// The raw name field (FString) in the index.
    name_raw: Vec<u8>,
    name: String,
    offset: i64,
    uncompressed: i64,
    compression: i32,
    /// Compressed blocks: start, end (file offsets).
    blocks: Vec<(i64, i64)>,
}

fn fstring(idx: &[u8], p: &mut usize) -> Result<(Vec<u8>, String)> {
    let start = *p;
    let n = rd32(idx, *p);
    *p += 4;
    let text = if n >= 0 {
        let s = String::from_utf8_lossy(&idx[*p..*p + n as usize]).trim_end_matches('\0').to_string();
        *p += n as usize;
        s
    } else {
        *p += (-n) as usize * 2;
        String::new()
    };
    check(*p <= idx.len(), "index string past its end")?;
    Ok((idx[start..*p].to_vec(), text))
}

/// The pak's mount point (raw FString) and the entries named by `wanted` (suffixes), in order.
fn read_index(pak: &mut File, wanted: &[&str]) -> Result<(Vec<u8>, Vec<Entry>)> {
    let size = pak.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
    let mut footer = [0u8; 45];
    pak.seek(SeekFrom::Start(size - 45)).map_err(|e| e.to_string())?;
    pak.read_exact(&mut footer).map_err(|e| e.to_string())?;
    check(u32::from_le_bytes(footer[1..5].try_into().unwrap()) == PAK_MAGIC && rd32(&footer, 5) == 4, "not a version 4 pak")?;
    let (index_offset, index_size) = (rd64(&footer, 9) as u64, rd64(&footer, 17) as usize);
    let mut idx = vec![0u8; index_size];
    pak.seek(SeekFrom::Start(index_offset)).map_err(|e| e.to_string())?;
    pak.read_exact(&mut idx).map_err(|e| e.to_string())?;
    let mut p = 0;
    let (mount, _) = fstring(&idx, &mut p)?;
    let count = rd32(&idx, p);
    p += 4;
    let mut found: Vec<Option<Entry>> = wanted.iter().map(|_| None).collect();
    for _ in 0..count {
        let (name_raw, name) = fstring(&idx, &mut p)?;
        let (offset, uncompressed, compression) = (rd64(&idx, p), rd64(&idx, p + 16), rd32(&idx, p + 24));
        p += 28 + 20;
        let mut blocks = Vec::new();
        if compression != 0 {
            let n = rd32(&idx, p) as usize;
            p += 4;
            for _ in 0..n {
                blocks.push((rd64(&idx, p), rd64(&idx, p + 8)));
                p += 16;
            }
        }
        p += 1 + 4;
        if let Some(i) = wanted.iter().position(|w| name.ends_with(w)) {
            check(found[i].is_none(), "an asset twice in the pak")?;
            found[i] = Some(Entry { name_raw, name, offset, uncompressed, compression, blocks });
        }
    }
    check(p == index_size, "index size")?;
    let entries = found.into_iter().collect::<Option<Vec<_>>>().ok_or("an asset missing from the pak")?;
    Ok((mount, entries))
}

fn read_entry(pak: &mut File, e: &Entry) -> Result<Vec<u8>> {
    let read = |pak: &mut File, at: i64, n: usize| -> Result<Vec<u8>> {
        let mut b = vec![0u8; n];
        pak.seek(SeekFrom::Start(at as u64)).map_err(|err| err.to_string())?;
        pak.read_exact(&mut b).map_err(|err| err.to_string())?;
        Ok(b)
    };
    if e.compression == 0 {
        return read(pak, e.offset + RECORD as i64, e.uncompressed as usize);
    }
    let mut out = Vec::with_capacity(e.uncompressed as usize);
    for &(start, end) in &e.blocks {
        let packed = read(pak, start, (end - start) as usize)?;
        out.extend(miniz_oxide::inflate::decompress_to_vec_zlib(&packed).map_err(|_| format!("{}: inflate", e.name))?);
    }
    check(out.len() as i64 == e.uncompressed, "decompressed size")?;
    Ok(out)
}

// The edits

/// `bGoreFlag`: `EX_False` -> `EX_True`.
fn flip(asset: &mut [u8], at: usize) -> Result<()> {
    check(asset.get(at) == Some(&0x28), "gore flag not EX_False")?;
    asset[at] = 0x27;
    Ok(())
}

// BP_EnemyNormal.uasset summary fields, export records
const H_TOTAL: usize = 24;
const H_NAME_COUNT: usize = 41;
const H_NAME_OFF: usize = 45;
const H_EXPORT_COUNT: usize = 57;
const H_EXPORT_OFF: usize = 61;
const H_IMPORT_COUNT: usize = 65;
const H_IMPORT_OFF: usize = 69;
const H_DEPENDS_OFF: usize = 73;
const H_ASSETREG_OFF: usize = 165;
const H_BULK_START: usize = 169;
const H_PRELOAD_COUNT: usize = 185;
const H_PRELOAD_OFF: usize = 189;
const EXPORT_STRIDE: usize = 104;
const E_SERIAL_SIZE: usize = 28;
const E_SERIAL_OFF: usize = 36;
const E_FIRST_DEP: usize = 84;
const E_CREATE_BEFORE_SER: usize = 92;
/// The `BurnedBody` component template: its export, its data in the .uexp, its `None`
/// terminator, its first preload dependency.
const BB_EXPORT: usize = 398;
const BB_UEXP: usize = 30645;
const BB_NONE: usize = 55;
const BB_FIRST_DEP: i32 = 1411;
const PRELOAD_INSERT: usize = 1413;
const MESH_PATH: &str = "/Game/HodZero_v1/Characters/Enemy/Others/Burned_Middle/Meshes/SK_Burned_Middle";

/// The names of the .uasset (their table: length, text, two hashes).
fn names(ua: &[u8]) -> (Vec<String>, usize) {
    let mut p = rd32(ua, H_NAME_OFF) as usize;
    let mut out = Vec::new();
    for _ in 0..rd32(ua, H_NAME_COUNT) {
        let n = rd32(ua, p);
        p += 4;
        let len = if n >= 0 { n as usize } else { (-n) as usize * 2 };
        out.push(if n > 0 { String::from_utf8_lossy(&ua[p..p + n as usize - 1]).into_owned() } else { String::new() });
        p += len + 4;
    }
    (out, p)
}

fn emit_name(out: &mut Vec<u8>, name: &str) {
    out.extend(((name.len() + 1) as i32).to_le_bytes());
    out.extend(name.as_bytes());
    out.push(0);
    let (h0, h1) = name_hashes(name);
    out.extend(h0.to_le_bytes());
    out.extend(h1.to_le_bytes());
}

fn fname(out: &mut Vec<u8>, index: usize) {
    out.extend((index as i32).to_le_bytes());
    out.extend(0i32.to_le_bytes());
}

/// The burned body's mesh bound: returns the new .uasset and .uexp.
fn bind_mesh(ua: &[u8], ux: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    let total = rd32(ua, H_TOTAL) as usize;
    let name_count = rd32(ua, H_NAME_COUNT) as usize;
    let export_count = rd32(ua, H_EXPORT_COUNT) as usize;
    let export_off = rd32(ua, H_EXPORT_OFF) as usize;
    let import_count = rd32(ua, H_IMPORT_COUNT) as usize;
    let import_off = rd32(ua, H_IMPORT_OFF) as usize;
    let depends_off = rd32(ua, H_DEPENDS_OFF);
    let assetreg_off = rd32(ua, H_ASSETREG_OFF);
    let bulk_start = rd64(ua, H_BULK_START);
    let preload_count = rd32(ua, H_PRELOAD_COUNT);
    let preload_off = rd32(ua, H_PRELOAD_OFF) as usize;
    check(total == ua.len() && total == 90908, "BP_EnemyNormal.uasset size")?;
    check(name_count == 812 && import_count == 306 && export_count == 459, "BP_EnemyNormal counts")?;
    check(import_off == 26720 && export_off == 35288 && depends_off == 83024, "BP_EnemyNormal sections")?;
    check(preload_count == 1511 && preload_off == 84864, "BP_EnemyNormal preloads")?;
    check(ux.len() == 34191 && ux[ux.len() - 4..] == [0xc1, 0x83, 0x2a, 0x9e], "BP_EnemyNormal.uexp")?;
    let (table, names_end) = names(ua);
    check(names_end == import_off, "names end")?;
    let find = |n: &str| table.iter().position(|t| t == n).ok_or(format!("name {n} missing"));
    let (engine, core, package, object_property) = (find("/Script/Engine")?, find("/Script/CoreUObject")?, find("Package")?, find("ObjectProperty")?);
    let bb = export_off + BB_EXPORT * EXPORT_STRIDE;
    check(rd32(ua, bb) == -138 && rd64(ua, bb + E_SERIAL_SIZE) == 67, "BurnedBody export")?;
    check(rd64(ua, bb + E_SERIAL_OFF) - total as i64 == BB_UEXP as i64, "BurnedBody data")?;
    check(rd32(ua, bb + E_FIRST_DEP) == BB_FIRST_DEP && rd32(ua, bb + E_CREATE_BEFORE_SER) == 1, "BurnedBody dependencies")?;

    // names: SkeletalMesh, SK_Burned_Middle, its package
    let (mesh_class, mesh_name, mesh_path) = (name_count, name_count + 1, name_count + 2);
    let mut new_names = Vec::new();
    emit_name(&mut new_names, "SkeletalMesh");
    emit_name(&mut new_names, "SK_Burned_Middle");
    emit_name(&mut new_names, MESH_PATH);
    // imports: the package, the SkeletalMesh object in it
    let (package_import, mesh_import) = (-(import_count as i32 + 1), -(import_count as i32 + 2));
    let mut imports = Vec::new();
    fname(&mut imports, core);
    fname(&mut imports, package);
    imports.extend(0i32.to_le_bytes());
    fname(&mut imports, mesh_path);
    fname(&mut imports, engine);
    fname(&mut imports, mesh_class);
    imports.extend(package_import.to_le_bytes());
    fname(&mut imports, mesh_name);
    // the property: SkeletalMesh (ObjectProperty, 4 bytes, no GUID) = the mesh import
    let mut property = Vec::new();
    fname(&mut property, mesh_class);
    fname(&mut property, object_property);
    property.extend(4i32.to_le_bytes());
    property.extend(0i32.to_le_bytes());
    property.push(0);
    property.extend(mesh_import.to_le_bytes());
    let (dn, di, dp, dx) = (new_names.len(), imports.len(), 4usize, property.len());

    // .uexp: the property before the template's None terminator
    let at = BB_UEXP + BB_NONE;
    check(rd32(ux, at + 4) == 0 && table.get(rd32(ux, at) as usize).map(String::as_str) == Some("None"), "BurnedBody None terminator")?;
    let mut nux = Vec::with_capacity(ux.len() + dx);
    nux.extend(&ux[..at]);
    nux.extend(&property);
    nux.extend(&ux[at..]);

    // .uasset: the names, the imports, the preload entry spliced in
    let preload_at = preload_off + PRELOAD_INSERT * 4;
    let mut nua = Vec::with_capacity(ua.len() + dn + di + dp);
    nua.extend(&ua[..names_end]);
    nua.extend(&new_names);
    nua.extend(&ua[names_end..export_off]);
    nua.extend(&imports);
    nua.extend(&ua[export_off..preload_at]);
    nua.extend(mesh_import.to_le_bytes());
    nua.extend(&ua[preload_at..]);
    wr32(&mut nua, H_NAME_COUNT, name_count as i32 + 3);
    wr32(&mut nua, H_IMPORT_COUNT, import_count as i32 + 2);
    wr32(&mut nua, H_PRELOAD_COUNT, preload_count + 1);
    wr32(&mut nua, H_IMPORT_OFF, (import_off + dn) as i32);
    wr32(&mut nua, H_EXPORT_OFF, (export_off + dn + di) as i32);
    wr32(&mut nua, H_DEPENDS_OFF, depends_off + (dn + di) as i32);
    wr32(&mut nua, H_ASSETREG_OFF, assetreg_off + (dn + di) as i32);
    wr32(&mut nua, H_PRELOAD_OFF, (preload_off + dn + di) as i32);
    wr32(&mut nua, H_TOTAL, (total + dn + di + dp) as i32);
    wr64(&mut nua, H_BULK_START, bulk_start + (dn + di + dp + dx) as i64);
    // the exports' data offsets and first dependencies after the insertions
    let new_export_off = export_off + dn + di;
    for k in 0..export_count {
        let r = new_export_off + k * EXPORT_STRIDE;
        let so = rd64(&nua, r + E_SERIAL_OFF);
        let moved = (dn + di + dp) as i64 + if so - total as i64 > BB_UEXP as i64 { dx as i64 } else { 0 };
        wr64(&mut nua, r + E_SERIAL_OFF, so + moved);
        let first = rd32(&nua, r + E_FIRST_DEP);
        if first >= PRELOAD_INSERT as i32 {
            wr32(&mut nua, r + E_FIRST_DEP, first + 1);
        }
    }
    let nbb = new_export_off + BB_EXPORT * EXPORT_STRIDE;
    wr64(&mut nua, nbb + E_SERIAL_SIZE, 67 + dx as i64);
    let create = rd32(&nua, nbb + E_CREATE_BEFORE_SER);
    wr32(&mut nua, nbb + E_CREATE_BEFORE_SER, create + 1);
    check(rd32(&nua, H_TOTAL) as usize == nua.len(), "new BP_EnemyNormal.uasset size")?;
    Ok((nua, nux))
}

/// The construction script's `BurnedBody.SetVisibility(False, False)` made `(True, False)`.
fn always_visible(ux: &mut [u8]) -> Result<()> {
    flip(ux, 10689)
}

/// `BurnedBody.SetVisibility(False, False)` inserted before `StartDieMotion`'s return.
fn hide_at_death(ua: &mut [u8], ux: &[u8]) -> Result<Vec<u8>> {
    // Context(instance variable 351 BurnedBody, skip 12, no return property)
    // FinalFunction(import -234 SetVisibility) False False EndFunctionParms
    let mut code = vec![0x19, 0x01];
    code.extend(351i32.to_le_bytes());
    code.extend(12i32.to_le_bytes());
    code.extend(0i32.to_le_bytes());
    code.push(0x1c);
    code.extend((-234i32).to_le_bytes());
    code.extend([0x28, 0x28, 0x16]);
    let total = rd32(ua, H_TOTAL) as i64;
    let export_off = rd32(ua, H_EXPORT_OFF) as usize;
    let (table, _) = names(ua);
    let record = (0..rd32(ua, H_EXPORT_COUNT) as usize)
        .map(|k| export_off + k * EXPORT_STRIDE)
        .find(|&r| table.get(rd32(ua, r + 16) as usize).map(String::as_str) == Some("StartDieMotion"))
        .ok_or("StartDieMotion missing")?;
    let (size, so) = (rd64(ua, record + E_SERIAL_SIZE), rd64(ua, record + E_SERIAL_OFF));
    let start = (so - total) as usize + 32;
    // its bytecode sizes: in memory, serialized
    let (memory, serialized) = (rd32(ux, start - 8), rd32(ux, start - 4));
    check(memory == 40 && serialized == 28, "StartDieMotion bytecode sizes")?;
    let at = start + serialized as usize - 3;
    check(ux[at..at + 3] == [0x04, 0x0b, 0x53], "StartDieMotion return")?;
    let mut nux = Vec::with_capacity(ux.len() + code.len());
    nux.extend(&ux[..at]);
    nux.extend(&code);
    nux.extend(&ux[at..]);
    wr32(&mut nux, start - 8, memory + 34);
    wr32(&mut nux, start - 4, serialized + code.len() as i32);
    wr64(ua, record + E_SERIAL_SIZE, size + code.len() as i64);
    for k in 0..rd32(ua, H_EXPORT_COUNT) as usize {
        let r = export_off + k * EXPORT_STRIDE;
        let o = rd64(ua, r + E_SERIAL_OFF);
        if o > so {
            wr64(ua, r + E_SERIAL_OFF, o + code.len() as i64);
        }
    }
    let bulk = rd64(ua, H_BULK_START);
    wr64(ua, H_BULK_START, bulk + code.len() as i64);
    Ok(nux)
}

// The patch pak

/// A stored entry's record: offset, sizes, no compression, SHA-1, not encrypted, no blocks.
fn record(offset: i64, data: &[u8]) -> Vec<u8> {
    let mut r = Vec::with_capacity(RECORD);
    r.extend(offset.to_le_bytes());
    r.extend((data.len() as i64).to_le_bytes());
    r.extend((data.len() as i64).to_le_bytes());
    r.extend(0i32.to_le_bytes());
    r.extend(sha1_smol::Sha1::from(data).digest().bytes());
    r.push(0);
    r.extend(0u32.to_le_bytes());
    r
}

/// The patch pak: the edited assets stored, at the base pak's mount point and paths.
fn build(paks: &Path) -> Result<Vec<u8>> {
    const ASSETS: [&str; 5] = [
        "Blueprints/DamageEffect/BPC_ClothDamage.uexp",
        "Blueprints/CrashBone/BP_CrashBones.uexp",
        "Blueprints/DamageEffect/BPC_BurnBodyDamage.uexp",
        "Human_Common/BP_EnemyNormal.uasset",
        "Human_Common/BP_EnemyNormal.uexp",
    ];
    let mut pak = File::open(paks.join(PAK)).map_err(|e| format!("{PAK}: {e}"))?;
    let (mount, entries) = read_index(&mut pak, &ASSETS)?;
    let mut data: Vec<Vec<u8>> = entries.iter().map(|e| read_entry(&mut pak, e)).collect::<Result<_>>()?;
    for (i, at) in [(0, 5582), (1, 2103), (2, 2195)] {
        flip(&mut data[i], at).map_err(|e| format!("{}: {e}", entries[i].name))?;
    }
    let (mut ua, mut ux) = bind_mesh(&data[3], &data[4])?;
    always_visible(&mut ux)?;
    ux = hide_at_death(&mut ua, &ux)?;
    data[3] = ua;
    data[4] = ux;

    let mut out = Vec::new();
    let mut index = mount;
    index.extend((entries.len() as i32).to_le_bytes());
    for (e, d) in entries.iter().zip(&data) {
        let offset = out.len() as i64;
        out.extend(record(0, d));
        out.extend(d);
        index.extend(&e.name_raw);
        index.extend(record(offset, d));
    }
    // footer: not encrypted, magic, version 4, the index's offset, size, SHA-1
    let index_offset = out.len() as i64;
    out.extend(&index);
    out.push(0);
    out.extend(PAK_MAGIC.to_le_bytes());
    out.extend(4i32.to_le_bytes());
    out.extend(index_offset.to_le_bytes());
    out.extend((index.len() as i64).to_le_bytes());
    out.extend(sha1_smol::Sha1::from(&index).digest().bytes());
    Ok(out)
}

/// Writes the gore pak (option on, not there yet) or removes it.
pub(crate) fn init() {
    let Ok(exe) = std::env::current_exe() else { return };
    // Binaries\Win64\<exe> -> Content\Paks
    let Some(paks) = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()).map(|p| p.join("Content").join("Paks")) else { return };
    let gore = paks.join(GORE_PAK);
    let on = std::env::var("WAL_HOTD_GORE").map_or(true, |v| v.trim() != "0");
    if !on {
        if gore.exists() {
            match std::fs::remove_file(&gore) {
                Ok(()) => log!("gore: {GORE_PAK} removed"),
                Err(e) => log!("gore: {GORE_PAK}: {e}"),
            }
        }
        return;
    }
    if gore.metadata().is_ok_and(|m| m.len() > 0) {
        log!("gore: on ({GORE_PAK})");
        return;
    }
    match build(&paks) {
        Ok(pak) => {
            let tmp = paks.join(format!("{GORE_PAK}.tmp"));
            match std::fs::write(&tmp, &pak).and_then(|_| std::fs::rename(&tmp, &gore)) {
                Ok(()) => log!("gore: {GORE_PAK} written ({} bytes)", pak.len()),
                Err(e) => log!("gore: {GORE_PAK}: {e}"),
            }
        }
        Err(e) => log!("gore: not restored ({e})"),
    }
}
