//! Folder compare (Alt+D on two marked folders or files): which files are only on the left, only on
//! the right, or different (by content), with a line diff of the selected text file. Files can be
//! copied across with > and <.

use crate::config::Theme;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum State { Left, Right, Changed, Same }

#[derive(Clone, Debug)]
pub struct Item { pub rel: String, pub state: State, pub dir: bool, pub ls: Option<u64>, pub rs: Option<u64> }

fn walk(root: &Path, base: &Path, out: &mut BTreeMap<String, (bool, u64)>, n: &mut usize) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name == ".git" { continue; }
        let p = e.path();
        let rel = p.strip_prefix(base).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or(name);
        let Ok(md) = e.metadata() else { continue };
        *n += 1;
        if md.is_dir() { out.insert(rel, (true, 0)); walk(&p, base, out, n); } else { out.insert(rel, (false, md.len())); }
    }
}

fn same_content(a: &Path, b: &Path) -> bool {
    let (Ok(mut fa), Ok(mut fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else { return false };
    let (mut ba, mut bb) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let na = fa.read(&mut ba).unwrap_or(0);
        let mut nb = 0;
        while nb < na { let k = fb.read(&mut bb[nb..na]).unwrap_or(0); if k == 0 { break; } nb += k; }
        if na != nb || ba[..na] != bb[..nb] { return false; }
        if na == 0 { return fb.read(&mut bb[..1]).unwrap_or(0) == 0; }
    }
}

/// Compare two folders: every file and folder with its state (folders only on one side are one item).
pub fn compare(left: &Path, right: &Path) -> Vec<Item> {
    let (mut l, mut r) = (BTreeMap::new(), BTreeMap::new());
    let mut n = 0;
    walk(left, left, &mut l, &mut n);
    walk(right, right, &mut r, &mut n);
    let mut keys: Vec<&String> = l.keys().chain(r.keys()).collect();
    keys.sort();
    keys.dedup();
    let mut out: Vec<Item> = Vec::new();
    let mut skip_under: Option<String> = None;
    for k in keys {
        if let Some(s) = &skip_under { if k.starts_with(s.as_str()) { continue; } else { skip_under = None; } }
        let (a, b) = (l.get(k), r.get(k));
        let dir = a.map(|x| x.0).or(b.map(|x| x.0)).unwrap_or(false);
        let state = match (a, b) {
            (Some(_), None) => State::Left,
            (None, Some(_)) => State::Right,
            (Some(x), Some(y)) if x.0 || y.0 => { if x.0 != y.0 { State::Changed } else { State::Same } }
            (Some(x), Some(y)) => if x.1 != y.1 || !same_content(&left.join(k), &right.join(k)) { State::Changed } else { State::Same },
            _ => continue,
        };
        // a folder that is only on one side: one line, not every file in it
        if dir && matches!(state, State::Left | State::Right) { skip_under = Some(format!("{k}/")); }
        if dir && state == State::Same { continue; }
        out.push(Item { rel: k.clone(), state, dir, ls: a.map(|x| x.1), rs: b.map(|x| x.1) });
    }
    out
}

fn read_text(p: &Path) -> Option<String> {
    let b = std::fs::read(p).ok()?;
    if b.len() > 8 << 20 || crate::text::looks_binary(&b) { return None; }
    Some(crate::text::decode(&b).0)
}

/// Unified diff lines (" ", "+", "-", "@@") of two texts.
pub fn diff_lines(a: &str, b: &str) -> Vec<String> {
    let d = similar::TextDiff::from_lines(a, b);
    let mut out = Vec::new();
    for group in d.grouped_ops(3) {
        let (first, last) = (group.first().unwrap(), group.last().unwrap());
        let (o1, n1) = (first.old_range().start, first.new_range().start);
        let (o2, n2) = (last.old_range().end, last.new_range().end);
        out.push(format!("@@ -{},{} +{},{} @@", o1 + 1, o2 - o1, n1 + 1, n2 - n1));
        for op in group {
            for c in d.iter_changes(&op) {
                let sign = match c.tag() { similar::ChangeTag::Delete => '-', similar::ChangeTag::Insert => '+', similar::ChangeTag::Equal => ' ' };
                out.push(format!("{sign}{}", c.value().trim_end_matches(['\n', '\r'])));
            }
        }
    }
    out
}

pub struct View {
    left: PathBuf,
    right: PathBuf,
    files: bool,                       // comparing two files, not folders
    items: Vec<Item>,
    show_same: bool,
    sel: usize,
    preview: Vec<String>,
    pv_scroll: usize,
    msg: String,
    msg_err: bool,
    confirm: Option<(PathBuf, PathBuf)>,
    loading: Option<Receiver<Vec<Item>>>,
}

impl View {
    pub fn new(left: &Path, right: &Path) -> View {
        let files = left.is_file() && right.is_file();
        let mut v = View { left: left.to_path_buf(), right: right.to_path_buf(), files, items: Vec::new(), show_same: false, sel: 0, preview: Vec::new(), pv_scroll: 0,
            msg: String::new(), msg_err: false, confirm: None, loading: None };
        v.start();
        v
    }

    fn start(&mut self) {
        if self.files { self.load_preview(); return; }
        let (l, r) = (self.left.clone(), self.right.clone());
        let (tx, rx) = channel();
        std::thread::spawn(move || { let _ = tx.send(compare(&l, &r)); });
        self.loading = Some(rx);
        self.msg = "comparing…".into();
    }

    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.loading else { return false };
        let Ok(items) = rx.try_recv() else { return false };
        self.loading = None;
        self.items = items;
        let c = |s: State| self.items.iter().filter(|i| i.state == s).count();
        self.msg = format!("{} only left  ·  {} only right  ·  {} different  ·  {} the same", c(State::Left), c(State::Right), c(State::Changed), c(State::Same));
        self.msg_err = false;
        self.sel = 0;
        self.load_preview();
        true
    }

    fn visible(&self) -> Vec<&Item> { self.items.iter().filter(|i| self.show_same || i.state != State::Same).collect() }

    fn load_preview(&mut self) {
        self.pv_scroll = 0;
        let (a, b) = if self.files { (self.left.clone(), self.right.clone()) } else {
            let Some(it) = self.visible().get(self.sel).map(|i| (*i).clone()) else { self.preview = vec!["The two folders have the same files.".into()]; return };
            if it.dir { self.preview = vec![format!("folder {}", match it.state { State::Left => "only on the left", State::Right => "only on the right", _ => "is a file on one side" })]; return; }
            (self.left.join(&it.rel), self.right.join(&it.rel))
        };
        self.preview = match (a.is_file(), b.is_file()) {
            (true, true) => match (read_text(&a), read_text(&b)) {
                (Some(x), Some(y)) => { let d = diff_lines(&x, &y); if d.is_empty() { vec!["identical".into()] } else { d } }
                _ => vec![format!("binary files  ·  left {}  ·  right {}", size(&a), size(&b))],
            },
            (true, false) => read_text(&a).map(|t| t.lines().take(500).map(|l| format!("-{l}")).collect()).unwrap_or_else(|| vec![format!("binary file only on the left  ·  {}", size(&a))]),
            (false, true) => read_text(&b).map(|t| t.lines().take(500).map(|l| format!("+{l}")).collect()).unwrap_or_else(|| vec![format!("binary file only on the right  ·  {}", size(&b))]),
            _ => Vec::new(),
        };
    }

    fn copy(&mut self, to_right: bool) {
        let Some(it) = self.visible().get(self.sel).map(|i| (*i).clone()) else { return };
        if it.dir { self.msg = "Copying whole folders is not supported here - copy it in the browser".into(); self.msg_err = true; return; }
        let (from, to) = if to_right { (self.left.join(&it.rel), self.right.join(&it.rel)) } else { (self.right.join(&it.rel), self.left.join(&it.rel)) };
        if !from.is_file() { self.msg = "Nothing to copy from that side".into(); self.msg_err = true; return; }
        self.msg = format!("Copy {} {} ? {}  Y / N", it.rel, if to_right { "→ right" } else { "← left" }, if to.exists() { "(replaces the other one)" } else { "" });
        self.msg_err = to.exists();
        self.confirm = Some((from, to));
    }

    /// false = close
    pub fn on_key(&mut self, k: KeyEvent) -> bool {
        if let Some((from, to)) = self.confirm.take() {
            if matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Enter) {
                if let Some(p) = to.parent() { let _ = std::fs::create_dir_all(p); }
                match std::fs::copy(&from, &to) { Ok(_) => { self.msg = "Copied".into(); self.msg_err = false; self.start(); } Err(e) => { self.msg = format!("Cannot copy: {e}"); self.msg_err = true; } }
            } else { self.msg = "Cancelled".into(); self.msg_err = false; }
            return true;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let n = if self.files { 0 } else { self.visible().len() };
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => return false,
            KeyCode::Up if ctrl => self.pv_scroll = self.pv_scroll.saturating_sub(3),
            KeyCode::Down if ctrl => self.pv_scroll += 3,
            KeyCode::Up if !self.files => { if self.sel > 0 { self.sel -= 1; self.load_preview(); } }
            KeyCode::Down if !self.files => { if self.sel + 1 < n { self.sel += 1; self.load_preview(); } }
            KeyCode::Up => self.pv_scroll = self.pv_scroll.saturating_sub(1),
            KeyCode::Down => self.pv_scroll += 1,
            KeyCode::PageUp => self.pv_scroll = self.pv_scroll.saturating_sub(20),
            KeyCode::PageDown => self.pv_scroll += 20,
            KeyCode::Home if !self.files => { self.sel = 0; self.load_preview(); }
            KeyCode::End if !self.files => { self.sel = n.saturating_sub(1); self.load_preview(); }
            KeyCode::Char('s') | KeyCode::Char('S') if !self.files => { self.show_same = !self.show_same; self.sel = 0; self.load_preview(); }
            KeyCode::Char('>') if !self.files => self.copy(true),
            KeyCode::Char('<') if !self.files => self.copy(false),
            KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::F(5) => self.start(),
            _ => {}
        }
        true
    }

    pub fn draw(&self, f: &mut Frame, t: &Theme) {
        let area = f.area();
        let base = match t.background { Some(bg) => Style::default().bg(bg).fg(t.text), None => Style::default().fg(t.text) };
        f.render_widget(Block::default().style(base), area);
        let dim = Style::default().fg(t.dim);
        let w = area.width;
        let row = |y: u16| Rect { x: area.x, y, width: w, height: 1 };
        let bar = Span::styled("  ▌ ", Style::default().fg(t.accent));
        f.render_widget(Paragraph::new(Line::from(vec![bar.clone(), Span::styled(if self.files { "COMPARE FILES" } else { "COMPARE FOLDERS" }, Style::default().fg(t.text).add_modifier(Modifier::BOLD))])), row(area.y + 1));
        f.render_widget(Paragraph::new(Line::from(vec![bar.clone(), Span::styled("left   ", Style::default().fg(t.error)), Span::styled(self.left.display().to_string(), dim)])), row(area.y + 2));
        f.render_widget(Paragraph::new(Line::from(vec![bar.clone(), Span::styled("right  ", Style::default().fg(t.ok)), Span::styled(self.right.display().to_string(), dim)])), row(area.y + 3));
        let body = Rect { x: area.x, y: area.y + 5, width: w, height: area.height.saturating_sub(8) };
        let (list, pv) = if self.files { (Rect::default(), Rect { x: body.x + 2, width: w.saturating_sub(4), ..body }) } else {
            let lw = (w as u32 * 40 / 100).clamp(28, 70) as u16;
            for y in body.y..body.y + body.height { f.buffer_mut().set_string(body.x + lw, y, "│", dim); }
            (Rect { width: lw, ..body }, Rect { x: body.x + lw + 2, width: w.saturating_sub(lw + 3), ..body })
        };
        if !self.files {
            let vis = self.visible();
            let top = self.sel.saturating_sub(list.height.saturating_sub(1) as usize);
            let mut lines = Vec::new();
            if vis.is_empty() && self.loading.is_none() { lines.push(Line::styled("  no differences ✓", Style::default().fg(t.ok))); }
            for (i, it) in vis.iter().enumerate().skip(top).take(list.height as usize) {
                let sel = i == self.sel;
                let bg = if sel { Style::default().bg(t.select_bg) } else { Style::default() };
                let (mark, col) = match it.state { State::Left => ("−", t.error), State::Right => ("+", t.ok), State::Changed => ("~", t.highlight), State::Same => ("=", t.dim) };
                let name = format!("{}{} {}{}", if sel { "► " } else { "  " }, mark, it.rel, if it.dir { "/" } else { "" });
                let sz = |v: Option<u64>| v.map(crate::text::format_size).unwrap_or_else(|| "-".into());
                let sizes = if it.dir { String::new() } else if it.state == State::Changed { format!(" {} → {} ", sz(it.ls), sz(it.rs)) } else { format!(" {} ", sz(it.ls.or(it.rs))) };
                let room = (list.width as usize).saturating_sub(sizes.width());
                let txt = crate::ui::fit(&name, room);
                lines.push(Line::from(vec![Span::styled(txt, bg.fg(col).add_modifier(if sel { Modifier::BOLD } else { Modifier::empty() })), Span::styled(sizes, bg.fg(t.dim))]));
            }
            f.render_widget(Paragraph::new(lines), list);
        }
        let mut pl = Vec::new();
        for l in self.preview.iter().skip(self.pv_scroll).take(pv.height as usize) {
            let st = if l.starts_with('+') { Style::default().fg(t.ok) } else if l.starts_with('-') { Style::default().fg(t.error) } else if l.starts_with("@@") { Style::default().fg(t.accent) } else { Style::default().fg(t.text) };
            pl.push(Line::styled(crate::ui::fit(&l.replace('\t', "    "), pv.width as usize).trim_end().to_string(), st));
        }
        f.render_widget(Paragraph::new(pl), pv);
        if self.preview.len() > pv.height as usize {
            let more = format!(" {}-{} of {} ", self.pv_scroll + 1, (self.pv_scroll + pv.height as usize).min(self.preview.len()), self.preview.len());
            f.buffer_mut().set_string(pv.x + pv.width.saturating_sub(more.width() as u16 + 1), pv.y, &more, dim);
        }
        f.buffer_mut().set_string(area.x + 2, area.y + area.height.saturating_sub(2), crate::ui::fit(&self.msg, w.saturating_sub(4) as usize).trim_end(), Style::default().fg(if self.msg_err { t.error } else { t.ok }));
        let hints: Vec<(&str, &str)> = if self.files { vec![("↑↓ PgUp PgDn", "scroll"), ("Esc", "back")] }
            else { vec![("↑↓", "file"), ("Ctrl+↑↓ PgUp PgDn", "scroll diff"), ("S", if self.show_same { "hide same" } else { "show same" }), (">", "copy to right"), ("<", "copy to left"), ("R", "compare again"), ("Esc", "back")] };
        f.render_widget(Paragraph::new(crate::ui::hint_line(&hints, w, t)), row(area.y + area.height.saturating_sub(1)));
    }
}

fn size(p: &Path) -> String { std::fs::metadata(p).map(|m| crate::text::format_size(m.len())).unwrap_or_default() }

pub fn run(terminal: &mut ratatui::DefaultTerminal, left: &Path, right: &Path) -> std::io::Result<()> {
    let mut cfg = crate::config::Config::load();
    let mut v = View::new(left, right);
    let _ = terminal.clear();
    loop {
        terminal.draw(|f| v.draw(f, &cfg.theme))?;
        loop {
            if v.poll() { break; }
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => {
                        if k.code == KeyCode::F(9) { cfg.cycle_theme(1); let _ = terminal.clear(); }
                        else if !v.on_key(k) { return Ok(()); }
                    }
                    Event::Mouse(m) => match m.kind {
                        MouseEventKind::ScrollDown => v.pv_scroll += 3,
                        MouseEventKind::ScrollUp => v.pv_scroll = v.pv_scroll.saturating_sub(3),
                        _ => continue,
                    },
                    Event::Resize(_, _) => { let _ = terminal.clear(); }
                    _ => continue,
                }
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn folders() {
        let base = std::env::temp_dir().join(format!("fb_diff_{}", std::process::id()));
        let (l, r) = (base.join("l"), base.join("r"));
        for d in [&l, &r] { std::fs::create_dir_all(d.join("sub")).unwrap(); }
        std::fs::write(l.join("same.txt"), "x").unwrap(); std::fs::write(r.join("same.txt"), "x").unwrap();
        std::fs::write(l.join("sub/ch.txt"), "a\nb\nc\n").unwrap(); std::fs::write(r.join("sub/ch.txt"), "a\nB\nc\n").unwrap();
        std::fs::write(l.join("only_l.txt"), "1").unwrap();
        std::fs::create_dir_all(r.join("newdir/deep")).unwrap(); std::fs::write(r.join("newdir/deep/f"), "1").unwrap();
        let v = compare(&l, &r);
        let s: Vec<(String, State)> = v.iter().map(|i| (i.rel.clone(), i.state)).collect();
        assert!(s.contains(&("only_l.txt".into(), State::Left)));
        assert!(s.contains(&("newdir".into(), State::Right)));
        assert!(!s.iter().any(|(r, _)| r.starts_with("newdir/")));
        assert!(s.contains(&("sub/ch.txt".into(), State::Changed)));
        assert!(s.contains(&("same.txt".into(), State::Same)));
        let d = diff_lines("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(d, vec!["@@ -1,3 +1,3 @@", " a", "-b", "+B", " c"]);
        let _ = std::fs::remove_dir_all(base);
    }
}
