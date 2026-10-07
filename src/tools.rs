//! Tools window (Esc, F10 / ≡ Tools button): every extra view in one list, each with a check box.
//! Space turns a tool on or off (saved in [features] in config.ini), Enter opens it.
//! A tool that is off has no keys and is not shown in footers.

use crate::config::{Config, Theme};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool { Tasks, Chat, Calendar, Heatmap, Typing, Themes, Gallery, Git, Diff }

pub struct Feature { pub tool: Tool, pub key: &'static str, pub name: &'static str, pub keys: &'static str, pub what: &'static str }

pub const ALL: &[Feature] = &[
    Feature { tool: Tool::Tasks, key: "tasks", name: "Tasks & clock", keys: "Alt+T  F8", what: "to-do list, daily tasks, reminders, watch, work-hours countdown, work log" },
    Feature { tool: Tool::Chat, key: "ai", name: "Local AI", keys: "Alt+A", what: "chat with a model on your PC (Ollama, LM Studio…) that knows your tasks and can add them" },
    Feature { tool: Tool::Calendar, key: "calendar", name: "Calendar", keys: "Alt+C  K", what: "month view: holidays, vacations, events and the tasks due each day" },
    Feature { tool: Tool::Heatmap, key: "heatmap", name: "Activity heatmap", keys: "M in Tasks", what: "a year of your work log as coloured squares, streaks and stats" },
    Feature { tool: Tool::Typing, key: "typing", name: "Typing test", keys: "Alt+Y", what: "monkeytype-style speed test: words or time, WPM, accuracy, history" },
    Feature { tool: Tool::Themes, key: "themes", name: "Themes", keys: "Alt+P", what: "pick a colour theme with a live preview: the built-in ones and all of monkeytype's" },
    Feature { tool: Tool::Gallery, key: "gallery", name: "Image gallery", keys: "Alt+I", what: "the pictures of a folder as a grid of thumbnails" },
    Feature { tool: Tool::Git, key: "git", name: "Git panel", keys: "Ctrl+G", what: "changes, diff, stage, commit, history, branches, pull / push" },
    Feature { tool: Tool::Diff, key: "diff", name: "Folder compare", keys: "Alt+D", what: "compare two folders (or two files): new, missing and changed files, line diff" },
];

/// The tools that are on, in list order (what Spotlight shows).
pub fn on_list(cfg: &Config) -> Vec<Tool> { ALL.iter().map(|f| f.tool).filter(|&t| enabled(cfg, t)).collect() }

pub fn feature(tool: Tool) -> &'static Feature { ALL.iter().find(|f| f.tool == tool).unwrap() }

/// On unless config.ini says `[features] name = off`.
pub fn enabled(cfg: &Config, tool: Tool) -> bool {
    if tool == Tool::Chat && !crate::chat::available(cfg) { return false; }
    cfg.ini.get("features", feature(tool).key).map(|v| !matches!(v.to_lowercase().as_str(), "off" | "no" | "0" | "false")).unwrap_or(true)
}

pub fn set_enabled(cfg: &mut Config, tool: Tool, on: bool) -> Result<(), String> {
    cfg.set_value("features", feature(tool).key, Some(if on { "on" } else { "off" })).map_err(|e| e.to_string())?;
    cfg.reload();
    Ok(())
}

pub enum Act { None, Close, Open(Tool), Changed(String) }

pub struct Window { pub sel: usize }

impl Window {
    pub fn new() -> Window { Window { sel: 0 } }

    pub fn on_key(&mut self, k: KeyEvent, cfg: &mut Config) -> Act {
        match k.code {
            // Tab opens the window and Tab again closes it (Ctrl+Tab stays the theme key)
            KeyCode::Tab if !k.modifiers.contains(ratatui::crossterm::event::KeyModifiers::CONTROL) => Act::Close,
            KeyCode::Esc | KeyCode::F(10) | KeyCode::Char('q') | KeyCode::Char('Q') => Act::Close,
            KeyCode::Up => { self.sel = self.sel.checked_sub(1).unwrap_or(ALL.len() - 1); Act::None }
            KeyCode::Down => { self.sel = (self.sel + 1) % ALL.len(); Act::None }
            KeyCode::Home => { self.sel = 0; Act::None }
            KeyCode::End => { self.sel = ALL.len() - 1; Act::None }
            KeyCode::Char(c @ '1'..='9') => { let i = (c as u8 - b'1') as usize; if i < ALL.len() { self.sel = i; } Act::None }
            KeyCode::Char(' ') => {
                let f = &ALL[self.sel];
                if f.tool == Tool::Chat && !crate::chat::available(cfg) { return Act::Changed(crate::chat::NOT_INSTALLED.into()); }
                let on = !enabled(cfg, f.tool);
                match set_enabled(cfg, f.tool, on) {
                    Ok(()) => Act::Changed(format!("{}: {}", f.name, if on { "on" } else { "off" })),
                    Err(e) => Act::Changed(format!("Could not save config.ini: {e}")),
                }
            }
            KeyCode::Enter | KeyCode::Right => {
                let f = &ALL[self.sel];
                if f.tool == Tool::Chat && !crate::chat::available(cfg) { Act::Changed(crate::chat::NOT_INSTALLED.into()) }
                else if enabled(cfg, f.tool) { Act::Open(f.tool) } else { Act::Changed(format!("{} is off - Space turns it on", f.name)) }
            }
            _ => Act::None,
        }
    }

    pub fn draw(&self, f: &mut Frame, t: &Theme, cfg: &Config) {
        let area = f.area();
        let w = area.width.saturating_sub(4).min(92).max(40);
        let h = (ALL.len() as u16 * 2 + 5).min(area.height.saturating_sub(2));
        let r = Rect { x: area.x + (area.width.saturating_sub(w)) / 2, y: area.y + (area.height.saturating_sub(h)) / 3, width: w, height: h };
        f.render_widget(Clear, r);
        let base = match t.background { Some(bg) => Style::default().bg(bg).fg(t.text), None => Style::default().fg(t.text) };
        let block = Block::default().borders(Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(t.accent))
            .title(Span::styled(" ≡ Tools ", Style::default().fg(t.accent).add_modifier(Modifier::BOLD))).style(base);
        let inner = block.inner(r);
        f.render_widget(block, r);
        let dim = Style::default().fg(t.dim);
        let mut lines = vec![Line::raw("")];
        for (i, ft) in ALL.iter().enumerate() {
            let sel = i == self.sel;
            let on = enabled(cfg, ft.tool);
            let missing = ft.tool == Tool::Chat && !crate::chat::available(cfg);
            let bg = if sel { Style::default().bg(t.select_bg) } else { Style::default() };
            let name_w = 20usize;
            let mut spans = vec![
                Span::styled(if sel { " ► " } else { "   " }, bg.fg(t.accent)),
                Span::styled(if missing { "[–] " } else if on { "[✓] " } else { "[ ] " }, bg.fg(if on { t.ok } else { t.dim }).add_modifier(Modifier::BOLD)),
                Span::styled(format!("{:<name_w$}", ft.name), bg.fg(if on { t.text } else { t.dim }).add_modifier(if sel { Modifier::BOLD } else { Modifier::empty() })),
                Span::styled(format!("{:<12}", ft.keys), bg.fg(if on { t.accent } else { t.dim })),
            ];
            let used: usize = spans.iter().map(|s| s.content.width()).sum();
            spans.push(Span::styled(" ".repeat((inner.width as usize).saturating_sub(used)), bg));
            lines.push(Line::from(spans));
            let what = crate::ui::fit(if missing { "needs Ollama (ollama.com) - not installed on this PC" } else { ft.what }, (inner.width as usize).saturating_sub(8));
            lines.push(Line::from(Span::styled(format!("        {what}"), dim)));
        }
        f.render_widget(Paragraph::new(lines), Rect { height: inner.height.saturating_sub(1), ..inner });
        let foot = crate::ui::hint_line(&[("↑↓", "choose"), ("Enter", "open"), ("Space", "on / off"), ("Esc", "close")], inner.width, t);
        f.render_widget(Paragraph::new(foot), Rect { y: inner.y + inner.height.saturating_sub(1), height: 1, ..inner });
    }
}
