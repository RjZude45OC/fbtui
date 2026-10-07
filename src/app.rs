//! Application state and input handling.

use crate::config::{Config, Theme};
use crate::editor::{self, Editor};
use crate::fileops::{self, PasteMsg};
use crate::fsutil::{entry_for, list_dir, list_drives, Entry};
use crate::preview::{job_key, Engine, Job};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui_image::picker::{Picker, ProtocolType};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone, PartialEq, Debug)]
pub enum View { Dir, Grep(String), Bookmarks }

/// A full-screen tool the main loop runs (it leaves the browser and comes back).
#[derive(Clone, Debug)]
pub enum ToolReq { Tasks, TaskLink(PathBuf), Chat, Calendar, Heatmap, Typing, Themes, Gallery(PathBuf, Option<PathBuf>), Git(PathBuf), Diff(PathBuf, PathBuf) }

#[derive(Clone, Debug)]
pub enum PromptKind { Rename(PathBuf), NewFile, NewFolder, Grep, EdFind, EdGoto, Cmd }

/// One-line input on the status row (rename, new file, find ...): Enter = OK, Esc = cancel.
pub struct Prompt { pub kind: PromptKind, pub label: String, pub text: String, pub cursor: usize, pub fresh: bool }

pub enum ConfirmAction { Delete { targets: Vec<PathBuf>, permanent: bool } }
pub struct Confirm { pub question: String, pub action: ConfirmAction }

/// Text files open in the editor; pictures, archives, programs ... do not.
pub fn is_text_file(e: &Entry) -> bool {
    if e.is_dir { return false; }
    if e.size == 0 { return true; }
    let ext = crate::fsutil::ext_of(&e.path);
    if crate::preview::OFFICE_EXTS.contains(&ext.as_str()) || crate::preview::ZIP_EXTS.contains(&ext.as_str())
        || crate::preview::EXE_EXTS.contains(&ext.as_str()) || ext == ".lnk" { return false; }
    let mut b = vec![0u8; 8192];
    let n = std::fs::File::open(&e.path).and_then(|mut f| std::io::Read::read(&mut f, &mut b)).unwrap_or(0);
    n > 0 && !crate::text::looks_binary(&b[..n])
}

fn target_label(t: &[PathBuf]) -> String {
    if t.len() == 1 { format!("'{}'", t[0].file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()) } else { format!("{} items", t.len()) }
}

pub struct App {
    pub cfg: Config,
    pub theme: Arc<Theme>,
    pub picker: Option<Picker>,
    pub sixel_capable: bool,
    pub cell_px: (u16, u16),
    pub engine: Engine,

    pub cwd: Option<PathBuf>,          // None = drive list ("This PC")
    pub all: Vec<Entry>,
    pub entries: Vec<Entry>,
    pub filter: String,
    pub sel: usize,
    pub top: usize,
    pub show_hidden: bool,
    memory: HashMap<Option<PathBuf>, PathBuf>,

    pub status: String,
    pub status_err: bool,
    pub show_help: bool,
    pub pv_scroll: usize,
    pv_for: Option<PathBuf>,
    pv_for_line: usize,
    last_request: String,

    // from the last draw, for the mouse
    pub list_area: Rect,
    pub preview_area: Rect,
    pub tasks_button: Rect,                       // header button that opens Tasks & clock
    pub tools_button: Rect,                       // header button that opens the Tools window
    pub tools: Option<crate::tools::Window>,      // F10: the Tools window
    pub open_tool: Option<ToolReq>,               // main loop: run this full-screen tool
    pub diff_left: Option<PathBuf>,               // Alt+D pressed once: the first side to compare
    pub tasks_info: Option<String>,               // "2 late · 3 today"
    pub visible_rows: usize,
    last_click: Option<(usize, Instant)>,
    pub quit: bool,
    pub pick: Option<PathBuf>,                    // --pick: choosing a file / folder for a task; the choice is written to this file
    pub viewer: Option<crate::viewer::Viewer>,

    pub view: View,
    pub marks: HashSet<PathBuf>,
    pub clip: Option<(Vec<PathBuf>, bool)>,       // files from Ctrl+C / Ctrl+X, true = cut
    pub editor: Option<Editor>,
    pub prompt: Option<Prompt>,
    pub confirm: Option<Confirm>,
    pub search: Option<crate::grep::Search>,
    grep_query: String,
    pub search_opts: crate::grep::Opts,
    pub results: Vec<Entry>,                      // the last search's results (kept when you leave the list)
    pub results_title: String,
    results_sel: usize,
    search_matcher: Option<crate::grep::Matcher>,
    last_find: String,
    paste_rx: Option<Receiver<PasteMsg>>,
    paste_move: bool,
    paste_into: String,
    pub input_pending: bool,                      // more keys already queued (a paste is arriving)
    drag_editor: bool,
    pub editor_area: Rect,                        // text area of the editor from the last draw
    pub egg: Option<crate::egg::Egg>,
    pub cmd_run: Option<crate::shell::CmdRun>,
    cmd_history: Vec<String>,
    cmd_hist_pos: usize,
    pub open_shell: bool,                         // main loop: leave the UI for a full Command Prompt
    pub open_tasks: bool,                         // main loop: show the Tasks & clock screen
    pub spotlight: Option<crate::spotlight::Spotlight>,
    typed: String,
    pub clear_screen: bool,                       // wipe the terminal before the next draw (pictures left behind)
}

impl App {
    pub fn new(start: Option<PathBuf>) -> App {
        let cfg = Config::load();
        let theme = Arc::new(cfg.theme.clone());
        let mut app = App {
            theme, cfg, picker: None, sixel_capable: false, cell_px: (10, 20), engine: Engine::new(3),
            cwd: None, all: Vec::new(), entries: Vec::new(), filter: String::new(), sel: 0, top: 0,
            show_hidden: false, memory: HashMap::new(), status: String::new(), status_err: false, show_help: false,
            pv_scroll: 0, pv_for: None, pv_for_line: 0, last_request: String::new(),
            list_area: Rect::default(), preview_area: Rect::default(), visible_rows: 10, last_click: None, quit: false, pick: None, viewer: None,
            view: View::Dir, marks: HashSet::new(), clip: None, editor: None, prompt: None, confirm: None, search: None,
            grep_query: String::new(), search_opts: crate::grep::Opts { what: crate::grep::What::Both, regex: false, case: false, global: false },
            results: Vec::new(), results_title: String::new(), results_sel: 0, search_matcher: None, last_find: String::new(), paste_rx: None, paste_move: false, paste_into: String::new(), input_pending: false,
            drag_editor: false, editor_area: Rect::default(), egg: None, clear_screen: false, cmd_run: None, cmd_history: Vec::new(), cmd_hist_pos: 0, open_shell: false, open_tasks: false, tasks_button: Rect::default(), tools_button: Rect::default(), tools: None, open_tool: None, diff_left: None, tasks_info: crate::planner::summary(), spotlight: None, typed: String::new(),
        };
        if let Some(w) = app.cfg.warning.clone() { app.set_status(&w, true); }
        app.load_search_opts();
        let start = start.or_else(|| app.cfg.start_dir()).filter(|p| p.is_dir())
            .or_else(|| std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from));
        app.load(start);
        app
    }

    /// Choose how pictures are drawn: [preview] images = auto | sixel | blocks
    /// Like the PowerShell version, ask the terminal itself: DA1 (ESC [ c) lists "4" when it can draw
    /// Sixel, and ESC [ 16 t reports the real cell size in pixels, so pictures are drawn 1:1 (sharp)
    /// instead of being stretched by the terminal.
    pub fn setup_picker(&mut self) {
        let (p, cell) = build_picker(&self.cfg.image_mode());
        let mut p = p;
        let (r, g, b) = self.theme.bg_rgb;
        p.set_background_color(Some(image::Rgba([r, g, b, 255])));
        self.sixel_capable = p.protocol_type() != ProtocolType::Halfblocks;
        self.cell_px = if self.sixel_capable { cell } else { (1, 2) };
        self.picker = Some(p);
    }

    /// Open a tool (if it is turned on in the Tools window).
    pub fn start_tool(&mut self, tool: crate::tools::Tool) {
        use crate::tools::Tool;
        if tool == crate::tools::Tool::Chat && !crate::chat::available(&self.cfg) { self.set_status(crate::chat::NOT_INSTALLED, true); return; }
        if !crate::tools::enabled(&self.cfg, tool) {
            self.set_status(&format!("{} is off  ·  turn it on in the tools list (Esc)", crate::tools::feature(tool).name), true);
            return;
        }
        let cur = self.current().cloned();
        self.open_tool = match tool {
            Tool::Tasks => Some(ToolReq::Tasks),
            Tool::Heatmap => Some(ToolReq::Heatmap),
            Tool::Calendar => Some(ToolReq::Calendar),
            Tool::Typing => Some(ToolReq::Typing),
            Tool::Themes => Some(ToolReq::Themes),
            Tool::Chat => Some(ToolReq::Chat),
            Tool::Gallery => match &self.cwd {
                Some(d) => Some(ToolReq::Gallery(d.clone(), cur.filter(|e| !e.is_dir).map(|e| e.path))),
                None => { self.set_status("Open a folder with pictures first", true); None }
            },
            Tool::Git => match &self.cwd { Some(d) => Some(ToolReq::Git(d.clone())), None => { self.set_status("Open a folder first", true); None } },
            Tool::Diff => {
                let marked = self.targets();
                if self.marks.len() == 2 && marked.len() == 2 {
                    self.diff_left = None;
                    Some(ToolReq::Diff(marked[0].clone(), marked[1].clone()))
                } else if let Some(cur) = cur.map(|e| e.path) {
                    match self.diff_left.take() {
                        Some(left) if left != cur => Some(ToolReq::Diff(left, cur)),
                        _ => {
                            let name = cur.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                            self.set_status(&format!("Compare: left = {name}  ·  now go to the other folder or file and press Alt+D again"), false);
                            self.diff_left = Some(cur);
                            None
                        }
                    }
                } else { self.set_status("Mark two folders or files with Space and press Alt+D", true); None }
            }
        };
    }

    pub fn set_status(&mut self, s: &str, err: bool) { self.status = s.to_string(); self.status_err = err; }

    pub fn load(&mut self, dir: Option<PathBuf>) -> bool {
        if matches!(self.view, View::Grep(_)) { self.results_sel = self.current().map(|c| self.results.iter().position(|r| r.path == c.path && r.line == c.line).unwrap_or(self.sel)).unwrap_or(0); }
        let list = match &dir {
            None => list_drives(),
            Some(d) => match list_dir(d, self.show_hidden) {
                Ok(v) => v,
                Err(e) => { self.set_status(&format!("Cannot open: {e}"), true); return false; }
            },
        };
        if dir != self.cwd { self.marks.clear(); }
        self.cwd = dir;
        self.view = View::Dir;
        self.all = list;
        self.filter.clear();
        self.entries = self.all.clone();
        self.sel = 0;
        if let Some(want) = self.memory.get(&self.cwd) {
            if let Some(i) = self.entries.iter().position(|e| &e.path == want) { self.sel = i; }
        }
        self.top = self.sel.saturating_sub(self.visible_rows / 2);
        true
    }

    fn remember(&mut self) {
        if let Some(e) = self.current() { let p = e.path.clone(); self.memory.insert(self.cwd.clone(), p); }
    }

    pub fn reload(&mut self) {
        if self.view != View::Dir { return; }
        self.remember();
        let f = self.filter.clone();
        let cwd = self.cwd.clone();
        if self.load(cwd) && !f.is_empty() { self.filter = f; self.apply_filter(); }
    }

    pub fn current(&self) -> Option<&Entry> { self.entries.get(self.sel) }

    /// Tools listed in Spotlight (none while picking a file for a task).
    fn spot_tools(&self) -> Vec<crate::tools::Tool> { if self.pick.is_some() { Vec::new() } else { crate::tools::on_list(&self.cfg) } }

    /// Pick mode: hand the chosen file / folder back to Tasks and close.
    fn pick_path(&mut self, p: &std::path::Path) {
        if let Some(out) = &self.pick { let _ = std::fs::write(out, p.to_string_lossy().as_bytes()); }
        self.quit = true;
    }

    fn apply_filter(&mut self) {
        let keep = self.current().map(|e| e.path.clone());
        let q = self.filter.to_lowercase();
        self.entries = if q.is_empty() { self.all.clone() } else {
            self.all.iter().filter(|e| e.display().to_lowercase().contains(&q)).cloned().collect()
        };
        self.sel = keep.and_then(|k| self.entries.iter().position(|e| e.path == k)).unwrap_or(0);
        self.top = 0;
    }

    pub fn move_to(&mut self, i: isize) {
        if self.entries.is_empty() { return; }
        self.sel = i.clamp(0, self.entries.len() as isize - 1) as usize;
    }

    pub fn fix_scroll(&mut self) {
        let v = self.visible_rows.max(1);
        if self.sel < self.top { self.top = self.sel; }
        if self.sel >= self.top + v { self.top = self.sel + 1 - v; }
        let n = self.entries.len();
        if n > v && self.top > n - v { self.top = n - v; }
        if n <= v { self.top = 0; }
    }

    fn enter(&mut self, e: &Entry) {
        if self.view == View::Dir { self.remember(); }
        self.load(Some(e.path.clone()));
    }

    /// Back from search results / bookmarks to the folder.
    fn leave_view(&mut self) { let c = self.cwd.clone(); self.load(c); }

    fn up(&mut self) {
        if self.view != View::Dir { self.leave_view(); return; }
        let Some(cwd) = self.cwd.clone() else { return };
        let parent = cwd.parent().map(|p| p.to_path_buf()).filter(|p| p != &cwd);
        self.memory.insert(parent.clone(), cwd);
        self.load(parent);
    }

    fn open(&mut self) {
        let Some(e) = self.current().cloned() else { return };
        if e.is_dir { self.enter(&e); return; }
        if e.line > 0 { self.open_editor(&e); return; }         // search hit: edit at that line
        match crate::winapi::open_default(&e.path) {
            Ok(_) => self.set_status(&format!("Opened: {}", e.name), false),
            Err(err) => self.set_status(&format!("Cannot open: {err}"), true),
        }
    }

    pub fn apply_theme(&mut self) {
        self.theme = Arc::new(self.cfg.theme.clone());
        if let Some(ed) = &mut self.editor { ed.hl.set_theme(&self.theme); ed.wrap = self.cfg.editor_wrap(); }
        if let Some(p) = &mut self.picker {
            let (r, g, b) = self.theme.bg_rgb;
            p.set_background_color(Some(image::Rgba([r, g, b, 255])));
        }
        self.engine.clear();
        self.last_request.clear();
    }

    pub fn check_config(&mut self) -> bool {
        if !self.cfg.changed() { return false; }
        let old_mode = self.cfg.image_mode();
        self.cfg.reload();
        self.apply_theme();
        if self.cfg.image_mode() != old_mode { self.setup_picker(); }
        match self.cfg.warning.clone() { Some(w) => self.set_status(&w, true), None => self.set_status(&format!("Settings reloaded  ·  {}", self.cfg.label()), false) }
        true
    }

    /// Width of the file list: 48%, or (100 - split)% while editing.
    pub fn list_width(&self, w: u16) -> u16 {
        if w < 100 { return w; }
        if self.editor.is_some() { return (w as u32 * (100 - self.cfg.editor_split()) as u32 / 100) as u16; }
        (w as f32 * 0.48) as u16
    }

    /// Preview size in cells: (columns, rows below the title/meta lines)
    pub fn preview_dims(&self, w: u16, h: u16) -> (u16, u16) {
        let list_w = (w as f32 * 0.48) as u16;
        (w.saturating_sub(list_w + 2), h.saturating_sub(9 + 3))
    }

    fn job_for(&self, e: &Entry, cols: u16, rows: u16) -> Job {
        Job {
            key: job_key(&e.path, e.modified, cols, rows, &self.theme.name, self.show_hidden),
            path: e.path.clone(), is_dir: e.is_dir, is_drive: e.is_drive, size: e.size, modified: e.modified,
            cols, rows, show_hidden: self.show_hidden, theme: self.theme.clone(), picker: self.picker.clone(),
        }
    }

    pub fn current_key(&self, w: u16, h: u16) -> Option<String> {
        let (c, r) = self.preview_dims(w, h);
        self.current().map(|e| job_key(&e.path, e.modified, c, r, &self.theme.name, self.show_hidden))
    }

    /// Queue the selected item and its neighbours (preload) for the background workers.
    pub fn request_previews(&mut self, w: u16, h: u16) {
        if w < 100 || self.show_help || self.editor.is_some() { return; }
        let (c, r) = self.preview_dims(w, h);
        let Some(cur) = self.current().cloned() else { return };
        if self.pv_for.as_ref() != Some(&cur.path) || (cur.line > 0 && self.pv_for_line != cur.line) {
            self.pv_for = Some(cur.path.clone());
            self.pv_for_line = cur.line;
            self.pv_scroll = cur.line.saturating_sub(5);          // search hit: show the line
        }
        let key = job_key(&cur.path, cur.modified, c, r, &self.theme.name, self.show_hidden);
        if key == self.last_request { return; }
        self.last_request = key;
        let mut jobs = vec![self.job_for(&cur, c, r)];
        for d in [1isize, -1, 2, -2] {
            let i = self.sel as isize + d;
            if i >= 0 && (i as usize) < self.entries.len() { jobs.push(self.job_for(&self.entries[i as usize].clone(), c, r)); }
        }
        self.engine.request(jobs);
    }

    pub fn on_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl_only = ctrl && !alt;                       // AltGr = Ctrl+Alt counts as typing
        if let Some(egg) = &self.egg {
            if egg.age() > 0.5 { self.egg = None; self.clear_screen = true; self.last_request.clear(); self.engine.clear(); }
            return;
        }
        if let Some(w) = &mut self.tools {
            match w.on_key(k, &mut self.cfg) {
                crate::tools::Act::None => {}
                crate::tools::Act::Close => self.tools = None,
                crate::tools::Act::Changed(m) => self.set_status(&m, false),
                crate::tools::Act::Open(tool) => { self.tools = None; self.start_tool(tool); }
            }
            return;
        }
        let tab = k.code == KeyCode::Tab && !k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT) && self.editor.is_none() && self.viewer.is_none();
        if k.code == KeyCode::F(10) && self.prompt.is_none() && self.confirm.is_none() && self.spotlight.is_none() { self.tools = Some(crate::tools::Window::new()); return; }
        // Tab: Spotlight search over the tools that are on and the bookmarks (Tab again closes it)
        if tab && self.prompt.is_none() && self.confirm.is_none() && self.spotlight.is_none() {
            self.spotlight = Some(crate::spotlight::Spotlight::new(&self.cfg.bookmarks(), if self.pick.is_some() { "cancel" } else { "close the file browser" }, &self.spot_tools()));
            return;
        }
        if let Some(sp) = &mut self.spotlight {
            match sp.on_key(k) {
                crate::spotlight::Act::None => {}
                crate::spotlight::Act::Close => { self.spotlight = None; self.clear_screen = true; }
                crate::spotlight::Act::Tool(t) => { self.spotlight = None; self.clear_screen = true; self.start_tool(t); }
                crate::spotlight::Act::Open(p) | crate::spotlight::Act::Reveal(p) if self.pick.is_some() => { self.spotlight = None; self.pick_path(&p); }
                crate::spotlight::Act::Open(p) => { self.spotlight = None; self.viewer = None; self.open_path(&p); }
                crate::spotlight::Act::Reveal(p) => { self.spotlight = None; self.viewer = None; self.reveal_path(&p); }
                crate::spotlight::Act::Quit => { self.spotlight = None; self.quit = true; }
                crate::spotlight::Act::Goto(p) => {
                    self.spotlight = None; self.viewer = None;
                    if p.is_dir() { self.open_path(&p) } else { self.reveal_path(&p) }
                }
                crate::spotlight::Act::Remove(slot) => {
                    match self.cfg.set_value("bookmarks", &slot.to_string(), None) {
                        Ok(()) => { self.cfg.reload(); self.set_status(&format!("Bookmark {slot} removed"), false); if self.view == View::Bookmarks { self.show_bookmarks(); } }
                        Err(e) => self.set_status(&format!("Could not save config.ini: {e}"), true),
                    }
                }
            }
            return;
        }
        // Ctrl+Space / Ctrl+K: quick open over the bookmarks (works everywhere except while typing in a prompt)
        if self.prompt.is_none() && self.confirm.is_none() && (k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT))
            && matches!(k.code, KeyCode::Char(' ') | KeyCode::Char('k') | KeyCode::Char('K') | KeyCode::Char('@')) {
            if let Some(ed) = &self.editor { if ed.dirty { self.set_status("Unsaved changes - Ctrl+S first", true); return; } }
            self.spotlight = Some(crate::spotlight::Spotlight::new(&self.cfg.bookmarks(), if self.pick.is_some() { "cancel" } else { "close the file browser" }, &self.spot_tools()));
            return;
        }
        // picking a file / folder for a task: Enter takes the one under the cursor, Esc gives up
        if self.pick.is_some() && self.prompt.is_none() && self.confirm.is_none() && self.editor.is_none() && self.viewer.is_none() && self.cmd_run.is_none() {
            match k.code {
                KeyCode::Enter if !alt && !ctrl => {
                    let cur = self.current().filter(|e| e.name != "..").map(|e| e.path.clone()).or_else(|| self.cwd.clone());
                    if let Some(p) = cur { self.pick_path(&p); }
                    return;
                }
                KeyCode::Esc if self.filter.is_empty() && self.marks.is_empty() && self.view == View::Dir && !self.show_help => { self.quit = true; return; }
                _ => {}
            }
        }
        if let Some(c) = self.confirm.take() { self.confirm_key(k, c); return; }
        if self.prompt.is_some() { self.prompt_key(k); return; }
        if self.search.is_some() && k.code == KeyCode::Esc {
            if let Some(s) = &self.search { s.stop.store(true, std::sync::atomic::Ordering::Relaxed); }
            self.set_status("Search stopped", false);
            return;
        }
        let had_status = !self.status.is_empty();
        self.status.clear();
        if self.viewer.is_some() { self.viewer_key(k); return; }

        // global keys
        match k.code {
            KeyCode::Tab | KeyCode::BackTab if ctrl => { self.cycle_theme(if shift || k.code == KeyCode::BackTab { -1 } else { 1 }); return; }
            KeyCode::F(9) => { self.cycle_theme(if shift { -1 } else { 1 }); return; }
            KeyCode::F(1) => { self.show_help = !self.show_help; return; }
            _ => {}
        }

        // the editor gets the keys first
        if let Some(ed) = &mut self.editor {
            let out = ed.on_key(k, self.input_pending);
            if let Some((m, e)) = ed.msg.take() { self.status = m; self.status_err = e; }
            match out {
                editor::Out::Handled => {
                    if matches!(k.code, KeyCode::Char('s') | KeyCode::Char('S')) && ctrl_only && !ed.dirty {
                        self.reload();                  // saved: refresh size / date in the list
                        if let Some((m, e)) = self.editor.as_mut().and_then(|e| e.msg.take()) { self.status = m; self.status_err = e; }
                    }
                    return;
                }
                editor::Out::Close => { self.editor = None; self.last_request.clear(); self.engine.clear(); return; }
                editor::Out::AskFind(init) => { self.ask(PromptKind::EdFind, "Find:", &init); return; }
                editor::Out::AskGoto => { let n = ed.lines.len(); self.ask(PromptKind::EdGoto, &format!("Go to line (1-{n}):"), ""); return; }
                editor::Out::NotHandled => {
                    // while editing only these list keys still work
                    if k.code == KeyCode::F(6) { self.next_result(if shift { -1 } else { 1 }); return; }
                    let allowed = matches!(k.code, KeyCode::F(5)) || (alt && !ctrl && matches!(k.code, KeyCode::Char('z') | KeyCode::Char('Z')))
                        || (ctrl_only && matches!(k.code, KeyCode::Char('p') | KeyCode::Char('P') | KeyCode::Char('o') | KeyCode::Char('O') | KeyCode::Char('h') | KeyCode::Char('H')));
                    if !allowed { return; }
                }
            }
        }
        if alt && !ctrl && matches!(k.code, KeyCode::Char('z') | KeyCode::Char('Z')) { self.toggle_wrap(); return; }
        // Alt+V: paste files (Windows Terminal keeps Ctrl+V for itself)
        if alt && !ctrl && matches!(k.code, KeyCode::Char('v') | KeyCode::Char('V')) { self.paste_here(); return; }

        let page = self.visible_rows.max(1) as isize;
        let cur = self.current().cloned();
        // command output in the preview pane: Esc stops / closes it, Ctrl+Up/Down scroll it
        if let Some(r) = &mut self.cmd_run {
            match k.code {
                KeyCode::Esc => {
                    if r.running() { r.kill(); self.set_status("Command stopped", true); } else { self.cmd_run = None; self.last_request.clear(); }
                    return;
                }
                KeyCode::Up if ctrl_only => { r.follow = false; r.scroll = r.scroll.saturating_sub(1); return; }
                KeyCode::Down if ctrl_only => { r.scroll += 1; return; }
                _ => {}
            }
        }
        if k.code == KeyCode::F(8) || (alt && !ctrl && matches!(k.code, KeyCode::Char('t') | KeyCode::Char('T'))) { self.start_tool(crate::tools::Tool::Tasks); return; }
        if alt && !ctrl {
            match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => { self.start_tool(crate::tools::Tool::Typing); return; }
                KeyCode::Char('p') | KeyCode::Char('P') => { self.start_tool(crate::tools::Tool::Themes); return; }
                KeyCode::Char('a') | KeyCode::Char('A') => { self.start_tool(crate::tools::Tool::Chat); return; }
                KeyCode::Char('l') | KeyCode::Char('L') => {
                    // a new task linked to the file or folder under the cursor (or this folder)
                    if !crate::tools::enabled(&self.cfg, crate::tools::Tool::Tasks) { self.set_status("Tasks & clock is off  ·  turn it on in the tools list (Esc)", true); return; }
                    let target = self.current().filter(|e| e.name != "..").map(|e| e.path.clone()).or_else(|| self.cwd.clone());
                    match target { Some(p) => self.open_tool = Some(ToolReq::TaskLink(p)), None => self.set_status("Nothing to link here", true) }
                    return;
                }
                KeyCode::Char('c') | KeyCode::Char('C') => { self.start_tool(crate::tools::Tool::Calendar); return; }
                KeyCode::Char('i') | KeyCode::Char('I') => { self.start_tool(crate::tools::Tool::Gallery); return; }
                KeyCode::Char('d') | KeyCode::Char('D') => { self.start_tool(crate::tools::Tool::Diff); return; }
                _ => {}
            }
        }
        if ctrl_only && matches!(k.code, KeyCode::Char('g') | KeyCode::Char('G')) { self.start_tool(crate::tools::Tool::Git); return; }
        if k.code == KeyCode::F(4) {
            if self.cwd.is_none() { self.set_status("Open a drive or folder first", true); return; }
            self.cmd_hist_pos = self.cmd_history.len();
            self.ask(PromptKind::Cmd, "cmd>", "");
            return;
        }
        if ctrl_only && matches!(k.code, KeyCode::Char('t') | KeyCode::Char('T')) {
            if self.cwd.is_none() { self.set_status("Open a drive or folder first", true); return; }
            self.open_shell = true;
            return;
        }
        if ctrl_only {
            match k.code {
                KeyCode::Up => { self.pv_scroll = self.pv_scroll.saturating_sub(1); }
                KeyCode::Down => { self.pv_scroll += 1; }
                KeyCode::Char(c) => match c.to_ascii_lowercase() {
                    'h' => { self.show_hidden = !self.show_hidden; self.reload(); self.set_status(if self.show_hidden { "Hidden files: shown" } else { "Hidden files: not shown" }, false); }
                    'o' => { let cur = cur.map(|e| e.path); crate::winapi::show_in_explorer(cur.as_deref(), self.cwd.as_deref()); self.set_status("Opened in Explorer", false); }
                    'p' => self.copy_path_text(),
                    'c' => self.copy_targets(false),
                    'x' => self.copy_targets(true),
                    'v' => self.paste_here(),
                    'n' => self.new_item(shift),
                    'f' => { let q = self.grep_query.clone(); self.ask(PromptKind::Grep, "Find:", &q) }
                    'd' => self.toggle_bookmark(),
                    'b' => self.show_bookmarks(),
                    'a' => self.select_all(),
                    d @ '1'..='9' => self.open_bookmark(d as u8 - b'0'),
                    _ => { if had_status { } }
                },
                _ => {}
            }
            return;
        }
        match k.code {
            KeyCode::F(5) => {
                self.cfg.reload(); self.apply_theme();
                match self.view.clone() {
                    View::Grep(_) => { let q = self.grep_query.clone(); self.start_search(&q); }
                    View::Bookmarks => self.show_bookmarks(),
                    View::Dir => { self.reload(); self.set_status(&format!("Refreshed  ·  {}", self.cfg.label()), false); }
                }
                if let Some(w) = self.cfg.warning.clone() { self.set_status(&w, true); }
            }
            KeyCode::Up => self.move_to(self.sel as isize - 1),
            KeyCode::Down => self.move_to(self.sel as isize + 1),
            KeyCode::PageUp => self.move_to(self.sel as isize - page),
            KeyCode::PageDown => self.move_to(self.sel as isize + page),
            KeyCode::Home => self.move_to(0),
            KeyCode::End => self.move_to(self.entries.len() as isize - 1),
            KeyCode::Right => {
                if let Some(e) = cur {
                    if e.is_dir { self.enter(&e) }
                    else if e.line > 0 { self.open_editor(&e) }
                    else if crate::media::kind_of(&e.path) != crate::media::Kind::None { self.open_viewer(&e.path.clone()); }
                    else if is_text_file(&e) { self.show_help = false; self.open_editor(&e) }
                    else { self.set_status("Preview only - press Enter to open it in its own app", false) }
                }
            }
            KeyCode::Enter => { self.show_help = false; self.open(); }
            KeyCode::F(3) if matches!(self.view, View::Grep(_)) => { let d = if shift { -1 } else { 1 }; self.step_result(d); }
            KeyCode::F(6) => self.next_result(if shift { -1 } else { 1 }),
            KeyCode::F(3) => { if let Some(e) = cur { if !e.is_dir { self.open_viewer(&e.path); } } }
            KeyCode::F(2) => self.rename_current(),
            KeyCode::F(7) => self.new_item(true),
            KeyCode::Insert => { self.switch_mark(cur.as_ref()); self.move_to(self.sel as isize + 1); }
            KeyCode::Delete => {
                if self.view == View::Bookmarks { self.remove_bookmark(cur.as_ref()) } else { self.remove_targets(shift) }
            }
            KeyCode::Left => self.up(),
            KeyCode::Backspace => { if self.filter.pop().is_some() { self.apply_filter() } else { self.up() } }
            KeyCode::Esc => {
                if self.show_help { self.show_help = false }
                else if !self.filter.is_empty() { self.filter.clear(); self.apply_filter(); }
                else if !self.marks.is_empty() { self.marks.clear(); self.set_status("Selection cleared", false); }
                else if self.view != View::Dir { self.leave_view(); }
                else { self.tools = Some(crate::tools::Window::new()); }   // the tools check-box list (Tab = Spotlight, type exit there to quit)
            }
            KeyCode::Char(' ') if self.filter.is_empty() && self.view == View::Dir => { self.switch_mark(cur.as_ref()); self.move_to(self.sel as isize + 1); }
            KeyCode::Char(c) => {
                self.filter.push(c);
                if crate::egg::typed(&mut self.typed, c) {
                    self.filter.clear();
                    self.egg = Some(crate::egg::Egg::new());
                    self.clear_screen = true;
                }
                self.apply_filter();
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ prompts

    fn ask(&mut self, kind: PromptKind, label: &str, text: &str) {
        let cursor = match &kind {
            PromptKind::Rename(p) if !p.is_dir() => text.rfind('.').filter(|&d| d > 0).map(|d| text[..d].chars().count()).unwrap_or(text.chars().count()),
            _ => text.chars().count(),
        };
        let fresh = matches!(kind, PromptKind::Grep) && !text.is_empty();     // the old search text is replaced by typing
        self.prompt = Some(Prompt { kind, label: label.to_string(), text: text.to_string(), cursor, fresh });
    }

    fn prompt_key(&mut self, k: KeyEvent) {
        let Some(p) = &mut self.prompt else { return };
        let ctrl_only = k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT);
        let n = p.text.chars().count();
        let bi = |s: &str, ci: usize| crate::textdoc::byte_at(s, ci);
        let alt_only = k.modifiers.contains(KeyModifiers::ALT) && !k.modifiers.contains(KeyModifiers::CONTROL);
        if matches!(p.kind, PromptKind::Grep) {
            let o = &mut self.search_opts;
            let handled = match k.code {
                KeyCode::Tab => { o.what = o.what.next(); true }
                KeyCode::Char(c) if alt_only => match c.to_ascii_lowercase() {
                    'r' => { o.regex = !o.regex; true }
                    'c' => { o.case = !o.case; true }
                    'g' => { o.global = !o.global; true }
                    'n' => { o.what = crate::grep::What::Names; true }
                    'f' => { o.what = crate::grep::What::Folders; true }
                    't' => { o.what = crate::grep::What::Text; true }
                    'b' => { o.what = crate::grep::What::Both; true }
                    _ => false,
                },
                _ => false,
            };
            if handled { return; }
        }
        let Some(p) = &mut self.prompt else { return };
        let was_fresh = std::mem::replace(&mut p.fresh, false);
        if was_fresh && matches!(k.code, KeyCode::Char(_)) && !ctrl_only && !alt_only { p.text.clear(); p.cursor = 0; }
        match k.code {
            KeyCode::Enter => { let p = self.prompt.take().unwrap(); self.submit_prompt(p); }
            KeyCode::Char('u') | KeyCode::Char('U') if ctrl_only => { p.text.clear(); p.cursor = 0; }
            KeyCode::Esc => { self.prompt = None; }
            KeyCode::Up | KeyCode::Down if matches!(p.kind, PromptKind::Cmd) && !self.cmd_history.is_empty() => {
                let n = self.cmd_history.len();
                if k.code == KeyCode::Up { self.cmd_hist_pos = self.cmd_hist_pos.saturating_sub(1); } else { self.cmd_hist_pos = (self.cmd_hist_pos + 1).min(n); }
                p.text = self.cmd_history.get(self.cmd_hist_pos).cloned().unwrap_or_default();
                p.cursor = p.text.chars().count();
            }
            KeyCode::Left => { if p.cursor > 0 { p.cursor -= 1; } }
            KeyCode::Right => { if p.cursor < n { p.cursor += 1; } }
            KeyCode::Home => p.cursor = 0,
            KeyCode::End => p.cursor = n,
            KeyCode::Backspace => { if p.cursor > 0 { let b = bi(&p.text, p.cursor - 1); let e = bi(&p.text, p.cursor); p.text.replace_range(b..e, ""); p.cursor -= 1; } }
            KeyCode::Delete => { if p.cursor < n { let b = bi(&p.text, p.cursor); let e = bi(&p.text, p.cursor + 1); p.text.replace_range(b..e, ""); } }
            KeyCode::Char('v') | KeyCode::Char('V') if ctrl_only => {
                if let Some(t) = crate::winapi::get_clipboard() {
                    let line = t.lines().next().unwrap_or("").to_string();
                    let b = bi(&p.text, p.cursor);
                    p.text.insert_str(b, &line);
                    p.cursor += line.chars().count();
                }
            }
            KeyCode::Char(c) if !ctrl_only && !c.is_control() => { let b = bi(&p.text, p.cursor); p.text.insert(b, c); p.cursor += 1; }
            _ => {}
        }
    }

    fn submit_prompt(&mut self, p: Prompt) {
        let text = p.text.trim().to_string();
        match p.kind {
            PromptKind::Rename(path) => {
                let old = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if text == old || text.is_empty() { return; }
                if let Err(e) = fileops::valid_name(&text) { self.set_status(&e, true); return; }
                match fileops::rename(&path, &text) {
                    Ok(dest) => { self.marks.clear(); self.select_path(&dest); self.set_status(&format!("Renamed to {text}"), false); }
                    Err(e) => self.set_status(&format!("Rename failed: {e}"), true),
                }
            }
            PromptKind::NewFile | PromptKind::NewFolder => {
                let folder = matches!(p.kind, PromptKind::NewFolder);
                if text.is_empty() { return; }
                if let Err(e) = fileops::valid_name(&text) { self.set_status(&e, true); return; }
                let Some(cwd) = self.cwd.clone() else { return };
                let dest = cwd.join(&text);
                if dest.exists() { self.set_status(&format!("'{text}' already exists"), true); return; }
                let r = if folder { std::fs::create_dir_all(&dest) } else { std::fs::write(&dest, b"") };
                match r {
                    Err(e) => self.set_status(&format!("Could not create: {e}"), true),
                    Ok(()) => {
                        self.select_path(&dest);
                        if folder { self.set_status(&format!("Folder created: {text}"), false) }
                        else {
                            if let Some(e) = self.current().cloned() { self.open_editor(&e); }
                            self.set_status(&format!("File created: {text}  -  type to write, Ctrl+S to save"), false);
                        }
                    }
                }
            }
            PromptKind::Grep => { if !p.text.is_empty() { self.start_search(&p.text); } }
            PromptKind::Cmd => {
                if text.is_empty() { return; }
                self.cmd_history.retain(|h| h != &text);
                self.cmd_history.push(text.clone());
                if self.cmd_history.len() > 100 { self.cmd_history.remove(0); }
                // cd / drive letter: open that folder in the browser
                if let Some(dir) = crate::shell::cd_target(&text, self.cwd.as_deref()) {
                    if dir.is_dir() { if self.view == View::Dir { self.remember(); } self.load(Some(dir)); }
                    else { self.set_status("The system cannot find the path specified.", true); }
                    return;
                }
                let Some(cwd) = self.cwd.clone() else { return };
                match crate::shell::CmdRun::start(&text, &cwd) {
                    Ok(r) => { self.cmd_run = Some(r); self.show_help = false; }
                    Err(e) => self.set_status(&e, true),
                }
            }
            PromptKind::EdFind => {
                if p.text.is_empty() { return; }
                self.last_find = p.text.clone();
                if let Some(ed) = &mut self.editor {
                    ed.last_find = p.text;
                    ed.find_next(false);
                    if let Some((m, e)) = ed.msg.take() { self.status = m; self.status_err = e; }
                }
            }
            PromptKind::EdGoto => {
                if let (Ok(n), Some(ed)) = (text.parse::<usize>(), self.editor.as_mut()) { ed.goto_line(n); }
            }
        }
    }

    fn confirm_key(&mut self, k: KeyEvent, c: Confirm) {
        let yes = matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y'));
        if !yes { self.set_status("Cancelled", false); return; }
        match c.action {
            ConfirmAction::Delete { targets, permanent } => {
                let old = self.sel;
                let mut fails = Vec::new();
                for t in &targets {
                    if let Err(e) = fileops::delete(t, permanent) {
                        fails.push(format!("{}: {e}", t.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
                    }
                }
                self.marks.clear();
                let what = target_label(&targets);
                let cwd = self.cwd.clone();
                self.load(cwd);
                self.move_to(old.min(self.entries.len().saturating_sub(1)) as isize);
                if !fails.is_empty() { self.set_status(&format!("Some items could not be deleted - {}", fails.join("; ")), true) }
                else if permanent { self.set_status(&format!("Deleted {what}"), false) }
                else { self.set_status(&format!("Moved {what} to the Recycle Bin"), false) }
            }
        }
    }

    // ------------------------------------------------------------------ editor

    pub fn open_editor(&mut self, e: &Entry) {
        let find = String::new();
        let line = if e.line > 0 { e.line } else if self.pv_scroll > 0 { self.pv_scroll + 1 } else { 0 };
        match Editor::open(&e.path, &self.theme, self.cfg.editor_wrap(), line, &find) {
            Ok(mut ed) => {
                if e.line == 0 && line > 0 { ed.top = line - 1; ed.row = line - 1; }
                if e.line > 0 {
                    if let Some(m) = &self.search_matcher {
                        let r = ed.row;
                        if let Some((a, b)) = m.find(&ed.lines[r]) { ed.anchor = Some((r, a)); ed.col = b; }
                    }
                }
                if let Some((m, err)) = ed.msg.take() { self.set_status(&m, err); }
                ed.last_find = self.last_find.clone();
                self.editor = Some(ed);
                self.show_help = false;
            }
            Err(m) => self.set_status(&m, true),
        }
    }

    fn toggle_wrap(&mut self) {
        let val = if self.cfg.editor_wrap() { "off" } else { "on" };
        if let Err(e) = self.cfg.set_value("editor", "wrap", Some(val)) { self.set_status(&format!("Could not save config.ini: {e}"), true); return; }
        self.cfg.reload();
        if let Some(ed) = &mut self.editor { ed.wrap = val == "on"; ed.left = 0; ed.top_sub = 0; ed.follow = true; }
        self.set_status(&format!("Word wrap {val}  (Alt+Z)"), false);
    }

    // ------------------------------------------------------------------ selection and file operations

    fn switch_mark(&mut self, e: Option<&Entry>) {
        let Some(e) = e else { return };
        if e.is_drive || self.view != View::Dir { return; }
        if !self.marks.remove(&e.path) { self.marks.insert(e.path.clone()); }
    }

    fn select_all(&mut self) {
        if self.view != View::Dir || self.cwd.is_none() { return; }
        let all: Vec<PathBuf> = self.entries.iter().filter(|e| !e.is_drive).map(|e| e.path.clone()).collect();
        if !all.is_empty() && all.iter().all(|p| self.marks.contains(p)) { self.marks.clear(); self.set_status("Selection cleared", false); }
        else { let n = all.len(); self.marks.extend(all); self.set_status(&format!("{n} items selected"), false); }
    }

    /// The selected items, or the one under the cursor.
    fn targets(&self) -> Vec<PathBuf> {
        if !self.marks.is_empty() { return self.all.iter().filter(|e| self.marks.contains(&e.path)).map(|e| e.path.clone()).collect(); }
        match self.current() { Some(e) if !e.is_drive => vec![e.path.clone()], _ => vec![] }
    }

    fn file_ops_here(&mut self) -> bool {
        if self.view != View::Dir { self.set_status("Not available here - go back to a folder first (Esc)", true); return false; }
        if self.cwd.is_none() { self.set_status("Open a drive or folder first", true); return false; }
        true
    }

    pub fn select_path(&mut self, p: &Path) {
        self.memory.insert(self.cwd.clone(), p.to_path_buf());
        let c = self.cwd.clone();
        self.load(c);
    }

    fn rename_current(&mut self) {
        if !self.file_ops_here() { return; }
        let t = self.targets();
        if t.len() != 1 { self.set_status("Rename works on one item - clear the selection first (Esc)", true); return; }
        let name = t[0].file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        self.ask(PromptKind::Rename(t[0].clone()), "Rename to:", &name);
    }

    fn new_item(&mut self, folder: bool) {
        if !self.file_ops_here() { return; }
        if folder { self.ask(PromptKind::NewFolder, "New folder name:", "") } else { self.ask(PromptKind::NewFile, "New file name:", "") }
    }

    fn remove_targets(&mut self, permanent: bool) {
        if !self.file_ops_here() { return; }
        let t = self.targets();
        if t.is_empty() { return; }
        let what = target_label(&t);
        let question = if permanent { format!("Permanently delete {what}? This cannot be undone.") } else { format!("Move {what} to the Recycle Bin?") };
        self.confirm = Some(Confirm { question, action: ConfirmAction::Delete { targets: t, permanent } });
    }

    fn copy_targets(&mut self, mv: bool) {
        if !self.file_ops_here() { return; }
        let t = self.targets();
        if t.is_empty() { return; }
        crate::winapi::set_clipboard_files(&t);               // so Explorer can paste them too
        let what = target_label(&t);
        self.clip = Some((t, mv));
        self.marks.clear();
        self.set_status(&format!("{} {what}  -  go to another folder and press Ctrl+V", if mv { "Cut" } else { "Copied" }), false);
    }

    fn paste_here(&mut self) {
        if !self.file_ops_here() { return; }
        if self.paste_rx.is_some() { self.set_status("Still busy with the last paste", true); return; }
        let win = crate::winapi::get_clipboard_files();
        let (paths, mv) = match &self.clip {
            Some((p, m)) if win.is_empty() || win.iter().map(|x| x.to_string_lossy().to_lowercase()).eq(p.iter().map(|x| x.to_string_lossy().to_lowercase())) => (p.clone(), *m),
            _ if !win.is_empty() => (win, false),
            _ => { self.set_status("Nothing to paste - select files and press Ctrl+C or Ctrl+X first", true); return; }
        };
        let Some(cwd) = self.cwd.clone() else { return };
        // Everything comes from this folder and the cursor is on a folder: paste INTO that folder
        // (pasting a cut file back where it already is would do nothing).
        let same = |a: &Path, b: &Path| a.to_string_lossy().trim_end_matches(['\\', '/']).eq_ignore_ascii_case(b.to_string_lossy().trim_end_matches(['\\', '/']));
        let all_here = paths.iter().all(|p| p.parent().map(|d| same(d, &cwd)).unwrap_or(false));
        let mut dir = cwd.clone();
        if all_here {
            match self.current() {
                Some(e) if e.is_dir && !e.is_drive && !paths.iter().any(|p| same(p, &e.path)) => dir = e.path.clone(),
                _ if mv => { self.set_status("Already in this folder - open the target folder (→), or put the cursor on a folder, then Alt+V", true); return; }
                _ => {}
            }
        }
        let into = if dir != cwd { format!(" into {}", dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()) } else { String::new() };
        self.paste_into = into;
        self.paste_move = mv;
        self.paste_rx = Some(fileops::paste(paths, dir, mv));
    }

    fn copy_path_text(&mut self) {
        let t = self.targets();
        if t.is_empty() { return; }
        let text = t.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\r\n");
        if crate::winapi::set_clipboard(&text) { self.set_status(&format!("Path copied: {}", text.replace("\r\n", "; ")), false) }
        else { self.set_status("Could not copy to the clipboard", true) }
    }

    // ------------------------------------------------------------------ find in files

    fn load_search_opts(&mut self) {
        let get = |k: &str| self.cfg.ini.get("search", k).map(|v| v.trim().to_lowercase());
        let on = |v: Option<String>| matches!(v.as_deref(), Some("on" | "yes" | "true" | "1"));
        self.search_opts.what = crate::grep::What::from_key(&get("what").unwrap_or_default());
        self.search_opts.regex = on(get("regex"));
        self.search_opts.case = on(get("case"));
    }

    fn save_search_opts(&mut self) {
        let o = self.search_opts.clone();
        let cur = |k: &str| self.cfg.ini.get("search", k).map(|v| v.trim().to_lowercase()).unwrap_or_default();
        let want = [("what", o.what.key()), ("regex", if o.regex { "on" } else { "off" }), ("case", if o.case { "on" } else { "off" })];
        if want.iter().all(|(k, v)| cur(k) == *v) { return; }
        for (k, v) in want { let _ = self.cfg.set_value("search", k, Some(v)); }
        self.cfg.reload();
    }

    fn start_search(&mut self, q: &str) {
        if let Some(s) = &self.search { s.stop.store(true, std::sync::atomic::Ordering::Relaxed); }
        let opts = self.search_opts.clone();
        let root = if opts.global { None } else { self.cwd.clone() };
        match crate::grep::Search::start(root.as_deref(), q, opts.clone(), self.show_hidden) {
            Err(e) => { self.set_status(&e, true); let q = q.to_string(); self.ask(PromptKind::Grep, "Find:", &q); }
            Ok(s) => {
                self.grep_query = q.to_string();
                self.save_search_opts();
                self.search_matcher = Some(s.matcher.clone());
                let where_ = if opts.global { "all drives".to_string() } else { root.as_ref().map(|r| r.display().to_string()).unwrap_or_default() };
                self.results_title = format!("'{q}' in {where_}  ·  {}", opts.what.label());
                if opts.regex { self.results_title += "  ·  regex"; }
                if opts.case { self.results_title += "  ·  match case"; }
                self.results.clear();
                self.results_sel = 0;
                self.search = Some(s);
                self.editor = None;
                let t = self.results_title.clone();
                self.set_entries(Vec::new(), View::Grep(t));
                self.status.clear();
            }
        }
    }

    /// Move new hits from the running search into the result list (live).
    fn take_hits(&mut self) {
        let Some(s) = &mut self.search else { return };
        if s.taken >= s.hits.len() { return; }
        let single = if s.roots.len() == 1 { Some(s.roots[0].clone()) } else { None };
        let mut new = Vec::new();
        for h in &s.hits[s.taken..] {
            let mut e = entry_for(&h.path);
            let rel = match &single { Some(r) => h.path.strip_prefix(r).map(|x| x.display().to_string()).unwrap_or_else(|_| h.path.display().to_string()), None => h.path.display().to_string() };
            e.name = rel.clone();
            if h.line > 0 {
                e.label = Some(format!("{rel}:{}  {}", h.line, h.text));
                e.line = h.line;
                e.is_dir = false;
            } else {
                e.is_dir = h.is_dir;
                e.label = Some(if h.is_dir { format!("{rel}\\") } else { rel });
            }
            new.push(e);
        }
        s.taken = s.hits.len();
        self.results.extend(new.iter().cloned());
        if matches!(self.view, View::Grep(_)) {
            let first = self.all.is_empty();
            self.all.extend(new);
            if self.filter.is_empty() { self.entries = self.all.clone(); } else { self.apply_filter(); }
            if first { self.sel = 0; self.last_request.clear(); }
        }
    }

    fn finish_search(&mut self) {
        self.take_hits();
        let Some(s) = self.search.take() else { return };
        let stopped = s.stop.load(std::sync::atomic::Ordering::Relaxed);
        let files: HashSet<&PathBuf> = self.results.iter().map(|e| &e.path).collect();
        let n = self.results.len();
        let note = if stopped { "  (stopped)".to_string() } else if s.capped { format!("  (first {})", crate::grep::MAX_HITS) } else { String::new() };
        self.results_title = format!("{}  ·  {n} matches in {} files / folders{note}", self.results_title, files.len());
        if let View::Grep(_) = self.view { self.view = View::Grep(self.results_title.clone()); }
        if !s.opts.regex { self.last_find = s.query.clone(); }
        let secs = s.start.elapsed().as_secs_f64();
        if n == 0 { self.set_status(&format!("No matches for '{}' in {} files ({secs:.1}s)  -  Esc goes back", s.query, s.files), true) }
        else { self.set_status(&format!("{n} matches  ·  {} files searched in {secs:.1}s  ·  Enter opens  ·  F3 / Shift+F3 next / previous", s.files), false) }
    }

    /// F3 in the result list: next / previous match.
    fn step_result(&mut self, d: isize) {
        let n = self.entries.len() as isize;
        if n == 0 { return; }
        self.sel = ((self.sel as isize + d).rem_euclid(n)) as usize;
    }

    /// F6 anywhere: back to the search results at the next / previous match
    /// (from the editor: straight into the next match's file and line).
    fn next_result(&mut self, d: isize) {
        if self.results.is_empty() { self.set_status("No search results - Ctrl+F searches", true); return; }
        let was_editing = self.editor.is_some();
        if let Some(ed) = &self.editor { if ed.dirty { self.set_status("Unsaved changes - Ctrl+S first (or Esc to discard)", true); return; } }
        self.editor = None;
        if !matches!(self.view, View::Grep(_)) {
            let (keep, t) = (self.results_sel, self.results_title.clone());
            if self.view == View::Dir { self.remember(); }
            self.set_entries(self.results.clone(), View::Grep(t));
            self.sel = keep.min(self.entries.len().saturating_sub(1));
        }
        self.step_result(d);
        let i = self.sel;
        self.set_status(&format!("Match {} of {}", i + 1, self.entries.len()), false);
        if was_editing { if let Some(e) = self.current().cloned() { if e.line > 0 { self.open_editor(&e); } } }
    }

    fn set_entries(&mut self, list: Vec<Entry>, view: View) {
        self.view = view;
        self.all = list;
        self.filter.clear();
        self.entries = self.all.clone();
        self.sel = 0;
        self.top = 0;
        self.last_request.clear();
    }

    // ------------------------------------------------------------------ bookmarks

    /// Ctrl+D: bookmark the file under the cursor, or else the current folder (again = remove).
    fn toggle_bookmark(&mut self) {
        if self.view != View::Dir { self.set_status("Go back to a folder first (Esc)", true); return; }
        let target = match self.current() {
            Some(e) if !e.is_dir && !e.is_drive => e.path.clone(),
            _ => match self.cwd.clone() { Some(c) => c, None => { self.set_status("Open a folder to bookmark it", true); return } },
        };
        let what = target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| target.display().to_string());
        let b = self.cfg.bookmarks();
        let here = target.display().to_string();
        let r = if let Some((slot, _)) = b.iter().find(|(_, p)| p.eq_ignore_ascii_case(&here)) {
            let slot = *slot;
            self.cfg.set_value("bookmarks", &slot.to_string(), None).map(|_| format!("Bookmark {slot} removed: {what}"))
        } else {
            let free = (1u32..).find(|n| !b.iter().any(|(s, _)| s == n)).unwrap_or(1);
            let quick = if free <= 9 { format!("  ·  Ctrl+{free} opens it") } else { String::new() };
            self.cfg.set_value("bookmarks", &free.to_string(), Some(&here)).map(|_| format!("Bookmarked {what} as {free}{quick}  ·  Ctrl+Space finds it, Ctrl+B lists bookmarks"))
        };
        self.cfg.reload();
        match r { Ok(m) => self.set_status(&m, false), Err(e) => self.set_status(&format!("Could not save config.ini: {e}"), true) }
    }

    fn show_bookmarks(&mut self) {
        let b = self.cfg.bookmarks();
        if b.is_empty() { self.set_status("No bookmarks yet - press Ctrl+D on a file, or in a folder", true); return; }
        let list = b.into_iter().map(|(k, p)| {
            let it = crate::spotlight::item_for(k, &p);
            let path = PathBuf::from(&p);
            let mut e = entry_for(&path);
            e.is_dir = it.kind == crate::spotlight::Kind::Folder;
            e.hidden = it.kind == crate::spotlight::Kind::Missing;
            e.slot = k;
            let tag = match it.kind { crate::spotlight::Kind::Folder => "", crate::spotlight::Kind::App => "   (app)", crate::spotlight::Kind::File => "", crate::spotlight::Kind::Missing => "   (missing)", crate::spotlight::Kind::Exit | crate::spotlight::Kind::Tool(_) => "" };
            e.label = Some(format!("{k:>2}  {p}{tag}"));
            e
        }).collect();
        self.set_entries(list, View::Bookmarks);
        self.set_status("Enter opens  ·  Delete removes  ·  Ctrl+1..9 / Ctrl+Space open from anywhere  ·  Esc goes back", false);
    }

    fn remove_bookmark(&mut self, e: Option<&Entry>) {
        let Some(e) = e.filter(|e| e.slot > 0) else { return };
        let slot = e.slot;
        if let Err(err) = self.cfg.set_value("bookmarks", &slot.to_string(), None) { self.set_status(&format!("Could not save config.ini: {err}"), true); return; }
        self.cfg.reload();
        if self.cfg.bookmarks().is_empty() { self.leave_view(); self.set_status("Bookmark removed - no bookmarks left", false); return; }
        let old = self.sel;
        self.show_bookmarks();
        self.move_to(old.min(self.entries.len().saturating_sub(1)) as isize);
        self.set_status(&format!("Bookmark {slot} removed"), false);
    }

    fn open_bookmark(&mut self, slot: u8) {
        let b = self.cfg.bookmarks();
        let Some((_, p)) = b.into_iter().find(|(s, _)| *s == slot as u32) else { self.set_status(&format!("No bookmark {slot} - press Ctrl+D on a file or in a folder to add one"), true); return };
        let path = PathBuf::from(&p);
        if !path.exists() { self.set_status(&format!("Bookmark {slot} points to something that no longer exists: {p}"), true); return; }
        self.open_path(&path);
    }

    /// Open a bookmark: folders open in the browser, apps run, other files open in their app.
    pub fn open_path(&mut self, path: &Path) {
        if path.is_dir() {
            if self.view == View::Dir { self.remember(); }
            self.editor = None;
            let name = path.display().to_string();
            if self.load(Some(path.to_path_buf())) { self.set_status(&format!("Opened {name}"), false); }
        } else {
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            match crate::winapi::open_default(path) {
                Ok(_) => self.set_status(&format!("Opened {name}"), false),
                Err(e) => self.set_status(&format!("Cannot open {name}: {e}"), true),
            }
        }
    }

    /// Go to the folder that contains `path` and put the cursor on it.
    pub fn reveal_path(&mut self, path: &Path) {
        let Some(dir) = path.parent().map(|p| p.to_path_buf()) else { return };
        if !dir.is_dir() { self.set_status(&format!("Folder not found: {}", dir.display()), true); return; }
        if self.view == View::Dir { self.remember(); }
        self.editor = None;
        self.memory.insert(Some(dir.clone()), path.to_path_buf());
        self.load(Some(dir));
    }

    fn wave_rgb(&self) -> (u8, u8, u8) {
        match self.theme.accent { ratatui::style::Color::Rgb(r, g, b) => (r, g, b), _ => (139, 233, 253) }
    }

    pub fn open_viewer(&mut self, path: &std::path::Path) {
        match crate::viewer::Viewer::open(path, self.picker.clone(), self.wave_rgb()) {
            Ok(v) => { self.viewer = Some(v); self.show_help = false; }
            Err(e) => self.set_status(&e, true),
        }
    }

    /// Previous / next file in this folder that the viewer can show.
    fn viewer_step(&mut self, dir: isize) {
        let n = self.entries.len() as isize;
        let mut i = self.sel as isize;
        loop {
            i += dir;
            if i < 0 || i >= n { self.set_status(if dir < 0 { "First file" } else { "Last file" }, false); return; }
            let e = &self.entries[i as usize];
            if !e.is_dir && crate::media::kind_of(&e.path) != crate::media::Kind::None {
                let p = e.path.clone();
                let zoom = self.viewer.as_ref().map(|v| v.zoom_i).unwrap_or(0);
                self.sel = i as usize;
                self.open_viewer(&p);
                if let Some(v) = &mut self.viewer { if v.kind == crate::media::Kind::Picture { v.zoom_i = zoom; } }
                return;
            }
        }
    }

    fn viewer_key(&mut self, k: KeyEvent) {
        if matches!(k.code, KeyCode::F(9)) || (matches!(k.code, KeyCode::Tab | KeyCode::BackTab) && k.modifiers.contains(KeyModifiers::CONTROL)) {
            let back = k.modifiers.contains(KeyModifiers::SHIFT) || k.code == KeyCode::BackTab;
            self.cycle_theme(if back { -1 } else { 1 });
            return;
        }
        let Some(v) = self.viewer.as_mut() else { return };
        match v.on_key(k) {
            crate::viewer::Action::None => { let m = v.message.clone(); if !m.is_empty() { self.set_status(&m, true); } }
            crate::viewer::Action::Close => { self.viewer = None; self.last_request.clear(); }
            crate::viewer::Action::Prev => self.viewer_step(-1),
            crate::viewer::Action::Next => self.viewer_step(1),
            crate::viewer::Action::Open => {
                let p = v.path.clone();
                match crate::winapi::open_default(&p) { Ok(_) => self.set_status("Opened in its app", false), Err(e) => self.set_status(&format!("Cannot open: {e}"), true) }
            }
            crate::viewer::Action::PlayExternal => {
                let p = v.path.clone();
                match crate::media::play_external(&p) { Ok(how) => self.set_status(&format!("Playing with sound ({how})"), false), Err(e) => self.set_status(&format!("Cannot play: {e}"), true) }
            }
        }
    }

    /// Background work for the viewer; true = redraw.
    pub fn tick(&mut self) -> bool {
        let mut dirty = self.viewer.as_mut().map(|v| v.update()).unwrap_or(false) || self.egg.is_some();
        if let Some(r) = &mut self.cmd_run {
            let was = r.running();
            if r.poll() {
                dirty = true;
                if was && !r.running() {
                    let (code, took, killed) = (r.exit.flatten(), r.took, r.killed);
                    self.reload();                                    // the command may have changed files
                    let m = match code { Some(0) => format!("Done  ·  {took:.1}s  ·  Esc closes the output"), Some(c) => format!("Exit code {c}  ·  {took:.1}s"), None if killed => "Command stopped".into(), None => "Command ended".into() };
                    self.set_status(&m, code.map(|c| c != 0).unwrap_or(killed));
                }
            }
        }
        if let Some(s) = &mut self.search {
            if s.poll() {
                dirty = true;
                let done = s.done;
                self.take_hits();
                if done { self.finish_search(); }
            }
        }
        if let Some(rx) = &self.paste_rx {
            let mut done = None;
            while let Ok(m) = rx.try_recv() {
                dirty = true;
                match m {
                    PasteMsg::Progress { i, n, name } => { let v = if self.paste_move { "Moving" } else { "Copying" }; self.status = format!("{v} {i} of {n}: {name} ..."); self.status_err = false; }
                    PasteMsg::Done { done: d, fails, first } => done = Some((d, fails, first)),
                }
            }
            if let Some((d, fails, first)) = done {
                self.paste_rx = None;
                if self.paste_move { self.clip = None; }
                let into = std::mem::take(&mut self.paste_into);
                // pasted into the current folder: select the first new item; into a sub-folder: stay on it
                match first { Some(f) if into.is_empty() => self.select_path(&f), _ => self.reload() }
                let v = if self.paste_move { "Moved" } else { "Copied" };
                let place = if into.is_empty() { " here".to_string() } else { into };
                if fails.is_empty() { self.set_status(&format!("{v} {d} item(s){place}"), false) }
                else { self.set_status(&format!("{v} {d} item(s){place}; failed: {}", fails.join("; ")), true) }
            }
        }
        dirty
    }

    /// Something is running in the background (search, paste): keep the loop awake.
    pub fn busy(&self) -> bool { self.search.is_some() || self.paste_rx.is_some() || self.egg.is_some() || self.cmd_run.as_ref().map(|r| r.running()).unwrap_or(false) }

    fn cycle_theme(&mut self, step: i32) {
        self.cfg.cycle_theme(step);
        self.apply_theme();
        match self.cfg.warning.clone() { Some(w) => self.set_status(&w, true), None => self.set_status(&format!("Theme: {}   (Ctrl+Tab / F9 next, Shift for previous)", self.theme.name), false) }
    }

    pub fn on_mouse(&mut self, m: MouseEvent) {
        if let Some(v) = &mut self.viewer {
            match m.kind {
                MouseEventKind::ScrollUp => v.zoom_in(),
                MouseEventKind::ScrollDown => v.zoom_out(),
                _ => {}
            }
            return;
        }
        if self.prompt.is_some() || self.confirm.is_some() || self.egg.is_some() { return; }
        let (x, y) = (m.column, m.row);
        let hit = |b: Rect| matches!(m.kind, MouseEventKind::Down(MouseButton::Left)) && b.width > 0 && y == b.y && x >= b.x && x < b.x + b.width;
        if hit(self.tasks_button) { self.start_tool(crate::tools::Tool::Tasks); return; }
        if hit(self.tools_button) { self.tools = Some(crate::tools::Window::new()); return; }
        let in_rect = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        let in_list = in_rect(self.list_area);
        let in_prev = in_rect(self.preview_area);
        let idx = self.top + (y.saturating_sub(self.list_area.y)) as usize;
        let shift = m.modifiers.contains(KeyModifiers::SHIFT);

        // editor: wheel scrolls, click moves the cursor, drag selects, double-click selects a word
        if self.editor.is_some() {
            let ea = self.editor_area;
            let in_ed = x >= ea.x.saturating_sub(8) && x < ea.x + ea.width && y >= ea.y && y < ea.y + ea.height;
            let ed = self.editor.as_mut().unwrap();
            let pos = if in_ed { ed.pos_at((y - ea.y) as usize, x.saturating_sub(ea.x) as usize) } else { None };
            match m.kind {
                MouseEventKind::ScrollDown if in_prev || in_ed => { ed.follow = false; ed.step_top(3); return; }
                MouseEventKind::ScrollUp if in_prev || in_ed => { ed.follow = false; ed.step_top(-3); return; }
                MouseEventKind::Down(MouseButton::Left) if in_ed => {
                    let Some((r, c)) = pos else { return };
                    let now = Instant::now();
                    let double = matches!(self.last_click, Some((i, t)) if i == usize::MAX - r && now.duration_since(t).as_millis() < 450);
                    if double { ed.select_word(r, c); self.last_click = None; self.drag_editor = false; return; }
                    if shift { if ed.anchor.is_none() { ed.anchor = Some((ed.row, ed.col)); } } else { ed.anchor = None; }
                    let a = ed.anchor;
                    ed.set_cursor(r, c);
                    ed.anchor = if shift { a } else { Some((ed.row, ed.col)) };
                    ed.follow = true; ed.confirm_close = false;
                    self.drag_editor = true;
                    self.last_click = Some((usize::MAX - r, now));
                    return;
                }
                MouseEventKind::Drag(MouseButton::Left) if self.drag_editor => {
                    if let Some((r, c)) = pos { let a = ed.anchor; ed.set_cursor(r, c); ed.anchor = a; ed.follow = true; }
                    return;
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.drag_editor = false;
                    if ed.sel().is_none() { ed.anchor = None; }
                    return;
                }
                MouseEventKind::Down(MouseButton::Left) if in_list && idx < self.entries.len() => {
                    if ed.dirty { self.set_status("Unsaved changes - Ctrl+S to save or Esc to close first", true); return; }
                    self.editor = None;
                    self.last_request.clear();
                    self.sel = idx;
                    self.last_click = Some((idx, Instant::now()));
                    return;
                }
                MouseEventKind::Moved => return,
                _ => {}
            }
        }

        if let Some(r) = &mut self.cmd_run {
            match m.kind {
                MouseEventKind::ScrollDown if in_prev => { r.scroll += 3; return; }
                MouseEventKind::ScrollUp if in_prev => { r.follow = false; r.scroll = r.scroll.saturating_sub(3); return; }
                _ => {}
            }
        }
        match m.kind {
            MouseEventKind::ScrollDown => { if in_prev { self.pv_scroll += 3 } else { self.move_to(self.sel as isize + 1) } }
            MouseEventKind::ScrollUp => { if in_prev { self.pv_scroll = self.pv_scroll.saturating_sub(3) } else { self.move_to(self.sel as isize - 1) } }
            MouseEventKind::Moved => { if in_list && idx < self.entries.len() { self.sel = idx; } }
            MouseEventKind::Down(MouseButton::Left) if in_list && idx < self.entries.len() => {
                if shift { let e = self.entries[idx].clone(); self.switch_mark(Some(&e)); self.sel = idx; return; }
                let now = Instant::now();
                let double = matches!(self.last_click, Some((i, t)) if i == idx && now.duration_since(t).as_millis() < 450);
                self.sel = idx;
                if double { self.last_click = None; self.open(); } else { self.last_click = Some((idx, now)); }
            }
            MouseEventKind::Down(MouseButton::Left) if in_prev && !self.show_help => {
                // click the preview of a text file: edit it
                if let Some(e) = self.current().cloned() {
                    if !e.is_dir && crate::media::kind_of(&e.path) == crate::media::Kind::None && is_text_file(&e) { self.open_editor(&e); }
                }
            }
            _ => {}
        }
    }
}

pub struct TermCaps { pub sixel: bool, pub cell: Option<(u16, u16)> }

/// Send "ESC [ 16 t  ESC [ c" and read the answer (it arrives as typed keys), 500 ms at most.
fn query_terminal() -> Option<TermCaps> {
    use ratatui::crossterm::event::{self, Event, KeyEventKind};
    use std::io::Write;
    let mut out = std::io::stdout();
    out.write_all(b"\x1b[16t\x1b[c").ok()?;
    out.flush().ok()?;
    let start = Instant::now();
    let limit = std::time::Duration::from_millis(500);
    let mut ans = String::new();
    while start.elapsed() < limit {
        if !event::poll(limit - start.elapsed()).ok()? { break; }
        match event::read().ok()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => match k.code {
                KeyCode::Esc => ans.push('\x1b'),
                KeyCode::Char(c) => ans.push(c),
                _ => {}
            },
            _ => {}
        }
        if ans.ends_with('c') && ans.contains("[?") { break; }
    }
    let mut caps = TermCaps { sixel: false, cell: None };
    if let Some(i) = ans.find("[?") {
        let rest = &ans[i + 2..];
        if let Some(end) = rest.find('c') { caps.sixel = rest[..end].split(';').any(|x| x == "4"); }
    }
    if let Some(i) = ans.find("[6;") {
        let rest = &ans[i + 3..];
        if let Some(end) = rest.find('t') {
            let v: Vec<u16> = rest[..end].split(';').filter_map(|x| x.parse().ok()).collect();
            if v.len() == 2 && (8..=128).contains(&v[0]) && (4..=64).contains(&v[1]) { caps.cell = Some((v[1], v[0])); }
        }
    }
    if ans.is_empty() { None } else { Some(caps) }
}

/// Picture output for this terminal: Sixel when it can (Windows Terminal), else half blocks.
/// Returns the picker and the cell size in pixels. Call it after the full-screen UI started.
/// True once the terminal told us its real cell size in pixels (pictures can then be updated piece by piece).
pub static CELL_MEASURED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[allow(deprecated)]   // from_fontsize: we measured the cell ourselves
pub fn build_picker(mode: &str) -> (Picker, (u16, u16)) {
    // Windows: the answer arrives as key events we can read; elsewhere ratatui-image queries itself
    let caps = if mode == "blocks" || !cfg!(windows) { None } else { query_terminal() };
    if caps.as_ref().and_then(|c| c.cell).is_some() { CELL_MEASURED.store(true, std::sync::atomic::Ordering::Relaxed); }
    let cell = caps.as_ref().and_then(|c| c.cell).unwrap_or((10, 20));
    let sixel = match mode {
        "blocks" => false,
        "sixel" => true,
        _ => caps.as_ref().map(|c| c.sixel).unwrap_or(false) || std::env::var_os("WT_SESSION").is_some(),
    };
    let p = if sixel {
        let mut p = Picker::from_fontsize(cell.into());
        p.set_protocol_type(ProtocolType::Sixel);
        p
    } else if cfg!(windows) || caps.is_some() {
        Picker::halfblocks()
    } else {
        match Picker::from_query_stdio() {
            Ok(p) => { CELL_MEASURED.store(true, std::sync::atomic::Ordering::Relaxed); p }
            Err(_) => Picker::halfblocks(),
        }
    };
    (p, cell)
}
