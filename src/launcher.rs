//! Script launcher (menu.bat): lists the scripts in app\ and runs them.
//! `file-browser.exe --menu [folder]`. Same keys and look as menu_scripts.ps1.

use crate::config::Config;
use crate::fsutil::fmt_time;
use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

const LANGS: &[(&str, &str, &str)] = &[
    (".py", "Python", "python"), (".bat", "Batch", "batch"), (".cmd", "CMD", "cmd"), (".ps1", "PowerShell", "powershell"), (".vbs", "VBScript", "vbscript"),
];
const EXCLUDE: &[&str] = &["menu.bat", "menu_scripts.bat", "menu_scripts.ps1"];

struct Item { name: String, path: PathBuf, ext: String, size: u64, modified: Option<std::time::SystemTime> }

struct Menu {
    dir: PathBuf,
    cfg: Config,
    items: Vec<Item>,
    sel: usize,
    top: usize,
    visible: usize,
    list_y: u16,
    last_click: Option<(usize, Instant)>,
    status: String,
    egg: Option<crate::egg::Egg>,
    spot: Option<crate::spotlight::Spotlight>,
    typed: String,
    tasks: Option<String>,
    tasks_button: Rect,
    tools_button: Rect,
    tools: Option<crate::tools::Window>,
}

fn lang(ext: &str) -> Option<&'static (&'static str, &'static str, &'static str)> { LANGS.iter().find(|l| l.0 == ext) }

impl Menu {
    fn load(&mut self) {
        let mut v = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&self.dir) {
            for e in rd.flatten() {
                let Ok(md) = e.metadata() else { continue };
                if !md.is_file() { continue; }
                let name = e.file_name().to_string_lossy().to_string();
                let ext = crate::fsutil::ext_of(&e.path());
                if lang(&ext).is_none() || EXCLUDE.iter().any(|x| x.eq_ignore_ascii_case(&name)) { continue; }
                v.push(Item { name, path: e.path(), ext, size: md.len(), modified: md.modified().ok() });
            }
        }
        v.sort_by_key(|i| i.name.to_lowercase());
        self.items = v;
        if self.sel >= self.items.len() { self.sel = self.items.len().saturating_sub(1); }
    }

    fn fix_scroll(&mut self) {
        let v = self.visible.max(1);
        if self.sel < self.top { self.top = self.sel; }
        if self.sel >= self.top + v { self.top = self.sel + 1 - v; }
        let max_top = self.items.len().saturating_sub(v);
        if self.top > max_top { self.top = max_top; }
    }

    fn move_to(&mut self, i: isize) {
        if self.items.is_empty() { return; }
        self.sel = i.clamp(0, self.items.len() as isize - 1) as usize;
    }

    fn draw(&mut self, f: &mut ratatui::Frame) {
        let t = self.cfg.theme.clone();
        let area = f.area();
        if let Some(egg) = self.egg.as_mut() { egg.draw(f.buffer_mut(), area, t.bg_rgb, t.dim); return; }
        self.draw_menu(f);
        if let Some(sp) = &self.spot { sp.draw(f, &t); }
        if let Some(tw) = &self.tools { tw.draw(f, &t, &self.cfg); }
    }

    fn draw_menu(&mut self, f: &mut ratatui::Frame) {
        let t = self.cfg.theme.clone();
        let area = f.area();
        let (w, h) = (area.width, area.height);
        let base = match t.background { Some(bg) => Style::default().bg(bg).fg(t.text), None => Style::default().fg(t.text) };
        f.render_widget(Block::default().style(base), area);
        let row = |y: u16| Rect { x: 0, y, width: w, height: 1 };
        let dim = Style::default().fg(t.dim);
        let bar = Span::styled("  ▌ ", Style::default().fg(t.accent));
        let n = self.items.len();
        f.render_widget(Paragraph::new(Line::from(vec![bar.clone(), Span::styled("SCRIPT LAUNCHER", Style::default().fg(t.text).add_modifier(Modifier::BOLD))])), row(1));
        let tasks_on = crate::tools::enabled(&self.cfg, crate::tools::Tool::Tasks);
        self.tasks_button = if tasks_on { crate::ui::tasks_button(f, row(1), &t, self.tasks.as_deref(), 21) } else { Rect::default() };
        self.tools_button = crate::ui::tools_button(f, row(1), &t, if tasks_on { self.tasks_button.x } else { w - 1 }, 21);
        f.render_widget(Paragraph::new(Line::from(vec![bar.clone(), Span::styled(self.dir.display().to_string(), dim)])), row(2));
        let now = chrono::Local::now().format("%H:%M");
        f.render_widget(Paragraph::new(Line::from(vec![bar.clone(), Span::styled(format!("{n} {}  ·  {}  ·  {now}", if n == 1 { "script" } else { "scripts" }, self.cfg.label()), dim),
            Span::styled(self.tasks.as_ref().map(|s| format!("  ·  {s}  (T)")).unwrap_or_default(), Style::default().fg(t.accent))])), row(3));
        let warn = self.cfg.warning.clone().unwrap_or_else(|| self.status.clone());
        f.render_widget(Paragraph::new(Span::styled(format!("  {warn}"), Style::default().fg(if self.cfg.warning.is_some() { t.error } else { t.ok }))), row(4));
        let rule = format!("  {}", "─".repeat(w.saturating_sub(4) as usize));
        f.render_widget(Paragraph::new(Span::styled(rule.clone(), dim)), row(5));
        let fixed = 28usize;
        let show_date = (w as usize).saturating_sub(1 + fixed + 18) >= 20;
        let date_w = if show_date { 18 } else { 0 };
        let name_w = (w as usize).saturating_sub(1 + fixed + date_w).clamp(8, 48);
        let mut hdr = format!("      {:<nw$} {:<14}{:>7}", "NAME", "TYPE", "SIZE", nw = name_w);
        if show_date { hdr += "  MODIFIED"; }
        f.render_widget(Paragraph::new(Span::styled(hdr, dim)), row(6));
        f.render_widget(Paragraph::new(Span::styled(rule.clone(), dim)), row(7));
        self.list_y = 8;
        self.visible = h.saturating_sub(11).max(1) as usize;
        self.fix_scroll();
        let badge_fg = t.rgb.get("badge_text").copied().unwrap_or((20, 20, 20));
        for k in 0..self.visible {
            let i = self.top + k;
            let y = self.list_y + k as u16;
            if let Some(it) = self.items.get(i) {
                let is_sel = i == self.sel;
                let bg = if is_sel { Style::default().bg(t.select_bg) } else { Style::default() };
                let l = lang(&it.ext).unwrap();
                let (r, g, b) = t.rgb.get(l.2).copied().unwrap_or((128, 128, 128));
                let kb = ((it.size as f64 / 1024.0).ceil() as u64).max(1);
                let mut name = it.name.clone();
                if name.width() > name_w { name = name.chars().take(name_w.saturating_sub(1)).collect::<String>() + "…"; }
                let pad = name_w.saturating_sub(name.width());
                let mut spans = vec![
                    Span::styled(if is_sel { "► " } else { "  " }, bg.fg(t.accent)),
                    Span::styled(format!("{:02}  ", i + 1), bg.fg(t.accent)),
                    Span::styled(format!("{name}{} ", " ".repeat(pad)), if is_sel { bg.fg(t.text).add_modifier(Modifier::BOLD) } else { bg.fg(t.text) }),
                    Span::styled(format!(" {:<10} ", l.1), Style::default().bg(Color::Rgb(r, g, b)).fg(Color::Rgb(badge_fg.0, badge_fg.1, badge_fg.2))),
                    Span::styled(format!("  {:>7}", format!("{kb} KB")), bg.fg(t.dim)),
                ];
                if show_date { spans.push(Span::styled(format!("  {}", fmt_time(it.modified)), bg.fg(t.dim))); }
                let used: usize = spans.iter().map(|s| s.content.width()).sum();
                spans.push(Span::styled(" ".repeat((w as usize).saturating_sub(used)), bg));
                f.render_widget(Paragraph::new(Line::from(spans)), row(y));
            } else if n == 0 && k == 1 {
                let exts: Vec<&str> = LANGS.iter().map(|l| l.0).collect();
                f.render_widget(Paragraph::new(Span::styled(format!("  No scripts found  ·  {}", exts.join("  ")), dim)), row(y));
            }
        }
        f.render_widget(Paragraph::new(Span::styled(rule, dim)), row(h.saturating_sub(2)));
        let range = format!("{}-{}", self.top + 1, (self.top + self.visible).min(n));
        let of = format!("of {n}");
        let mut segs: Vec<(&str, &str)> = vec![("↑↓", "move"), ("Enter", "run"), ("1-9", "jump"), ("R", "refresh"), ("O", "folder"), ("B", "file browser"), ("T", "tasks & clock"), ("A", "local AI"), ("Tab", "search"), ("Esc", "tools on/off"), ("Ctrl+Tab", "theme"), ("Q", "quit")];
        if n > self.visible { segs.push((&range, &of)); }
        f.render_widget(Paragraph::new(crate::ui::hint_line(&segs, w, &t)), row(h.saturating_sub(1)));
    }
}

/// A tool from the launcher (the app folder is the folder for git and the gallery).
fn run_tool(terminal: &mut ratatui::DefaultTerminal, menu: &mut Menu, tool: crate::tools::Tool) -> std::io::Result<()> {
    use crate::tools::Tool;
    if tool == Tool::Chat && !crate::chat::available(&menu.cfg) { menu.status = crate::chat::NOT_INSTALLED.into(); return Ok(()); }
    if !crate::tools::enabled(&menu.cfg, tool) { menu.status = format!("{} is off  ·  turn it on in the tools list (Esc)", crate::tools::feature(tool).name); return Ok(()); }
    match tool {
        Tool::Tasks => crate::planner::run(terminal, None)?,
        Tool::Heatmap => crate::planner::run_with(terminal, None, true)?,
        Tool::Typing => crate::typing::run(terminal)?,
        Tool::Themes => crate::themes::run(terminal)?,
        Tool::Chat => { if let Some(t) = crate::chat::run(terminal)? { menu.cfg.reload(); return run_tool(terminal, menu, t); } }
        Tool::Calendar => crate::calendar::run(terminal)?,
        Tool::Git => crate::git::run(terminal, &menu.dir)?,
        Tool::Gallery => {
            let (p, _) = crate::app::build_picker(&menu.cfg.image_mode());
            let (open, _) = crate::gallery::run(terminal, p, &menu.dir, None, false)?;
            if let Some(p) = open { let _ = crate::winapi::open_default(&p); }
        }
        Tool::Diff => { menu.status = "Folder compare works in the file browser (B): mark two folders, Alt+D".into(); return Ok(()); }
    }
    menu.tasks = crate::planner::summary();
    terminal.clear()
}

fn find_python() -> Option<PathBuf> {
    let paths: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    let mut found = Vec::new();
    for name in ["py", "python", "python3"] {
        let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
        if let Some(p) = paths.iter().map(|d| d.join(&file)).find(|p| p.is_file()) { found.push(p); }
    }
    // skip the Microsoft Store stub unless it is all there is
    found.iter().find(|p| !p.to_string_lossy().to_lowercase().contains("\\windowsapps\\")).cloned().or_else(|| found.first().cloned())
}

/// Leave the full-screen UI, run the script in this console, wait for a key, come back.
fn run_script(terminal: &mut ratatui::DefaultTerminal, menu: &Menu, it: &Item) -> std::io::Result<()> {
    let t = &menu.cfg.theme;
    let fg = |c: Color| match c { Color::Rgb(r, g, b) => format!("\x1b[38;2;{r};{g};{b}m"), _ => String::new() };
    let (acc, dim, ok, err) = (fg(t.accent), fg(t.dim), fg(t.ok), fg(t.error));
    let (bold, rst) = ("\x1b[1m", "\x1b[0m");
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    let lang_name = lang(&it.ext).map(|l| l.1).unwrap_or("");
    let rule = "─".repeat(60.min(terminal.size().map(|s| s.width as usize).unwrap_or(80).saturating_sub(4)).max(10));
    let mut out = std::io::stdout();
    write!(out, "\n  {acc}▌{rst} {bold}RUNNING{rst}\n  {acc}▌{rst} {bold}{}{rst}  {dim}·  {lang_name}{rst}\n\n  {dim}{rule}{rst}\n\n", it.name)?;
    out.flush()?;
    let dir = it.path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let start = Instant::now();
    let mut cmd = match it.ext.as_str() {
        ".py" => match find_python() {
            Some(py) => { let mut c = std::process::Command::new(py); c.arg(&it.path); Some(c) }
            None => { writeln!(out, "  {err}Python was not found on PATH (tried py, python, python3).{rst}")?; None }
        },
        ".bat" | ".cmd" => { let mut c = std::process::Command::new("cmd"); c.arg("/c").arg(&it.path); Some(c) }
        ".ps1" => { let mut c = std::process::Command::new("powershell.exe"); c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&it.path); Some(c) }
        ".vbs" => { let mut c = std::process::Command::new("cscript.exe"); c.arg("//nologo").arg(&it.path); Some(c) }
        _ => None,
    };
    let rc = match cmd.as_mut() {
        Some(c) => match c.current_dir(&dir).status() {
            Ok(s) => s.code().unwrap_or(1),
            Err(e) => { writeln!(out, "  {err}{e}{rst}")?; 1 }
        },
        None => 1,
    };
    let took = format!("{:.1}s", start.elapsed().as_secs_f64());
    write!(out, "\n  {dim}{rule}{rst}\n")?;
    if rc == 0 { writeln!(out, "  {ok}■ Completed successfully{rst}  {dim}·  {took}{rst}")?; }
    else { writeln!(out, "  {err}■ Finished with errors  ·  exit code {rc}{rst}  {dim}·  {took}{rst}")?; }
    write!(out, "\n  {dim}Press any key to return to the menu...{rst}\n")?;
    out.flush()?;
    ratatui::crossterm::terminal::enable_raw_mode()?;
    while event::poll(Duration::from_millis(0))? { let _ = event::read()?; }      // ignore keys typed while the script ran
    loop {
        if let Event::Key(k) = event::read()? { if k.kind == KeyEventKind::Press { break; } }
    }
    *terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;
    terminal.clear()?;
    Ok(())
}

pub fn run(dir: Option<PathBuf>) -> std::io::Result<()> {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_default();
    // default: the app folder next to menu.bat (script\app), found from script\migration\file-browser.exe
    // script\migration\file-browser.exe -> script\app;  script\file browser rust\migration\file-browser.exe -> script\app
    let dir = dir.or_else(|| [exe_dir.join("..").join("app"), exe_dir.join("..").join("..").join("app"), exe_dir.join("app")].into_iter().find(|d| d.is_dir()))
        .unwrap_or_else(|| exe_dir.clone());
    let dir = std::fs::canonicalize(&dir).map(|d| strip_unc(&d)).unwrap_or(dir);
    let mut menu = Menu { dir, cfg: Config::load(), items: Vec::new(), sel: 0, top: 0, visible: 10, list_y: 8, last_click: None, status: String::new(), egg: None, spot: None, typed: String::new(), tasks: crate::planner::summary(), tasks_button: Rect::default(), tools_button: Rect::default(), tools: None };
    menu.load();
    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let mut last_cfg = Instant::now();
    let mut dirty = true;
    let mut last_minute = chrono::Local::now().format("%H:%M").to_string();
    let result = (|| -> std::io::Result<()> {
        loop {
            if dirty { terminal.draw(|f| menu.draw(f))?; dirty = false; }
            if last_cfg.elapsed() > Duration::from_millis(800) {
                last_cfg = Instant::now();
                if menu.cfg.changed() { menu.cfg.reload(); terminal.clear()?; dirty = true; }
            }
            if menu.egg.is_some() { dirty = true; }
            if !event::poll(Duration::from_millis(if menu.egg.is_some() { 30 } else { 250 }))? {
                let minute = chrono::Local::now().format("%H:%M").to_string();
                if minute != last_minute { last_minute = minute; menu.tasks = crate::planner::summary(); dirty = true; }
                continue;
            }
            dirty = true;
            let mut run: Option<usize> = None;
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release && menu.egg.is_some() => {
                    if menu.egg.as_ref().map(|e| e.age() > 0.5).unwrap_or(true) { menu.egg = None; terminal.clear()?; }
                }
                Event::Key(k) if k.kind != KeyEventKind::Release && menu.tools.is_some() => {
                    let act = menu.tools.as_mut().unwrap().on_key(k, &mut menu.cfg);
                    match act {
                        crate::tools::Act::None => {}
                        crate::tools::Act::Close => { menu.tools = None; terminal.clear()?; }
                        crate::tools::Act::Changed(m) => { menu.status = m; }
                        crate::tools::Act::Open(tool) => { menu.tools = None; run_tool(&mut terminal, &mut menu, tool)?; }
                    }
                }
                Event::Key(k) if k.kind != KeyEventKind::Release && menu.spot.is_none() && k.code == KeyCode::F(10) => { menu.tools = Some(crate::tools::Window::new()); }
                Event::Key(k) if k.kind != KeyEventKind::Release && menu.spot.is_none() && k.code == KeyCode::Tab && !k.modifiers.contains(KeyModifiers::CONTROL) => {
                    menu.spot = Some(crate::spotlight::Spotlight::new(&menu.cfg.bookmarks(), "close the launcher", &crate::tools::on_list(&menu.cfg)));
                }
                Event::Key(k) if k.kind != KeyEventKind::Release && menu.spot.is_some() => {
                    let act = menu.spot.as_mut().unwrap().on_key(k);
                    let exe = std::env::current_exe().ok();
                    let run_browser = |dir: &Path, terminal: &mut ratatui::DefaultTerminal| -> std::io::Result<()> {
                        let _ = execute!(std::io::stdout(), DisableMouseCapture);
                        ratatui::restore();
                        if let Some(exe) = &exe { let _ = std::process::Command::new(exe).arg(dir).status(); }
                        *terminal = ratatui::init();
                        execute!(std::io::stdout(), EnableMouseCapture)?;
                        terminal.clear()
                    };
                    match act {
                        crate::spotlight::Act::None => {}
                        crate::spotlight::Act::Close => { menu.spot = None; terminal.clear()?; }
                        crate::spotlight::Act::Tool(t) => { menu.spot = None; run_tool(&mut terminal, &mut menu, t)?; }
                        crate::spotlight::Act::Open(p) => {
                            menu.spot = None;
                            if p.is_dir() { run_browser(&p, &mut terminal)?; }
                            else {
                                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                                menu.status = match crate::winapi::open_default(&p) { Ok(_) => format!("Opened {name}"), Err(e) => format!("Cannot open {name}: {e}") };
                            }
                        }
                        crate::spotlight::Act::Reveal(p) => { menu.spot = None; if let Some(d) = p.parent() { run_browser(d, &mut terminal)?; } }
                        crate::spotlight::Act::Quit => return Ok(()),
                        crate::spotlight::Act::Goto(p) => {
                            menu.spot = None;
                            let dir = if p.is_dir() { Some(p.as_path()) } else { p.parent() };
                            if let Some(d) = dir { run_browser(d, &mut terminal)?; }
                        }
                        crate::spotlight::Act::Remove(slot) => {
                            menu.status = match menu.cfg.set_value("bookmarks", &slot.to_string(), None) { Ok(()) => { menu.cfg.reload(); format!("Bookmark {slot} removed") } Err(e) => format!("Could not save config.ini: {e}") };
                        }
                    }
                }
                Event::Key(k) if k.kind != KeyEventKind::Release && k.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(k.code, KeyCode::Char(' ') | KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Char('@')) => {
                    menu.spot = Some(crate::spotlight::Spotlight::new(&menu.cfg.bookmarks(), "close the launcher", &crate::tools::on_list(&menu.cfg)));
                }
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    if let KeyCode::Char(c) = k.code {
                        if !k.modifiers.contains(KeyModifiers::CONTROL) && crate::egg::typed(&mut menu.typed, c) {
                            menu.egg = Some(crate::egg::Egg::new());
                            terminal.clear()?;
                            continue;
                        }
                    }
                    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
                    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
                    let page = menu.visible as isize;
                    menu.status.clear();
                    match k.code {
                        KeyCode::Tab | KeyCode::BackTab if ctrl => { menu.cfg.cycle_theme(if shift || k.code == KeyCode::BackTab { -1 } else { 1 }); terminal.clear()?; }
                        KeyCode::F(9) => { menu.cfg.cycle_theme(if shift { -1 } else { 1 }); terminal.clear()?; }
                        KeyCode::Up => menu.move_to(menu.sel as isize - 1),
                        KeyCode::Down => menu.move_to(menu.sel as isize + 1),
                        KeyCode::PageUp => menu.move_to(menu.sel as isize - page),
                        KeyCode::PageDown => menu.move_to(menu.sel as isize + page),
                        KeyCode::Home => menu.move_to(0),
                        KeyCode::End => menu.move_to(menu.items.len() as isize - 1),
                        KeyCode::Enter => { if !menu.items.is_empty() { run = Some(menu.sel); } }
                        KeyCode::Esc => { menu.tools = Some(crate::tools::Window::new()); }
                        KeyCode::Char('c') if ctrl => return Ok(()),
                        KeyCode::Char('q') | KeyCode::Char('Q') => return Ok(()),
                        KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::F(5) => { menu.cfg.reload(); menu.load(); terminal.clear()?; }
                        KeyCode::Char('o') | KeyCode::Char('O') => { crate::winapi::show_in_explorer(None, Some(&menu.dir)); }
                        KeyCode::Char('t') | KeyCode::Char('T') | KeyCode::F(8) => { run_tool(&mut terminal, &mut menu, crate::tools::Tool::Tasks)?; }
                        KeyCode::Char('y') | KeyCode::Char('Y') => { run_tool(&mut terminal, &mut menu, crate::tools::Tool::Typing)?; }
                        KeyCode::Char('a') | KeyCode::Char('A') => { run_tool(&mut terminal, &mut menu, crate::tools::Tool::Chat)?; }
                        KeyCode::Char('p') | KeyCode::Char('P') => { run_tool(&mut terminal, &mut menu, crate::tools::Tool::Themes)?; menu.cfg.reload(); }
                        KeyCode::Char('k') | KeyCode::Char('K') => { run_tool(&mut terminal, &mut menu, crate::tools::Tool::Calendar)?; }
                        KeyCode::Char('g') | KeyCode::Char('G') => { run_tool(&mut terminal, &mut menu, crate::tools::Tool::Git)?; }
                        KeyCode::Char('b') | KeyCode::Char('B') => {
                            // open the file browser in the app folder, come back here when it closes
                            let _ = execute!(std::io::stdout(), DisableMouseCapture);
                            ratatui::restore();
                            if let Ok(exe) = std::env::current_exe() { let _ = std::process::Command::new(exe).arg(&menu.dir).status(); }
                            terminal = ratatui::init();
                            execute!(std::io::stdout(), EnableMouseCapture)?;
                            terminal.clear()?;
                        }
                        KeyCode::Char(d @ '1'..='9') => { let i = (d as u8 - b'1') as usize; if i < menu.items.len() { menu.sel = i; } }
                        _ => {}
                    }
                }
                Event::Mouse(m) if matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) && menu.spot.is_none()
                    && m.row == menu.tasks_button.y && m.column >= menu.tasks_button.x && m.column < menu.tasks_button.x + menu.tasks_button.width && menu.tasks_button.width > 0 => {
                    run_tool(&mut terminal, &mut menu, crate::tools::Tool::Tasks)?;
                }
                Event::Mouse(m) if matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) && menu.spot.is_none()
                    && m.row == menu.tools_button.y && m.column >= menu.tools_button.x && m.column < menu.tools_button.x + menu.tools_button.width && menu.tools_button.width > 0 => {
                    menu.tools = Some(crate::tools::Window::new());
                }
                Event::Mouse(m) => {
                    let idx = menu.top + m.row.saturating_sub(menu.list_y) as usize;
                    let in_list = m.row >= menu.list_y && (m.row - menu.list_y) < menu.visible as u16 && idx < menu.items.len();
                    match m.kind {
                        MouseEventKind::ScrollDown => menu.move_to(menu.sel as isize + 1),
                        MouseEventKind::ScrollUp => menu.move_to(menu.sel as isize - 1),
                        MouseEventKind::Moved if in_list => menu.sel = idx,
                        MouseEventKind::Down(MouseButton::Left) if in_list => {
                            let now = Instant::now();
                            if matches!(menu.last_click, Some((i, t)) if i == idx && now.duration_since(t).as_millis() < 450) { menu.last_click = None; run = Some(idx); }
                            else { menu.sel = idx; menu.last_click = Some((idx, now)); }
                        }
                        _ => {}
                    }
                }
                Event::Resize(_, _) => { terminal.clear()?; }
                _ => {}
            }
            if let Some(i) = run {
                menu.sel = i;
                let it = Item { name: menu.items[i].name.clone(), path: menu.items[i].path.clone(), ext: menu.items[i].ext.clone(), size: 0, modified: None };
                run_script(&mut terminal, &menu, &it)?;
                menu.last_click = None;
                menu.load();
            }
        }
    })();
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// canonicalize() on Windows gives \\?\C:\... - show the normal form.
fn strip_unc(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") { Some(r) if !r.starts_with("UNC") => PathBuf::from(r), _ => p.to_path_buf() }
}
