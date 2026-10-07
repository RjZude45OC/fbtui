//! Folder listing, drives and small file helpers.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_drive: bool,
    pub hidden: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// search hit: 1-based line (0 = not a hit)
    pub line: usize,
    /// text shown instead of the name (search hits, bookmarks)
    pub label: Option<String>,
    /// bookmark number (0 = none)
    pub slot: u32,
}

impl Entry {
    pub fn display(&self) -> String {
        if let Some(l) = &self.label { return l.clone(); }
        if self.is_dir && !self.is_drive { format!("{}\\", self.name) } else { self.name.clone() }
    }
}

#[cfg(windows)]
fn is_hidden(md: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    md.file_attributes() & (0x2 | 0x4) != 0 // hidden | system
}
#[cfg(not(windows))]
fn is_hidden(_md: &std::fs::Metadata) -> bool { false }

pub fn list_dir(dir: &Path, show_hidden: bool) -> std::io::Result<Vec<Entry>> {
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for de in std::fs::read_dir(dir)? {
        let de = match de { Ok(d) => d, Err(_) => continue };
        let name = de.file_name().to_string_lossy().into_owned();
        let md = match de.metadata() { Ok(m) => m, Err(_) => continue };
        let hidden = is_hidden(&md) || (cfg!(not(windows)) && name.starts_with('.'));
        if hidden && !show_hidden { continue; }
        let is_dir = md.is_dir();
        let e = Entry { name, path: de.path(), is_dir, is_drive: false, hidden, size: if is_dir { 0 } else { md.len() }, modified: md.modified().ok(), line: 0, label: None, slot: 0 };
        if is_dir { dirs.push(e) } else { files.push(e) }
    }
    let key = |e: &Entry| e.name.to_lowercase();
    dirs.sort_by_key(key);
    files.sort_by_key(key);
    dirs.extend(files);
    Ok(dirs)
}

/// "This PC": the drive letters that exist.
pub fn list_drives() -> Vec<Entry> {
    let mut v = Vec::new();
    #[cfg(windows)]
    for c in b'A'..=b'Z' {
        let root = format!("{}:\\", c as char);
        let p = PathBuf::from(&root);
        if p.is_dir() {
            v.push(Entry { name: root.clone(), path: p, is_dir: true, is_drive: true, hidden: false, size: 0, modified: None, line: 0, label: None, slot: 0 });
        }
    }
    #[cfg(not(windows))]
    v.push(Entry { name: "/".into(), path: PathBuf::from("/"), is_dir: true, is_drive: true, hidden: false, size: 0, modified: None, line: 0, label: None, slot: 0 });
    v
}

pub fn fmt_time(t: Option<SystemTime>) -> String {
    match t {
        Some(t) => chrono::DateTime::<chrono::Local>::from(t).format("%Y-%m-%d %H:%M").to_string(),
        None => String::new(),
    }
}

pub fn ext_of(p: &Path) -> String {
    p.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default()
}

/// A file entry for a path (search hits, bookmarks).
pub fn entry_for(p: &Path) -> Entry {
    let md = std::fs::metadata(p).ok();
    Entry {
        name: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.display().to_string()),
        path: p.to_path_buf(),
        is_dir: md.as_ref().map(|m| m.is_dir()).unwrap_or(false),
        is_drive: false,
        hidden: false,
        size: md.as_ref().map(|m| if m.is_dir() { 0 } else { m.len() }).unwrap_or(0),
        modified: md.and_then(|m| m.modified().ok()),
        line: 0, label: None, slot: 0,
    }
}
