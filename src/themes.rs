//! Theme picker (Alt+P, or "Themes" in Spotlight): every built-in preset, every monkeytype theme and your own
//! [preset.name] sections. The whole screen previews the theme under the cursor; Enter keeps it, Esc goes back.

use crate::config::{Config, Theme};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;
use std::time::Duration;

struct Picker {
    names: Vec<String>,
    info: Vec<(&'static str, bool)>,              // kind, light background
    shown: Vec<usize>,
    sel: usize,
    top: usize,
    filter: String,
    saved: String,
}

fn kind(name: &str) -> &'static str {
    if crate::config::is_mt(name) { "monkeytype" }
    else if crate::config::all_preset_names().iter().any(|n| n == name) { "built-in" }
    else { "yours" }
}

impl Picker {
    fn new(cfg: &Config) -> Picker {
        let names = cfg.preset_names();
        let saved = cfg.theme.name.clone();
        let info = names.iter().map(|n| {
            let (r, g, b) = cfg.theme_for(n).bg_rgb;
            (kind(n), (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000 > 140)
        }).collect();
        let mut p = Picker { names, info, shown: Vec::new(), sel: 0, top: 0, filter: String::new(), saved };
        p.apply_filter();
        if let Some(i) = p.shown.iter().position(|&i| p.names[i] == p.saved) { p.sel = i; }
        p
    }

    fn apply_filter(&mut self) {
        let q = self.filter.to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        self.shown = (0..self.names.len()).filter(|&i| {
            let (k, light) = self.info[i];
            let hay = format!("{} {k} {}", self.names[i], if light { "light" } else { "dark" });
            words.iter().all(|w| hay.contains(w))
        }).collect();
        self.sel = 0;
        self.top = 0;
    }

    fn current(&self) -> Option<&str> { self.shown.get(self.sel).map(|&i| self.names[i].as_str()) }

    fn draw(&mut self, f: &mut Frame, t: &Theme) {
        let area = f.area();
        let base = match t.background { Some(bg) => Style::default().bg(bg).fg(t.text), None => Style::default().fg(t.text) };
        f.render_widget(Block::default().style(base), area);
        let dim = Style::default().fg(t.dim);
        let acc = Style::default().fg(t.accent).add_modifier(Modifier::BOLD);
        // header
        let cur = self.current().unwrap_or("").to_string();
        f.render_widget(Paragraph::new(vec![
            Line::from(vec![Span::styled("▌ ", acc), Span::styled("THEMES", Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
                Span::styled(format!("   {} of {}", self.shown.len(), self.names.len()), dim)]),
            Line::from(vec![Span::styled("▌ ", acc), Span::styled("filter › ", dim),
                if self.filter.is_empty() { Span::styled("type a name, or monkeytype / built-in / light…", dim) } else { Span::styled(self.filter.clone(), Style::default().fg(t.highlight).add_modifier(Modifier::BOLD)) }]),
        ]), Rect { x: area.x + 1, y: area.y + 1, width: area.width.saturating_sub(2), height: 2 });
        // list
        let list_w = 34u16.min(area.width / 2);
        let list = Rect { x: area.x + 1, y: area.y + 4, width: list_w, height: area.height.saturating_sub(6) };
        let h = list.height as usize;
        if self.sel < self.top { self.top = self.sel; }
        if self.sel >= self.top + h { self.top = self.sel + 1 - h; }
        let mut lines = Vec::new();
        for (k, &i) in self.shown.iter().enumerate().skip(self.top).take(h) {
            let n = &self.names[i];
            let sel = k == self.sel;
            let bg = if sel { Style::default().bg(t.select_bg) } else { Style::default() };
            let mark = if *n == self.saved { "● " } else { "  " };
            let tag = match self.info[i].0 { "monkeytype" => "mt", "built-in" => "", _ => "yours" };
            let room = (list_w as usize).saturating_sub(4 + tag.len());
            lines.push(Line::from(vec![
                Span::styled(if sel { "►" } else { " " }, bg.fg(t.accent)),
                Span::styled(mark, bg.fg(t.ok)),
                Span::styled(crate::ui::fit(n, room), bg.fg(t.text).add_modifier(if sel { Modifier::BOLD } else { Modifier::empty() })),
                Span::styled(format!("{tag} "), bg.fg(t.dim)),
            ]));
        }
        if self.shown.is_empty() { lines.push(Line::styled("  no theme matches", dim)); }
        f.render_widget(Paragraph::new(lines), list);
        // preview
        let pv = Rect { x: list.x + list_w + 2, y: list.y, width: area.width.saturating_sub(list_w + 4), height: list.height };
        if pv.width > 20 { draw_preview(f, pv, t, &cur); }
        let foot = crate::ui::hint_line(&[("type", "filter"), ("↑↓ PgUp PgDn", "choose"), ("Enter", "use this theme"), ("Ctrl+R", "random"), ("Esc", "back")], area.width, t);
        f.render_widget(Paragraph::new(foot), Rect { x: area.x, y: area.y + area.height.saturating_sub(1), width: area.width, height: 1 });
    }
}

fn draw_preview(f: &mut Frame, r: Rect, t: &Theme, name: &str) {
    let dim = Style::default().fg(t.dim);
    let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(t.accent))
        .title(Span::styled(format!(" {name} · {} ", kind(name)), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)));
    let inner = block.inner(r);
    f.render_widget(block, r);
    let syn = |i: usize| { let (a, b, c) = t.syn[i]; Style::default().fg(Color::Rgb(a, b, c)) };
    let sel = Style::default().bg(t.select_bg).fg(t.text).add_modifier(Modifier::BOLD);
    let w = inner.width as usize;
    let pad = |s: &str| crate::ui::fit(s, w.saturating_sub(1));
    let mut l = vec![
        Line::from(vec![Span::styled(" FILE BROWSER", Style::default().fg(t.text).add_modifier(Modifier::BOLD)), Span::styled("   C:\\Users\\you\\Desktop", dim)]),
        Line::raw(""),
        Line::from(vec![Span::styled(" ▸ ", Style::default().fg(t.accent)), Span::styled("projects\\", Style::default().fg(t.accent))]),
        Line::from(vec![Span::styled("►", Style::default().fg(t.accent).bg(t.select_bg)), Span::styled(pad(" notes.md"), sel)]),
        Line::from(vec![Span::styled("   report.pdf", Style::default().fg(t.text)), Span::styled("      2.4 MB", dim)]),
        Line::from(vec![Span::styled("   ✔ Saved", Style::default().fg(t.ok)), Span::styled("   ·   ", dim), Span::styled("✕ Access denied", Style::default().fg(t.error))]),
        Line::raw(""),
        Line::from(vec![Span::styled(" fn ", syn(0)), Span::styled("main", syn(4)), Span::raw("() { "), Span::styled("let ", syn(0)), Span::styled("n", syn(4)),
            Span::raw(": "), Span::styled("u32", syn(5)), Span::raw(" = "), Span::styled("42", syn(3)), Span::raw("; }")]),
        Line::from(vec![Span::styled(" // a comment", syn(2)), Span::raw("   "), Span::styled("\"a string\"", syn(1)), Span::raw("   "), Span::styled("#[attr]", syn(6))]),
        Line::raw(""),
    ];
    // a typing-test line: typed words, an error, the caret and the words to come
    l.push(Line::from(vec![
        Span::styled(" the quick ", Style::default().fg(t.text)),
        Span::styled("brwn", Style::default().fg(t.error).add_modifier(Modifier::UNDERLINED)),
        Span::styled(" fo", Style::default().fg(t.text)),
        Span::styled("x", Style::default().fg(t.dim).add_modifier(Modifier::UNDERLINED)),
        Span::styled("▏", Style::default().fg(t.highlight)),
        Span::styled("jumps over the lazy dog", dim),
    ]));
    l.push(Line::from(vec![Span::styled(" 87", Style::default().fg(t.accent).add_modifier(Modifier::BOLD)), Span::styled(" wpm   ", dim),
        Span::styled("96%", Style::default().fg(t.accent).add_modifier(Modifier::BOLD)), Span::styled(" acc", dim)]));
    l.push(Line::raw(""));
    // colour swatches
    let sw: Vec<(&str, Color)> = vec![("accent", t.accent), ("text", t.text), ("dim", t.dim), ("ok", t.ok), ("error", t.error), ("select", t.select_bg), ("highlight", t.highlight)];
    let mut spans = vec![Span::raw(" ")];
    for (n, c) in &sw { spans.push(Span::styled("██", Style::default().fg(*c))); spans.push(Span::styled(format!(" {n}  "), dim)); }
    l.push(Line::from(spans));
    f.render_widget(Paragraph::new(l).wrap(ratatui::widgets::Wrap { trim: false }), inner);
}

pub fn run(terminal: &mut ratatui::DefaultTerminal) -> std::io::Result<()> {
    let mut cfg = Config::load();
    let mut p = Picker::new(&cfg);
    let mut seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7);
    let _ = terminal.clear();
    loop {
        let t = match p.current() { Some(n) => cfg.theme_for(n), None => cfg.theme.clone() };
        terminal.draw(|f| p.draw(f, &t))?;
        if !event::poll(Duration::from_millis(500))? { continue; }
        let Event::Key(k) = event::read()? else { continue };
        if k.kind == KeyEventKind::Release { continue; }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT);
        let n = p.shown.len();
        match k.code {
            KeyCode::Esc => { if p.filter.is_empty() { break; } p.filter.clear(); p.apply_filter(); }
            KeyCode::Enter => {
                if let Some(name) = p.current().map(|s| s.to_string()) {
                    let _ = cfg.set_value("theme", "preset", Some(&name));
                    cfg.reload();
                    break;
                }
            }
            KeyCode::Up => { if n > 0 { p.sel = (p.sel + n - 1) % n; } }
            KeyCode::Down => { if n > 0 { p.sel = (p.sel + 1) % n; } }
            KeyCode::PageUp => p.sel = p.sel.saturating_sub(10),
            KeyCode::PageDown => p.sel = (p.sel + 10).min(n.saturating_sub(1)),
            KeyCode::Home => p.sel = 0,
            KeyCode::End => p.sel = n.saturating_sub(1),
            KeyCode::Char('r') | KeyCode::Char('R') if ctrl => {
                seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17;
                if n > 0 { p.sel = (seed % n as u64) as usize; }
            }
            KeyCode::Char('u') | KeyCode::Char('U') if ctrl => { p.filter.clear(); p.apply_filter(); }
            KeyCode::Backspace => { if p.filter.pop().is_some() { p.apply_filter(); } }
            KeyCode::Char(c) if !ctrl => { p.filter.push(c); p.apply_filter(); }
            _ => {}
        }
    }
    let _ = terminal.clear();
    Ok(())
}
