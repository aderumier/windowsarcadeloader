//! Pokken Tournament's texts (`Win32/text/master/*.drp`) for translation.
//!
//! ```text
//! wal-pokken-text export <master dir>... <memory.tsv>
//! wal-pokken-text import <master dir> <memory.tsv> <out dir>
//! wal-pokken-text check <master dir>
//! ```
//!
//! The translation memory has one line per text segment (a run of text between line breaks,
//! number slots and style changes): the text, its translation (empty until translated), the
//! message it is for (`table:label`, empty: every message with that text) and, as a comment,
//! where it first appears. `export` writes it for the given dumps (game versions differ in
//! texts), `import` rebuilds into `<out dir>` the archives with a translated segment. Tabs, line feeds and backslashes are escaped (`\t`, `\n`, `\\`).
//! `check` rebuilds every table unchanged and compares.

mod drp;
mod ntxl;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

fn archives(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("drp")))
        .collect();
    v.sort();
    Ok(v)
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn file_name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

/// Reads the archives of `dir` that hold text tables (the layout archive `resident_tlt.drp` has
/// none and is skipped when unreadable).
fn text_archives(dir: &Path) -> Result<Vec<(PathBuf, drp::Archive)>> {
    let mut v = Vec::new();
    for path in archives(dir)? {
        match drp::Archive::read(&fs::read(&path)?) {
            Ok(a) => v.push((path, a)),
            Err(e) => println!("{}: skipped ({e:#})", file_name(&path)),
        }
    }
    Ok(v)
}

/// Every text segment of `archive`: (table, entry, label, segment, text).
fn segments(archive: &drp::Archive) -> Result<Vec<(String, String, String)>> {
    let mut v = Vec::new();
    for e in &archive.entries {
        for f in e.files.iter().filter(|f| ntxl::Table::is_table(f)) {
            let table = ntxl::Table::read(f).with_context(|| e.name.clone())?;
            for t in &table.entries {
                for text in t.records.iter().flatten().filter_map(ntxl::Record::text) {
                    if !text.trim().is_empty() {
                        v.push((e.name.clone(), t.label.clone(), text));
                    }
                }
            }
        }
    }
    Ok(v)
}

fn export(dirs: &[&str], out: &Path) -> Result<()> {
    let mut seen = HashMap::new();
    let mut order = Vec::new();
    for dir in dirs {
        for (path, archive) in text_archives(Path::new(dir))? {
            for (table, label, text) in segments(&archive)? {
                seen.entry(text.clone()).or_insert_with(|| {
                    order.push(text.clone());
                    format!("{}:{table}:{label}", file_name(&path))
                });
            }
        }
    }
    let mut s = String::from("# text\ttranslation\tmessage (table:label, empty: all)\tfirst place (archive:table:label)\n");
    for text in &order {
        s += &format!("{}\t\t\t{}\n", escape(text), seen[text]);
    }
    fs::write(out, s)?;
    println!("{} distinct segments written to {}", order.len(), out.display());
    Ok(())
}

/// The translation memory: (table:label, text) and text alone -> translation, the lines with a
/// translation. The same text can read differently in two messages (a piece of a sentence):
/// a line with a third column (table:label) is for that message only.
#[derive(Default)]
struct Memory {
    by_message: HashMap<(String, String), String>,
    by_text: HashMap<String, String>,
}

impl Memory {
    fn get(&self, message: &str, text: &str) -> Option<&String> {
        self.by_message.get(&(message.to_string(), text.to_string())).or_else(|| self.by_text.get(text))
    }
}

fn translations(tsv: &Path) -> Result<Memory> {
    let mut m = Memory::default();
    for (n, line) in fs::read_to_string(tsv)?.split('\n').enumerate() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let c: Vec<&str> = line.split('\t').collect();
        ensure!(c.len() >= 2, "{}:{}: {} columns", tsv.display(), n + 1, c.len());
        let (text, tr) = (unescape(c[0]), unescape(c[1]));
        // An empty translation of a message's piece is one (the words moved to another piece).
        match c.get(2).filter(|m| !m.is_empty()) {
            Some(msg) => {
                m.by_message.insert((msg.to_string(), text), tr);
            }
            None if !tr.is_empty() => {
                m.by_text.insert(text, tr);
            }
            None => {}
        }
    }
    Ok(m)
}

fn import(dir: &Path, tsv: &Path, out: &Path) -> Result<()> {
    let map = translations(tsv)?;
    fs::create_dir_all(out)?;
    let (mut done, mut missing) = (0, 0);
    for (path, mut archive) in text_archives(dir)? {
        let name = file_name(&path);
        let mut changed = 0;
        for e in &mut archive.entries {
            for f in e.files.iter_mut().filter(|f| ntxl::Table::is_table(f)) {
                let mut table = ntxl::Table::read(f)?;
                let mut hit = false;
                for t in &mut table.entries {
                    let message = format!("{}:{}", e.name, t.label);
                    for r in t.records.iter_mut().flatten() {
                        let Some(text) = r.text().filter(|t| !t.trim().is_empty()) else { continue };
                        match map.get(&message, &text) {
                            Some(s) => {
                                r.set_text(s);
                                changed += 1;
                                hit = true;
                            }
                            None => {
                                if missing < 10 {
                                    println!("no translation: {message}: {}", escape(&text));
                                }
                                missing += 1;
                            }
                        }
                    }
                }
                if hit {
                    *f = table.write();
                }
            }
        }
        if changed > 0 {
            fs::write(out.join(&name), archive.write()?)?;
            println!("{name}: {changed} segments");
            done += changed;
        }
    }
    println!("{done} segments translated, {missing} without a translation");
    Ok(())
}

fn check(dir: &Path) -> Result<()> {
    for (path, archive) in text_archives(dir)? {
        for e in &archive.entries {
            for f in e.files.iter().filter(|f| ntxl::Table::is_table(f)) {
                let rebuilt = ntxl::Table::read(f)?.write();
                ensure!(&rebuilt == f, "{}: {}: rebuilt table differs", path.display(), e.name);
            }
        }
        let again = drp::Archive::read(&archive.write()?)?;
        for (a, b) in archive.entries.iter().zip(&again.entries) {
            ensure!(a.files == b.files, "{}: {}: rebuilt archive differs", path.display(), a.name);
        }
        println!("{}: {} files ok", file_name(&path), archive.entries.len());
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["export", ref dirs @ .., out] if !dirs.is_empty() => export(dirs, Path::new(out)),
        ["import", dir, tsv, out] => import(Path::new(dir), Path::new(tsv), Path::new(out)),
        ["check", dir] => check(Path::new(dir)),
        _ => bail!("usage: wal-pokken-text export <dir>... <tsv> | import <dir> <tsv> <out dir> | check <dir>"),
    }
}
