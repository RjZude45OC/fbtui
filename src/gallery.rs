//! Image gallery (Alt+I in the file browser): the pictures of a folder as a grid of thumbnails.
//! Thumbnails load in the background, the visible ones first. Enter opens the picture in the viewer.

use crate::config::Theme;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseButton, MouseEventKind};
use ratatui::layout::{Rect, Size};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::Protocol;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

pub fn is_picture(p: &Path) -> bool { matches!(crate::media::kind_of(p), crate::media::Kind::Picture | crate::media::Kind::Svg) }

/// Pictures in `dir` (name order).
pub fn pictures(dir: &Path, hidden: bool) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path())
        .filter(|p| p.is_file() && is_picture(p) && (hidden || !p.file_name().map(|n| n.to_string_lossy().starts_with('.')).unwrap_or(false))).collect()).unwrap_or_default();
    v.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default());
    v
}

type Job = (usize, PathBuf, u32, u32);

struct Loader { queue: Arc<(Mutex<Vec<Job>>, Condvar)>, rx: Receiver<(usize, u32, u32, Option<image::DynamicImage>)> }

impl Loader {
    fn new() -> Loader {
        let queue: Arc<(Mutex<Vec<Job>>, Condvar)> = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
        let (tx, rx): (Sender<_>, _) = channel();
        for _ in 0..3 {
            let q = queue.clone();
            let tx = tx.clone();
            std::thread::spawn(move || loop {
                let job = {
                    let (m, cv) = &*q;
                    let mut g = m.lock().unwrap();
                    while g.is_empty() { g = cv.wait(g).unwrap(); }
                    g.remove(0)
                };
                let (i, path, w, h) = job;
                let img = crate::media::load_picture(&path, w.max(h)).map(|(img, _, _)| crate::media::fit_sharp(img, w, h, 4.0));
                if tx.send((i, w, h, img)).is_err() { return; }
            });
        }
        Loader { queue, rx }
    }
    /// Replace what is waiting with these (visible first).
    fn want(&self, jobs: Vec<Job>) {
        let (m, cv) = &*self.queue;
        *m.lock().unwrap() = jobs;
        cv.notify_all();
    }
}

pub struct Gallery {
    dir: PathBuf,
    files: Vec<PathBuf>,
    sel: usize,
    top_row: usize,
    tile: (u16, u16),                 // tile size in cells (image area + label row)
    cols: usize,
    rows: usize,
    grid: Rect,
    thumbs: HashMap<usize, (u32, u32, image::DynamicImage)>,
    failed: HashSet<usize>,
    protos: HashMap<usize, (Rect, Protocol)>,
    pending: HashSet<usize>,
    loader: Loader,
    picker: Picker,
    last_click: Option<(usize, Instant)>,
    pub repaint: bool,
}

pub enum Out { Stay, Close(Option<PathBuf>), Open(PathBuf) }

impl Gallery {
    pub fn new(dir: &Path, start: Option<&Path>, picker: Picker, hidden: bool) -> Gallery {
        let files = pictures(dir, hidden);
        let sel = start.and_then(|s| files.iter().position(|f| f == s)).unwrap_or(0);
        Gallery { dir: dir.to_path_buf(), files, sel, top_row: 0, tile: (22, 11), cols: 1, rows: 1, grid: Rect::default(), thumbs: HashMap::new(), failed: HashSet::new(),
            protos: HashMap::new(), pending: HashSet::new(), loader: Loader::new(), picker, last_click: None, repaint: true }
    }

    fn img_px(&self) -> (u32, u32) {
        let fs = self.picker.font_size();
        ((self.tile.0 - 2) as u32 * fs.width as u32, (self.tile.1 - 2) as u32 * fs.height as u32)
    }

    /// New pictures from the loader. True if something arrived.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok((i, w, h, img)) = self.loader.rx.try_recv() {
            self.pending.remove(&i);
            match img { Some(img) => { self.thumbs.insert(i, (w, h, img)); self.protos.remove(&i); } None => { self.failed.insert(i); } }
            any = true;
        }
        any
    }

    fn request(&mut self) {
        let (w, h) = self.img_px();
        let first = self.top_row * self.cols;
        let vis_end = (first + self.cols * self.rows).min(self.files.len());
        // visible first, then the next screen
        let mut jobs = Vec::new();
        for i in (first..vis_end).chain(vis_end..(vis_end + self.cols * self.rows).min(self.files.len())) {
            let have = self.thumbs.get(&i).map(|(tw, th, _)| (*tw, *th) == (w, h)).unwrap_or(false);
            if !have && !self.failed.contains(&i) { jobs.push((i, self.files[i].clone(), w, h)); self.pending.insert(i); }
        }
        self.loader.want(jobs);
    }

    fn fix_scroll(&mut self) {
        let row = self.sel / self.cols.max(1);
        let old = self.top_row;
        if row < self.top_row { self.top_row = row; }
        if row >= self.top_row + self.rows { self.top_row = row + 1 - self.rows; }
        if self.top_row != old { self.repaint = true; }
    }

    pub fn on_key(&mut self, k: event::KeyEvent) -> Out {
        let n = self.files.len();
        if n == 0 { return if matches!(k.code, KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q')) { Out::Close(None) } else { Out::Stay }; }
        let c = self.cols.max(1);
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => return Out::Close(self.files.get(self.sel).cloned()),
            KeyCode::Enter => return Out::Open(self.files[self.sel].clone()),
            KeyCode::Left => self.sel = self.sel.saturating_sub(1),
            KeyCode::Right => self.sel = (self.sel + 1).min(n - 1),
            KeyCode::Up => self.sel = self.sel.saturating_sub(c),
            KeyCode::Down => self.sel = (self.sel + c).min(n - 1),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(c * self.rows.max(1)),
            KeyCode::PageDown => self.sel = (self.sel + c * self.rows.max(1)).min(n - 1),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = n - 1,
            KeyCode::Char('+') | KeyCode::Char('=') => { self.tile = ((self.tile.0 + 6).min(60), (self.tile.1 + 3).min(30)); self.resized(); }
            KeyCode::Char('-') => { self.tile = ((self.tile.0.saturating_sub(6)).max(12), (self.tile.1.saturating_sub(3)).max(6)); self.resized(); }
            _ => {}
        }
        self.fix_scroll();
        Out::Stay
    }

    fn resized(&mut self) { self.protos.clear(); self.repaint = true; }

    pub fn on_mouse(&mut self, m: event::MouseEvent) -> Out {
        let g = self.grid;
        match m.kind {
            MouseEventKind::ScrollDown => { self.sel = (self.sel + self.cols).min(self.files.len().saturating_sub(1)); self.fix_scroll(); }
            MouseEventKind::ScrollUp => { self.sel = self.sel.saturating_sub(self.cols); self.fix_scroll(); }
            MouseEventKind::Down(MouseButton::Left) if m.column >= g.x && m.row >= g.y && m.row < g.y + g.height => {
                let col = ((m.column - g.x) / self.tile.0) as usize;
                let row = ((m.row - g.y) / self.tile.1) as usize;
                let i = (self.top_row + row) * self.cols + col;
                if col < self.cols && i < self.files.len() {
                    let now = Instant::now();
                    if matches!(self.last_click, Some((j, t)) if j == i && now.duration_since(t).as_millis() < 450) { return Out::Open(self.files[i].clone()); }
                    self.sel = i;
                    self.last_click = Some((i, now));
                }
            }
            _ => {}
        }
        Out::Stay
    }

    pub fn draw(&mut self, f: &mut Frame, t: &Theme) {
        let area = f.area();
        let base = match t.background { Some(bg) => Style::default().bg(bg).fg(t.text), None => Style::default().fg(t.text) };
        f.render_widget(Block::default().style(base), area);
        let dim = Style::default().fg(t.dim);
        let buf = f.buffer_mut();
        buf.set_string(area.x + 2, area.y + 1, "▌ ", Style::default().fg(t.accent));
        buf.set_string(area.x + 4, area.y + 1, "GALLERY", Style::default().fg(t.text).add_modifier(Modifier::BOLD));
        buf.set_string(area.x + 2, area.y + 2, "▌ ", Style::default().fg(t.accent));
        buf.set_string(area.x + 4, area.y + 2, crate::ui::fit(&self.dir.display().to_string(), area.width.saturating_sub(8) as usize).trim_end(), dim);
        let n = self.files.len();
        let info = if n == 0 { "no pictures in this folder".to_string() } else {
            let p = &self.files[self.sel];
            let size = std::fs::metadata(p).map(|m| crate::text::format_size(m.len())).unwrap_or_default();
            format!("{}/{}  ·  {}  ·  {}", self.sel + 1, n, p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), size)
        };
        buf.set_string(area.x + 2, area.y + 3, "▌ ", Style::default().fg(t.accent));
        buf.set_string(area.x + 4, area.y + 3, crate::ui::fit(&info, area.width.saturating_sub(8) as usize).trim_end(), dim);
        self.grid = Rect { x: area.x + 2, y: area.y + 5, width: area.width.saturating_sub(4), height: area.height.saturating_sub(7) };
        let cols = (self.grid.width / self.tile.0).max(1) as usize;
        let rows = (self.grid.height / self.tile.1).max(1) as usize;
        if (cols, rows) != (self.cols, self.rows) { self.cols = cols; self.rows = rows; self.repaint = true; self.protos.clear(); }
        self.fix_scroll();
        self.request();
        let first = self.top_row * cols;
        for k in 0..cols * rows {
            let i = first + k;
            if i >= n { break; }
            let x = self.grid.x + (k % cols) as u16 * self.tile.0;
            let y = self.grid.y + (k / cols) as u16 * self.tile.1;
            let sel = i == self.sel;
            let frame = Rect { x, y, width: self.tile.0 - 1, height: self.tile.1 - 1 };
            // frame around the selected one
            let fs = if sel { Style::default().fg(t.accent).add_modifier(Modifier::BOLD) } else { Style::default().fg(t.select_bg) };
            let buf = f.buffer_mut();
            let (tl, tr, bl, br, hz, vt) = if sel { ("┏", "┓", "┗", "┛", "━", "┃") } else { ("╭", "╮", "╰", "╯", "─", "│") };
            buf.set_string(frame.x, frame.y, format!("{tl}{}{tr}", hz.repeat(frame.width as usize - 2)), fs);
            for yy in frame.y + 1..frame.y + frame.height - 2 { buf.set_string(frame.x, yy, vt, fs); buf.set_string(frame.x + frame.width - 1, yy, vt, fs); }
            buf.set_string(frame.x, frame.y + frame.height - 2, format!("{bl}{}{br}", hz.repeat(frame.width as usize - 2)), fs);
            let name = self.files[i].file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let label = crate::ui::fit(&name, frame.width as usize);
            buf.set_string(frame.x, frame.y + frame.height - 1, label.trim_end(), if sel { Style::default().fg(t.text).add_modifier(Modifier::BOLD) } else { dim });
            let inner = Rect { x: frame.x + 1, y: frame.y + 1, width: frame.width - 2, height: frame.height - 3 };
            if let Some((_, _, img)) = self.thumbs.get(&i) {
                let fresh = self.protos.get(&i).map(|(r, _)| *r != inner).unwrap_or(true);
                if fresh {
                    if let Ok(p) = self.picker.new_protocol(img.clone(), Size::new(inner.width, inner.height), ratatui_image::Resize::Fit(None)) { self.protos.insert(i, (inner, p)); }
                }
                if let Some((_, p)) = self.protos.get(&i) {
                    let sz = p.size();
                    let r = Rect { x: inner.x + inner.width.saturating_sub(sz.width) / 2, y: inner.y + inner.height.saturating_sub(sz.height) / 2, width: sz.width.min(inner.width), height: sz.height.min(inner.height) };
                    f.render_widget(ratatui_image::Image::new(p), r);
                }
            } else {
                let msg = if self.failed.contains(&i) { "cannot read" } else { "loading…" };
                f.buffer_mut().set_string(inner.x + inner.width.saturating_sub(msg.width() as u16) / 2, inner.y + inner.height / 2, msg, dim);
            }
        }
        let foot = crate::ui::hint_line(&[("arrows", "move"), ("Enter", "view"), ("+ −", "thumbnail size"), ("Esc", "back to the list")], area.width, t);
        f.render_widget(Paragraph::new(Line::from(foot)), Rect { x: area.x, y: area.y + area.height.saturating_sub(1), width: area.width, height: 1 });
    }
}

/// Run the gallery. Returns (picture to open in the viewer, last selected picture).
pub fn run(terminal: &mut ratatui::DefaultTerminal, picker: Picker, dir: &Path, start: Option<&Path>, hidden: bool) -> std::io::Result<(Option<PathBuf>, Option<PathBuf>)> {
    let cfg = crate::config::Config::load();
    let mut g = Gallery::new(dir, start, picker, hidden);
    loop {
        if g.repaint { g.repaint = false; let _ = terminal.clear(); }
        terminal.draw(|f| g.draw(f, &cfg.theme))?;
        let mut dirty = false;
        while !dirty {
            if g.poll() { dirty = true; }
            if event::poll(Duration::from_millis(40))? {
                let out = match event::read()? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => g.on_key(k),
                    Event::Mouse(m) => g.on_mouse(m),
                    Event::Resize(_, _) => { g.repaint = true; Out::Stay }
                    _ => continue,
                };
                match out {
                    Out::Stay => {}
                    Out::Close(last) => return Ok((None, last)),
                    Out::Open(p) => return Ok((Some(p.clone()), Some(p))),
                }
                dirty = true;
            }
        }
    }
}
