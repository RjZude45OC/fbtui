//! File operations: rename, new file / folder, delete (Recycle Bin or permanent), copy / move / paste.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};

pub fn valid_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name == "." || name == ".." { return Err("Empty name".into()); }
    if name.chars().any(|c| matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || (c as u32) < 32) {
        return Err("A name cannot contain  \\ / : * ? \" < > |".into());
    }
    Ok(())
}

/// dir\name, or "name (2).ext", "name (3).ext" ... if it exists.
pub fn unique_path(dir: &Path, name: &str, is_dir: bool) -> Option<PathBuf> {
    let p = dir.join(name);
    if !p.exists() { return Some(p); }
    let (base, ext) = if is_dir { (name.to_string(), String::new()) } else {
        let pp = Path::new(name);
        match (pp.file_stem(), pp.extension()) {
            (Some(s), Some(e)) => (s.to_string_lossy().to_string(), format!(".{}", e.to_string_lossy())),
            _ => (name.to_string(), String::new()),
        }
    };
    (2..10000).map(|i| dir.join(format!("{base} ({i}){ext}"))).find(|p| !p.exists())
}

pub fn rename(from: &Path, new_name: &str) -> Result<PathBuf, String> {
    let dir = from.parent().ok_or("No parent folder")?;
    let dest = dir.join(new_name);
    let old = from.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if old.eq_ignore_ascii_case(new_name) {
        // only the letter case changes: go via a temporary name
        let tmp = dir.join(format!("~ren_{}", std::process::id()));
        std::fs::rename(from, &tmp).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())?;
    } else {
        if dest.exists() { return Err(format!("'{new_name}' already exists")); }
        std::fs::rename(from, &dest).map_err(|e| e.to_string())?;
    }
    Ok(dest)
}

pub fn delete(p: &Path, permanent: bool) -> Result<(), String> {
    if !permanent { return crate::winapi::recycle(p); }
    let md = std::fs::symlink_metadata(p).map_err(|e| e.to_string())?;
    if md.is_dir() { std::fs::remove_dir_all(p) } else { std::fs::remove_file(p) }.map_err(|e| e.to_string())
}

fn copy_rec(src: &Path, dest: &Path) -> std::io::Result<()> {
    let md = std::fs::metadata(src)?;
    if md.is_dir() {
        std::fs::create_dir_all(dest)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_rec(&e.path(), &dest.join(e.file_name()))?;
        }
    } else {
        std::fs::copy(src, dest)?;
    }
    Ok(())
}

fn move_one(src: &Path, dest: &Path) -> std::io::Result<()> {
    match std::fs::rename(src, dest) {
        Ok(()) => Ok(()),
        Err(_) => {
            // another drive: copy, then remove the original
            copy_rec(src, dest)?;
            if std::fs::metadata(src)?.is_dir() { std::fs::remove_dir_all(src) } else { std::fs::remove_file(src) }
        }
    }
}

pub enum PasteMsg { Progress { i: usize, n: usize, name: String }, Done { done: usize, fails: Vec<String>, first: Option<PathBuf> } }

/// Copy or move `paths` into `dir` on a background thread.
pub fn paste(paths: Vec<PathBuf>, dir: PathBuf, mv: bool) -> Receiver<PasteMsg> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let (mut done, mut fails, mut first) = (0usize, Vec::new(), None);
        let n = paths.len();
        let dir_s = dir.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
        for (i, src) in paths.iter().enumerate() {
            let name = src.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            if !src.exists() { fails.push(format!("{} (no longer exists)", src.display())); continue; }
            let is_dir = src.is_dir();
            let parent = src.parent().map(|p| p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase()).unwrap_or_default();
            if mv && parent == dir_s { continue; }                         // moving into the same folder: nothing to do
            let src_s = src.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
            if is_dir && (dir_s == src_s || dir_s.starts_with(&format!("{src_s}\\")) || dir_s.starts_with(&format!("{src_s}/"))) {
                fails.push(format!("{name} (cannot put a folder inside itself)")); continue;
            }
            let Some(dest) = unique_path(&dir, &name, is_dir) else { fails.push(format!("{name} (no free name)")); continue };
            let _ = tx.send(PasteMsg::Progress { i: i + 1, n, name: name.clone() });
            let r = if mv { move_one(src, &dest) } else { copy_rec(src, &dest) };
            match r {
                Ok(()) => { done += 1; if first.is_none() { first = Some(dest); } }
                Err(e) => fails.push(format!("{name} ({e})")),
            }
        }
        let _ = tx.send(PasteMsg::Done { done, fails, first });
    });
    rx
}
