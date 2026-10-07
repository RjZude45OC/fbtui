//! Optional helper programs: PDFium (PDF pages) and FFmpeg (video / audio / extra picture formats).
//! Looked for in <exe folder>\deps, <exe folder>\deps\ffmpeg, next to the exe, then on PATH.
//! Everything still works without them - the previews just fall back to Windows thumbnails.

use pdfium_render::prelude::*;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_default()
}

pub fn search_dirs() -> Vec<PathBuf> {
    let e = exe_dir();
    let d = e.join("deps");
    vec![d.clone(), d.join("ffmpeg"), d.join("ffmpeg").join("bin"), d.join("pdfium").join("bin"), d.join("pdfium").join("lib"), e]
}

fn find_tool(name: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    for d in search_dirs() {
        let p = d.join(&file);
        if p.is_file() { return Some(p); }
    }
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join(&file)).find(|p| p.is_file()))
}

pub fn ffmpeg() -> Option<&'static Path> { static F: OnceLock<Option<PathBuf>> = OnceLock::new(); F.get_or_init(|| find_tool("ffmpeg")).as_deref() }
pub fn ffprobe() -> Option<&'static Path> { static F: OnceLock<Option<PathBuf>> = OnceLock::new(); F.get_or_init(|| find_tool("ffprobe")).as_deref() }
pub fn ffplay() -> Option<&'static Path> { static F: OnceLock<Option<PathBuf>> = OnceLock::new(); F.get_or_init(|| find_tool("ffplay")).as_deref() }

/// PDFium is not thread-safe: one shared instance behind a lock.
pub fn pdfium() -> Option<&'static Mutex<Pdfium>> {
    static P: OnceLock<Option<Mutex<Pdfium>>> = OnceLock::new();
    P.get_or_init(|| {
        for d in search_dirs() {
            let lib = Pdfium::pdfium_platform_library_name_at_path(&d);
            if lib.is_file() {
                if let Ok(b) = Pdfium::bind_to_library(&lib) { return Some(Mutex::new(Pdfium::new(b))); }
            }
        }
        Pdfium::bind_to_system_library().ok().map(|b| Mutex::new(Pdfium::new(b)))
    }).as_ref()
}

/// One line for the header / help: which helpers were found.
pub fn status() -> String {
    let yes = |b: bool| if b { "yes" } else { "missing" };
    format!("PDFium: {}  ·  FFmpeg: {}", yes(pdfium().is_some()), yes(ffmpeg().is_some() && ffprobe().is_some()))
}

/// A console-less process (no black window flashing on Windows).
pub fn command(p: &Path) -> Command {
    let mut c = Command::new(p);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000);
    }
    c.stdin(Stdio::null());
    c
}

/// Run and collect stdout; the process is killed after `timeout`.
pub fn run_capture(mut c: Command, timeout: Duration) -> Option<Vec<u8>> {
    c.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = c.spawn().ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = std::io::Read::read_to_end(&mut out, &mut b);
        b
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => break,
            Ok(None) => {}
        }
        if start.elapsed() > timeout { let _ = child.kill(); let _ = child.wait(); break; }
        std::thread::sleep(Duration::from_millis(5));
    }
    reader.join().ok().filter(|b| !b.is_empty())
}
