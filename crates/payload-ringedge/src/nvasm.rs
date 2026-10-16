//! NVIDIA assembly (`NV_fragment_program2` / `NV_vertex_program3` options of ARB programs, as
//! Cg's fp40 / vp40 profiles write them) as plain ARB programs, for the GPUs without these
//! options (Mesa).
//!
//! * Options, `SHORT` / `LONG` precisions and branch labels dropped; opcode precision suffixes
//!   (`MULH`, `MADR`) removed.
//! * Condition codes: an instruction with the `C` suffix also copies its result to the `CC`
//!   temporary; a conditional write (`MOV R0.x(NE.y), ...`) computes into `TC` and selects
//!   with `CMP` (fragment programs only).
//! * `NRM`, `SGT`, `SLE`, absolute values (`|x|`) and vertex `_SAT` with ARB instructions.
//! * Shadow samplers as plain 2D textures (the programs compare the depth themselves).
//! * The output color through a temporary (a conditional write reads it back).
//! * The vertex weight (`ARB_vertex_blend`, absent) as generic attribute 1, where NVIDIA reads
//!   it and the game feeds it.
//!
//! `NV_fragment_program` programs (`!!FP1.0`) too. Programs with branches, other address
//! components than `.x` or other formats: `None`.

const BASES: [&str; 40] = [
    "TXP", "TEX", "TXB", "DP3", "DP4", "DPH", "FLR", "FRC", "RCP", "RSQ", "MAX", "MIN", "MAD", "MUL", "ADD", "SUB", "MOV", "SLT", "SGT", "SGE", "SLE", "SEQ",
    "SNE", "NRM", "LRP", "CMP", "EX2", "LG2", "POW", "XPD", "ARL", "KIL", "ABS", "DST", "LIT", "SCS", "SIN", "COS", "EXP", "LOG",
];

/// A program for NVIDIA GPUs only: NVIDIA options or formats.
pub(crate) fn nvidia(text: &str) -> bool {
    text.contains("OPTION NV_") || text.starts_with("!!FP") || text.starts_with("!!VP")
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
        matches!(rest, "" | "H" | "R" | "X" | "C" | "HC" | "RC" | "XC").then_some((b, rest.ends_with('C'), sat))
    })
}

/// A destination: register, write mask (`.xyz` or empty), condition and its swizzle.
fn destination(d: &str) -> Option<(&str, &str, Option<&str>, &str)> {
    let (reg_mask, cond) = match d.find('(') {
        Some(i) => (&d[..i], Some(d[i + 1..].strip_suffix(')')?)),
        None => (d, None),
    };
    let (reg, mask) = split_swizzle(reg_mask);
    let (cond, swizzle) = match cond {
        Some(c) => {
            let (c, sw) = split_swizzle(c);
            (Some(c), sw)
        }
        None => (None, ""),
    };
    Some((reg, mask, cond, swizzle))
}

/// `value.xyz` as `(value, ".xyz")`.
fn split_swizzle(s: &str) -> (&str, &str) {
    match s.rfind('.') {
        Some(i) if (2..=5).contains(&(s.len() - i)) && s[i + 1..].chars().all(|c| "xyzw".contains(c)) => (&s[..i], &s[i..]),
        _ => (s, ""),
    }
}

/// Operands, split on the commas outside brackets and braces.
fn operands(args: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (Vec::new(), String::new(), 0);
    for c in args.chars() {
        match c {
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
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

fn is_label(s: &str) -> bool {
    s.strip_prefix("BB").and_then(|r| r.strip_suffix(':')).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
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

pub(crate) fn translate(text: &str) -> Option<String> {
    if text.starts_with("!!FP1.0") {
        return translate(&fp10(text)?);
    }
    let fragment = text.starts_with("!!ARBfp1.0");
    if !fragment && !text.starts_with("!!ARBvp1.0") {
        return None;
    }
    let words = |w: &str| text.split(|c: char| !c.is_ascii_alphanumeric()).any(|t| t == w);
    if ["BRA", "IF", "REP", "CAL", "RET"].iter().any(|w| words(w)) {
        return None;
    }
    if (0..10).any(|n| ["y", "z", "w"].iter().any(|c| text.contains(&format!("A{n}.{c}")))) {
        return None;
    }
    let mut out: Vec<String> = Vec::new();
    let mut temps: Vec<String> = Vec::new();
    let add_temp = |temps: &mut Vec<String>, t: &str| {
        if !temps.iter().any(|x| x == t) {
            temps.push(t.to_string());
        }
    };
    let mut color_temp = false;
    for line in text.lines() {
        let mut s = line.trim();
        if s.is_empty() || s.starts_with('#') {
            out.push(line.to_string());
            continue;
        }
        if s.starts_with("OPTION NV_") || s.starts_with("OPTION ARB_fragment_program_shadow") || is_label(s) {
            continue;
        }
        for p in ["SHORT ", "LONG "] {
            if let Some(r) = s.strip_prefix(p) {
                s = r;
            }
        }
        if s.starts_with("OUTPUT oCol") {
            out.push("TEMP oCol;".into());
            color_temp = true;
            continue;
        }
        let first = s.split_whitespace().next().unwrap_or("");
        let declaration = ["TEMP", "PARAM", "ATTRIB", "ADDRESS", "ALIAS", "OUTPUT"].contains(&first);
        if s == "END" {
            if color_temp {
                out.push("MOV result.color, oCol;".into());
            }
            out.push("END".into());
            continue;
        }
        if s.starts_with("!!") || declaration || !first.chars().next().is_some_and(|c| c.is_ascii_uppercase()) || !s.contains(' ') {
            out.push(s.to_string());
            continue;
        }
        let body = s.strip_suffix(';')?;
        let (op, args) = body.split_once(char::is_whitespace)?;
        let (base, cc, sat) = opcode(op)?;
        if base == "KIL" {
            out.push(format!("KIL {};", args.trim()));
            continue;
        }
        let parts = operands(args);
        let (reg, mask, cond, swizzle) = destination(&parts[0])?;
        let mut sources: Vec<String> = parts[1..].iter().map(|x| x.replace("SHADOW2D", "2D")).collect();
        // |x|: MAX t, x, -x first
        let mut abs = 0;
        for src in sources.iter_mut() {
            let (neg, rest) = match src.strip_prefix('-') {
                Some(r) => ("-", r),
                None => ("", src.as_str()),
            };
            let Some(inner) = rest.strip_prefix('|').and_then(|r| r.strip_suffix('|')) else { continue };
            let (value, sw) = split_swizzle(inner);
            let t = format!("TA{abs}");
            abs += 1;
            add_temp(&mut temps, &t);
            out.push(format!("MAX {t}, {value}, -{value};"));
            *src = format!("{neg}{t}{sw}");
        }
        let vertex_sat = sat && !fragment;
        let satx = if sat && fragment { "_SAT" } else { "" };
        let cc_only = reg == "RC" || reg == "HC";
        let target = if cond.is_some() || cc_only { "TC" } else { reg };
        if target == "TC" {
            add_temp(&mut temps, "TC");
        }
        match base {
            "NRM" => {
                let a = sources.first()?;
                add_temp(&mut temps, "TN");
                out.push(format!("DP3 TN.w, {a}, {a};"));
                out.push("RSQ TN.w, TN.w;".into());
                out.push(format!("MUL{satx} {target}{mask}, {a}, TN.w;"));
            }
            "SGT" | "SLE" => {
                let [a, b] = sources.as_slice() else { return None };
                let op = if base == "SGT" { "SLT" } else { "SGE" };
                out.push(format!("{op}{satx} {target}{mask}, {b}, {a};"));
            }
            "SEQ" | "SNE" => return None,
            _ => out.push(format!("{base}{satx} {target}{mask}, {};", sources.join(", "))),
        }
        if vertex_sat {
            out.push(format!("MIN {target}{mask}, {target}, {{1,1,1,1}};"));
            out.push(format!("MAX {target}{mask}, {target}, {{0,0,0,0}};"));
        }
        if cc {
            add_temp(&mut temps, "CC");
            out.push(format!("MOV CC{mask}, {target};"));
        }
        if let Some(cond) = cond {
            if !fragment {
                return None;
            }
            let ccs = format!("CC{swizzle}");
            let line = match cond {
                "NE" | "EQ" => {
                    add_temp(&mut temps, "TK");
                    out.push(format!("MUL TK, {ccs}, {ccs};"));
                    if cond == "NE" { format!("CMP {reg}{mask}, -TK, TC, {reg};") } else { format!("CMP {reg}{mask}, -TK, {reg}, TC;") }
                }
                "GT" => format!("CMP {reg}{mask}, -{ccs}, TC, {reg};"),
                "LT" => format!("CMP {reg}{mask}, {ccs}, TC, {reg};"),
                "LE" => format!("CMP {reg}{mask}, -{ccs}, {reg}, TC;"),
                "GE" => format!("CMP {reg}{mask}, {ccs}, {reg}, TC;"),
                _ => return None,
            };
            out.push(line);
        }
    }
    // the added temporaries after the header line
    let header = out.iter().position(|l| l.starts_with("!!"))?;
    for (i, t) in temps.iter().enumerate() {
        out.insert(header + 1 + i, format!("TEMP {t};"));
    }
    let text = out.join("\n") + "\n";
    Some(if fragment { text } else { text.replace("vertex.weight", "vertex.attrib[1]") })
}
