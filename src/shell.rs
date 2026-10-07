//! Command prompt inside the browser.
//! F4 runs one command (cmd /c) in the current folder and shows its output in the preview pane;
//! Ctrl+T opens a full interactive Command Prompt there (type exit to come back).

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const MAX_LINES: usize = 20_000;

enum Msg { Data(Vec<u8>), Exit(Option<i32>) }

pub struct CmdRun {
    pub cmd: String,
    pub lines: Vec<String>,
    pub exit: Option<Option<i32>>,       // None = still running; Some(code)
    pub start: Instant,
    pub took: f64,
    pub scroll: usize,
    pub follow: bool,                     // keep showing the newest output
    rx: Receiver<Msg>,
    child: Arc<Mutex<Option<Child>>>,
    partial: Vec<u8>,
    pub killed: bool,
}

impl CmdRun {
    pub fn start(cmd: &str, dir: &Path) -> Result<CmdRun, String> {
        let mut c = shell_command(cmd);
        c.current_dir(dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
            .env("PYTHONIOENCODING", "utf-8").env("PYTHONUTF8", "1");          // Python prints UTF-8 into the pipe
        let mut child = c.spawn().map_err(|e| format!("Cannot start the command: {e}"))?;
        let (tx, rx) = channel();
        for pipe in [child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>), child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>)].into_iter().flatten() {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let mut pipe = pipe;
                let mut buf = [0u8; 8192];
                while let Ok(n) = pipe.read(&mut buf) {
                    if n == 0 || tx.send(Msg::Data(buf[..n].to_vec())).is_err() { break; }
                }
            });
        }
        let child = Arc::new(Mutex::new(Some(child)));
        let ch = child.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(20));
            let mut g = ch.lock().unwrap();
            let Some(c) = g.as_mut() else { let _ = tx.send(Msg::Exit(None)); return };
            match c.try_wait() {
                Ok(Some(st)) => { std::thread::sleep(Duration::from_millis(30)); let _ = tx.send(Msg::Exit(st.code())); return; }
                Ok(None) => {}
                Err(_) => { let _ = tx.send(Msg::Exit(None)); return; }
            }
        });
        Ok(CmdRun { cmd: cmd.to_string(), lines: Vec::new(), exit: None, start: Instant::now(), took: 0.0, scroll: 0, follow: true,
                    rx, child, partial: Vec::new(), killed: false })
    }

    pub fn running(&self) -> bool { self.exit.is_none() }

    /// Collect new output; true if anything changed.
    pub fn poll(&mut self) -> bool {
        let mut any = false;
        while let Ok(m) = self.rx.try_recv() {
            any = true;
            match m {
                Msg::Data(d) => { self.partial.extend_from_slice(&d); self.split(false); }
                Msg::Exit(code) => {
                    // late output from the pipes
                    while let Ok(Msg::Data(d)) = self.rx.try_recv() { self.partial.extend_from_slice(&d); }
                    self.split(true);
                    self.exit = Some(code);
                    self.took = self.start.elapsed().as_secs_f64();
                }
            }
        }
        any
    }

    fn split(&mut self, end: bool) {
        while let Some(i) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=i).collect();
            self.push(&line[..line.len() - 1]);
        }
        if end && !self.partial.is_empty() { let rest = std::mem::take(&mut self.partial); self.push(&rest); }
        if self.lines.len() > MAX_LINES { let cut = self.lines.len() - MAX_LINES; self.lines.drain(..cut); }
    }

    fn push(&mut self, raw: &[u8]) {
        let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
        // UTF-8 (chcp 65001, Python ...) or else the console's OEM code page (850 in Spain: ipconfig, tree, ping ...)
        let text = match std::str::from_utf8(raw) { Ok(s) => s.to_string(), Err(_) => oem_decode(raw) };
        // a progress line overwritten with \r: keep the last part
        let text = text.rsplit('\r').next().unwrap_or("").to_string();
        self.lines.push(crate::text::clean_line(&strip_ansi(&text)));
    }

    /// Stop the command (and everything it started).
    pub fn kill(&mut self) {
        if let Some(c) = self.child.lock().unwrap().as_mut() {
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                let _ = Command::new("taskkill").args(["/T", "/F", "/PID", &c.id().to_string()]).creation_flags(0x08000000)
                    .stdout(Stdio::null()).stderr(Stdio::null()).status();
            }
            let _ = c.kill();
        }
        self.killed = true;
    }
}

impl Drop for CmdRun {
    fn drop(&mut self) { if self.running() { self.kill(); } }
}

/// Bytes in the OEM code page (what most console programs write when their output is piped).
fn oem_decode(b: &[u8]) -> String {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Globalization::{MultiByteToWideChar, MULTI_BYTE_TO_WIDE_CHAR_FLAGS};
        let n = MultiByteToWideChar(1 /* CP_OEMCP */, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), b, None);
        if n > 0 {
            let mut w = vec![0u16; n as usize];
            let n = MultiByteToWideChar(1, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), b, Some(&mut w));
            if n > 0 { return String::from_utf16_lossy(&w[..n as usize]); }
        }
    }
    encoding_rs::WINDOWS_1252.decode(b).0.into_owned()
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\x1b' {
            if it.peek() == Some(&'[') {
                it.next();
                while let Some(&n) = it.peek() { it.next(); if ('@'..='~').contains(&n) { break; } }
            } else if it.peek() == Some(&']') {
                while let Some(n) = it.next() { if n == '\x07' { break; } }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// One command line run by cmd.exe (UTF-8 output), or sh elsewhere.
fn shell_command(cmd: &str) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd");
        c.args(["/d", "/s", "/c"]).raw_arg(format!("\"chcp 65001 >nul & {cmd}\""));
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    }
}

/// "cd folder", "cd /d D:\x", "D:" -> the folder to open in the browser (None = not a folder change).
pub fn cd_target(cmd: &str, cwd: Option<&Path>) -> Option<std::path::PathBuf> {
    let t = cmd.trim();
    let low = t.to_lowercase();
    let arg = if low == "cd" || low == "chdir" { return cwd.map(|p| p.to_path_buf()) }
        else if let Some(r) = low.strip_prefix("cd ").or_else(|| low.strip_prefix("chdir ")) { t[t.len() - r.len()..].trim() }
        else if low.starts_with("cd..") || low.starts_with("cd\\") || low.starts_with("cd/") { t[2..].trim() }
        else if t.len() == 2 && t.ends_with(':') && t.chars().next().map(|c| c.is_ascii_alphabetic()).unwrap_or(false) { return Some(format!("{t}\\").into()) }
        else { return None };
    let arg = arg.strip_prefix("/d ").or_else(|| arg.strip_prefix("/D ")).unwrap_or(arg).trim().trim_matches('"');
    if arg.is_empty() { return cwd.map(|p| p.to_path_buf()); }
    let p = Path::new(arg);
    let full = if p.is_absolute() || (arg.len() >= 2 && arg.as_bytes()[1] == b':') { p.to_path_buf() } else { cwd?.join(p) };
    let full = std::fs::canonicalize(&full).ok()?;
    let s = full.to_string_lossy();
    Some(s.strip_prefix(r"\\?\").map(std::path::PathBuf::from).unwrap_or(full))
}

/// Full interactive Command Prompt in `dir` (the caller has left the full-screen UI).
pub fn interactive(dir: &Path) {
    println!("\n  Command Prompt in {}  -  type  exit  to go back to the file browser\n", dir.display());
    #[cfg(windows)]
    let mut c = { use std::os::windows::process::CommandExt; let mut c = Command::new("cmd"); c.arg("/k").raw_arg("chcp 65001 >nul"); c.env("PYTHONUTF8", "1"); c };
    #[cfg(not(windows))]
    let mut c = Command::new(std::env::var("SHELL").unwrap_or_else(|_| "sh".into()));
    let _ = c.current_dir(dir).status();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cd() {
        let d = std::env::temp_dir();
        assert_eq!(cd_target("dir", Some(&d)), None);
        assert!(cd_target("cd ..", Some(&d)).is_some());
        assert!(cd_target("cd..", Some(&d)).is_some());
        assert_eq!(strip_ansi("\x1b[31mred\x1b[0m"), "red");
    }
}
