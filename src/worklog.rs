//! Work log: what was finished and what was done, one section per day.
//! A plain Markdown file next to tasks.json (script\worklog.md) that can also be edited by hand.
//!
//!   ## 2026-10-05 · Monday
//!
//!   - 09:14  ✔ Call the bank — sent the transfer form
//!   - 11:02  Fixed the login bug in the client app

use chrono::NaiveDateTime;
use std::io::Write;
use std::path::PathBuf;

pub fn path() -> PathBuf {
    let tasks = crate::tasks::default_path();
    let cfg = crate::config::Config::load();
    match cfg.ini.get("tasks", "log") {
        Some(name) => {
            let p = PathBuf::from(name);
            if p.is_absolute() { p } else { tasks.with_file_name(p) }
        }
        None => tasks.with_file_name("worklog.md"),
    }
}

fn header(now: NaiveDateTime) -> String { format!("## {}", now.format("%Y-%m-%d · %A")) }

/// Add one line under today's heading (the heading is added when the day changes).
pub fn append(text: &str, now: NaiveDateTime) -> Result<PathBuf, String> {
    let p = path();
    append_to(&p, text, now)?;
    Ok(p)
}

pub fn append_to(p: &std::path::Path, text: &str, now: NaiveDateTime) -> Result<(), String> {
    let old = std::fs::read_to_string(p).unwrap_or_default();
    let today = header(now);
    let last_header = old.lines().rev().find(|l| l.starts_with("## ")).map(|l| l.trim_end().to_string());
    let mut add = String::new();
    if old.is_empty() { add.push_str("# Work log\n\n"); }
    if last_header.as_deref() != Some(today.as_str()) {
        if !old.is_empty() {
            if !old.ends_with('\n') { add.push('\n'); }
            if !old.ends_with("\n\n") { add.push('\n'); }
        }
        add.push_str(&today);
        add.push_str("\n\n");
    } else if !old.is_empty() && !old.ends_with('\n') {
        add.push('\n');
    }
    let line = text.trim().replace(['\r', '\n'], " ");
    add.push_str(&format!("- {}  {line}\n", now.format("%H:%M")));
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(p).map_err(|e| format!("Cannot write {}: {e}", p.display()))?;
    f.write_all(add.as_bytes()).map_err(|e| format!("Cannot write {}: {e}", p.display()))
}

/// "✔ Call the bank — sent the form" / "✔ Call the bank"
pub fn finished(title: &str, note: &str) -> String {
    let note = note.trim();
    if note.is_empty() { format!("✔ {title}") } else { format!("✔ {title} — {note}") }
}

/// All lines of the log (for the log view).
pub fn read() -> Vec<String> {
    std::fs::read_to_string(path()).map(|t| t.lines().map(|l| l.trim_end().to_string()).collect()).unwrap_or_default()
}

/// Entries written today (for the counter on the screen).
pub fn today_count(now: NaiveDateTime) -> usize {
    let lines = read();
    let h = header(now);
    match lines.iter().rposition(|l| *l == h) {
        Some(i) => lines[i + 1..].iter().take_while(|l| !l.starts_with("## ")).filter(|l| l.starts_with("- ")).count(),
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn days() {
        let dir = std::env::temp_dir().join(format!("fb_log_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("worklog.md");
        let day1 = chrono::NaiveDate::from_ymd_opt(2026, 10, 5).unwrap().and_hms_opt(9, 14, 0).unwrap();
        let day2 = chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap().and_hms_opt(8, 30, 0).unwrap();
        let w = |text: &str, now: NaiveDateTime| append_to(&p, text, now).unwrap();
        w(&finished("Call the bank", "sent the form"), day1);
        w("Fixed the login bug", day1 + chrono::Duration::hours(2));
        w(&finished("Gym", ""), day2);
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text, "# Work log\n\n## 2026-10-05 · Monday\n\n- 09:14  ✔ Call the bank — sent the form\n- 11:14  Fixed the login bug\n\n## 2026-10-06 · Tuesday\n\n- 08:30  ✔ Gym\n");
        let _ = std::fs::remove_dir_all(dir);
    }
}
