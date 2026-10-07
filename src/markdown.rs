//! Markdown rendered for the preview: headings, emphasis, lists, quotes, tables, links,
//! and code blocks coloured by language. Text is wrapped to the pane width.

use crate::config::Theme;
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

struct R<'t> {
    t: &'t Theme,
    width: usize,
    out: Vec<Line<'static>>,
    cur: Vec<Span<'static>>,
    bold: u32, italic: u32, strike: u32, link: u32, heading: Option<HeadingLevel>,
    quote: usize,
    lists: Vec<Option<u64>>,
    item_marker: Option<String>,
    code: Option<(String, String)>,     // (language, text)
    table: Option<Vec<Vec<String>>>,
    cell: String,
    link_urls: Vec<String>,
}

pub fn render(src: &str, width: usize, t: &Theme) -> Vec<Line<'static>> {
    let mut r = R { t, width: width.max(20), out: Vec::new(), cur: Vec::new(), bold: 0, italic: 0, strike: 0, link: 0, heading: None,
        quote: 0, lists: Vec::new(), item_marker: None, code: None, table: None, cell: String::new(), link_urls: Vec::new() };
    let opts = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    for ev in Parser::new_ext(src, opts) { r.event(ev); }
    r.flush();
    while r.out.last().map(|l| l.width() == 0).unwrap_or(false) { r.out.pop(); }
    r.out
}

impl R<'_> {
    fn style(&self) -> Style {
        let t = self.t;
        let mut s = Style::default().fg(t.text);
        if let Some(h) = self.heading {
            s = s.fg(if h == HeadingLevel::H1 || h == HeadingLevel::H2 { t.accent } else { t.highlight }).add_modifier(Modifier::BOLD);
        }
        if self.bold > 0 { s = s.add_modifier(Modifier::BOLD); }
        if self.italic > 0 { s = s.add_modifier(Modifier::ITALIC); }
        if self.strike > 0 { s = s.add_modifier(Modifier::CROSSED_OUT).fg(t.dim); }
        if self.link > 0 { s = s.fg(t.accent).add_modifier(Modifier::UNDERLINED); }
        if self.quote > 0 && self.heading.is_none() { s = s.add_modifier(Modifier::ITALIC); }
        s
    }

    fn prefix(&self, first: bool) -> Vec<Span<'static>> {
        let dim = Style::default().fg(self.t.dim);
        let mut v = Vec::new();
        for _ in 0..self.quote { v.push(Span::styled("│ ", dim)); }
        let depth = self.lists.len();
        if depth > 0 {
            let indent = "  ".repeat(depth - 1);
            match (&self.item_marker, first) {
                (Some(m), true) => { v.push(Span::raw(indent)); v.push(Span::styled(m.clone(), Style::default().fg(self.t.accent))); }
                (Some(m), false) => v.push(Span::raw(format!("{indent}{}", " ".repeat(m.width())))),
                (None, _) => v.push(Span::raw(format!("{indent}  "))),
            }
        }
        v
    }

    fn blank(&mut self) {
        if self.out.last().map(|l| l.width() > 0).unwrap_or(false) {
            let p = if self.quote > 0 { self.prefix(false) } else { Vec::new() };
            self.out.push(Line::from(p));
        }
    }

    /// Wrap the collected spans into lines.
    fn flush(&mut self) {
        if self.cur.is_empty() { return; }
        let spans = std::mem::take(&mut self.cur);
        let first_prefix = self.prefix(true);
        let rest_prefix = self.prefix(false);
        let pw = |p: &Vec<Span>| p.iter().map(|s| s.content.width()).sum::<usize>();
        let mut line: Vec<Span<'static>> = first_prefix.clone();
        let mut used = pw(&first_prefix);
        let mut avail = self.width;
        let mut started = false;
        for sp in spans {
            let st = sp.style;
            let text = sp.content.to_string();
            // split keeping spaces attached to the following word
            let mut words: Vec<String> = Vec::new();
            let mut w = String::new();
            for ch in text.chars() {
                if ch == '\n' { words.push(std::mem::take(&mut w)); words.push("\n".into()); continue; }
                if ch == ' ' && !w.is_empty() && !w.ends_with(' ') { words.push(std::mem::take(&mut w)); }
                w.push(ch);
            }
            if !w.is_empty() { words.push(w); }
            for word in words {
                if word == "\n" {
                    self.out.push(Line::from(std::mem::take(&mut line)));
                    line = rest_prefix.clone(); used = pw(&rest_prefix); started = false;
                    continue;
                }
                let ww = word.width();
                if started && used + ww > avail {
                    self.out.push(Line::from(std::mem::take(&mut line)));
                    line = rest_prefix.clone(); used = pw(&rest_prefix);
                    let trimmed = word.trim_start().to_string();
                    let tw = trimmed.width();
                    line.push(Span::styled(trimmed, st)); used += tw;
                    avail = self.width;
                    continue;
                }
                let word = if !started { word.trim_start().to_string() } else { word };
                used += word.width();
                line.push(Span::styled(word, st));
                started = true;
            }
        }
        if started || line.len() > first_prefix.len() { self.out.push(Line::from(line)); }
        self.item_marker = None;
    }

    fn event(&mut self, ev: Event) {
        let t = self.t;
        if let Some(rows) = &mut self.table {
            match &ev {
                Event::Text(s) | Event::Code(s) => { self.cell.push_str(s); return; }
                Event::End(TagEnd::TableCell) => { let c = std::mem::take(&mut self.cell); if let Some(r) = rows.last_mut() { r.push(c.trim().to_string()); } return; }
                Event::Start(Tag::TableHead) | Event::Start(Tag::TableRow) => { rows.push(Vec::new()); return; }
                Event::End(TagEnd::Table) => {}
                _ => return,
            }
        }
        if let Some((_, code)) = &mut self.code {
            match ev {
                Event::Text(s) => { code.push_str(&s); return; }
                Event::End(TagEnd::CodeBlock) => {
                    let (lang, code) = self.code.take().unwrap();
                    let lines: Vec<&str> = code.trim_end_matches('\n').lines().collect();
                    let fake = std::path::PathBuf::from(format!("x.{}", lang_ext(&lang)));
                    let hl = crate::highlight::highlight(&fake, &lines, t);
                    let bar = Style::default().fg(t.dim);
                    for l in hl {
                        let mut spans = self.prefix(false);
                        spans.push(Span::styled("▏ ", bar));
                        spans.extend(l.spans);
                        self.out.push(Line::from(spans));
                    }
                    self.blank();
                    return;
                }
                _ => return,
            }
        }
        match ev {
            Event::Start(tag) => match tag {
                Tag::Paragraph => { self.flush(); }
                Tag::Heading { level, .. } => {
                    self.flush(); self.blank();
                    self.heading = Some(level);
                    let marks = match level { HeadingLevel::H1 => "", HeadingLevel::H2 => "", _ => "› " };
                    if !marks.is_empty() { self.cur.push(Span::styled(marks, Style::default().fg(t.dim))); }
                }
                Tag::BlockQuote(_) => { self.flush(); self.quote += 1; }
                Tag::CodeBlock(kind) => {
                    self.flush();
                    let lang = match kind { CodeBlockKind::Fenced(l) => l.split([',', ' ']).next().unwrap_or("").to_string(), _ => String::new() };
                    self.code = Some((lang, String::new()));
                }
                Tag::List(start) => { self.flush(); if self.lists.is_empty() { self.blank(); } self.lists.push(start); }
                Tag::Item => {
                    self.flush();
                    let m = match self.lists.last_mut() {
                        Some(Some(n)) => { let s = format!("{n}. "); *n += 1; s }
                        _ => ["• ", "◦ ", "▪ "][(self.lists.len().saturating_sub(1)) % 3].to_string(),
                    };
                    self.item_marker = Some(m);
                }
                Tag::Emphasis => self.italic += 1,
                Tag::Strong => self.bold += 1,
                Tag::Strikethrough => self.strike += 1,
                Tag::Link { dest_url, .. } => { self.link += 1; self.link_urls.push(dest_url.to_string()); }
                Tag::Image { dest_url, .. } => { self.cur.push(Span::styled(format!("[image: {dest_url}] "), Style::default().fg(t.dim))); self.link += 1; self.link_urls.push(String::new()); }
                Tag::Table(_) => { self.flush(); self.blank(); self.table = Some(Vec::new()); }
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => { self.flush(); if self.lists.is_empty() { self.blank(); } }
                TagEnd::Heading(level) => {
                    self.flush();
                    if level == HeadingLevel::H1 || level == HeadingLevel::H2 {
                        let ch = if level == HeadingLevel::H1 { "═" } else { "─" };
                        let w = self.out.last().map(|l| l.width()).unwrap_or(10).min(self.width);
                        self.out.push(Line::styled(ch.repeat(w), Style::default().fg(t.dim)));
                    }
                    self.heading = None;
                    self.blank();
                }
                TagEnd::BlockQuote(_) => { self.flush(); self.quote = self.quote.saturating_sub(1); self.blank(); }
                TagEnd::List(_) => { self.flush(); self.lists.pop(); if self.lists.is_empty() { self.blank(); } }
                TagEnd::Item => self.flush(),
                TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
                TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
                TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
                TagEnd::Link | TagEnd::Image => {
                    self.link = self.link.saturating_sub(1);
                    if let Some(u) = self.link_urls.pop() {
                        if !u.is_empty() && !u.starts_with('#') && !self.cur.iter().any(|s| s.content.contains(&u)) {
                            self.cur.push(Span::styled(format!(" ({u})"), Style::default().fg(t.dim)));
                        }
                    }
                }
                TagEnd::Table => {
                    let rows = self.table.take().unwrap_or_default();
                    self.table_out(rows);
                    self.blank();
                }
                _ => {}
            },
            Event::Text(s) => { let st = self.style(); self.cur.push(Span::styled(s.to_string(), st)); }
            Event::Code(s) => self.cur.push(Span::styled(format!("`{s}`"), Style::default().fg(t.highlight))),
            Event::SoftBreak => { let st = self.style(); self.cur.push(Span::styled(" ", st)); }
            Event::HardBreak => self.cur.push(Span::raw("\n")),
            Event::Rule => { self.flush(); self.out.push(Line::styled("─".repeat(self.width.min(60)), Style::default().fg(t.dim))); self.blank(); }
            Event::TaskListMarker(done) => self.cur.push(Span::styled(if done { "☑ " } else { "☐ " }, Style::default().fg(if done { t.ok } else { t.dim }))),
            Event::Html(s) | Event::InlineHtml(s) => {
                let s = s.trim();
                if !s.is_empty() && !s.starts_with("<!--") { self.cur.push(Span::styled(s.to_string(), Style::default().fg(t.dim))); }
            }
            Event::FootnoteReference(s) => self.cur.push(Span::styled(format!("[{s}]"), Style::default().fg(t.dim))),
            _ => {}
        }
    }

    fn table_out(&mut self, rows: Vec<Vec<String>>) {
        let t = self.t;
        let n = rows.iter().map(|r| r.len()).max().unwrap_or(0);
        if n == 0 { return; }
        let mut w = vec![0usize; n];
        for r in &rows { for (i, c) in r.iter().enumerate() { w[i] = w[i].max(c.width()).min(40); } }
        let dim = Style::default().fg(t.dim);
        for (ri, r) in rows.iter().enumerate() {
            let mut spans = self.prefix(false);
            for i in 0..n {
                let c = r.get(i).map(|s| s.as_str()).unwrap_or("");
                let c: String = if c.width() > w[i] { c.chars().take(w[i].saturating_sub(1)).collect::<String>() + "…" } else { c.to_string() };
                let pad = w[i].saturating_sub(c.width());
                if i > 0 { spans.push(Span::styled(" │ ", dim)); }
                let st = if ri == 0 { Style::default().fg(t.accent).add_modifier(Modifier::BOLD) } else { Style::default().fg(t.text) };
                spans.push(Span::styled(format!("{c}{}", " ".repeat(pad)), st));
            }
            self.out.push(Line::from(spans));
            if ri == 0 {
                let rule: Vec<String> = w.iter().map(|x| "─".repeat(*x)).collect();
                let mut spans = self.prefix(false);
                spans.push(Span::styled(rule.join("─┼─"), dim));
                self.out.push(Line::from(spans));
            }
        }
    }
}

fn lang_ext(l: &str) -> &str {
    match l.to_lowercase().as_str() {
        "powershell" | "pwsh" | "ps" | "ps1" => "ps1",
        "python" | "py" => "py",
        "bash" | "sh" | "shell" | "zsh" | "console" => "sh",
        "javascript" | "js" | "jsx" | "ts" | "typescript" | "tsx" => "js",
        "rust" | "rs" => "rs",
        "csharp" | "cs" | "c#" => "cs",
        "cpp" | "c++" | "cc" => "cpp",
        "c" | "h" => "c",
        "batch" | "bat" | "cmd" | "dos" => "bat",
        "json" | "jsonc" => "json",
        "yaml" | "yml" => "yaml",
        "html" | "xml" | "svg" => "html",
        "ini" | "toml" | "cfg" => "ini",
        "sql" => "sql",
        "go" | "golang" => "go",
        "java" | "kotlin" => "java",
        "php" => "php",
        "ruby" | "rb" => "rb",
        "lua" => "lua",
        "css" => "css",
        "md" | "markdown" => "md",
        "diff" | "patch" => "diff",
        _ => "txt",
    }
}
