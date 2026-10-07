//! Speed test: `file-browser.exe --bench <steps file> <folder>`.
//! Runs the same key sequence as `file_browser.ps1 -Bench` and writes log\bench_rust.csv:
//! for every key the time until the first new screen and until the screen is complete (preview drawn).

use crate::app::App;
use crate::ui;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn key(k: &str) -> Option<KeyEvent> {
    let code = match k {
        "Up" => KeyCode::Up, "Down" => KeyCode::Down, "PgUp" => KeyCode::PageUp, "PgDn" => KeyCode::PageDown,
        "Home" => KeyCode::Home, "End" => KeyCode::End, "Right" => KeyCode::Right, "Left" => KeyCode::Left,
        "Esc" => KeyCode::Esc, "Backspace" => KeyCode::Backspace, "Enter" => KeyCode::Enter,
        "F1" => KeyCode::F(1), "F3" => KeyCode::F(3), "F5" => KeyCode::F(5), "F9" => KeyCode::F(9),
        _ => KeyCode::Char(k.strip_prefix("Char:")?.chars().next()?),
    };
    Some(KeyEvent::new(code, KeyModifiers::NONE))
}

pub fn steps(file: &Path) -> Vec<(String, String)> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let mut out = Vec::new();
    for l in text.lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') { continue; }
        let p: Vec<&str> = l.split('|').collect();
        if p.len() < 2 { continue; }
        let n: usize = p.get(2).and_then(|x| x.trim().parse().ok()).unwrap_or(1);
        for _ in 0..n.max(1) { out.push((p[0].trim().to_string(), p[1].trim().to_string())); }
    }
    out
}

fn complete(app: &App, w: u16, h: u16) -> bool {
    if let Some(v) = &app.viewer { return !v.loading; }
    if app.show_help || w < 100 { return true; }
    match app.current_key(w, h) { Some(k) => app.engine.get(&k).is_some(), None => true }
}

fn ms(d: Duration) -> String { format!("{:.1}", d.as_secs_f64() * 1000.0) }

/// Draw, then keep drawing until the preview for the selection is on screen.
/// Returns (first frame, complete).
fn settle(terminal: &mut ratatui::DefaultTerminal, app: &mut App, sw: Instant) -> std::io::Result<(Duration, Duration)> {
    let size = terminal.size()?;
    app.request_previews(size.width, size.height);
    terminal.draw(|f| ui::draw(f, app))?;
    let first = sw.elapsed();
    loop {
        let size = terminal.size()?;
        app.request_previews(size.width, size.height);
        let mut changed = app.engine.poll();
        if app.tick() { changed = true; }
        if changed { terminal.draw(|f| ui::draw(f, app))?; }
        if complete(app, size.width, size.height) { if !changed { terminal.draw(|f| ui::draw(f, app))?; } break; }
        if sw.elapsed() > Duration::from_secs(30) { break; }
        std::thread::sleep(Duration::from_micros(500));
    }
    Ok((first, sw.elapsed()))
}

pub fn run(terminal: &mut ratatui::DefaultTerminal, app: &mut App, file: &Path, t0: Instant) -> std::io::Result<()> {
    let mut out = String::from("# step|label|first_frame_ms|complete_ms\n");
    // start-up: from process launch (Windows) or from main()
    let launch = crate::winapi::process_age().map(|age| Instant::now() - age).unwrap_or(t0);
    let (first, done) = settle(terminal, app, launch)?;
    let _ = writeln!(out, "0|start-up (launch to first screen)|{}|{}", ms(first), ms(done));
    for (i, (label, k)) in steps(file).into_iter().enumerate() {
        let Some(ev) = key(&k) else { continue };
        let sw = Instant::now();
        app.on_key(ev);
        let (first, done) = settle(terminal, app, sw)?;
        let _ = writeln!(out, "{}|{label}|{}|{}", i + 1, ms(first), ms(done));
        if app.quit { break; }
    }
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_default();
    let log = [exe_dir.join("..").join("log"), exe_dir.clone()].into_iter().find(|d| d.is_dir()).unwrap_or(PathBuf::from("."));
    std::fs::write(log.join("bench_rust.csv"), out)?;
    Ok(())
}
