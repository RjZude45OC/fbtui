//! file-browser: Rust port of browser.bat / file_browser.ps1 (migration step 1: browsing + previews).

mod app;
mod bench;
mod calendar;
mod chat;
mod clock;
mod config;
mod deps;
mod editor;
mod egg;
mod fileops;
mod folderdiff;
mod gallery;
mod git;
mod heatmap;
mod grep;
mod textdoc;
mod tools;
mod typing;
mod markdown;
mod media;
mod mt_themes;
mod notify;
mod planner;
mod viewer;
mod fsutil;
mod highlight;
mod launcher;
mod preview;
mod shell;
mod shortcuts;
mod spotlight;
mod tasks;
mod themes;
mod text;
mod ui;
mod watch;
mod winapi;
mod worklog;

use ratatui::crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use ratatui::crossterm::execute;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() -> std::io::Result<()> {
    let t0 = Instant::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    // an update that could not replace the exe while it ran: swap it in now (the next start uses it)
    if args.first().map(|a| a != "--notify-daemon").unwrap_or(true) {
        shortcuts::cleanup_old();
        shortcuts::apply_pending_update();
    }
    if args.first().map(|a| a == "--shortcuts").unwrap_or(false) { shortcuts::run_cli(args.get(1).map(|s| s.as_str())); return Ok(()); }
    if args.first().map(|a| !a.starts_with("--notify")).unwrap_or(true) { shortcuts::set_console_icon(); }
    // file-browser.exe --notify: start the task reminders (Startup entry) / --notify-daemon: the reminders themselves
    match args.first().map(|a| a.as_str()) {
        Some("--notify") => { if let Err(e) = notify::ensure_running() { eprintln!("Reminders could not start: {e}"); } return Ok(()); }
        Some("--notify-daemon") => {
            let tasks = args.get(1).map(PathBuf::from).unwrap_or_else(tasks::default_path);
            let exe = args.get(2).map(PathBuf::from).unwrap_or_default();
            notify::daemon(tasks, exe);
            return Ok(());
        }
        Some("--typing") => {
            let mut terminal = ratatui::init();
            let r = typing::run(&mut terminal);
            ratatui::restore();
            return r;
        }
        Some("--pick") => {
            // file-browser.exe --pick <answer file> [folder]: the browser as a picker for a task's link
            let mut app = app::App::new(args.get(2).map(|a| PathBuf::from(a.trim_matches('"'))));
            app.pick = args.get(1).map(PathBuf::from);
            winapi::com_init();
            let mut terminal = ratatui::init();
            execute!(std::io::stdout(), EnableMouseCapture)?;
            app.setup_picker();
            app.set_status("Pick a file or folder for the task: Enter takes the selected one (on .. this folder), Ctrl+B / Tab for bookmarks, Esc cancels", false);
            let r = run(&mut terminal, &mut app);
            let _ = execute!(std::io::stdout(), DisableMouseCapture);
            ratatui::restore();
            return r;
        }
        Some("--tasks") => {
            let mut terminal = ratatui::init();
            execute!(std::io::stdout(), EnableMouseCapture)?;
            let r = planner::run(&mut terminal, None);
            let _ = execute!(std::io::stdout(), DisableMouseCapture);
            ratatui::restore();
            return r;
        }
        _ => {}
    }
    start_reminders();
    // file-browser.exe --menu [folder]: the script launcher (menu.bat)
    if args.first().map(|a| a == "--menu").unwrap_or(false) {
        winapi::com_init();
        return launcher::run(args.get(1).map(|a| PathBuf::from(a.trim_matches('"'))));
    }
    // file-browser.exe [folder]   or   file-browser.exe --bench <steps file> <folder>
    let bench = if args.first().map(|a| a == "--bench").unwrap_or(false) { args.get(1).map(PathBuf::from) } else { None };
    let start_arg = if bench.is_some() { args.get(2) } else { args.first() };
    let start = start_arg.map(|a| PathBuf::from(a.trim_matches('"')));
    winapi::com_init();
    let mut app = app::App::new(start);

    let mut terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;
    app.setup_picker();
    let result = match bench {
        Some(steps) => bench::run(&mut terminal, &mut app, &steps, t0),
        None => run(&mut terminal, &mut app),
    };
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut app::App) -> std::io::Result<()> {
    let mut dirty = true;
    let mut last_cfg_check = Instant::now();
    let mut last_tasks = (Instant::now(), chrono::Local::now().format("%H:%M").to_string());
    loop {
        let size = terminal.size()?;
        app.request_previews(size.width, size.height);
        if app.open_shell {
            // Ctrl+T: a real Command Prompt in this folder; back here after "exit"
            app.open_shell = false;
            let _ = execute!(std::io::stdout(), DisableMouseCapture);
            ratatui::restore();
            if let Some(dir) = app.cwd.clone() { shell::interactive(&dir); }
            *terminal = ratatui::init();
            execute!(std::io::stdout(), EnableMouseCapture)?;
            app.reload();
            app.clear_screen = true;
        }
        if app.open_tasks { app.open_tasks = false; app.open_tool = Some(app::ToolReq::Tasks); }
        if let Some(tool) = app.open_tool.take() {
            // a full-screen tool: back here when it closes
            match tool {
                app::ToolReq::Tasks => planner::run(terminal, app.picker.clone())?,
                app::ToolReq::TaskLink(p) => planner::run_link(terminal, app.picker.clone(), p)?,
                app::ToolReq::Heatmap => planner::run_with(terminal, app.picker.clone(), true)?,
                app::ToolReq::Typing => typing::run(terminal)?,
                app::ToolReq::Themes => themes::run(terminal)?,
                app::ToolReq::Chat => { if let Some(t) = chat::run(terminal)? { app.start_tool(t); } }
                app::ToolReq::Calendar => calendar::run(terminal)?,
                app::ToolReq::Git(dir) => git::run(terminal, &dir)?,
                app::ToolReq::Diff(a, b) => folderdiff::run(terminal, &a, &b)?,
                app::ToolReq::Gallery(dir, start) => {
                    if let Some(picker) = app.picker.clone() {
                        let (open, last) = gallery::run(terminal, picker, &dir, start.as_deref(), app.show_hidden)?;
                        if let Some(p) = last { app.reveal_path(&p); }
                        if let Some(p) = open { app.clear_screen = true; app.open_viewer(&p); }
                    }
                }
            }
            app.tasks_info = planner::summary();
            app.reload();
            app.clear_screen = true;
        }
        if app.clear_screen { app.clear_screen = false; terminal.clear()?; dirty = true; }
        if dirty {
            terminal.draw(|f| ui::draw(f, app))?;
            dirty = false;
        }
        let mut wait = app.viewer.as_ref().map(|v| v.wake_in()).unwrap_or(Duration::from_millis(30));
        if app.busy() { wait = wait.min(Duration::from_millis(if app.egg.is_some() { 30 } else { 15 })); }
        if event::poll(wait)? {
            // handle everything that is queued before drawing again (fast typing / key repeat)
            loop {
                match event::read()? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => {
                        app.input_pending = event::poll(Duration::from_millis(0))?;
                        let had_overlay = app.spotlight.is_some() || app.tools.is_some();
                        app.on_key(k);
                        // pictures (Sixel) are only redrawn on a full repaint: do one when the quick-open box opens or closes
                        if had_overlay != (app.spotlight.is_some() || app.tools.is_some()) { app.clear_screen = true; }
                    }
                    Event::Mouse(m) => app.on_mouse(m),
                    Event::Resize(_, _) => { terminal.clear()?; }
                    _ => {}
                }
                if app.quit { return Ok(()); }
                if !event::poll(Duration::from_millis(0))? { break; }
            }
            dirty = true;
        }
        if app.engine.poll() { dirty = true; }
        if app.tick() { dirty = true; }
        // the header button shows the time and the task count: refresh them every minute / 20 s
        let minute = chrono::Local::now().format("%H:%M").to_string();
        if minute != last_tasks.1 || last_tasks.0.elapsed() > Duration::from_secs(20) {
            if last_tasks.0.elapsed() > Duration::from_secs(20) { app.tasks_info = planner::summary(); last_tasks.0 = Instant::now(); }
            last_tasks.1 = minute;
            dirty = true;
        }
        if last_cfg_check.elapsed() > Duration::from_millis(800) {
            last_cfg_check = Instant::now();
            if app.check_config() { terminal.clear()?; dirty = true; }
        }
    }
}

/// Keep the task reminders running while the tools are used (only once there is a task list).
fn start_reminders() {
    std::thread::spawn(|| {
        let cfg = config::Config::load();
        if cfg.ini.get("tasks", "reminders").map(|v| v.eq_ignore_ascii_case("no")).unwrap_or(false) { return; }
        if tasks::exists(&tasks::default_path()) { let _ = notify::ensure_running(); }
    });
}
