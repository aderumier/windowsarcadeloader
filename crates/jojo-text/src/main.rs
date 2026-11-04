//! JoJo's Bizarre Adventure: Last Survivor's texts (`AJB/Content/AJB`, UE 4.20 cooked
//! assets) for translation.
//!
//! ```text
//! wal-jojo-text export <content dir> <memory.tsv>
//! wal-jojo-text locres <content dir> <memory.tsv> <Game.locres>
//! ```
//!
//! The game's texts are keyed (namespace, key) `FText`s and string table entries, and the
//! engine looks each one up in the current culture's `Localization/Game/<culture>/Game.locres`
//! (the culture is the system's, en-US under the US locale: its `en` resources apply). No
//! asset is rebuilt.
//!
//! The translation memory has one line per Japanese source text: the text, its translation
//! (empty until translated), the text it is for (`namespace:key`, empty: every text with that
//! source) and, as a comment, the file where it first appears. `export` adds the texts not in
//! the memory yet, `locres` writes the resource of the translated ones. Tabs, line feeds,
//! carriage returns and backslashes are escaped (`\t`, `\n`, `\r`, `\\`).

mod asset;
mod locres;
mod package;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};

use asset::Text;

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n").replace('\r', "\\r")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('t') => out.push('\t'),
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The files under `dir` with the extension `ext`.
fn files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) -> Result<()> {
    for e in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = e?.path();
        if path.is_dir() {
            files(&path, ext, out)?;
        } else if path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
    Ok(())
}

/// The keyed texts with a Japanese source under `dir` (once each, with the file they are in)
/// and the words the widgets spell out one text block per character.
#[derive(Default)]
struct Scan {
    texts: Vec<(Text, String)>,
    words: Vec<(Vec<Text>, String)>,
}

/// The keyed texts of the export `stem` (`b`) whose source `keep` accepts: a string table's
/// entries, or the texts found in it.
fn file_texts(stem: &str, b: &[u8], keep: impl Fn(&str) -> bool) -> Result<Vec<Text>> {
    Ok(if stem.starts_with("SDT_") {
        asset::string_table(b)?.into_iter().filter(|t| keep(&t.source)).collect()
    } else {
        asset::texts(b, keep)
    })
}

fn scan(dir: &Path) -> Result<Scan> {
    let mut exports = Vec::new();
    files(dir, "uexp", &mut exports)?;
    exports.sort();
    let mut seen = HashSet::new();
    let mut seen_words = HashSet::new();
    let mut s = Scan::default();
    for path in exports {
        let b = fs::read(&path)?;
        let rel = path.strip_prefix(dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        for t in file_texts(&stem, &b, asset::is_japanese).with_context(|| rel.clone())? {
            if seen.insert((t.namespace.clone(), t.key.clone(), t.source.clone())) {
                s.texts.push((t, rel.clone()));
            }
        }
        // widgets: their trees, for the spelled words
        if stem.starts_with("WB_") {
            let header = fs::read(path.with_extension("uasset"))?;
            let p = package::Package::read(&header, &b).with_context(|| rel.clone())?;
            for w in package::spelled_words(&p).with_context(|| rel.clone())? {
                if seen_words.insert(w.iter().map(context).collect::<Vec<_>>()) {
                    s.words.push((w, rel.clone()));
                }
            }
        }
    }
    Ok(s)
}

fn context(t: &Text) -> String {
    format!("{}:{}", t.namespace, t.key)
}

fn word(w: &[Text]) -> String {
    w.iter().map(|t| t.source.as_str()).collect()
}

/// The context of a spelled word's line.
const SPELLED: &str = "spelled";

/// A spelled word's translation over its `n` text blocks: its pieces when it gives them
/// (`n` pieces separated by `|`: blocks placed on a canvas, two words in a row), else the
/// characters shared out evenly, the first blocks taking the remainder. Spaces are no-break
/// spaces: a block drops its leading and trailing spaces.
fn spread(translation: &str, n: usize) -> Vec<String> {
    let translation = &translation.replace(' ', "\u{a0}");
    let pieces: Vec<&str> = translation.split('|').collect();
    if pieces.len() == n {
        return pieces.into_iter().map(String::from).collect();
    }
    let chars: Vec<char> = translation.chars().collect();
    let (base, extra) = (chars.len() / n, chars.len() % n);
    let mut start = 0;
    (0..n)
        .map(|i| {
            let len = base + usize::from(i < extra);
            let piece: String = chars[start..start + len].iter().collect();
            start += len;
            piece
        })
        .collect()
}

/// The translation memory: (context, source) and source alone -> translation; a context is
/// `namespace:key` (that text only) or `spelled` (a spelled word).
#[derive(Default)]
struct Memory {
    lines: Vec<String>,
    entries: HashSet<(String, String)>,
    by_text: HashMap<(String, String), String>,
    by_source: HashMap<String, String>,
}

impl Memory {
    fn read(tsv: &Path) -> Result<Memory> {
        let mut m = Memory::default();
        if !tsv.exists() {
            return Ok(m);
        }
        for (n, line) in fs::read_to_string(tsv)?.lines().enumerate() {
            m.lines.push(line.to_string());
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let c: Vec<&str> = line.split('\t').collect();
            ensure!(c.len() >= 2, "{}:{}: {} columns", tsv.display(), n + 1, c.len());
            let (source, tr) = (unescape(c[0]), unescape(c[1]));
            let ctx = c.get(2).copied().unwrap_or_default();
            m.entries.insert((if ctx == SPELLED { SPELLED } else { "" }.to_string(), source.clone()));
            if tr.is_empty() {
                continue;
            }
            match ctx {
                "" => m.by_source.insert(source, tr),
                ctx => m.by_text.insert((ctx.to_string(), source), tr),
            };
        }
        Ok(m)
    }

    fn get(&self, t: &Text) -> Option<&String> {
        self.by_text.get(&(context(t), t.source.clone())).or_else(|| self.by_source.get(&t.source))
    }

    fn get_word(&self, w: &[Text]) -> Option<&String> {
        self.by_text.get(&(SPELLED.to_string(), word(w)))
    }
}

fn export(dir: &Path, tsv: &Path) -> Result<()> {
    let mut memory = Memory::read(tsv)?;
    if memory.lines.is_empty() {
        memory.lines.push("# source\ttranslation\ttext (namespace:key or spelled, empty: all)\tfirst place".into());
    }
    let scan = scan(dir)?;
    let spelled: HashSet<String> = scan.words.iter().flat_map(|(w, _)| w.iter().map(context)).collect();
    let mut added = 0;
    for (w, file) in &scan.words {
        if memory.entries.insert((SPELLED.to_string(), word(w))) {
            memory.lines.push(format!("{}\t\t{SPELLED}\t{file}", escape(&word(w))));
            added += 1;
        }
    }
    for (t, file) in &scan.texts {
        if !spelled.contains(&context(t)) && memory.entries.insert((String::new(), t.source.clone())) {
            memory.lines.push(format!("{}\t\t\t{file}", escape(&t.source)));
            added += 1;
        }
    }
    fs::write(tsv, memory.lines.join("\n") + "\n")?;
    println!("{added} new source texts written to {}", tsv.display());
    Ok(())
}

/// Adopts a translation made by replacing exports: under `dir`, each `<name>.uexp.orig` is an
/// original beside its translated `<name>.uexp`. The translated sources of its Japanese texts
/// (paired by key) fill the memory's empty translations, the texts it lacks are added.
fn adopt(dir: &Path, tsv: &Path) -> Result<()> {
    let mut memory = Memory::read(tsv)?;
    let mut line_of = HashMap::new();
    for (i, line) in memory.lines.iter().enumerate() {
        let c: Vec<&str> = line.split('\t').collect();
        if c.len() >= 3 && c[2].is_empty() && !line.starts_with('#') {
            line_of.insert(unescape(c[0]), i);
        }
    }
    let mut originals = Vec::new();
    files(dir, "orig", &mut originals)?;
    originals.sort();
    let (mut filled, mut added) = (0, 0);
    for orig in originals {
        let translated = orig.with_extension("");
        let rel = translated.strip_prefix(dir).unwrap_or(&translated).to_string_lossy().replace('\\', "/");
        let stem = translated.file_stem().unwrap_or_default().to_string_lossy();
        let english: HashMap<String, String> = file_texts(&stem, &fs::read(&translated)?, |s| !s.is_empty())
            .with_context(|| rel.clone())?
            .into_iter()
            .map(|t| (context(&t), t.source))
            .collect();
        for t in file_texts(&stem, &fs::read(&orig)?, asset::is_japanese).with_context(|| rel.clone())? {
            let Some(en) = english.get(&context(&t)).filter(|en| **en != t.source) else { continue };
            match line_of.get(&t.source) {
                Some(&i) => {
                    let mut c: Vec<String> = memory.lines[i].split('\t').map(String::from).collect();
                    if c[1].is_empty() {
                        c[1] = escape(en);
                        memory.lines[i] = c.join("\t");
                        filled += 1;
                    }
                }
                None => {
                    line_of.insert(t.source.clone(), memory.lines.len());
                    memory.lines.push(format!("{}\t{}\t\t{rel}", escape(&t.source), escape(en)));
                    added += 1;
                }
            }
        }
    }
    fs::write(tsv, memory.lines.join("\n") + "\n")?;
    println!("{filled} translations filled, {added} texts added to {}", tsv.display());
    Ok(())
}

fn build(dir: &Path, tsv: &Path, out: &Path) -> Result<()> {
    let memory = Memory::read(tsv)?;
    let scan = scan(dir)?;
    let mut resource = locres::Resource::default();
    let (mut done, mut missing) = (0, 0);
    let mut no_translation = |what: &str, file: &str| {
        if missing < 10 {
            println!("no translation: {file}: {}", escape(what));
        }
        missing += 1;
    };
    let mut spelled = HashSet::new();
    for (w, file) in &scan.words {
        spelled.extend(w.iter().map(context));
        let Some(tr) = memory.get_word(w) else {
            no_translation(&word(w), file);
            continue;
        };
        for (t, piece) in w.iter().zip(spread(tr, w.len())) {
            resource.0.entry(t.namespace.clone()).or_default().insert(t.key.clone(), (t.source.clone(), piece));
        }
        done += 1;
    }
    for (t, file) in &scan.texts {
        if spelled.contains(&context(t)) {
            continue;
        }
        match memory.get(t) {
            Some(tr) => {
                resource.0.entry(t.namespace.clone()).or_default().insert(t.key.clone(), (t.source.clone(), tr.clone()));
                done += 1;
            }
            None => no_translation(&format!("{}: {}", context(t), t.source), file),
        }
    }
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(out, resource.write())?;
    println!("{done} texts translated, {missing} without a translation: {}", out.display());
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["export", dir, tsv] => export(Path::new(dir), Path::new(tsv)),
        ["locres", dir, tsv, out] => build(Path::new(dir), Path::new(tsv), Path::new(out)),
        ["adopt", dir, tsv] => adopt(Path::new(dir), Path::new(tsv)),
        _ => bail!("usage: wal-jojo-text export <content dir> <memory.tsv> | locres <content dir> <memory.tsv> <Game.locres> | adopt <content dir> <memory.tsv>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spread_shares_out_the_characters() {
        assert_eq!(spread("NATIONAL", 4), ["NA", "TI", "ON", "AL"]);
        assert_eq!(spread("END GAME", 5), ["EN", "D\u{a0}", "GA", "M", "E"]);
        assert_eq!(spread("OK", 3), ["O", "K", ""]);
        assert_eq!(spread("RE|TIRED|!", 3), ["RE", "TIRED", "!"]);
    }
}
