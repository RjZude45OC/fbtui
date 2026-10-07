//! Search (Ctrl+F): file and folder names and/or the text inside files, below the current folder
//! or on every drive, plain text or regular expression. Runs on a background thread and sends
//! results while it works, so the list fills in live and you can jump between matches right away.

use regex::{Regex, RegexBuilder};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Instant;

pub const MAX_HITS: usize = 5000;
const MAX_BYTES: u64 = 20 * 1024 * 1024;

const SKIP_DIRS: &[&str] = &[".git", ".svn", ".hg", "node_modules", "__pycache__", ".venv", "venv", ".vs", ".idea", "bin", "obj",
    "$recycle.bin", "system volume information"];
/// Extra folders skipped when searching everywhere (huge and never what you look for).
const SKIP_GLOBAL: &[&str] = &["windows", "$windows.~bt", "$windows.~ws", "$winreagent", "windowsapps", "winsxs", "recovery", "config.msi", "msocache"];
const SKIP_EXT: &[&str] = &["exe", "dll", "sys", "msi", "zip", "7z", "rar", "gz", "tar", "iso", "png", "jpg", "jpeg", "gif", "bmp", "ico", "webp", "tif", "tiff",
    "mp3", "mp4", "mkv", "avi", "mov", "wav", "pdf", "docx", "xlsx", "pptx", "doc", "xls", "ppt", "pdb", "lib", "obj", "pyc", "class", "jar", "db", "sqlite", "mdf", "ldf", "bak",
    "heic", "avif", "flac", "ogg", "webm", "psd", "svgz", "vhdx", "vmdk", "pak", "bin", "dat"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum What { Names, Folders, Text, Both }

impl What {
    pub fn label(self) -> &'static str { match self { What::Names => "file + folder names", What::Folders => "folders only", What::Text => "text", What::Both => "names + text" } }
    pub fn next(self) -> What { match self { What::Both => What::Names, What::Names => What::Folders, What::Folders => What::Text, What::Text => What::Both } }
    pub fn key(self) -> &'static str { match self { What::Names => "names", What::Folders => "folders", What::Text => "text", What::Both => "both" } }
    pub fn from_key(s: &str) -> What { match s { "names" => What::Names, "folders" => What::Folders, "text" => What::Text, _ => What::Both } }
    fn folders(self) -> bool { self != What::Text }
    fn files(self) -> bool { matches!(self, What::Names | What::Both) }
    fn text(self) -> bool { matches!(self, What::Text | What::Both) }
}

#[derive(Clone, Debug)]
pub struct Opts { pub what: What, pub regex: bool, pub case: bool, pub global: bool }

impl Opts {
    /// Short description for prompts and titles: "names + text · here · regex · Aa"
    #[allow(dead_code)]
    pub fn label(&self) -> String {
        let mut s = format!("{}  ·  {}", self.what.label(), if self.global { "all drives" } else { "this folder + subfolders" });
        if self.regex { s += "  ·  regex"; }
        if self.case { s += "  ·  match case"; }
        s
    }
}

/// Plain text or a regular expression.
#[derive(Clone)]
pub enum Matcher { Plain { q: String, case: bool }, Re(Regex) }

impl Matcher {
    pub fn new(query: &str, o: &Opts) -> Result<Matcher, String> {
        if o.regex {
            RegexBuilder::new(query).case_insensitive(!o.case).size_limit(1 << 22).build()
                .map(Matcher::Re).map_err(|e| format!("Bad regular expression: {}", e.to_string().lines().last().unwrap_or("")))
        } else {
            Ok(Matcher::Plain { q: if o.case { query.to_string() } else { query.to_lowercase() }, case: o.case })
        }
    }
    pub fn is_match(&self, s: &str) -> bool {
        match self {
            Matcher::Plain { q, case: true } => s.contains(q.as_str()),
            Matcher::Plain { q, case: false } => s.to_lowercase().contains(q.as_str()),
            Matcher::Re(r) => r.is_match(s),
        }
    }
    /// Char range of the first match (to select it in the editor).
    pub fn find(&self, s: &str) -> Option<(usize, usize)> {
        let (b, e) = match self {
            Matcher::Plain { q, case } => {
                let hay = if *case { s.to_string() } else { s.to_lowercase() };
                let b = hay.find(q.as_str())?;
                if hay.len() != s.len() { let c = hay[..b].chars().count(); return Some((c, c + q.chars().count())); }
                (b, b + q.len())
            }
            Matcher::Re(r) => { let m = r.find(s)?; (m.start(), m.end()) }
        };
        Some((s[..b].chars().count(), s[..e].chars().count()))
    }
}

pub struct Hit { pub path: PathBuf, pub line: usize, pub text: String, pub is_dir: bool }

pub enum Msg {
    Hits(Vec<Hit>),
    Progress { files: usize, dirs: usize, pct: f32, current: String },
    Done { capped: bool },
}

pub struct Search {
    pub query: String,
    pub opts: Opts,
    pub roots: Vec<PathBuf>,
    pub matcher: Matcher,
    pub rx: Receiver<Msg>,
    pub stop: Arc<AtomicBool>,
    pub hits: Vec<Hit>,
    pub files: usize,
    pub dirs: usize,
    pub pct: f32,
    pub current: String,
    pub done: bool,
    pub capped: bool,
    pub start: Instant,
    pub taken: usize,          // hits already moved into the result list
}

impl Search {
    pub fn start(root: Option<&Path>, query: &str, opts: Opts, hidden: bool) -> Result<Search, String> {
        let matcher = Matcher::new(query, &opts)?;
        let roots: Vec<PathBuf> = if opts.global || root.is_none() {
            crate::fsutil::list_drives().into_iter().map(|e| e.path).collect()
        } else { vec![root.unwrap().to_path_buf()] };
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let (rs, m, st, o) = (roots.clone(), matcher.clone(), stop.clone(), opts.clone());
        std::thread::spawn(move || {
            let (mut files, mut dirs_done, mut hits) = (0usize, 0usize, 0usize);
            let mut batch = Vec::new();
            let mut last = Instant::now();
            let mut best = 0f32;
            let n_roots = rs.len().max(1);
            for (ri, r) in rs.iter().enumerate() {
                // every folder carries its share of the drive; a folder without subfolders is "done" with its share
                let mut stack: Vec<(PathBuf, f64)> = vec![(r.clone(), 1.0)];
                let mut done_w = 0f64;
                while let Some((d, wgt)) = stack.pop() {
                    if st.load(Ordering::Relaxed) { let _ = tx.send(Msg::Hits(batch)); let _ = tx.send(Msg::Done { capped: false }); return; }
                    dirs_done += 1;
                    let Ok(rd) = std::fs::read_dir(&d) else { done_w += wgt; continue };
                    let mut subs = Vec::new();
                    let mut fl = Vec::new();
                    for e in rd.flatten() {
                        let Ok(ft) = e.file_type() else { continue };
                        if ft.is_symlink() { continue; }
                        if !hidden && is_hidden(&e) { continue; }
                        let name = e.file_name().to_string_lossy().to_string();
                        let low = name.to_lowercase();
                        if ft.is_dir() {
                            if o.what.folders() && m.is_match(&name) {
                                batch.push(Hit { path: e.path(), line: 0, text: String::new(), is_dir: true });
                                hits += 1;
                            }
                            if !SKIP_DIRS.contains(&low.as_str()) && !(o.global && SKIP_GLOBAL.contains(&low.as_str())) { subs.push(e.path()); }
                        } else {
                            fl.push((e.path(), name));
                        }
                    }
                    fl.sort();
                    for (f, name) in fl {
                        files += 1;
                        if o.what.files() && m.is_match(&name) {
                            batch.push(Hit { path: f.clone(), line: 0, text: String::new(), is_dir: false });
                            hits += 1;
                        }
                        if o.what.text() {
                            let ext = f.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                            if !SKIP_EXT.contains(&ext.as_str()) { search_file(&f, &m, &mut batch, &mut hits); }
                        }
                        if hits >= MAX_HITS { let _ = tx.send(Msg::Hits(batch)); let _ = tx.send(Msg::Done { capped: true }); return; }
                        if st.load(Ordering::Relaxed) { break; }
                    }
                    subs.sort();
                    if subs.is_empty() { done_w += wgt; }
                    else {
                        // a little of the share for this folder's own files, the rest split between its subfolders
                        done_w += wgt * 0.02;
                        let each = wgt * 0.98 / subs.len() as f64;
                        stack.extend(subs.into_iter().rev().map(|p| (p, each)));
                    }
                    if last.elapsed().as_millis() > 80 {
                        last = Instant::now();
                        if !batch.is_empty() && tx.send(Msg::Hits(std::mem::take(&mut batch))).is_err() { return; }
                        // folders done vs. folders seen so far, per drive, never going backwards
                        let frac = done_w.min(1.0) as f32;
                        best = best.max((ri as f32 + frac) / n_roots as f32).min(0.999);
                        if tx.send(Msg::Progress { files, dirs: dirs_done, pct: best, current: d.display().to_string() }).is_err() { return; }
                    }
                }
            }
            let _ = tx.send(Msg::Hits(batch));
            let _ = tx.send(Msg::Progress { files, dirs: dirs_done, pct: 1.0, current: String::new() });
            let _ = tx.send(Msg::Done { capped: false });
        });
        Ok(Search { query: query.to_string(), opts, roots, matcher, rx, stop, hits: Vec::new(), files: 0, dirs: 0, pct: 0.0,
                    current: String::new(), done: false, capped: false, start: Instant::now(), taken: 0 })
    }

    /// Collect results; true if something changed.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok(m) = self.rx.try_recv() {
            any = true;
            match m {
                Msg::Hits(h) => self.hits.extend(h),
                Msg::Progress { files, dirs, pct, current } => { self.files = files; self.dirs = dirs; self.pct = pct; self.current = current; }
                Msg::Done { capped } => { self.done = true; self.capped = capped; self.pct = 1.0; }
            }
        }
        any
    }
}

fn is_hidden(e: &std::fs::DirEntry) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        e.metadata().map(|m| m.file_attributes() & 0x6 != 0).unwrap_or(false)
    }
    #[cfg(not(windows))]
    { e.file_name().to_string_lossy().starts_with('.') }
}

fn search_file(p: &Path, m: &Matcher, out: &mut Vec<Hit>, hits: &mut usize) {
    let Ok(md) = std::fs::metadata(p) else { return };
    if md.len() == 0 || md.len() > MAX_BYTES { return; }
    let Ok(b) = std::fs::read(p) else { return };
    if crate::text::looks_binary(&b) { return; }
    let doc = crate::textdoc::load(&b);
    for (i, l) in doc.lines.iter().enumerate() {
        if l.len() > 100_000 { continue; }                       // minified one-liners
        if m.is_match(l) {
            let mut t = l.trim().to_string();
            if t.chars().count() > 300 { t = t.chars().take(300).collect(); }
            out.push(Hit { path: p.to_path_buf(), line: i + 1, text: t, is_dir: false });
            *hits += 1;
            if *hits >= MAX_HITS { return; }
        }
    }
}

/// A text progress bar: ▕████████░░░░░░░░▏
pub fn bar(pct: f32, width: usize) -> String {
    let full = (pct.clamp(0.0, 1.0) * width as f32 * 8.0).round() as usize;
    let parts = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let mut s = String::from("▕");
    for i in 0..width {
        let v = full.saturating_sub(i * 8).min(8);
        s.push(if v == 8 { '█' } else if v == 0 { '░' } else { parts[v] });
    }
    s.push('▏');
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matchers() {
        let o = Opts { what: What::Both, regex: false, case: false, global: false };
        let m = Matcher::new("Hello", &o).unwrap();
        assert!(m.is_match("say hello there"));
        assert_eq!(m.find("say hello"), Some((4, 9)));
        let o = Opts { regex: true, ..o };
        let m = Matcher::new(r"fn \w+\(", &o).unwrap();
        assert!(m.is_match("pub fn main() {"));
        assert_eq!(m.find("pub fn main("), Some((4, 12)));
        assert!(Matcher::new("(", &o).is_err());
        assert_eq!(bar(0.5, 4), "▕██░░▏");
    }
}
