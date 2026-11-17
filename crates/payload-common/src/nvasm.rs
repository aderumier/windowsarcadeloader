//! NVIDIA assembly (`NV_fragment_program2` / `NV_vertex_program3` options of ARB programs, as
//! Cg's fp40 / vp40 profiles write them) as plain ARB programs, for the GPUs without these
//! options (Mesa).
//!
//! * Options, `SHORT` / `LONG` precisions dropped; opcode precision suffixes (`MULH`, `MADR`)
//!   removed; several statements on a line (`MOV a, b; ADD c, d, e;`) and labels before them.
//! * Condition codes: an instruction with the `C` suffix also copies its result to the `_wCC`
//!   temporary; a conditional write (`MOV R0.x(NE.y), ...`) computes into `_wT` and selects
//!   (`CMP` in fragment programs, `SUB` + `MAD` in vertex ones) on the condition as 0 / 1.
//! * Branches as predicates, every write selected on the current one: fragment `IF` / `ELSE` /
//!   `ENDIF` and `RET`, vertex forward `BRA` to a label (Project DIVA Arcade's skinning);
//!   subroutines inlined at their calls ([`subroutines`]); `REP` loops unrolled
//!   `WAL_RINGEDGE_CG_REPEAT` times, their integer index registers followed ([`loops`]); vertex
//!   backward `BRA` loops unrolled as forward branches ([`backward_loops`]).
//! * The vertex address register's 4 components (`_adr.y`): `ARL` again from the component's
//!   source before each use, through `.x`.
//! * `NRM`, `SGT`, `SLE`, `SEQ`, `SNE`, `SSG`, absolute values (`|x|`), vertex `LRP` / `CMP`
//!   and vertex `_SAT` with ARB instructions; `TXL` as `TXB` (the level as a bias: exact for
//!   the base and last levels, the others on a texture drawn 1:1); vertex `TEX` at a constant
//!   position as a local parameter (`TEXEL_LOCAL`), the others zero.
//! * NVIDIA's arithmetic: logarithms (`LG2`, `POW`) of the absolute value, reciprocals
//!   (`RCP`, `RSQ`) finite (its 0 * infinity is 0), vertex temporaries zeroed first.
//! * Shadow samplers as plain 2D textures (the programs compare the depth themselves), kept
//!   with `WAL_RINGEDGE_CG_SHADOW=1` (Project DIVA Arcade's use the comparison).
//! * Outputs through temporaries (a conditional write reads them back), copied at the end;
//!   `fragment.facing` (absent) as front facing.
//! * The vertex weight (`ARB_vertex_blend`, absent) as generic attribute 1, where NVIDIA reads
//!   it and the game feeds it; vertex inputs Mesa refuses together ([`vertex_inputs`]) as the
//!   ones NVIDIA aliases them with.
//!
//! `NV_fragment_program` programs (`!!FP1.0`) too, NV_gpu_program4 ones (`!!NVvp4.0` /
//! `!!NVfp4.0`) through [`gpu_program4`], and NV_gpu_program5 vertex programs (`!!NVvp5.0`,
//! Project DIVA Arcade Future Tone's skinning: its parameter buffer as local parameters) through
//! [`gpu_program5`]. Programs with `REP` loops without
//! `WAL_RINGEDGE_CG_REPEAT`, other loops, backward branches over branches or other formats: `None`.

const BASES: [&str; 50] = [
    "TXP", "TEX", "TXB", "TXL", "DP3", "DP4", "DPH", "FLR", "FRC", "RCP", "RSQ", "MAX", "MIN", "MAD", "MUL", "ADD", "SUB", "MOV", "SLT", "SGT", "SGE",
    "SLE", "SEQ", "SNE", "SSG", "NRM", "LRP", "CMP", "EX2", "LG2", "POW", "XPD", "ARL", "KIL", "ABS", "DST", "LIT", "SCS", "SIN", "COS", "EXP", "LOG",
    "SWZ", "DP2", "DDX", "DDY", "TRUNC", "DIV", "I2F", "OR",
];

/// The vertex programs' local parameter holding texel (0, 0) of texture unit `n`, at
/// `TEXEL_LOCAL + n`: their texture reads at a constant position.
pub(crate) const TEXEL_LOCAL: u32 = 240;
/// The fragment programs' local parameters `LEVELS_LOCAL + n`: the last level of texture
/// unit n's texture (`TXL`).
pub(crate) const LEVELS_LOCAL: u32 = 240;
/// The fragment programs' texture unit `LEVEL1_UNIT + n`: unit n's texture (`TXL`) with its level
/// of detail held at 1 (the payload binds it before each draw), its level 1 read exactly.
pub(crate) const LEVEL1_UNIT: u32 = 16;
/// NV_gpu_program4's vertex programs' instance (`vertex.instance`, absent in ARB): their local
/// parameter `INSTANCE_LOCAL`, set by the payload's instanced draws (one draw per instance).
pub(crate) const INSTANCE_LOCAL: u32 = 239;
/// NV_gpu_program4's generic interpolant 0 (`result.attrib[0]` / `fragment.attrib[0]`, absent in
/// ARB): Border Break Scramble's vertex programs write their object's color there, the vertex
/// environment parameter `ATTRIB_ENV` (its `env[instance + 59]`), which the payload copies to
/// the fragment environment parameter `ATTRIB_ENV` its fragment programs read instead.
pub(crate) const ATTRIB_ENV: u32 = 59;
/// NV_parameter_buffer_object's vertex parameter buffer 0 (`program.buffer[0]`, absent in Mesa):
/// the vertex programs' local parameters `PALETTE_LOCAL..PALETTE_LOCAL + PALETTE_LEN`, the
/// buffer's contents copied there by the payload before each draw (Project DIVA Arcade Future
/// Tone's skinning matrices: 256 bones of 3 rows).
pub(crate) const PALETTE_LOCAL: u32 = 256;
pub(crate) const PALETTE_LEN: u32 = 768;

const ZERO: &str = "{0,0,0,0}";
const ONE: &str = "{1,1,1,1}";
const BIG: &str = "{1e30,1e30,1e30,1e30}";

/// A program for NVIDIA GPUs only: NVIDIA options or formats, or texture arrays
/// ([`texture_arrays`]).
pub(crate) fn nvidia(text: &str) -> bool {
    // the option after any blanks (Border Break Scramble's `OPTION\t\tNV_fragment_program2`)
    text.split("OPTION").skip(1).any(|r| r.starts_with(char::is_whitespace) && r.trim_start().starts_with("NV_"))
        || text.starts_with("!!FP")
        || text.starts_with("!!VP")
        || text.starts_with("!!NV")
        || texture_arrays(text)
}

/// A program sampling texture arrays (`ARRAY1D`, `ARRAY2D` targets): ARB programs on Mesa have
/// none (no `MESA_texture_array` any more), the program refused. Border Break Scramble's is its
/// shadow maps' debug view (FUN_00888600), its refusal stopping the attract after the logo:
/// sampled as a 1D / 2D texture instead (its first layer's place).
fn texture_arrays(text: &str) -> bool {
    text.contains("ARRAY1D") || text.contains("ARRAY2D")
}

/// `(base opcode, updates the condition code, saturates)`.
fn opcode(op: &str) -> Option<(&'static str, bool, bool)> {
    let (op, sat) = match op.strip_suffix("_SAT") {
        Some(o) => (o, true),
        None => (op, false),
    };
    let mut bases = BASES;
    bases.sort_by_key(|b| std::cmp::Reverse(b.len()));
    bases.iter().find_map(|&b| {
        let rest = op.strip_prefix(b)?;
        matches!(rest, "" | "H" | "R" | "X" | "C" | "HC" | "RC" | "XC" | "C1").then_some((b, rest.ends_with('C') || rest == "C1", sat))
    })
}

/// A destination: register, write mask (`.xyz` or empty), condition with its swizzle
/// (`NE.y`).
fn destination(d: &str) -> Option<(&str, &str, Option<&str>)> {
    let d = d.trim();
    let (reg_mask, cond) = match d.find('(') {
        Some(i) => (d[..i].trim(), Some(d[i + 1..].trim().strip_suffix(')')?.trim())),
        None => (d, None),
    };
    let (reg, mask) = split_swizzle(reg_mask);
    Some((reg, mask, cond))
}

/// `value.xyz` as `(value, ".xyz")`.
fn split_swizzle(s: &str) -> (&str, &str) {
    match s.rfind('.') {
        Some(i) if (2..=5).contains(&(s.len() - i)) && s[i + 1..].chars().all(|c| "xyzw".contains(c)) => (&s[..i], &s[i..]),
        _ => (s, ""),
    }
}

/// A swizzle of 2 or 3 components with its last one repeated (NVIDIA's rule), as ARB takes 1
/// or 4.
fn full_swizzle(sw: &str) -> String {
    match sw.len() {
        3 | 4 => format!("{sw}{}", sw[sw.len() - 1..].repeat(5 - sw.len())),
        _ => sw.to_string(),
    }
}

/// Operands, split on the commas outside brackets, braces and parentheses.
fn operands(args: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0);
    for c in args.chars() {
        match c {
            '[' | '{' | '(' => depth += 1,
            ']' | '}' | ')' => depth -= 1,
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur.trim().to_string());
    out
}

/// An `NV_fragment_program` program (`!!FP1.0`) in the `!!ARBfp1.0` form with NVIDIA
/// instructions [`translate`] takes: its registers declared, its bindings named.
fn fp10(text: &str) -> Option<String> {
    let mut body = text.strip_prefix("!!FP1.0")?.to_string();
    for (from, to) in [("f[COL0]", "fragment.color.primary"), ("f[COL1]", "fragment.color.secondary"), ("f[WPOS]", "fragment.position"), ("f[FOGC]", "fragment.fogcoord"), ("o[COLH]", "oCol"), ("o[COLR]", "oCol")] {
        body = body.replace(from, to);
    }
    for n in (0..16).rev() {
        body = body.replace(&format!("f[TEX{n}]"), &format!("fragment.texcoord[{n}]")).replace(&format!(", TEX{n},"), &format!(", texture[{n}],"));
    }
    if ["f[", "o[", "p[", "DECLARE", "DEFINE"].iter().any(|t| body.contains(t)) {
        return None;
    }
    let mut regs: Vec<String> = Vec::new();
    for word in body.split(|c: char| !c.is_ascii_alphanumeric()) {
        let numbered = word.len() > 1 && word[1..].chars().all(|c| c.is_ascii_digit());
        if numbered && (word.starts_with('H') || word.starts_with('R')) && !regs.iter().any(|r| r == word) {
            regs.push(word.to_string());
        }
    }
    let decl: String = regs.iter().map(|r| format!("TEMP {r};\n")).collect();
    Some(format!("!!ARBfp1.0\n{decl}OUTPUT oCol = result.color;{body}"))
}

/// Vertex inputs Mesa takes: it refuses a generic attribute next to the fixed function input
/// NVIDIA aliases it with (generic 2 and the normal, 10 and texture coordinates 2), NVIDIA reads
/// the same data through both. Then the generic one is read as the fixed function one (0 the
/// position, 2 the normal, 3 the color, 4 the secondary color, 5 the fog coordinate, 8-15 the
/// texture coordinates 0-7; the game's generic arrays mirrored there by `cg.rs`).
fn vertex_inputs(text: &str) -> String {
    let mut text = text.replace("vertex.weight", "vertex.attrib[1]");
    for n in [0, 2, 3, 4, 5, 8, 9, 10, 11, 12, 13, 14, 15] {
        let alias = match n {
            0 => "vertex.position".to_string(),
            2 => "vertex.normal".to_string(),
            3 => "vertex.color".to_string(),
            4 => "vertex.color.secondary".to_string(),
            5 => "vertex.fogcoord".to_string(),
            _ => format!("vertex.texcoord[{}]", n - 8),
        };
        let used = match n {
            3 => text.replace("vertex.color.secondary", "").contains("vertex.color"),
            8 => text.contains("vertex.texcoord[0]") || text.contains("vertex.texcoord.") || text.contains("vertex.texcoord;") || text.contains("vertex.texcoord,"),
            _ => text.contains(&alias),
        };
        if used {
            text = text.replace(&format!("vertex.attrib[{n}]"), &alias);
        }
    }
    text
}

/// The condition `cond` (`NE.y`) on the condition code, as 0 / 1 per component in `dst`.
fn test(out: &mut Vec<String>, dst: &str, cond: &str) -> Option<()> {
    // its swizzle in `rgba` (Border Break Scramble's `LT.a`) as `xyzw`
    let cond = match cond.rsplit_once('.') {
        Some((c, sw)) if !sw.is_empty() && sw.chars().all(|ch| "rgba".contains(ch)) => {
            format!("{c}.{}", sw.chars().map(|ch| match ch { 'r' => 'x', 'g' => 'y', 'b' => 'z', _ => 'w' }).collect::<String>())
        }
        _ => cond.to_string(),
    };
    let (c, sw) = split_swizzle(&cond);
    // `GT1`: the second condition code (NV_gpu_program4's `CC1`)
    let (c, v) = match c.strip_suffix('1') {
        Some(c) => (c, format!("_wCC1{}", full_swizzle(sw))),
        None => (c, format!("_wCC{}", full_swizzle(sw))),
    };
    match c {
        "TR" => out.push(format!("MOV {dst}, {ONE};")),
        "FL" => out.push(format!("MOV {dst}, {ZERO};")),
        "GT" => out.push(format!("SLT {dst}, {ZERO}, {v};")),
        "LT" => out.push(format!("SLT {dst}, {v}, {ZERO};")),
        "GE" => out.push(format!("SGE {dst}, {v}, {ZERO};")),
        "LE" => out.push(format!("SGE {dst}, {ZERO}, {v};")),
        "EQ" => {
            out.push(format!("MAX {dst}, {v}, -{v};"));
            out.push(format!("SGE {dst}, {ZERO}, {dst};"));
        }
        "NE" => {
            out.push(format!("MAX {dst}, {v}, -{v};"));
            out.push(format!("SLT {dst}, {ZERO}, {dst};"));
        }
        _ => return None,
    }
    Some(())
}

/// The condition `cond` true for any of its components, 0 / 1 in `dst.x` (branches).
fn test_any(out: &mut Vec<String>, dst: &str, cond: &str) -> Option<()> {
    test(out, dst, cond)?;
    if split_swizzle(cond).1.len() != 2 {
        for c in ["y", "z", "w"] {
            out.push(format!("MAX {dst}.x, {dst}.x, {dst}.{c};"));
        }
    }
    Some(())
}

/// A condition operand (`NE.x`), not a register.
fn is_condition(s: &str) -> bool {
    let c = split_swizzle(s.trim()).0;
    ["EQ", "NE", "GT", "LT", "GE", "LE", "TR", "FL"].contains(&c.strip_suffix('1').unwrap_or(c))
}

/// The program's statements: the header line, then the rest without its comments split on
/// `;` (a statement can span lines), a label (`name:`) its own statement.
fn statements(text: &str) -> Vec<String> {
    // the header (`!!ARBfp1.0`), up to its version's digits: Border Break Scramble's programs
    // in one line, the first option right after it (`!!ARBfp1.0OPTION ...;...`)
    let version = text.strip_prefix("!!").and_then(|t| {
        let dot = t.find('.')?;
        let digits = t[dot + 1..].find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len() - dot - 1);
        Some(2 + dot + 1 + digits)
    });
    let (header, body) = match version {
        Some(n) if n < text.len() => (&text[..n], &text[n..]),
        _ => text.split_once('\n').unwrap_or((text, "")),
    };
    let mut out = vec![header.trim().to_string()];
    let body: Vec<&str> = body.lines().map(|l| l.split('#').next().unwrap_or("")).collect();
    for s in body.join(" ").split(';') {
        let mut s = s.split_whitespace().collect::<Vec<_>>().join(" ");
        // its labels, several in a row (Border Break Scramble's skinning: `BB4: BB5: BB6:`)
        while let Some(i) = s.find(':') {
            let label = &s[..i];
            if label.is_empty() || !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                break;
            }
            out.push(format!("{label}:"));
            s = s[i + 1..].trim().to_string();
        }
        if !s.is_empty() {
            out.push(s);
        }
    }
    out
}

/// Subroutines (`CAL NAME (GT.x)`, `NAME: ... RET`, Project DIVA Arcade's contour tone maps)
/// inlined at their calls, an `IF` block on the call's condition; their `RET` the last
/// statement only. NV_gpu_program4's opcode modifiers as the suffixes: `.F` dropped, `.CC` /
/// `.CC0` `C`, `.CC1` `C1` (the second condition code), `.SAT` `_SAT`.
fn subroutines(stmts: Vec<String>) -> Option<Vec<String>> {
    let stmts: Vec<String> = loops(stmts)?
        .into_iter()
        .map(|s| {
            let first = s.split_whitespace().next().unwrap_or("");
            if s.starts_with("!!") || !first.contains('.') || !first.starts_with(|c: char| c.is_ascii_uppercase()) {
                return s;
            }
            let mut parts = first.split('.');
            let mut op = parts.next().unwrap_or("").to_string();
            // an unsigned truncation (`TRUNC.U`: a non-negative value) is a floor (Border Break
            // Scramble's edge filter)
            if op == "TRUNC" && first.split('.').any(|m| m == "U") {
                op = "FLR".into();
            }
            let mut sat = false;
            for m in parts {
                match m {
                    "CC" | "CC0" => op.push('C'),
                    "CC1" => op.push_str("C1"),
                    "SAT" => sat = true,
                    _ => {}
                }
            }
            if sat {
                op.push_str("_SAT");
            }
            format!("{op}{}", &s[first.len()..])
        })
        .collect();
    let called: Vec<&str> = stmts.iter().filter_map(|s| s.strip_prefix("CAL ")).map(|r| r.split(|c: char| c == '(' || c.is_whitespace()).next().unwrap_or("")).collect();
    if called.is_empty() {
        return Some(stmts);
    }
    // each subroutine's body: from its label to its final RET (the next label or END after it)
    let mut bodies: Vec<(String, Vec<String>)> = Vec::new();
    let mut main = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for s in &stmts {
        if let Some(label) = s.strip_suffix(':')
            && called.contains(&label)
        {
            bodies.extend(current.take());
            current = Some((label.to_string(), Vec::new()));
            continue;
        }
        if s == "END" || (s.ends_with(':') && current.is_some()) {
            bodies.extend(current.take());
        }
        match &mut current {
            Some((_, body)) => body.push(s.clone()),
            None => main.push(s.clone()),
        }
    }
    bodies.extend(current);
    for (_, body) in bodies.iter_mut() {
        if body.last().is_some_and(|s| s == "RET") {
            body.pop();
        }
        if body.iter().any(|s| s == "RET" || s.starts_with("RET ") || s.starts_with("CAL ") || s.ends_with(':')) {
            return None;
        }
    }
    let mut out = Vec::new();
    for s in main {
        let Some(call) = s.strip_prefix("CAL ") else {
            out.push(s);
            continue;
        };
        let (name, cond) = match call.find('(') {
            Some(i) => (call[..i].trim(), Some(call[i + 1..].trim().strip_suffix(')')?.trim())),
            None => (call.trim(), None),
        };
        let (_, body) = bodies.iter().find(|(n, _)| n == name)?;
        if let Some(cond) = cond {
            out.push(format!("IF {cond}"));
        }
        out.extend(body.iter().cloned());
        if cond.is_some() {
            out.push("ENDIF".into());
        }
    }
    Some(out)
}

/// The loop count `REP` loops are unrolled to (`WAL_RINGEDGE_CG_REPEAT`, Project DIVA Arcade's
/// skin blur's: 5, its parameter's value): `None` without it.
fn repeat() -> Option<usize> {
    static N: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    *N.get_or_init(|| std::env::var("WAL_RINGEDGE_CG_REPEAT").ok().and_then(|v| v.trim().parse().ok()))
}

/// `REP count` loops (NV_gpu_program4, count a parameter) unrolled [`repeat`] times, their
/// integer index registers (`INT TEMP idx`: `MOV.U idx, 0`, `ADD.U idx.x, idx.x, 1`) followed:
/// the arrays they index read at constant indices (`p_coef2d[idx.x]` as `p_coef2d[6]`).
fn loops(stmts: Vec<String>) -> Option<Vec<String>> {
    if !stmts.iter().any(|s| s.starts_with("REP ") || s.starts_with("INT TEMP ")) {
        return Some(stmts);
    }
    let n = repeat()?;
    // the statements with each loop's body repeated, inner loops first
    fn unroll(stmts: &[String], n: usize) -> Option<Vec<String>> {
        let mut out = Vec::new();
        let mut i = 0;
        while i < stmts.len() {
            let s = &stmts[i];
            if !s.starts_with("REP ") {
                if s == "ENDREP" {
                    return None;
                }
                out.push(s.clone());
                i += 1;
                continue;
            }
            let mut depth = 0;
            let end = (i..stmts.len()).find(|&j| {
                if stmts[j].starts_with("REP ") {
                    depth += 1;
                } else if stmts[j] == "ENDREP" {
                    depth -= 1;
                }
                depth == 0
            })?;
            let body = unroll(&stmts[i + 1..end], n)?;
            for _ in 0..n {
                out.extend(body.iter().cloned());
            }
            i = end + 1;
        }
        Some(out)
    }
    let mut ints: Vec<(String, Option<i64>)> = Vec::new();
    let mut out = Vec::new();
    for s in unroll(&stmts, n)? {
        if let Some(names) = s.strip_prefix("INT TEMP ") {
            ints.extend(names.split(',').map(|n| (n.trim().to_string(), None)));
            continue;
        }
        let first = s.split_whitespace().next().unwrap_or("");
        let operands: Vec<String> = operands(s[first.len()..].trim());
        let reg = |o: &str| split_swizzle(o.trim()).0.to_string();
        if let Some(int) = operands.first().and_then(|d| ints.iter().position(|(n, _)| *n == reg(d))) {
            let value = match (first, operands.as_slice()) {
                ("MOV.U" | "MOV.S", [_, v]) => v.trim().parse().ok()?,
                ("ADD.U" | "ADD.S", [_, a, b]) if reg(a) == ints[int].0 => ints[int].1? + b.trim().parse::<i64>().ok()?,
                _ => return None,
            };
            ints[int].1 = Some(value);
            continue;
        }
        let mut s = s;
        for (name, value) in &ints {
            for c in ["x", "y", "z", "w"] {
                let at = format!("[{name}.{c}]");
                if s.contains(&at) {
                    s = s.replace(&at, &format!("[{}]", (*value)?));
                }
            }
            if s.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').any(|w| w == name) {
                return None;
            }
        }
        out.push(s);
    }
    Some(out)
}

/// A scalar instruction's operand with the component ARB requires (NVIDIA reads `.x`).
fn scalar(src: &str) -> String {
    // a component already, `xyzw` or `rgba` (Border Break Scramble's `RCP oa.a, out.a`)
    let rgba = src.rsplit_once('.').is_some_and(|(_, sw)| (1..=4).contains(&sw.len()) && sw.chars().all(|c| "rgba".contains(c)));
    if rgba || !split_swizzle(src).1.is_empty() {
        return src.to_string();
    }
    let (neg, value) = match src.strip_prefix('-') {
        Some(v) => ("-", v),
        None => ("", src),
    };
    if value.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        format!("{neg}{{{value}}}.x")
    } else {
        format!("{neg}{value}.x")
    }
}

/// `WAL_RINGEDGE_CG_SHADOW=1`: shadow samplers kept (ARB_fragment_program_shadow, the
/// texture's depth comparison: Project DIVA Arcade's programs use its result); else plain
/// textures (Initial D's programs compare the depth themselves).
fn shadow_samplers() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("WAL_RINGEDGE_CG_SHADOW").is_ok_and(|v| v.trim() == "1"))
}

/// `name[k]` (an array of bindings declared `name[] = { base[first..last] }`) as `base[first+k]`
/// in `s`: ARB has no arrays of attributes or outputs. A relative index: `None`.
fn binding_arrays(s: &str, arrays: &[(String, String, usize)]) -> Option<String> {
    let mut s = s.to_string();
    for (name, base, first) in arrays {
        let mut out = String::new();
        let mut rest = s.as_str();
        while let Some(i) = rest.find(&format!("{name}[")) {
            let before = rest[..i].chars().last();
            out.push_str(&rest[..i]);
            let after = &rest[i + name.len() + 1..];
            if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
                out.push_str(&rest[i..i + name.len() + 1]);
                rest = after;
                continue;
            }
            let end = after.find(']')?;
            let k: usize = after[..end].trim().parse().ok()?;
            out.push_str(&format!("{base}[{}]", first + k));
            rest = &after[end + 1..];
        }
        out.push_str(rest);
        s = out;
    }
    Some(s)
}

/// An NV_gpu_program4 program (`!!NVvp4.0` / `!!NVfp4.0`, Border Break Scramble's
/// `rom/shader.pfa`) in the NVIDIA option form [`translate`] takes:
/// * its arrays of attributes and outputs (`vertex_attrib7[0]`) as the bindings;
/// * its integer instructions (`MOV.S`, `MAD.S`, `SNE.S`: object and bone indices, small
///   integers) on floats, `TRUNC` translated; relative addressing through a temporary
///   (`env[R2.w + 112]`) through the address register (`ARL A0.x, R2.w`);
/// * the vertex inputs NVIDIA aliases (generic 0, 2, 3, 8-15) as the fixed function ones
///   (`vertex_inputs`), the instance as [`INSTANCE_LOCAL`];
/// * the generic interpolant 0 as [`ATTRIB_ENV`]: its vertex writes dropped, its fragment reads
///   the fragment environment parameter.
///
/// Fragment programs with relative addressing: `None`.
fn gpu_program4(text: &str) -> Option<String> {
    let vertex = text.starts_with("!!NVvp4.0");
    let stmts = statements(text);
    let mut arrays: Vec<(String, String, usize)> = Vec::new();
    let mut out = vec![if vertex { "!!ARBvp1.0".to_string() } else { stmts[0].clone() }];
    let mut address = None;
    for s in &stmts[1..] {
        if let Some(rest) = s.strip_prefix("ATTRIB ").or_else(|| s.strip_prefix("OUTPUT "))
            && let Some((name, binding)) = rest.split_once('=')
            && let Some(name) = name.trim().strip_suffix("[]")
        {
            let inner = binding.trim().strip_prefix('{')?.strip_suffix('}')?.trim();
            let (base, range) = inner.strip_suffix(']')?.split_once('[')?;
            let first = range.split("..").next()?.trim().parse().ok()?;
            arrays.push((name.trim().to_string(), base.trim().to_string(), first));
            continue;
        }
        let mut s = binding_arrays(s, &arrays)?;
        if s.starts_with("OPTION ") {
            out.push(s);
            continue;
        }
        if vertex {
            let first = s.split_whitespace().next().unwrap_or("");
            if s[first.len()..].trim_start().starts_with("result.attrib") {
                continue;
            }
            s = s.replace("vertex.instance", &format!("program.local[{INSTANCE_LOCAL}]"));
            for n in [0, 2, 3, 8, 9, 10, 11, 12, 13, 14, 15] {
                let alias = match n {
                    0 => "vertex.position".to_string(),
                    2 => "vertex.normal".to_string(),
                    3 => "vertex.color".to_string(),
                    _ => format!("vertex.texcoord[{}]", n - 8),
                };
                s = s.replace(&format!("vertex.attrib[{n}]"), &alias);
            }
        } else {
            s = s.replace("fragment.attrib[0]", &format!("program.env[{ATTRIB_ENV}]"));
            if s.contains("fragment.attrib[") {
                return None;
            }
        }
        // relative addressing through a temporary: `[R2.w + 112]`, `[R2.w]`
        let mut relative: Option<String> = None;
        let mut rewritten = String::new();
        let mut rest = s.as_str();
        while let Some(i) = rest.find('[') {
            let end = rest[i..].find(']')? + i;
            let inner = rest[i + 1..end].trim();
            rewritten.push_str(&rest[..=i]);
            let (reg, offset) = match inner.find(['+', '-']) {
                Some(j) => (inner[..j].trim(), &inner[j..]),
                None => (inner, ""),
            };
            if !reg.is_empty() && reg.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') && reg.contains('.') && !reg.contains("..") {
                if !vertex || relative.as_deref().is_some_and(|r| r != reg) {
                    return None;
                }
                relative = Some(reg.to_string());
                match offset.split_at_checked(1) {
                    Some((sign, n)) => rewritten.push_str(&format!("A0.x {sign} {}", n.trim())),
                    None => rewritten.push_str("A0.x"),
                }
            } else {
                rewritten.push_str(&rest[i + 1..end]);
            }
            rewritten.push(']');
            rest = &rest[end + 1..];
        }
        rewritten.push_str(rest);
        if let Some(reg) = relative {
            address.get_or_insert(out.len());
            out.push(format!("ARL A0.x, {reg}"));
        }
        out.push(rewritten);
    }
    if let Some(at) = address {
        let at = out.iter().take(at).position(|s| !s.starts_with("!!") && !s.starts_with("OPTION ")).unwrap_or(at);
        out.insert(at, "ADDRESS A0".into());
    }
    let mut text = out[0].clone() + "\n";
    for s in &out[1..] {
        text.push_str(s);
        text.push_str(if s.ends_with(':') { "\n" } else { ";\n" });
    }
    Some(text)
}

/// An NV_gpu_program5 vertex program (`!!NVvp5.0`, Project DIVA Arcade Future Tone's skinning)
/// as an NV_gpu_program4 one ([`gpu_program4`]): its parameter buffer array
/// (`BUFFER4 m[] = { program.buffer[0] }`) as the local parameters [`PALETTE_LOCAL`]; its integer
/// temporaries converted from a float (`CVT.S32.F32 a, f`: the bone indices, whole numbers) read
/// as that float, which relative addressing floors. Other integer temporaries or buffers: `None`.
fn gpu_program5(text: &str) -> Option<String> {
    let stmts = statements(text);
    let mut out = vec!["!!NVvp4.0".to_string()];
    // the integer temporaries, the float each one was converted from
    let mut ints: Vec<(String, Option<String>)> = Vec::new();
    for s in &stmts[1..] {
        if let Some(names) = s.strip_prefix("INT TEMP ") {
            ints.extend(names.split(',').map(|n| (n.trim().to_string(), None)));
            continue;
        }
        if let Some(rest) = s.strip_prefix("BUFFER4 ").or_else(|| s.strip_prefix("BUFFER ")) {
            let (name, binding) = rest.split_once('=')?;
            let name = name.trim().strip_suffix("[]")?.trim();
            if binding.replace(' ', "") != "{program.buffer[0]}" {
                return None;
            }
            out.push(format!("PARAM {name}[{PALETTE_LEN}] = {{ program.local[{PALETTE_LOCAL}..{}] }}", PALETTE_LOCAL + PALETTE_LEN - 1));
            continue;
        }
        if let Some(rest) = s.strip_prefix("CVT.S32.F32 ") {
            let (dst, src) = rest.split_once(',')?;
            let src = src.trim();
            if !src.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                return None;
            }
            ints.iter_mut().find(|(n, _)| n == dst.trim())?.1 = Some(src.to_string());
            continue;
        }
        let mut s = s.clone();
        for (name, src) in &ints {
            s = replace_word(&s, name, src.as_deref())?;
        }
        out.push(s);
    }
    let mut text = out[0].clone() + "\n";
    for s in &out[1..] {
        text.push_str(s);
        text.push_str(if s.ends_with(':') { "\n" } else { ";\n" });
    }
    Some(text)
}

/// `name` (a whole word) in `s` replaced with `with`; `None` if it is there and `with` is `None`.
fn replace_word(s: &str, name: &str, with: Option<&str>) -> Option<String> {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find(name) {
        let before = rest[..i].chars().last();
        let after = rest[i + name.len()..].chars().next();
        out.push_str(&rest[..i]);
        if before.is_some_and(ident) || after.is_some_and(ident) {
            out.push_str(name);
        } else {
            out.push_str(with?);
        }
        rest = &rest[i + name.len()..];
    }
    out.push_str(rest);
    Some(out)
}

/// The iterations a backward branch loop is unrolled to ([`backward_loops`]): Initial D's light
/// loops (`for( i=0; i<sLightNumber; i++ )`), S_MODEL_LIGHT_MAX lights at most.
const BACKWARD_REPEAT: usize = 4;

/// Vertex loops as Cg's vp40 writes them (`BB2: ... BRA BB2 (NE.x);`, Initial D's precompiled
/// lit shaders `shader\Cg\vs\vs_L*.asm_gl`) unrolled [`BACKWARD_REPEAT`] times, each copy but
/// the last followed by a forward branch out of the loop on the opposite condition (the
/// forward branches' predicates do the rest). Loops with labels or branches inside: `None`.
fn backward_loops(stmts: Vec<String>) -> Option<Vec<String>> {
    let target = |s: &str| s.strip_prefix("BRA ").map(|r| r.split(|c: char| c == '(' || c.is_whitespace()).next().unwrap_or("").to_string());
    let backward = |stmts: &[String], i: usize| target(&stmts[i]).and_then(|t| stmts[..i].iter().position(|s| *s == format!("{t}:")));
    if !(0..stmts.len()).any(|i| backward(&stmts, i).is_some()) {
        return Some(stmts);
    }
    let mut out = Vec::new();
    let mut i = 0;
    let mut loops = 0;
    while i < stmts.len() {
        // the loop starting here: its label, its body up to the backward branch
        let end = stmts[i].strip_suffix(':').and_then(|label| (i + 1..stmts.len()).find(|&j| target(&stmts[j]).as_deref() == Some(label)));
        let Some(end) = end else {
            out.push(stmts[i].clone());
            i += 1;
            continue;
        };
        let body = &stmts[i + 1..end];
        if body.iter().any(|s| s.ends_with(':') || s.starts_with("BRA ") || s.starts_with("CAL ") || s.starts_with("RET")) {
            return None;
        }
        let rest = stmts[end][4..].trim();
        let cond = rest.find('(').map(|at| rest[at + 1..].trim().strip_suffix(')').map(str::trim)).unwrap_or(Some("TR"))?;
        let (code, swizzle) = cond.split_once('.').map_or((cond, None), |(c, s)| (c, Some(s)));
        let opposite = match code {
            "EQ" => "NE",
            "NE" => "EQ",
            "LT" => "GE",
            "GE" => "LT",
            "GT" => "LE",
            "LE" => "GT",
            "TR" => "FL",
            "FL" => "TR",
            _ => return None,
        };
        let opposite = swizzle.map_or(opposite.to_string(), |s| format!("{opposite}.{s}"));
        let exit = format!("_wX{loops}");
        loops += 1;
        for n in 0..BACKWARD_REPEAT {
            out.extend(body.iter().cloned());
            if n + 1 < BACKWARD_REPEAT {
                out.push(format!("BRA {exit} ({opposite})"));
            }
        }
        out.push(format!("{exit}:"));
        i = end + 1;
    }
    Some(out)
}

pub(crate) fn translate(text: &str) -> Option<String> {
    if text.starts_with("!!NVvp5.0") {
        return translate_converted(&gpu_program4(&gpu_program5(text)?)?);
    }
    if text.starts_with("!!FP1.0") {
        return translate(&fp10(text)?);
    }
    if text.starts_with("!!NVvp4.0") || text.starts_with("!!NVfp4.0") && (text.contains("[]") || text.contains("fragment.attrib")) {
        return translate_converted(&gpu_program4(text)?);
    }
    translate_converted(text)
}

fn translate_converted(text: &str) -> Option<String> {
    // NV_gpu_program4's (`!!NVfp4.0`, DIVA's blur filters) without its integer instructions
    let fragment = text.starts_with("!!ARBfp1.0") || text.starts_with("!!NVfp4.0");
    if !fragment && !text.starts_with("!!ARBvp1.0") {
        return None;
    }
    let text = if fragment { text.to_string() } else { vertex_inputs(text) };
    let mut out: Vec<String> = Vec::new();
    let mut temps: Vec<String> = Vec::new();
    let add_temp = |temps: &mut Vec<String>, t: &str| {
        if !temps.iter().any(|x| x == t) {
            temps.push(t.to_string());
        }
    };
    // the header and options, the added temporaries after them
    let mut head = 0;
    // outputs: (name, binding, written); `result.` written directly: (binding, temporary)
    let mut outputs: Vec<(String, String, bool)> = Vec::new();
    let mut results: Vec<(String, String, String)> = Vec::new();
    // predicates: the open IF blocks (their numbers), the live lanes (`_wL`, after a RET or BRA)
    let mut levels: Vec<usize> = Vec::new();
    let mut blocks = 0;
    let mut live = false;
    let mut dead = false;
    // forward branches: (label, accumulated lanes temporary)
    let mut branches: Vec<(String, String)> = Vec::new();
    let mut labels: Vec<String> = Vec::new();
    // vertex address registers: (name, the source of each component, the component in .x)
    let mut addresses: Vec<(String, [String; 4], usize)> = Vec::new();
    // the temporaries declared before the first instruction (at `body` in `out`)
    let mut declared: Vec<String> = Vec::new();
    let mut body: Option<usize> = None;
    for s in subroutines(backward_loops(statements(&text))?)? {
        let mut s = s.as_str();
        for p in ["SHORT ", "LONG "] {
            if let Some(r) = s.strip_prefix(p) {
                s = r.trim_start();
            }
        }
        if s.starts_with("!!") {
            out.push(if fragment { "!!ARBfp1.0".into() } else { s.to_string() });
            head = out.len();
            continue;
        }
        if let Some(option) = s.strip_prefix("OPTION ") {
            if !option.starts_with("NV_") && (!option.starts_with("ARB_fragment_program_shadow") || shadow_samplers()) {
                out.push(format!("{s};"));
                head = out.len();
            }
            continue;
        }
        let cur = levels.last().map(|n| format!("_wP{n}")).or(live.then(|| "_wL".to_string()));
        if let Some(label) = s.strip_suffix(':') {
            body.get_or_insert(out.len());
            labels.push(label.to_string());
            if let Some((_, acc)) = branches.iter().find(|(l, _)| l == label) {
                out.push(format!("ADD _wL.x, _wL.x, {acc}.x;"));
            }
            continue;
        }
        let first = s.split_whitespace().next().unwrap_or("");
        match first {
            "END" => {
                for (name, binding, written) in &outputs {
                    if *written {
                        out.push(format!("MOV {binding}, {name};"));
                    }
                }
                // the components written only (Cg's eye vector in the tangent's .w)
                for (binding, t, lanes) in &results {
                    let mask: String = "xyzw".chars().filter(|c| lanes.contains(*c)).collect();
                    let mask = if mask.len() == 4 { String::new() } else { format!(".{mask}") };
                    out.push(format!("MOV {binding}{mask}, {t};"));
                }
                out.push("END".into());
                break;
            }
            "TEMP" | "PARAM" | "ALIAS" | "ADDRESS" => {
                if first == "TEMP" && body.is_none() {
                    declared.extend(s[4..].split(',').map(|t| t.trim().to_string()));
                }
                out.push(format!("{s};"));
                continue;
            }
            "ATTRIB" => {
                match s.split_once('=') {
                    Some((name, binding)) if binding.trim() == "fragment.facing" => out.push(format!("PARAM {} = {ONE};", name[6..].trim())),
                    _ => out.push(format!("{s};")),
                }
                continue;
            }
            "OUTPUT" => {
                let (name, binding) = s[6..].split_once('=')?;
                let name = name.trim().to_string();
                if body.is_none() {
                    declared.push(name.clone());
                }
                out.push(format!("TEMP {name};"));
                outputs.push((name, binding.trim().to_string(), false));
                continue;
            }
            "REP" | "ENDREP" | "LOOP" | "ENDLOOP" | "BRK" | "TXD" | "PK2H" | "PK2US" | "PK4B" | "PK4UB" | "UP2H" | "UP2US" | "UP4B" | "UP4UB" => return None,
            _ => {}
        }
        body.get_or_insert(out.len());
        if dead {
            continue;
        }
        let rest = s[first.len()..].trim();
        match first {
            "IF" => {
                let n = blocks;
                blocks += 1;
                let (c, p) = (format!("_wC{n}"), format!("_wP{n}"));
                add_temp(&mut temps, &c);
                add_temp(&mut temps, &p);
                test_any(&mut out, &c, rest)?;
                match &cur {
                    Some(parent) => out.push(format!("MUL {p}.x, {c}.x, {parent}.x;")),
                    None => out.push(format!("MOV {p}.x, {c}.x;")),
                }
                levels.push(n);
                continue;
            }
            "ELSE" => {
                let n = *levels.last()?;
                let parent = levels.iter().rev().nth(1).map(|n| format!("_wP{n}")).or(live.then(|| "_wL".to_string()));
                out.push(format!("SUB _wP{n}.x, {ONE}, _wC{n}.x;"));
                if let Some(parent) = parent {
                    out.push(format!("MUL _wP{n}.x, _wP{n}.x, {parent}.x;"));
                }
                continue;
            }
            "ENDIF" => {
                levels.pop()?;
                continue;
            }
            "RET" | "BRA" => {
                // the lanes leaving: the condition's under the current predicate
                let (target, cond) = match first {
                    "BRA" => {
                        let (label, cond) = match rest.find('(') {
                            Some(i) => (rest[..i].trim(), Some(rest[i + 1..].trim().strip_suffix(')')?.trim())),
                            None => (rest, None),
                        };
                        // forward only, outside IF blocks
                        if labels.iter().any(|l| l == label) || !levels.is_empty() {
                            return None;
                        }
                        (Some(label.to_string()), cond)
                    }
                    _ => (None, rest.strip_prefix('(').and_then(|r| r.strip_suffix(')')).map(str::trim).or((!rest.is_empty()).then_some(rest))),
                };
                let k = match (cond, &cur) {
                    (None, None) => None,
                    (None, Some(p)) => Some(format!("{p}.x")),
                    (Some(cond), p) => {
                        add_temp(&mut temps, "_wK");
                        test_any(&mut out, "_wK", cond)?;
                        if let Some(p) = p {
                            out.push(format!("MUL _wK.x, _wK.x, {p}.x;"));
                        }
                        Some("_wK.x".to_string())
                    }
                };
                if let Some(label) = target {
                    match branches.iter().find(|(l, _)| *l == label) {
                        Some((_, acc)) => out.push(format!("ADD {acc}.x, {acc}.x, {};", k.as_deref().unwrap_or(ONE))),
                        None => {
                            let acc = format!("_wB{}", branches.len());
                            add_temp(&mut temps, &acc);
                            out.push(format!("MOV {acc}.x, {};", k.as_deref().unwrap_or(ONE)));
                            branches.push((label, acc));
                        }
                    }
                    // the lanes branching leave the live ones (not every lane branched to the
                    // label so far: -1 after a vertex's first skinning branch, DIVA's one bone
                    // vertices, their writes after the label 2 * old - new)
                    add_temp(&mut temps, "_wL");
                    let k = k.as_deref().unwrap_or(ONE);
                    if live {
                        out.push(format!("SUB _wL.x, _wL.x, {k};"));
                    } else {
                        out.push(format!("SUB _wL.x, {ONE}, {k};"));
                    }
                    live = true;
                    continue;
                }
                let Some(k) = k else {
                    // every lane returns
                    dead = true;
                    continue;
                };
                // RET: the lanes off every open block and the live ones
                add_temp(&mut temps, "_wK");
                add_temp(&mut temps, "_wL");
                out.push(format!("SUB _wK.y, {ONE}, {k};"));
                for n in &levels {
                    out.push(format!("MUL _wP{n}.x, _wP{n}.x, _wK.y;"));
                }
                if live {
                    out.push("MUL _wL.x, _wL.x, _wK.y;".into());
                } else {
                    out.push("MOV _wL.x, _wK.y;".into());
                }
                live = true;
                continue;
            }
            _ => {}
        }
        if !first.chars().next().is_some_and(|c| c.is_ascii_uppercase()) || rest.is_empty() {
            out.push(format!("{s};"));
            continue;
        }
        let (base, cc, sat) = opcode(first)?;
        let mut args = rest.to_string();
        if !fragment && base != "ARL" {
            // the address component used, loaded in .x
            for (name, sources, loaded) in addresses.iter_mut() {
                let used: Vec<usize> = (0..4).filter(|&c| args.contains(&format!("{name}.{}", &"xyzw"[c..c + 1]))).collect();
                match used.as_slice() {
                    [] => {}
                    [c] => {
                        if *c != *loaded {
                            out.push(format!("ARL {name}.x, {};", sources[*c]));
                            *loaded = *c;
                        }
                        args = args.replace(&format!("{name}.{}", &"xyzw"[*c..*c + 1]), &format!("{name}.x"));
                    }
                    _ => return None,
                }
            }
        }
        if base == "KIL" {
            if is_condition(&args) {
                add_temp(&mut temps, "_wK");
                test_any(&mut out, "_wK", &args)?;
                if let Some(p) = &cur {
                    out.push(format!("MUL _wK.x, _wK.x, {p}.x;"));
                }
                out.push("KIL -_wK.x;".into());
            } else if let Some(p) = &cur {
                add_temp(&mut temps, "_wK");
                out.push(format!("CMP _wK, -{p}.x, {args}, {ZERO};"));
                out.push("KIL _wK;".into());
            } else {
                out.push(format!("KIL {args};"));
            }
            continue;
        }
        let parts = operands(&args);
        let (reg, mask, cond) = destination(&parts[0])?;
        // a texel offset (`TEX r, c, texture[0], 2D, (-1, 0)`, NV_gpu_program4) dropped: DIVA's
        // blur filters sample their center only
        let mut sources: Vec<String> = parts[1..].iter().filter(|x| !x.starts_with('(')).map(|x| if shadow_samplers() { x.to_string() } else { x.replace("SHADOW2D", "2D").replace("SHADOWRECT", "RECT").replace("SHADOWCUBE", "CUBE").replace("SHADOW1D", "1D") }).map(|x| match x.as_str() { "ARRAY2D" => "2D".to_string(), "ARRAY1D" => "1D".to_string(), _ => x }).collect();
        if base == "ARL" {
            let src = sources.first()?;
            let (value, sw) = split_swizzle(src);
            let sw = match full_swizzle(sw) {
                s if s.is_empty() => ".xyzw".to_string(),
                s if s.len() == 2 => s.repeat(4).replace('.', ""),
                s => s,
            };
            let comps: Vec<char> = sw.trim_start_matches('.').chars().collect();
            let sources: [String; 4] = std::array::from_fn(|i| format!("{value}.{}", comps[i]));
            out.push(format!("ARL {reg}.x, {};", sources[0]));
            match addresses.iter_mut().find(|(n, _, _)| n == reg) {
                Some(a) => *a = (reg.to_string(), sources, 0),
                None => addresses.push((reg.to_string(), sources, 0)),
            }
            continue;
        }
        // NVIDIA's logarithms (LG2, POW's) of the absolute value, Mesa's of the value: NaN on a
        // negative one (DIVA's specular powers of the cosines behind the light, NaN in its
        // characters' pixels, blown out in white blooms around black ones, its exposure NaN)
        if (base == "LG2" || base == "POW")
            && let Some(src) = sources.first_mut()
            && !src.contains('|')
        {
            *src = format!("|{}|", src.strip_prefix('-').unwrap_or(src));
        }
        // |x|: MAX t, x, -x first
        for src in sources.iter_mut() {
            let (neg, rest) = match src.strip_prefix('-') {
                Some(r) => ("-", r),
                None => ("", src.as_str()),
            };
            let Some(inner) = rest.strip_prefix('|').and_then(|r| r.strip_suffix('|')) else { continue };
            let (value, sw) = split_swizzle(inner);
            let t = format!("_wA{}", temps.iter().filter(|t| t.starts_with("_wA")).count());
            add_temp(&mut temps, &t);
            out.push(format!("MAX {t}, {value}, -{value};"));
            *src = format!("{neg}{t}{sw}");
        }
        if ["POW", "RCP", "RSQ", "EX2", "LG2", "EXP", "LOG", "SIN", "COS", "SCS"].contains(&base) {
            sources = sources.iter().map(|x| scalar(x)).collect();
        }
        // the register written: an output's temporary, `result.` through one
        let reg = if reg.starts_with("result.") {
            let lanes = if mask.is_empty() { "xyzw" } else { &mask[1..] };
            match results.iter_mut().find(|(b, _, _)| b == reg) {
                Some((_, t, l)) => {
                    l.push_str(lanes);
                    t.clone()
                }
                None => {
                    let t = format!("_wR{}", results.len());
                    add_temp(&mut temps, &t);
                    results.push((reg.to_string(), t.clone(), lanes.to_string()));
                    t
                }
            }
        } else {
            reg.to_string()
        };
        if let Some(o) = outputs.iter_mut().find(|o| o.0 == reg) {
            o.2 = true;
        }
        let cc_only = reg == "RC" || reg == "HC";
        let select = cond.is_some() || cur.is_some();
        let target = if select || cc_only { "_wT".to_string() } else { reg.clone() };
        if target == "_wT" {
            add_temp(&mut temps, "_wT");
        }
        let satx = if sat && fragment { "_SAT" } else { "" };
        let dst = format!("{target}{mask}");
        match base {
            "NRM" => {
                let a = sources.first()?;
                add_temp(&mut temps, "_wN");
                out.push(format!("DP3 _wN.w, {a}, {a};"));
                out.push("RSQ _wN.w, _wN.w;".into());
                out.push(format!("MIN _wN.w, _wN.w, {BIG};"));
                out.push(format!("MUL{satx} {dst}, {a}, _wN.w;"));
            }
            "SGT" | "SLE" => {
                let [a, b] = sources.as_slice() else { return None };
                let op = if base == "SGT" { "SLT" } else { "SGE" };
                out.push(format!("{op}{satx} {dst}, {b}, {a};"));
            }
            "SEQ" | "SNE" => {
                let [a, b] = sources.as_slice() else { return None };
                add_temp(&mut temps, "_wE");
                out.push(format!("SUB _wE, {a}, {b};"));
                out.push("MAX _wE, _wE, -_wE;".into());
                let op = if base == "SEQ" { "SGE" } else { "SLT" };
                out.push(format!("{op}{satx} {dst}, -_wE, {ZERO};"));
            }
            "SSG" => {
                let a = sources.first()?;
                add_temp(&mut temps, "_wE");
                add_temp(&mut temps, "_wF");
                out.push(format!("SLT _wE, {ZERO}, {a};"));
                out.push(format!("SLT _wF, {a}, {ZERO};"));
                out.push(format!("SUB{satx} {dst}, _wE, _wF;"));
            }
            // the level as a bias past the computed one (no explicit level in ARB): the base
            // level far below it, the texture's last one (`LEVELS_LOCAL + n`, the payload's)
            // far above it, the others a quarter above (exact on a texture drawn 1:1). Project
            // DIVA Arcade's YCbCr sprites: luma in level 0, chroma in level 1, the last one.
            // Level 1 below the last one (Border Break Scramble's YCbCr textures: luma, Cr, Cb
            // in levels 0, 1, 2; biased, its Cr took some luma: the bright places pink): read
            // exactly from unit `LEVEL1_UNIT + n`, the same texture with its level of detail
            // held at 1 (the payload's sampler)
            "TXL" if fragment => {
                let (coord, rest) = sources.split_first()?;
                let unit = rest.first().and_then(|t| t.strip_prefix("texture[")?.strip_suffix(']')?.parse::<u32>().ok())?;
                let level1 = rest.get(1).is_some_and(|t| t == "2D") && unit < LEVEL1_UNIT;
                add_temp(&mut temps, "_wE");
                add_temp(&mut temps, "_wF");
                out.push(format!("MOV _wE, {coord};"));
                if level1 {
                    add_temp(&mut temps, "_wG");
                    add_temp(&mut temps, "_wH");
                    out.push("MOV _wG.x, _wE.w;".into());
                }
                out.push(format!("SUB _wF.y, _wE.w, program.local[{}].x;", LEVELS_LOCAL + unit));
                out.push("SGE _wF.y, _wF.y, {-0.5,-0.5,-0.5,-0.5};".into());
                out.push("SGE _wF.x, _wE.w, {0.5,0.5,0.5,0.5};".into());
                out.push("ADD _wE.w, _wE.w, {0.25,0.25,0.25,0.25};".into());
                out.push("SUB _wF.z, {16,16,16,16}, _wE.w;".into());
                out.push("MAD _wE.w, _wF.y, _wF.z, _wE.w;".into());
                out.push("ADD _wF.z, _wE.w, {16,16,16,16};".into());
                out.push("MAD _wE.w, _wF.x, _wF.z, {-16,-16,-16,-16};".into());
                if !level1 {
                    out.push(format!("TXB{satx} {dst}, _wE, {};", rest.join(", ")));
                } else {
                    // level 1 (0.5 to 1.5) of a texture with more levels: the exact read
                    out.push("SGE _wF.x, _wG.x, {0.5,0.5,0.5,0.5};".into());
                    out.push("SLT _wF.y, _wG.x, {1.5,1.5,1.5,1.5};".into());
                    out.push("MUL _wF.x, _wF.x, _wF.y;".into());
                    out.push(format!("SGE _wF.y, program.local[{}].x, {{1.5,1.5,1.5,1.5}};", LEVELS_LOCAL + unit));
                    out.push("MUL _wG.y, _wF.x, _wF.y;".into());
                    out.push(format!("TEX _wH, _wE, texture[{}], 2D;", LEVEL1_UNIT + unit));
                    out.push(format!("TXB _wF, _wE, {};", rest.join(", ")));
                    out.push(format!("LRP{satx} {dst}, _wG.y, _wH, _wF;"));
                }
            }
            "LRP" if !fragment => {
                let [t, a, b] = sources.as_slice() else { return None };
                add_temp(&mut temps, "_wE");
                out.push(format!("SUB _wE, {a}, {b};"));
                out.push(format!("MAD {dst}, {t}, _wE, {b};"));
            }
            "CMP" if !fragment => {
                let [a, b, c] = sources.as_slice() else { return None };
                add_temp(&mut temps, "_wE");
                add_temp(&mut temps, "_wF");
                out.push(format!("SLT _wE, {a}, {ZERO};"));
                out.push(format!("SUB _wF, {b}, {c};"));
                out.push(format!("MAD {dst}, _wE, _wF, {c};"));
            }
            // towards zero (NV_gpu_program4's float to integer): the floor of the absolute value,
            // its sign back
            "TRUNC" => {
                let a = sources.first()?;
                add_temp(&mut temps, "_wE");
                add_temp(&mut temps, "_wF");
                out.push(format!("MAX _wE, {a}, -{a};"));
                out.push("FLR _wE, _wE;".into());
                out.push(format!("SLT _wF, {a}, {ZERO};"));
                out.push("MUL _wF, _wF, _wE;".into());
                out.push(format!("MAD{satx} {dst}, _wF, {{-2,-2,-2,-2}}, _wE;"));
            }
            // a vector by a scalar (NV_gpu_program4), through a finite reciprocal
            "DIV" => {
                let [a, b] = sources.as_slice() else { return None };
                add_temp(&mut temps, "_wE");
                out.push(format!("RCP _wE.x, {};", scalar(b)));
                out.push(format!("MIN _wE.x, _wE.x, {BIG};"));
                out.push(format!("MAX _wE.x, _wE.x, -{BIG};"));
                out.push(format!("MUL{satx} {dst}, {a}, _wE.x;"));
            }
            // integers on floats: a conversion is a copy, an or of truths (0 or not) nonzero
            "I2F" => out.push(format!("MOV{satx} {dst}, {};", sources.first()?)),
            "OR" => {
                let [a, b] = sources.as_slice() else { return None };
                add_temp(&mut temps, "_wE");
                add_temp(&mut temps, "_wF");
                out.push(format!("MAX _wE, {a}, -{a};"));
                out.push(format!("MAX _wF, {b}, -{b};"));
                out.push(format!("MAX{satx} {dst}, _wE, _wF;"));
            }
            "DP2" => {
                let [a, b] = sources.as_slice() else { return None };
                add_temp(&mut temps, "_wE");
                out.push(format!("MUL _wE, {a}, {b};"));
                out.push(format!("ADD{satx} {dst}, _wE.x, _wE.y;"));
            }
            // no derivatives in ARB programs: a flat surface
            "DDX" | "DDY" => out.push(format!("MOV {dst}, {ZERO};")),
            // no vertex textures in ARB programs: a texel at a constant position (DIVA's auto
            // exposure, the scene's luminance) as a local parameter the payload reads it into
            // (`cg::texel_parameters`), the others (its height fog density and displacement
            // maps) zero
            "TEX" | "TXL" | "TXP" | "TXB" if !fragment => match (sources.first(), sources.get(1).and_then(|t| t.strip_prefix("texture[")?.strip_suffix(']')?.parse::<u32>().ok())) {
                (Some(c), Some(unit)) if c.starts_with('{') && unit < 16 && sources.get(2).is_some_and(|t| t == "2D") => {
                    out.push(format!("MOV {dst}, program.local[{}];", TEXEL_LOCAL + unit));
                }
                _ => out.push(format!("MOV {dst}, {ZERO};")),
            },
            "SIN" | "COS" | "SCS" | "KIL" if !fragment => return None,
            _ => out.push(format!("{base}{satx} {dst}, {};", sources.join(", "))),
        }
        // NVIDIA's 0 * infinity is 0, Mesa's NaN: the reciprocals finite (DIVA's normals of a
        // zero vector, NaN pixels on its characters' edges, its exposure NaN: black frames)
        if base == "RSQ" || base == "RCP" {
            out.push(format!("MIN {dst}, {target}, {BIG};"));
            if base == "RCP" {
                out.push(format!("MAX {dst}, {target}, -{BIG};"));
            }
        }
        if sat && !fragment {
            out.push(format!("MIN {dst}, {target}, {ONE};"));
            out.push(format!("MAX {dst}, {target}, {ZERO};"));
        }
        // the lanes written: the condition's under the current predicate
        let lanes = match (cond, &cur) {
            (None, None) => None,
            (None, Some(p)) => Some(format!("{p}.x")),
            (Some(c), p) => {
                add_temp(&mut temps, "_wM");
                test(&mut out, "_wM", c)?;
                if let Some(p) = p {
                    out.push(format!("MUL _wM, _wM, {p}.x;"));
                }
                Some("_wM".to_string())
            }
        };
        let write = |out: &mut Vec<String>, temps: &mut Vec<String>, to: &str| match &lanes {
            None => out.push(format!("MOV {to}{mask}, {target};")),
            Some(m) if fragment => out.push(format!("CMP {to}{mask}, -{m}, _wT, {to};")),
            Some(m) => {
                add_temp(temps, "_wD");
                out.push(format!("SUB _wD, _wT, {to};"));
                out.push(format!("MAD {to}{mask}, _wD, {m}, {to};"));
            }
        };
        if select && !cc_only {
            write(&mut out, &mut temps, &reg);
        }
        if cc {
            let cc = if first.ends_with("C1") { "_wCC1" } else { "_wCC" };
            add_temp(&mut temps, cc);
            write(&mut out, &mut temps, cc);
        }
    }
    if !levels.is_empty() || out.last().is_none_or(|l| l != "END") {
        return None;
    }
    // vertex temporaries zeroed first: the predicated writes select arithmetically, from the
    // previous value (NaN or huge in a temporary never written on Mesa, its result NaN or
    // garbage: DIVA's hair and clothes, their fog coordinate and rim light in blown out blooms)
    if !fragment && let Some(b) = body {
        for (i, t) in declared.iter().chain(&temps).enumerate() {
            out.insert(b + i, format!("MOV {t}, {ZERO};"));
        }
    }
    for (i, t) in temps.iter().enumerate() {
        out.insert(head + i, format!("TEMP {t};"));
    }
    Some(out.join("\n") + "\n")
}
