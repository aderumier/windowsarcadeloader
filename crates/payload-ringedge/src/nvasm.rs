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
//!   `WAL_RINGEDGE_CG_REPEAT` times, their integer index registers followed ([`loops`]).
//! * The vertex address register's 4 components (`_adr.y`): `ARL` again from the component's
//!   source before each use, through `.x`.
//! * `NRM`, `SGT`, `SLE`, `SEQ`, `SNE`, `SSG`, absolute values (`|x|`), vertex `LRP` / `CMP`
//!   and vertex `_SAT` with ARB instructions; `TXL` as `TEX` (the level of detail dropped:
//!   exact on the textures without mipmaps, the shadow and screen buffers); vertex `TEX` at a
//!   constant position as a local parameter (`TEXEL_LOCAL`), the others zero.
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
//! `NV_fragment_program` programs (`!!FP1.0`) too. Programs with `REP` loops without
//! `WAL_RINGEDGE_CG_REPEAT`, other loops, backward branches or other formats: `None`.

const BASES: [&str; 46] = [
    "TXP", "TEX", "TXB", "TXL", "DP3", "DP4", "DPH", "FLR", "FRC", "RCP", "RSQ", "MAX", "MIN", "MAD", "MUL", "ADD", "SUB", "MOV", "SLT", "SGT", "SGE",
    "SLE", "SEQ", "SNE", "SSG", "NRM", "LRP", "CMP", "EX2", "LG2", "POW", "XPD", "ARL", "KIL", "ABS", "DST", "LIT", "SCS", "SIN", "COS", "EXP", "LOG",
    "SWZ", "DP2", "DDX", "DDY",
];

/// The vertex programs' local parameter holding texel (0, 0) of texture unit `n`, at
/// `TEXEL_LOCAL + n`: their texture reads at a constant position.
pub(crate) const TEXEL_LOCAL: u32 = 240;

const ZERO: &str = "{0,0,0,0}";
const ONE: &str = "{1,1,1,1}";
const BIG: &str = "{1e30,1e30,1e30,1e30}";

/// A program for NVIDIA GPUs only: NVIDIA options or formats.
pub(crate) fn nvidia(text: &str) -> bool {
    text.contains("OPTION NV_") || text.starts_with("!!FP") || text.starts_with("!!VP") || text.starts_with("!!NV")
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
    let (c, sw) = split_swizzle(cond);
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
    let (header, body) = text.split_once('\n').unwrap_or((text, ""));
    let mut out = vec![header.trim().to_string()];
    let body: Vec<&str> = body.lines().map(|l| l.split('#').next().unwrap_or("")).collect();
    for s in body.join(" ").split(';') {
        let mut s = s.split_whitespace().collect::<Vec<_>>().join(" ");
        if let Some(i) = s.find(':') {
            let label = &s[..i];
            if !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                out.push(format!("{label}:"));
                s = s[i + 1..].trim().to_string();
            }
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
    if !split_swizzle(src).1.is_empty() {
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

pub(crate) fn translate(text: &str) -> Option<String> {
    if text.starts_with("!!FP1.0") {
        return translate(&fp10(text)?);
    }
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
    for s in subroutines(statements(&text))? {
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
        let mut sources: Vec<String> = parts[1..].iter().filter(|x| !x.starts_with('(')).map(|x| if shadow_samplers() { x.to_string() } else { x.replace("SHADOW2D", "2D").replace("SHADOWRECT", "RECT").replace("SHADOWCUBE", "CUBE").replace("SHADOW1D", "1D") }).collect();
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
            "TXL" if fragment => out.push(format!("TEX{satx} {dst}, {};", sources.join(", "))),
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
