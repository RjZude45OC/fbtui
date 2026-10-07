//! Spotlight (Tab, Ctrl+Space or Ctrl+K): type a few letters of a tool (Tasks & clock, Typing test, Calendar…)
//! or of a bookmarked app, file or folder and press Enter to open it. Only tools that are on (Esc list) are shown.

use crate::config::Theme;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use std::path::PathBuf;
use unicode_width::UnicodeWidthStr;

pub const APP_EXTS: &[&str] = &[".exe", ".lnk", ".url", ".bat", ".cmd", ".ps1", ".py", ".pyw", ".msc", ".appref-ms", ".vbs", ".jar"];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind { Folder, App, File, Missing, Exit, Tool(crate::tools::Tool) }

#[derive(Clone)]
pub struct Item { pub slot: u32, pub path: PathBuf, pub name: String, pub kind: Kind }

pub enum Act { None, Close, Tool(crate::tools::Tool), Open(PathBuf), Reveal(PathBuf), Goto(PathBuf), Remove(u32), Quit }

pub struct Spotlight {
    pub query: String,
    items: Vec<Item>,
    results: Vec<(usize, Vec<usize>)>,       // item index, matched character positions in the name
    pub sel: usize,
}

pub fn item_for(slot: u32, p: &str) -> Item {
    let path = PathBuf::from(p);
    let ext = crate::fsutil::ext_of(&path);
    let kind = if path.is_dir() { Kind::Folder } else if !path.exists() { Kind::Missing }
        else if APP_EXTS.contains(&ext.as_str()) { Kind::App } else { Kind::File };
    let file = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string());
    // apps show without the extension ("notepad++" rather than "notepad++.exe")
    let name = if kind == Kind::App { path.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or(file) } else { file };
    Item { slot, path, name, kind }
}

/// Fuzzy match: all query letters in order. Higher score = better (start of words, runs of letters, name start).
fn score(name: &str, path: &str, q: &str) -> Option<(i64, Vec<usize>)> {
    if q.is_empty() { return Some((0, Vec::new())); }
    let n: Vec<char> = name.to_lowercase().chars().collect();
    let qc: Vec<char> = q.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    let mut pos = Vec::new();
    let mut sc = 0i64;
    let mut i = 0usize;
    let mut prev: Option<usize> = None;
    for &c in &qc {
        let mut found = None;
        while i < n.len() { if n[i] == c { found = Some(i); break; } i += 1; }
        match found {
            Some(k) => {
                sc += 10;
                if k == 0 { sc += 25; }
                else if !n[k - 1].is_alphanumeric() { sc += 15; }                // start of a word
                if prev.map(|p| p + 1 == k).unwrap_or(false) { sc += 12; }        // consecutive letters
                pos.push(k);
                prev = Some(k);
                i = k + 1;
            }
            None => {
                // not in the name: accept if the whole query is in the path (folder name, drive ...)
                let lp = path.to_lowercase();
                let qs: String = qc.iter().collect();
                return if lp.contains(&qs) { Some((1, Vec::new())) } else { None };
            }
        }
    }
    sc -= n.len() as i64 / 4;                                                    // shorter names first
    Some((sc, pos))
}

impl Spotlight {
    /// `exit_label`: what typing "exit" offers ("Exit the file browser").
    /// `tools`: the tools that are on, listed first.
    pub fn new(bookmarks: &[(u32, String)], exit_label: &str, tools: &[crate::tools::Tool]) -> Spotlight {
        let mut items: Vec<Item> = tools.iter().map(|&t| { let f = crate::tools::feature(t); Item { slot: 0, path: PathBuf::from(f.keys), name: f.name.to_string(), kind: Kind::Tool(t) } }).collect();
        items.extend(bookmarks.iter().map(|(s, p)| item_for(*s, p)));
        items.push(Item { slot: 0, path: PathBuf::from(exit_label), name: "exit".into(), kind: Kind::Exit });
        let mut s = Spotlight { query: String::new(), items, results: Vec::new(), sel: 0 };
        s.filter();
        s
    }

    fn filter(&mut self) {
        let q = self.query.trim().to_lowercase();
        let wants_exit = q.len() >= 2 && ["exit", "quit"].iter().any(|w| w.starts_with(&q) || q.starts_with(w));
        let mut r: Vec<(i64, usize, Vec<usize>)> = self.items.iter().enumerate()
            .filter_map(|(i, it)| {
                if it.kind == Kind::Exit { return if wants_exit { Some((i64::MAX, i, (0..q.len().min(4)).collect())) } else { None }; }
                score(&it.name, &it.path.to_string_lossy(), &self.query).map(|(s, p)| (s, i, p))
            }).collect();
        if !self.query.is_empty() { r.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1))); }
        self.results = r.into_iter().map(|(_, i, p)| (i, p)).collect();
        self.sel = 0;
    }

    pub fn current(&self) -> Option<&Item> { self.results.get(self.sel).map(|(i, _)| &self.items[*i]) }

    pub fn on_key(&mut self, k: KeyEvent) -> Act {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let n = self.results.len();
        match k.code {
            KeyCode::Esc => return Act::Close,
            KeyCode::Char(' ') if ctrl => return Act::Close,
            KeyCode::Char('k') | KeyCode::Char('K') if ctrl && !alt => return Act::Close,
            KeyCode::Up => { if n > 0 { self.sel = (self.sel + n - 1) % n; } }
            KeyCode::Tab if !ctrl => return Act::Close,
            KeyCode::Down => { if n > 0 { self.sel = (self.sel + 1) % n; } }
            KeyCode::PageUp => self.sel = 0,
            KeyCode::PageDown => self.sel = n.saturating_sub(1),
            KeyCode::Enter => {
                let Some(it) = self.current() else { return Act::None };
                if it.kind == Kind::Exit { return Act::Quit; }
                if let Kind::Tool(t) = it.kind { return Act::Tool(t); }
                if it.kind == Kind::Missing { return Act::Reveal(it.path.clone()); }
                return if alt || ctrl { Act::Reveal(it.path.clone()) } else { Act::Open(it.path.clone()) };
            }
            KeyCode::Right => {
                // go to it: a folder opens in the browser, a file / app is shown selected in its folder
                let Some(it) = self.current() else { return Act::None };
                if it.kind == Kind::Exit { return Act::None; }
                if let Kind::Tool(t) = it.kind { return Act::Tool(t); }
                return Act::Goto(it.path.clone());
            }
            KeyCode::Delete => {
                let Some(it) = self.current() else { return Act::None };
                if it.kind == Kind::Exit || matches!(it.kind, Kind::Tool(_)) { return Act::None; }
                let slot = it.slot;
                let i = self.results[self.sel].0;
                self.items.remove(i);
                let keep = self.sel;
                self.filter();
                self.sel = keep.min(self.results.len().saturating_sub(1));
                return Act::Remove(slot);
            }
            KeyCode::Backspace => { if self.query.pop().is_some() { self.filter(); } }
            KeyCode::Char('u') | KeyCode::Char('U') if ctrl => { self.query.clear(); self.filter(); }
            KeyCode::Char(c) if !(ctrl && !alt) && !c.is_control() => { self.query.push(c); self.filter(); }
            _ => {}
        }
        Act::None
    }

    pub fn draw(&self, f: &mut Frame, t: &Theme) {
        let area = f.area();
        let w = area.width.saturating_sub(4).min(84).max(30);
        let rows = self.results.len().clamp(1, 12) as u16;
        let h = (rows + 5).min(area.height.saturating_sub(2));
        let r = Rect { x: area.x + (area.width.saturating_sub(w)) / 2, y: area.y + (area.height.saturating_sub(h)) / 3, width: w, height: h };
        f.render_widget(Clear, r);
        let base = match t.background { Some(bg) => Style::default().bg(bg).fg(t.text), None => Style::default().fg(t.text) };
        let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(t.accent))
            .title(Span::styled(" Search · tools and bookmarks ", Style::default().fg(t.accent).add_modifier(Modifier::BOLD))).style(base);
        let inner = block.inner(r);
        f.render_widget(block, r);
        let dim = Style::default().fg(t.dim);
        // the input line
        let input = Rect { x: inner.x, y: inner.y, width: inner.width, height: 1 };
        let shown = if self.query.is_empty() { Span::styled("type a name…", dim) } else { Span::styled(self.query.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)) };
        f.render_widget(Paragraph::new(Line::from(vec![Span::styled(" › ", Style::default().fg(t.accent).add_modifier(Modifier::BOLD)), shown])), input);
        f.set_cursor_position((inner.x + 3 + self.query.width() as u16, inner.y));
        f.render_widget(Paragraph::new(Span::styled("─".repeat(inner.width as usize), dim)), Rect { y: inner.y + 1, height: 1, ..input });
        // results (scrolled so the selection is visible)
        let list_h = inner.height.saturating_sub(3) as usize;
        let top = self.sel.saturating_sub(list_h.saturating_sub(1));
        let mut lines = Vec::new();
        if self.results.is_empty() {
            lines.push(Line::styled(if self.items.len() <= 1 { "  Nothing to open yet - turn tools on with Esc, or press Ctrl+D in the browser to bookmark" } else { "  Nothing matches" }, dim));
        }
        for (k, (i, hits)) in self.results.iter().enumerate().skip(top).take(list_h) {
            let it = &self.items[*i];
            let sel = k == self.sel;
            let bg = if sel { Style::default().bg(t.select_bg) } else { Style::default() };
            let (icon, col) = match it.kind { Kind::Folder => ("▸ ", t.accent), Kind::App => ("◆ ", t.highlight), Kind::File => ("• ", t.text), Kind::Missing => ("✕ ", t.error), Kind::Exit => ("⏻ ", t.error), Kind::Tool(_) => ("★ ", t.ok) };
            let num = if (1..=9).contains(&it.slot) { format!("{}", it.slot) } else { " ".into() };
            let mut spans = vec![Span::styled(if sel { "►" } else { " " }, bg.fg(t.accent)), Span::styled(num, bg.fg(t.dim)), Span::styled(" ", bg), Span::styled(icon, bg.fg(col))];
            for (ci, ch) in it.name.chars().enumerate() {
                let mut st = bg.fg(if it.kind == Kind::Missing { t.dim } else { t.text });
                if hits.contains(&ci) { st = st.fg(t.highlight).add_modifier(Modifier::BOLD); } else if sel { st = st.add_modifier(Modifier::BOLD); }
                spans.push(Span::styled(ch.to_string(), st));
            }
            let used = 5 + it.name.width();
            let room = (inner.width as usize).saturating_sub(used + 3);
            let mut p = it.path.parent().map(|p| p.display().to_string()).unwrap_or_default();
            if it.kind == Kind::Missing { p = format!("missing: {}", it.path.display()); }
            if it.kind == Kind::Exit || matches!(it.kind, Kind::Tool(_)) { p = it.path.display().to_string(); }
            let p = if p.width() > room { format!("…{}", p.chars().rev().take(room.saturating_sub(1)).collect::<String>().chars().rev().collect::<String>()) } else { p };
            let pad = (inner.width as usize).saturating_sub(used + p.width() + 1);
            spans.push(Span::styled(format!("{}{p} ", " ".repeat(pad)), bg.fg(t.dim)));
            lines.push(Line::from(spans));
        }
        f.render_widget(Paragraph::new(lines), Rect { x: inner.x, y: inner.y + 2, width: inner.width, height: list_h as u16 });
        let foot = Rect { x: inner.x, y: inner.y + inner.height.saturating_sub(1), width: inner.width, height: 1 };
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled(" Enter", Style::default().fg(t.accent)), Span::styled(" open   ", dim),
            Span::styled("→", Style::default().fg(t.accent)), Span::styled(" go to   ", dim),
            Span::styled("Del", Style::default().fg(t.accent)), Span::styled(" remove   ", dim),
            Span::styled("↑↓", Style::default().fg(t.accent)), Span::styled(" choose   ", dim),
            Span::styled("Esc", Style::default().fg(t.accent)), Span::styled(" / Tab close   ", dim),
            Span::styled("exit", Style::default().fg(t.accent)), Span::styled(" quits", dim),
        ])), foot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fuzzy() {
        assert!(score("notepad++", "", "ntp").is_some());
        assert!(score("notepad++", "", "xyz").is_none());
        let a = score("Visual Studio Code", "", "vsc").unwrap().0;
        let b = score("devices", "", "vsc").map(|x| x.0).unwrap_or(-1);
        assert!(a > b);
    }
}
