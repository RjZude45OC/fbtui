//! Background preview engine: worker threads build previews off the UI thread,
//! results are cached (LRU) and neighbours of the selection are prepared in advance.

use crate::config::Theme;
use crate::fsutil::{ext_of, fmt_time};
use crate::highlight;
use crate::media::{self, Kind};
use crate::text::{clean_line, decode, format_size, looks_binary};
use image::DynamicImage;
use ratatui::layout::Size;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use ratatui_image::Resize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};

pub const TEXT_MAX_BYTES: u64 = 4 * 1024 * 1024;
pub const TEXT_MAX_LINES: usize = 20_000;
pub const HEX_MAX_BYTES: usize = 64 * 1024;

pub const SHELL_PIC_EXTS: &[&str] = &[".heic", ".heif", ".avif", ".svg", ".psd", ".dng", ".cr2", ".nef", ".arw", ".pdf",
    ".mp4", ".mkv", ".avi", ".mov", ".wmv", ".webm", ".m4v", ".mpg", ".mpeg", ".3gp", ".flv"];
pub const OFFICE_EXTS: &[&str] = &[".docx", ".pptx", ".xlsx", ".odt", ".ods", ".odp"];
pub const ZIP_EXTS: &[&str] = &[".zip", ".jar", ".apk", ".nupkg", ".vsix", ".whl", ".epub", ".xpi"];
pub const EXE_EXTS: &[&str] = &[".exe", ".dll", ".sys", ".ocx", ".cpl", ".scr", ".msi"];

pub struct Preview {
    pub meta: String,
    pub lines: Vec<Line<'static>>,
    pub image: Option<Protocol>,
    pub image_rows: u16,
    pub scrollable: bool,
}

#[derive(Clone)]
pub struct Job {
    pub key: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_drive: bool,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
    pub cols: u16,
    pub rows: u16,
    pub show_hidden: bool,
    pub theme: Arc<Theme>,
    pub picker: Option<Picker>,
}

pub fn job_key(path: &Path, modified: Option<std::time::SystemTime>, cols: u16, rows: u16, theme: &str, hidden: bool) -> String {
    let m = modified.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis()).unwrap_or(0);
    format!("{}|{m}|{cols}x{rows}|{theme}|{hidden}", path.display())
}

struct Queue { jobs: VecDeque<Job>, busy: HashSet<String> }

pub struct Engine {
    queue: Arc<(Mutex<Queue>, Condvar)>,
    results: Receiver<(String, Arc<Preview>)>,
    cache: HashMap<String, Arc<Preview>>,
    order: VecDeque<String>,
    capacity: usize,
}

impl Engine {
    pub fn new(threads: usize) -> Engine {
        let queue = Arc::new((Mutex::new(Queue { jobs: VecDeque::new(), busy: HashSet::new() }), Condvar::new()));
        let (tx, rx) = std::sync::mpsc::channel();
        for _ in 0..threads {
            let q = queue.clone();
            let tx: Sender<(String, Arc<Preview>)> = tx.clone();
            std::thread::spawn(move || worker(q, tx));
        }
        Engine { queue, results: rx, cache: HashMap::new(), order: VecDeque::new(), capacity: 96 }
    }

    pub fn get(&self, key: &str) -> Option<Arc<Preview>> { self.cache.get(key).cloned() }

    /// Ask for previews: the first job is urgent, the rest are preloads. Stale preloads are dropped.
    pub fn request(&self, jobs: Vec<Job>) {
        let (m, cv) = &*self.queue;
        let mut q = m.lock().unwrap();
        q.jobs.clear();
        for (i, j) in jobs.into_iter().enumerate() {
            if self.cache.contains_key(&j.key) || q.busy.contains(&j.key) { continue; }
            if i == 0 { q.jobs.push_front(j) } else { q.jobs.push_back(j) }
        }
        cv.notify_all();
    }

    /// Collect finished previews; returns true if anything new arrived.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok((k, p)) = self.results.try_recv() {
            if !self.cache.contains_key(&k) {
                self.order.push_back(k.clone());
                if self.order.len() > self.capacity { if let Some(old) = self.order.pop_front() { self.cache.remove(&old); } }
            }
            self.cache.insert(k, p);
            any = true;
        }
        any
    }

    pub fn clear(&mut self) { self.cache.clear(); self.order.clear(); }
}

fn worker(q: Arc<(Mutex<Queue>, Condvar)>, tx: Sender<(String, Arc<Preview>)>) {
    crate::winapi::com_init();
    loop {
        let job = {
            let (m, cv) = &*q;
            let mut g = m.lock().unwrap();
            loop {
                if let Some(j) = g.jobs.pop_front() {
                    if g.busy.contains(&j.key) { continue; }
                    g.busy.insert(j.key.clone());
                    break j;
                }
                g = cv.wait(g).unwrap();
            }
        };
        let key = job.key.clone();
        let p = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(&job)))
            .unwrap_or_else(|_| info_preview(String::new(), vec!["(preview failed)".into()]));
        let _ = tx.send((key.clone(), Arc::new(p)));
        q.0.lock().unwrap().busy.remove(&key);
    }
}

fn info_preview(meta: String, lines: Vec<String>) -> Preview {
    Preview { meta, lines: lines.into_iter().map(Line::from).collect(), image: None, image_rows: 0, scrollable: false }
}

fn build(j: &Job) -> Preview {
    let t = &j.theme;
    if j.is_drive { return drive_preview(j); }
    if j.is_dir { return dir_preview(j); }
    let ext = ext_of(&j.path);
    let meta = format!("{}  ·  {}", format_size(j.size), fmt_time(j.modified));
    if j.size == 0 { return info_preview(meta, vec!["(empty file)".into()]); }

    // pictures, PDF, video, audio (with PDFium / FFmpeg when available)
    if let Some(p) = media_preview(j, &ext, &meta) { return p; }

    // fallback: ask Windows for the thumbnail
    if SHELL_PIC_EXTS.contains(&ext.as_str()) {
        let label = if ext == ".pdf" { "PDF document" } else if media::VIDEO_EXTS.contains(&ext.as_str()) { "video" } else { "image" };
        let meta = format!("{} {label}  ·  {meta}", ext.trim_start_matches('.').to_uppercase());
        if let Some(img) = crate::winapi::shell_image(&j.path, 1024, false) {
            return image_preview(j, img, meta, j.rows, Vec::new());
        }
        if let Some(icon) = crate::winapi::shell_image(&j.path, 256, true) {
            let hint = if ext == ".pdf" { vec!["PDFium not found: run migration\\get-deps.bat for page previews.".to_string()] } else { vec!["No thumbnail available (run get-deps.bat for FFmpeg).".into()] };
            return image_preview(j, icon, meta, 8, hint);
        }
    }

    if ext == ".md" || ext == ".markdown" {
        let mut buf = Vec::new();
        if let Ok(f) = std::fs::File::open(&j.path) { let _ = f.take(TEXT_MAX_BYTES).read_to_end(&mut buf); }
        let (text, enc) = decode(&buf);
        let lines = crate::markdown::render(&text, j.cols.saturating_sub(1) as usize, t);
        return Preview { meta: format!("Markdown (rendered)  ·  {meta}  ·  {enc}"), lines, image: None, image_rows: 0, scrollable: true };
    }

    if OFFICE_EXTS.contains(&ext.as_str()) {
        let lines = office_text(&j.path, &ext);
        return icon_and_lines(j, format!("{} document  ·  {meta}", ext.trim_start_matches('.').to_uppercase()), lines);
    }
    if ZIP_EXTS.contains(&ext.as_str()) {
        let lines = zip_listing(&j.path);
        return icon_and_lines(j, format!("{} archive  ·  {meta}", ext.trim_start_matches('.').to_uppercase()), lines);
    }
    if EXE_EXTS.contains(&ext.as_str()) {
        let lines = exe_info(&j.path);
        return icon_and_lines(j, format!("{} file  ·  {meta}", ext.trim_start_matches('.').to_uppercase()), lines);
    }
    if ext == ".lnk" || ext == ".url" {
        let lines = crate::winapi::shortcut_info(&j.path);
        return icon_and_lines(j, format!("Shortcut  ·  {meta}"), lines);
    }

    // text or binary
    let mut buf = Vec::new();
    let limit = TEXT_MAX_BYTES.min(j.size);
    if let Ok(f) = std::fs::File::open(&j.path) { let _ = f.take(limit).read_to_end(&mut buf); }
    if looks_binary(&buf) {
        let n = buf.len().min(HEX_MAX_BYTES);
        let per = if j.cols >= 76 { 16 } else if j.cols >= 44 { 8 } else { 4 };
        let lines = hex_lines(&buf[..n], per, t);
        let mut meta = format!("{meta}  ·  binary (hex)");
        if (j.size as usize) > n { meta += &format!("  ·  first {}", format_size(n as u64)); }
        return Preview { meta, lines, image: None, image_rows: 0, scrollable: true };
    }
    let (text, enc) = decode(&buf);
    let mut meta = format!("{meta}  ·  {enc}");
    if j.size > limit { meta += &format!("  ·  first {}", format_size(limit)); }
    let raw: Vec<&str> = text.lines().take(TEXT_MAX_LINES).collect();
    let lines = highlight::highlight(&j.path, &raw, t);
    Preview { meta, lines, image: None, image_rows: 0, scrollable: true }
}

/// Pixel size of the preview pane.
fn pane_px(j: &Job, rows: u16) -> (u32, u32) {
    let fs = j.picker.as_ref().map(|p| p.font_size()).unwrap_or((10, 20).into());
    (j.cols.max(1) as u32 * fs.width as u32, rows.max(1) as u32 * fs.height as u32)
}

fn media_preview(j: &Job, ext: &str, meta: &str) -> Option<Preview> {
    let t = &j.theme;
    let up = ext.trim_start_matches('.').to_uppercase();
    let hint = |s: &str| format!("→ {s}");
    match media::kind_of(&j.path) {
        Kind::Picture | Kind::Svg => {
            let rows_guess = j.rows.saturating_sub(7);
            let (pw, ph) = pane_px(j, rows_guess);
            let (img, (w, h), mut lines) = media::load_picture(&j.path, pw.max(ph))?;
            let dims = if w > 0 { format!("  ·  {w} x {h}") } else { String::new() };
            lines.push(String::new());
            lines.push(hint("full screen, zoom"));
            let rows = j.rows.saturating_sub(lines.len() as u16 + 1).max(j.rows / 2);
            Some(image_preview(j, img, format!("{up} image{dims}  ·  {meta}"), rows, lines))
        }
        Kind::Pdf => {
            let info = media::pdf_info(&j.path).ok()?;
            let mut lines = info.lines;
            lines.push(String::new());
            lines.push(hint(if info.pages > 1 { "read all pages full screen" } else { "full screen, zoom" }));
            let rows = j.rows.saturating_sub(lines.len() as u16 + 1).max(j.rows / 2);
            let (pw, ph) = pane_px(j, rows);
            let meta = format!("PDF  ·  {} page{}  ·  {meta}", info.pages, if info.pages == 1 { "" } else { "s" });
            match media::pdf_render(&j.path, 0, pw, ph) {
                Ok(img) => Some(image_preview(j, img, meta, rows, lines)),
                Err(e) => Some(info_preview(meta, vec![e])),
            }
        }
        Kind::Video => {
            let m = media::probe(&j.path)?;
            let mut lines = m.lines.clone();
            lines.push(String::new());
            lines.push(hint("full screen: seek with ← →, Space plays, P plays with sound"));
            let rows = j.rows.saturating_sub(lines.len() as u16 + 1).max(j.rows / 2);
            let (pw, ph) = pane_px(j, rows);
            let at = if m.duration > 4.0 { m.duration * 0.2 } else { 0.0 };
            let meta = format!("{up} video  ·  {}  ·  {meta}", media::fmt_dur(m.duration));
            match (m.has_video, media::video_frame(&j.path, at, pw, ph)) {
                (true, Some(img)) => Some(image_preview(j, img, meta, rows, lines)),
                _ => Some(info_preview(meta, lines)),
            }
        }
        Kind::Audio => {
            let m = media::probe(&j.path)?;
            let mut lines = m.lines.clone();
            lines.push(String::new());
            lines.push("P plays with sound  ·  → full screen".into());
            let meta = format!("{up} audio  ·  {}  ·  {meta}", media::fmt_dur(m.duration));
            let (pw, _) = pane_px(j, 1);
            let wave_rows = (j.rows / 4).clamp(3, 8);
            let (_, wh) = pane_px(j, wave_rows);
            let rgb = match t.accent { ratatui::style::Color::Rgb(r, g, b) => (r, g, b), _ => (139, 233, 253) };
            if m.has_cover {
                let rows = j.rows.saturating_sub(lines.len() as u16 + 1).max(j.rows / 2);
                let (cw, ch) = pane_px(j, rows);
                if let Some(img) = media::cover_art(&j.path, cw, ch) { return Some(image_preview(j, img, meta, rows, lines)); }
            }
            match media::waveform(&j.path, pw, wh, rgb) {
                Some(img) => Some(image_preview(j, img, meta, wave_rows, lines)),
                None => Some(info_preview(meta, lines)),
            }
        }
        Kind::None => None,
    }
}

fn drive_preview(j: &Job) -> Preview {
    let lines = crate::winapi::drive_info(&j.path);
    info_preview("Drive".into(), lines)
}

fn dir_preview(j: &Job) -> Preview {
    let t = &j.theme;
    match crate::fsutil::list_dir(&j.path, j.show_hidden) {
        Ok(v) => {
            let n = v.len();
            let mut lines: Vec<Line<'static>> = v.iter().take(1000).map(|e| {
                if e.is_dir { Line::styled(e.display(), Style::default().fg(t.accent)) } else { Line::styled(e.name.clone(), Style::default().fg(t.text)) }
            }).collect();
            if n == 0 { lines.push(Line::styled("(empty folder)", Style::default().fg(t.dim))); }
            Preview { meta: format!("Folder  ·  {n} items"), lines, image: None, image_rows: 0, scrollable: true }
        }
        Err(e) => info_preview("Folder".into(), vec![format!("Cannot open: {e}")]),
    }
}

fn image_preview(j: &Job, img: DynamicImage, meta: String, max_rows: u16, extra: Vec<String>) -> Preview {
    let rows = max_rows.min(j.rows).max(1);
    let cols = if max_rows <= 8 { j.cols.min(16) } else { j.cols };
    let mut lines: Vec<Line<'static>> = Vec::new();
    for l in extra { lines.push(Line::from(l)); }
    match &j.picker {
        Some(p) => {
            // fit into the pane at the terminal's real pixel size, resampled once with a sharp filter
            let fs = p.font_size();
            let img = crate::media::fit_sharp(img, cols.max(1) as u32 * fs.width as u32, rows as u32 * fs.height as u32, 8.0);
            match p.new_protocol(img, Size::new(cols.max(1), rows), Resize::Fit(None)) {
            Ok(proto) => {
                let h = proto.size().height;
                Preview { meta, lines, image: Some(proto), image_rows: h, scrollable: false }
            }
            Err(e) => info_preview(meta, vec![format!("Cannot draw image: {e}")]),
            }
        }
        None => info_preview(meta, vec!["(image preview off)".into()]),
    }
}

fn icon_and_lines(j: &Job, meta: String, lines: Vec<String>) -> Preview {
    let t = &j.theme;
    let styled: Vec<Line<'static>> = lines.into_iter().map(|l| Line::styled(clean_line(&l), Style::default().fg(t.text))).collect();
    if let Some(icon) = crate::winapi::shell_image(&j.path, 256, true) {
        let mut p = image_preview(j, icon, meta.clone(), 6, Vec::new());
        if p.image.is_some() { p.lines = styled; p.scrollable = false; return p; }
    }
    Preview { meta, lines: styled, image: None, image_rows: 0, scrollable: true }
}

fn hex_lines(b: &[u8], per: usize, t: &Theme) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for (i, chunk) in b.chunks(per).enumerate() {
        let mut hex = String::new();
        for (k, byte) in chunk.iter().enumerate() {
            if per >= 8 && k == per / 2 { hex.push(' '); }
            hex.push_str(&format!("{byte:02X} "));
        }
        let width = per * 3 + if per >= 8 { 1 } else { 0 };
        while hex.len() < width { hex.push(' '); }
        let ascii: String = chunk.iter().map(|&c| if (0x20..0x7F).contains(&c) { c as char } else { '.' }).collect();
        out.push(Line::from(vec![
            Span::styled(format!("{:08X}  ", i * per), Style::default().fg(t.dim)),
            Span::styled(hex, Style::default().fg(t.text)),
            Span::styled(format!(" {ascii}"), Style::default().fg(t.accent)),
        ]));
    }
    out
}

fn zip_listing(p: &Path) -> Vec<String> {
    let f = match std::fs::File::open(p) { Ok(f) => f, Err(e) => return vec![format!("Cannot open: {e}")] };
    let mut z = match zip::ZipArchive::new(f) { Ok(z) => z, Err(e) => return vec![format!("Cannot read archive: {e}")] };
    let mut total = 0u64;
    let mut rows = Vec::new();
    for i in 0..z.len() {
        if let Ok(e) = z.by_index_raw(i) {
            total += e.size();
            if rows.len() < 400 { rows.push(format!("{:>9}  {}", format_size(e.size()), e.name())); }
        }
    }
    let mut out = vec![format!("{} entries  ·  {} unpacked", z.len(), format_size(total)), String::new()];
    out.extend(rows);
    out
}

fn office_text(p: &Path, ext: &str) -> Vec<String> {
    let f = match std::fs::File::open(p) { Ok(f) => f, Err(e) => return vec![format!("Cannot open: {e}")] };
    let mut z = match zip::ZipArchive::new(f) { Ok(z) => z, Err(e) => return vec![format!("Cannot read document: {e}")] };
    let mut parts: Vec<String> = match ext {
        ".docx" => vec!["word/document.xml".into()],
        ".xlsx" => vec!["xl/sharedStrings.xml".into()],
        ".pptx" => {
            let mut v: Vec<String> = z.file_names().filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml")).map(|s| s.to_string()).collect();
            v.sort_by_key(|n| n.trim_start_matches("ppt/slides/slide").trim_end_matches(".xml").parse::<u32>().unwrap_or(0));
            v
        }
        _ => vec!["content.xml".into()],
    };
    let mut out = Vec::new();
    if ext == ".xlsx" { out.push("Cell text:".to_string()); out.push(String::new()); }
    for (i, name) in parts.drain(..).enumerate() {
        let mut xml = String::new();
        if let Ok(mut e) = z.by_name(&name) { let _ = e.read_to_string(&mut xml); } else { continue; }
        if ext == ".pptx" { if i > 0 { out.push(String::new()); } out.push(format!("--- slide {} ---", i + 1)); }
        let mut text = String::new();
        let mut in_tag = false;
        let mut tag = String::new();
        for ch in xml.chars() {
            if ch == '<' { in_tag = true; tag.clear(); continue; }
            if ch == '>' {
                in_tag = false;
                let t = tag.trim_end_matches('/');
                if matches!(t, "/w:p" | "/a:p" | "/text:p" | "/text:h" | "/si") { text.push('\n'); }
                if t.starts_with("w:tab") || t.starts_with("text:tab") { text.push_str("    "); }
                continue;
            }
            if in_tag { tag.push(ch) } else { text.push(ch) }
        }
        let text = text.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'");
        let mut blank = false;
        for l in text.lines() {
            let l = l.trim_end();
            if l.is_empty() { if blank { continue; } blank = true; } else { blank = false; }
            out.push(l.to_string());
            if out.len() > 2000 { return out; }
        }
    }
    out
}

fn exe_info(p: &Path) -> Vec<String> {
    let mut out = crate::winapi::version_info(p);
    let mut b = [0u8; 1024];
    if let Ok(mut f) = std::fs::File::open(p) {
        let n = f.read(&mut b).unwrap_or(0);
        if n > 0x40 && b[0] == b'M' && b[1] == b'Z' {
            let pe = u32::from_le_bytes([b[0x3C], b[0x3D], b[0x3E], b[0x3F]]) as usize;
            if pe + 6 <= n && &b[pe..pe + 2] == b"PE" {
                let m = u16::from_le_bytes([b[pe + 4], b[pe + 5]]);
                let arch = match m { 0x14c => "x86 (32-bit)".to_string(), 0x8664 => "x64 (64-bit)".into(), 0xAA64 => "ARM64".into(), x => format!("0x{x:04X}") };
                out.push(format!("{:<12}{arch}", "Machine:"));
            }
        }
    }
    out
}

