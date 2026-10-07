//! Git panel (Ctrl+G in the file browser): status of the repository the folder is in, a coloured diff
//! of the selected file, stage / unstage, commit, history, branches, pull / push, and "git init" for
//! a folder that is not a repository yet. Uses the git command line.

use crate::config::Theme;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;
use unicode_width::UnicodeWidthStr;

/// Run git in `dir`. Ok(stdout) or Err(message).
pub fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut c = std::process::Command::new("git");
    c.arg("-c").arg("core.quotepath=off").arg("-c").arg("color.ui=never").args(args).current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0").env("GIT_PAGER", "").stdin(std::process::Stdio::null());
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; c.creation_flags(0x08000000); }
    let out = c.output().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "git is not installed (or not on PATH)  ·  https://git-scm.com".to_string() } else { e.to_string() })?;
    let so = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() { Ok(so) } else {
        let se = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if se.is_empty() { so.trim().to_string() } else { se })
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Area { Staged, Changed, Untracked, Conflict }

#[derive(Clone, Debug)]
pub struct Entry { pub area: Area, pub code: char, pub path: String }

#[derive(Default, Debug)]
pub struct Status { pub branch: String, pub upstream: String, pub ahead: u32, pub behind: u32, pub entries: Vec<Entry> }

/// `git status --porcelain=v1 -b`
pub fn parse_status(text: &str) -> Status {
    let mut st = Status::default();
    for l in text.lines() {
        if let Some(b) = l.strip_prefix("## ") {
            let (head, track) = b.split_once(" [").map(|(a, b)| (a, b.trim_end_matches(']'))).unwrap_or((b, ""));
            let (br, up) = head.split_once("...").unwrap_or((head, ""));
            st.branch = br.trim_start_matches("No commits yet on ").to_string();
            st.upstream = up.to_string();
            for part in track.split(", ") {
                if let Some(n) = part.strip_prefix("ahead ") { st.ahead = n.parse().unwrap_or(0); }
                if let Some(n) = part.strip_prefix("behind ") { st.behind = n.parse().unwrap_or(0); }
            }
            continue;
        }
        if l.len() < 4 { continue; }
        let (x, y) = (l.as_bytes()[0] as char, l.as_bytes()[1] as char);
        let mut path = l[3..].to_string();
        if let Some((_, new)) = path.split_once(" -> ") { path = new.to_string(); }
        let path = path.trim_matches('"').to_string();
        if x == '?' { st.entries.push(Entry { area: Area::Untracked, code: '?', path }); continue; }
        if x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D') { st.entries.push(Entry { area: Area::Conflict, code: 'U', path }); continue; }
        if x != ' ' { st.entries.push(Entry { area: Area::Staged, code: x, path: path.clone() }); }
        if y != ' ' { st.entries.push(Entry { area: Area::Changed, code: y, path }); }
    }
    // grouped: conflicts, staged, changes, untracked
    st.entries.sort_by_key(|e| match e.area { Area::Conflict => 0, Area::Staged => 1, Area::Changed => 2, Area::Untracked => 3 });
    st
}

enum Mode { Status, Log, Branches }

enum Ask { Commit, Name, Email(String), Confirm(String, Vec<String>) }

pub struct Panel {
    dir: PathBuf,
    root: Option<PathBuf>,
    status: Status,
    mode: Mode,
    sel: usize,
    log: Vec<String>,
    branches: Vec<String>,
    preview: Vec<String>,
    pv_scroll: usize,
    msg: String,
    msg_err: bool,
    ask: Option<(Ask, String)>,
    job: Option<(String, Receiver<Result<String, String>>)>,
}

impl Panel {
    pub fn new(dir: &Path) -> Panel {
        let mut p = Panel { dir: dir.to_path_buf(), root: None, status: Status::default(), mode: Mode::Status, sel: 0, log: Vec::new(), branches: Vec::new(),
            preview: Vec::new(), pv_scroll: 0, msg: String::new(), msg_err: false, ask: None, job: None };
        p.refresh();
        p
    }

    fn say(&mut self, m: &str, err: bool) { self.msg = m.lines().last().unwrap_or("").chars().take(300).collect(); self.msg_err = err; }

    fn refresh(&mut self) {
        match git(&self.dir, &["rev-parse", "--show-toplevel"]) {
            Ok(r) => self.root = Some(PathBuf::from(r.trim())),
            Err(e) => { self.root = None; if e.contains("not installed") { self.say(&e, true); } return; }
        }
        let root = self.root.clone().unwrap();
        match git(&root, &["status", "--porcelain=v1", "-b"]) {
            Ok(s) => self.status = parse_status(&s),
            Err(e) => self.say(&e, true),
        }
        match self.mode {
            Mode::Log => { self.log = git(&root, &["log", "--graph", "--oneline", "--decorate", "--date=short", "--format=%h %ad %s%d  · %an", "-n", "300"]).unwrap_or_default().lines().map(|s| s.to_string()).collect(); }
            Mode::Branches => { self.branches = git(&root, &["branch", "-a", "--format=%(HEAD) %(refname:short)  %(committerdate:relative)  %(subject)"]).unwrap_or_default().lines().map(|s| s.to_string()).collect(); }
            Mode::Status => {}
        }
        let n = self.len();
        if self.sel >= n { self.sel = n.saturating_sub(1); }
        self.load_preview();
    }

    fn len(&self) -> usize { match self.mode { Mode::Status => self.status.entries.len(), Mode::Log => self.log.len(), Mode::Branches => self.branches.len() } }

    fn load_preview(&mut self) {
        self.pv_scroll = 0;
        let Some(root) = self.root.clone() else { self.preview.clear(); return };
        self.preview = match self.mode {
            Mode::Status => match self.status.entries.get(self.sel) {
                None => vec!["Nothing to commit, working tree clean.".into()],
                Some(e) => match e.area {
                    Area::Staged => git(&root, &["diff", "--cached", "--", &e.path]).unwrap_or_else(|x| x),
                    Area::Changed | Area::Conflict => git(&root, &["diff", "--", &e.path]).unwrap_or_else(|x| x),
                    Area::Untracked => {
                        let p = root.join(&e.path);
                        if p.is_dir() { format!("new folder: {}", e.path) }
                        else { match std::fs::read(&p) { Ok(b) if !crate::text::looks_binary(&b) => crate::text::decode(&b).0.lines().take(400).map(|l| format!("+{l}")).collect::<Vec<_>>().join("\n"), Ok(b) => format!("new binary file  ·  {}", crate::text::format_size(b.len() as u64)), Err(x) => x.to_string() } }
                    }
                }.lines().map(|s| s.to_string()).collect(),
            },
            Mode::Log => {
                let hash = self.log.get(self.sel).and_then(|l| l.split_whitespace().find(|w| w.len() >= 7 && w.chars().all(|c| c.is_ascii_hexdigit()))).map(|s| s.to_string());
                match hash { Some(h) => git(&root, &["show", "--stat", "--patch", "--format=commit %H%nAuthor: %an <%ae>%nDate:   %ad%n%n    %s%n%n%b", &h]).unwrap_or_else(|x| x).lines().take(3000).map(|s| s.to_string()).collect(), None => Vec::new() }
            }
            Mode::Branches => {
                let name = self.branch_name();
                match name { Some(b) => git(&root, &["log", "--oneline", "--decorate", "-n", "40", &b]).unwrap_or_else(|x| x).lines().map(|s| s.to_string()).collect(), None => Vec::new() }
            }
        };
    }

    fn branch_name(&self) -> Option<String> { self.branches.get(self.sel).and_then(|l| l.get(2..)).and_then(|l| l.split("  ").next()).map(|s| s.trim().to_string()) }

    fn run_bg(&mut self, what: &str, args: Vec<String>) {
        let Some(root) = self.root.clone() else { return };
        let (tx, rx) = channel();
        std::thread::spawn(move || { let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect(); let _ = tx.send(git(&root, &a)); });
        self.job = Some((what.to_string(), rx));
        self.say(&format!("{what}…"), false);
    }

    pub fn poll(&mut self) -> bool {
        let Some((what, rx)) = &self.job else { return false };
        let Ok(r) = rx.try_recv() else { return false };
        let what = what.clone();
        self.job = None;
        match r {
            Ok(out) => { let last = out.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("").to_string(); self.say(&format!("{what}: done  {last}"), false); }
            Err(e) => self.say(&format!("{what} failed: {e}"), true),
        }
        self.refresh();
        true
    }

    fn act(&mut self, args: &[&str], ok: &str) {
        let Some(root) = self.root.clone() else { return };
        match git(&root, args) { Ok(_) => self.say(ok, false), Err(e) => self.say(&e, true) }
        self.refresh();
    }

    fn init_repo(&mut self) {
        match git(&self.dir, &["init"]) {
            Err(e) => { self.say(&e, true); return; }
            Ok(_) => {}
        }
        let gi = self.dir.join(".gitignore");
        if !gi.exists() { let _ = std::fs::write(&gi, "# made by the file browser\ntarget/\nnode_modules/\n__pycache__/\n*.log\n*.tmp\nThumbs.db\ndesktop.ini\n.DS_Store\n"); }
        self.refresh();
        self.say("Repository created (with a .gitignore)  ·  A stages everything, C commits", false);
        if git(&self.dir, &["config", "user.name"]).map(|s| s.trim().is_empty()).unwrap_or(true) { self.ask = Some((Ask::Name, String::new())); }
    }

    /// false = close the panel
    pub fn on_key(&mut self, k: KeyEvent) -> bool {
        if let Some((ask, text)) = self.ask.as_mut() {
            match k.code {
                KeyCode::Esc => { self.ask = None; self.say("Cancelled", false); }
                KeyCode::Enter => {
                    let (ask, text) = self.ask.take().unwrap();
                    let text = text.trim().to_string();
                    match ask {
                        Ask::Commit if !text.is_empty() => self.act(&["commit", "-m", &text], &format!("Committed: {text}")),
                        Ask::Commit => self.say("Commit message is empty - nothing done", true),
                        Ask::Name if !text.is_empty() => { self.ask = Some((Ask::Email(text), String::new())); }
                        Ask::Name => {}
                        Ask::Email(name) => {
                            let _ = git(&self.dir, &["config", "--global", "user.name", &name]);
                            if !text.is_empty() { let _ = git(&self.dir, &["config", "--global", "user.email", &text]); }
                            self.say(&format!("Commits will be signed as {name} <{text}>"), false);
                        }
                        Ask::Confirm(_, _) => {}
                    }
                }
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('s') | KeyCode::Char('S') if matches!(ask, Ask::Confirm(..)) => {
                    if let Some((Ask::Confirm(what, args), _)) = self.ask.take() {
                        if what == "push" || what == "pull" || what == "fetch" { self.run_bg(&what, args); }
                        else { let a: Vec<&str> = args.iter().map(|s| s.as_str()).collect(); self.act(&a, &format!("{what}: done")); }
                    }
                }
                _ if matches!(ask, Ask::Confirm(..)) => { self.ask = None; self.say("Cancelled", false); }
                KeyCode::Backspace => { text.pop(); }
                KeyCode::Char(c) => { text.push(c); }
                _ => {}
            }
            return true;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if self.root.is_none() {
            match k.code {
                KeyCode::Char('i') | KeyCode::Char('I') => self.init_repo(),
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => return false,
                _ => {}
            }
            return true;
        }
        let n = self.len();
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
                if matches!(self.mode, Mode::Status) { return false; }
                self.mode = Mode::Status; self.sel = 0; self.refresh();
            }
            KeyCode::Up if ctrl => self.pv_scroll = self.pv_scroll.saturating_sub(3),
            KeyCode::Down if ctrl => self.pv_scroll += 3,
            KeyCode::Up => { if self.sel > 0 { self.sel -= 1; self.load_preview(); } }
            KeyCode::Down => { if self.sel + 1 < n { self.sel += 1; self.load_preview(); } }
            KeyCode::PageUp => self.pv_scroll = self.pv_scroll.saturating_sub(20),
            KeyCode::PageDown => self.pv_scroll += 20,
            KeyCode::Home => { self.sel = 0; self.load_preview(); }
            KeyCode::End => { self.sel = n.saturating_sub(1); self.load_preview(); }
            KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::F(5) => { self.refresh(); self.say("Refreshed", false); }
            KeyCode::Char('l') | KeyCode::Char('L') => { self.mode = Mode::Log; self.sel = 0; self.refresh(); }
            KeyCode::Char('b') | KeyCode::Char('B') => { self.mode = Mode::Branches; self.sel = 0; self.refresh(); }
            KeyCode::Char('s') | KeyCode::Char('S') if matches!(self.mode, Mode::Log | Mode::Branches) => { self.mode = Mode::Status; self.sel = 0; self.refresh(); }
            KeyCode::Char('p') | KeyCode::Char('P') => { self.ask = Some((Ask::Confirm("pull".into(), vec!["pull".into(), "--ff-only".into()]), String::new())); self.say("Pull from the remote (fast-forward only)?  Y / N", false); }
            KeyCode::Char('u') | KeyCode::Char('U') => {
                let up = self.status.upstream.is_empty();
                let args = if up { vec!["push".into(), "-u".into(), "origin".into(), self.status.branch.clone()] } else { vec!["push".into()] };
                self.ask = Some((Ask::Confirm("push".into(), args), String::new()));
                self.say(&format!("Push {} to {}?  Y / N", self.status.branch, if up { "origin (new upstream)".to_string() } else { self.status.upstream.clone() }), false);
            }
            KeyCode::Char('f') | KeyCode::Char('F') => self.run_bg("fetch", vec!["fetch".into(), "--all".into(), "--prune".into()]),
            KeyCode::Char('c') | KeyCode::Char('C') if !ctrl => {
                if self.status.entries.iter().any(|e| e.area == Area::Staged) { self.ask = Some((Ask::Commit, String::new())); }
                else { self.say("Nothing staged  ·  Space stages the selected file, A stages everything", true); }
            }
            KeyCode::Char('a') | KeyCode::Char('A') if matches!(self.mode, Mode::Status) => self.act(&["add", "-A"], "Staged everything"),
            KeyCode::Char(' ') | KeyCode::Enter if matches!(self.mode, Mode::Status) => {
                if let Some(e) = self.status.entries.get(self.sel).cloned() {
                    match e.area {
                        Area::Staged => self.act(&["restore", "--staged", "--", &e.path], &format!("Unstaged {}", e.path)),
                        _ => self.act(&["add", "--", &e.path], &format!("Staged {}", e.path)),
                    }
                }
            }
            KeyCode::Enter if matches!(self.mode, Mode::Branches) => {
                if let Some(b) = self.branch_name() {
                    let local = b.strip_prefix("origin/").unwrap_or(&b).to_string();
                    self.ask = Some((Ask::Confirm("checkout".into(), vec!["switch".into(), local.clone()]), String::new()));
                    self.say(&format!("Switch to branch {local}?  Y / N"), false);
                }
            }
            KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Delete if matches!(self.mode, Mode::Status) => {
                if let Some(e) = self.status.entries.get(self.sel).cloned() {
                    match e.area {
                        Area::Changed => { self.ask = Some((Ask::Confirm("discard".into(), vec!["restore".into(), "--".into(), e.path.clone()]), String::new())); self.say(&format!("Throw away your changes to {}?  Y / N", e.path), true); }
                        Area::Untracked => {
                            if let Some(root) = &self.root {
                                match crate::winapi::recycle(&root.join(&e.path)) { Ok(()) => self.say(&format!("{} moved to the Recycle Bin", e.path), false), Err(x) => self.say(&x, true) }
                                self.refresh();
                            }
                        }
                        _ => self.say("Unstage it first (Space)", true),
                    }
                }
            }
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
        f.render_widget(Paragraph::new(Line::from(vec![bar.clone(), Span::styled("GIT", Style::default().fg(t.text).add_modifier(Modifier::BOLD))])), row(area.y + 1));
        let Some(root) = &self.root else {
            f.render_widget(Paragraph::new(Line::from(vec![bar.clone(), Span::styled(self.dir.display().to_string(), dim)])), row(area.y + 2));
            let lines = vec![Line::raw(""), Line::styled("  This folder is not in a git repository.", Style::default().fg(t.text)), Line::raw(""),
                Line::from(vec![Span::styled("  I", Style::default().fg(t.accent).add_modifier(Modifier::BOLD)), Span::styled("  create one here: git init + a .gitignore (then A stages everything, C commits)", dim)]),
                Line::from(vec![Span::styled("  Esc", Style::default().fg(t.accent).add_modifier(Modifier::BOLD)), Span::styled("  back", dim)])];
            f.render_widget(Paragraph::new(lines), Rect { x: area.x, y: area.y + 4, width: w, height: 6 });
            if let Some((Ask::Name | Ask::Email(_), _)) = &self.ask { self.draw_ask(f, t); }
            self.draw_msg(f, t);
            return;
        };
        let s = &self.status;
        let mut head = vec![bar.clone(), Span::styled(format!("⎇ {}", if s.branch.is_empty() { "?" } else { &s.branch }), Style::default().fg(t.accent).add_modifier(Modifier::BOLD))];
        if !s.upstream.is_empty() { head.push(Span::styled(format!("  →  {}", s.upstream), dim)); }
        if s.ahead > 0 { head.push(Span::styled(format!("  ↑{} to push", s.ahead), Style::default().fg(t.ok))); }
        if s.behind > 0 { head.push(Span::styled(format!("  ↓{} to pull", s.behind), Style::default().fg(t.highlight))); }
        head.push(Span::styled(format!("   {}", root.display()), dim));
        f.render_widget(Paragraph::new(Line::from(head)), row(area.y + 2));
        let tab = |name: &str, on: bool| Span::styled(format!(" {name} "), if on { Style::default().fg(t.accent).add_modifier(Modifier::BOLD | Modifier::UNDERLINED) } else { dim });
        f.render_widget(Paragraph::new(Line::from(vec![Span::raw("  "), tab("S changes", matches!(self.mode, Mode::Status)), tab("L history", matches!(self.mode, Mode::Log)), tab("B branches", matches!(self.mode, Mode::Branches))])), row(area.y + 3));
        let body = Rect { x: area.x, y: area.y + 5, width: w, height: area.height.saturating_sub(8) };
        let lw = (w as u32 * 38 / 100).clamp(30, 70) as u16;
        let list = Rect { width: lw, ..body };
        let pv = Rect { x: body.x + lw + 1, width: w.saturating_sub(lw + 2), ..body };
        for y in body.y..body.y + body.height { f.buffer_mut().set_string(body.x + lw, y, "│", dim); }
        // list
        let mut lines: Vec<Line> = Vec::new();
        let top = self.sel.saturating_sub(list.height.saturating_sub(3) as usize);
        match self.mode {
            Mode::Status => {
                if s.entries.is_empty() { lines.push(Line::styled("  nothing to commit, working tree clean ✓", Style::default().fg(t.ok))); }
                let mut last: Option<Area> = None;
                for (i, e) in s.entries.iter().enumerate() {
                    if last != Some(e.area) {
                        let (name, n) = (match e.area { Area::Staged => "STAGED", Area::Changed => "CHANGES", Area::Untracked => "UNTRACKED", Area::Conflict => "CONFLICTS" }, s.entries.iter().filter(|x| x.area == e.area).count());
                        if i >= top { lines.push(Line::from(vec![Span::styled(format!("  {name}"), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)), Span::styled(format!("  {n}"), dim)])); }
                        last = Some(e.area);
                    }
                    if i < top { continue; }
                    let sel = i == self.sel;
                    let bg = if sel { Style::default().bg(t.select_bg) } else { Style::default() };
                    let col = match (e.area, e.code) { (Area::Conflict, _) => t.error, (_, 'D') => t.error, (_, 'A') | (Area::Untracked, _) => t.ok, (Area::Staged, _) => t.ok, _ => t.highlight };
                    let txt = format!("{}{} {}", if sel { "► " } else { "  " }, e.code, e.path);
                    let pad = (lw as usize).saturating_sub(txt.width());
                    lines.push(Line::from(vec![Span::styled(crate::ui::fit(&txt, lw as usize).trim_end().to_string(), bg.fg(col).add_modifier(if sel { Modifier::BOLD } else { Modifier::empty() })), Span::styled(" ".repeat(pad), bg)]));
                }
            }
            Mode::Log | Mode::Branches => {
                let items = if matches!(self.mode, Mode::Log) { &self.log } else { &self.branches };
                if items.is_empty() { lines.push(Line::styled("  (nothing)", dim)); }
                for (i, l) in items.iter().enumerate().skip(top).take(list.height as usize) {
                    let sel = i == self.sel;
                    let bg = if sel { Style::default().bg(t.select_bg) } else { Style::default() };
                    let current = matches!(self.mode, Mode::Branches) && l.starts_with('*');
                    let txt = crate::ui::fit(&format!("{}{l}", if sel { "► " } else { "  " }), lw as usize);
                    lines.push(Line::styled(txt, bg.fg(if current { t.ok } else { t.text }).add_modifier(if sel { Modifier::BOLD } else { Modifier::empty() })));
                }
            }
        }
        f.render_widget(Paragraph::new(lines), list);
        // diff / details
        let mut pl: Vec<Line> = Vec::new();
        for l in self.preview.iter().skip(self.pv_scroll).take(pv.height as usize) {
            let st = if l.starts_with("+++") || l.starts_with("---") || l.starts_with("diff ") || l.starts_with("index ") { dim }
                else if l.starts_with('+') { Style::default().fg(t.ok) }
                else if l.starts_with('-') { Style::default().fg(t.error) }
                else if l.starts_with("@@") { Style::default().fg(t.accent) }
                else if l.starts_with("commit ") { Style::default().fg(t.highlight).add_modifier(Modifier::BOLD) }
                else { Style::default().fg(t.text) };
            pl.push(Line::styled(crate::ui::fit(&l.replace('\t', "    "), pv.width as usize).trim_end().to_string(), st));
        }
        f.render_widget(Paragraph::new(pl), pv);
        if self.preview.len() > pv.height as usize {
            let more = format!(" {}-{} of {} ", self.pv_scroll + 1, (self.pv_scroll + pv.height as usize).min(self.preview.len()), self.preview.len());
            f.buffer_mut().set_string(pv.x + pv.width.saturating_sub(more.len() as u16 + 1), pv.y, &more, dim);
        }
        self.draw_msg(f, t);
        if self.ask.as_ref().map(|(a, _)| !matches!(a, Ask::Confirm(..))).unwrap_or(false) { self.draw_ask(f, t); }
        let hints: Vec<(&str, &str)> = match self.mode {
            Mode::Status => vec![("Space", "stage / unstage"), ("A", "stage all"), ("C", "commit"), ("D", "discard"), ("P", "pull"), ("U", "push"), ("F", "fetch"), ("L", "history"), ("B", "branches"), ("Ctrl+↑↓ PgUp PgDn", "scroll diff"), ("Esc", "back")],
            Mode::Log => vec![("↑↓", "commit"), ("PgUp PgDn", "scroll"), ("S", "changes"), ("B", "branches"), ("Esc", "back")],
            Mode::Branches => vec![("Enter", "switch to it"), ("S", "changes"), ("L", "history"), ("Esc", "back")],
        };
        f.render_widget(Paragraph::new(crate::ui::hint_line(&hints, w, t)), row(area.y + area.height.saturating_sub(1)));
    }

    fn draw_msg(&self, f: &mut Frame, t: &Theme) {
        let area = f.area();
        let y = area.y + area.height.saturating_sub(2);
        let st = Style::default().fg(if self.msg_err { t.error } else { t.ok });
        f.buffer_mut().set_string(area.x + 2, y, crate::ui::fit(&self.msg, area.width.saturating_sub(4) as usize).trim_end(), st);
    }

    fn draw_ask(&self, f: &mut Frame, t: &Theme) {
        let Some((ask, text)) = &self.ask else { return };
        let area = f.area();
        let label = match ask { Ask::Commit => "Commit message › ", Ask::Name => "Your name for commits › ", Ask::Email(_) => "Your e-mail for commits › ", Ask::Confirm(..) => "" };
        let y = area.y + area.height.saturating_sub(2);
        let r = Rect { x: area.x, y, width: area.width, height: 1 };
        f.render_widget(Paragraph::new(Line::from(vec![Span::styled(format!("  {label}"), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)), Span::styled(text.clone(), Style::default().fg(t.text))])), r);
        f.set_cursor_position((area.x + 2 + label.width() as u16 + text.width() as u16, y));
        let _ = Color::Reset;
    }
}

pub fn run(terminal: &mut ratatui::DefaultTerminal, dir: &Path) -> std::io::Result<()> {
    let mut cfg = crate::config::Config::load();
    let mut p = Panel::new(dir);
    let _ = terminal.clear();
    loop {
        terminal.draw(|f| p.draw(f, &cfg.theme))?;
        loop {
            if p.poll() { break; }
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => {
                        if k.code == KeyCode::F(9) { cfg.cycle_theme(1); let _ = terminal.clear(); }
                        else if !p.on_key(k) { return Ok(()); }
                    }
                    Event::Mouse(m) => match m.kind {
                        MouseEventKind::ScrollDown => p.pv_scroll += 3,
                        MouseEventKind::ScrollUp => p.pv_scroll = p.pv_scroll.saturating_sub(3),
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
    fn status() {
        let s = parse_status("## main...origin/main [ahead 2, behind 1]\nM  src/a.rs\n M src/b.rs\nMM c.txt\n?? new file.txt\nR  old.rs -> new.rs\nUU conflict.rs\n");
        assert_eq!((s.branch.as_str(), s.upstream.as_str(), s.ahead, s.behind), ("main", "origin/main", 2, 1));
        let v: Vec<(Area, &str)> = s.entries.iter().map(|e| (e.area, e.path.as_str())).collect();
        assert_eq!(v, vec![(Area::Conflict, "conflict.rs"), (Area::Staged, "src/a.rs"), (Area::Staged, "c.txt"), (Area::Staged, "new.rs"), (Area::Changed, "src/b.rs"), (Area::Changed, "c.txt"), (Area::Untracked, "new file.txt")]);
        let s = parse_status("## No commits yet on master\n?? a\n");
        assert_eq!(s.branch, "master");
    }
}
