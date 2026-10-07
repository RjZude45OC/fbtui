//! Text editor (→ on a text file): cursor, selection, clipboard, undo/redo, find, go to line,
//! indent, word wrap, syntax colours. Same keys and behaviour as the PowerShell version.

use crate::config::Theme;
use crate::highlight::LineHighlighter;
use crate::textdoc::{self, byte_at, disp_col, index_at_col, nchars, Enc};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::{Path, PathBuf};

pub const MAX_BYTES: u64 = 8 * 1024 * 1024;

struct Undo { kind: &'static str, row: usize, old: Vec<String>, new_count: usize, crow: usize, ccol: usize }

/// One screen row of the editor (a whole line, or a piece of a wrapped line).
#[derive(Clone, Copy, Debug)]
pub struct VRow { pub row: usize, pub start: usize, pub cells: usize, pub first: bool, pub last: bool }

pub enum Out { Handled, NotHandled, Close, AskFind(String), AskGoto }

pub struct Editor {
    pub path: PathBuf,
    pub name: String,
    pub lines: Vec<String>,
    eol: &'static str,
    end_eol: bool,
    pub enc: Enc,
    pub read_only: bool,
    pub truncated: bool,
    pub row: usize,
    pub col: usize,           // char index in the line
    want: Option<usize>,      // display column to keep when moving up / down
    pub top: usize,
    pub top_sub: usize,       // first visual row inside line `top` (wrap)
    pub left: usize,          // horizontal scroll (no wrap)
    pub follow: bool,         // keep the cursor in view on the next draw
    pub anchor: Option<(usize, usize)>,
    pub dirty: bool,
    saved_depth: usize,
    no_merge: bool,
    undo: Vec<Undo>,
    redo: Vec<Undo>,
    pub confirm_close: bool,
    indent_tab: bool,
    pub hl: LineHighlighter,
    pub wrap: bool,
    pub text_w: usize,
    pub page: usize,
    pub rows_map: Vec<VRow>,
    pub msg: Option<(String, bool)>,
    pub last_find: String,
}

fn take(s: &str, n: usize) -> &str { &s[..byte_at(s, n)] }
fn skip(s: &str, n: usize) -> &str { &s[byte_at(s, n)..] }

impl Editor {
    /// Open a file. `line` (1-based) jumps there, `find` selects that text on the line (search hits).
    pub fn open(path: &Path, theme: &Theme, wrap: bool, line: usize, find: &str) -> Result<Editor, String> {
        let md = std::fs::metadata(path).map_err(|e| format!("Cannot open: {e}"))?;
        let truncated = md.len() > MAX_BYTES;
        let mut b = Vec::new();
        {
            use std::io::Read;
            let f = std::fs::File::open(path).map_err(|e| format!("Cannot open: {e}"))?;
            f.take(MAX_BYTES).read_to_end(&mut b).map_err(|e| format!("Cannot open: {e}"))?;
        }
        let doc = textdoc::load(&b);
        if doc.binary { return Err("Binary file - cannot edit".into()); }
        let mut indent_tab = false;
        for l in &doc.lines {
            if l.starts_with('\t') { indent_tab = true; break; }
            if l.starts_with("  ") { break; }
        }
        let hl = LineHighlighter::new(path, doc.lines.first().map(|s| s.as_str()), theme, doc.lines.len());
        let n = doc.lines.len();
        let mut e = Editor {
            path: path.to_path_buf(), name: path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            lines: doc.lines, eol: doc.eol, end_eol: doc.end_eol, enc: doc.enc,
            read_only: md.permissions().readonly() || truncated, truncated,
            row: 0, col: 0, want: None, top: 0, top_sub: 0, left: 0, follow: true, anchor: None,
            dirty: false, saved_depth: 0, no_merge: true, undo: Vec::new(), redo: Vec::new(), confirm_close: false,
            indent_tab, hl, wrap, text_w: 80, page: 20, rows_map: Vec::new(), msg: None, last_find: String::new(),
        };
        if line > 0 {
            e.row = (line - 1).min(n - 1);
            e.top = e.row.saturating_sub(4);
            if !find.is_empty() {
                if let Some(c) = textdoc::find_ci(&e.lines[e.row], find, 0) {
                    e.anchor = Some((e.row, c));
                    e.col = c + nchars(find);
                }
            }
        }
        if truncated { e.say(&format!("File is larger than {} - opened read-only (first part only)", crate::text::format_size(MAX_BYTES)), true); }
        else if e.read_only { e.say("File is read-only - changes cannot be saved", true); }
        Ok(e)
    }

    pub fn say(&mut self, s: &str, err: bool) { self.msg = Some((s.to_string(), err)); }

    fn writable(&mut self) -> bool {
        if self.read_only {
            let m = if self.truncated { "Read-only: file too large to edit here" } else { "Read-only file" };
            self.say(m, true);
            return false;
        }
        true
    }

    /// Replace `count` lines at `row` with `new`, recording undo (typing on one line merges into one step).
    fn set_lines(&mut self, row: usize, count: usize, new: Vec<String>, kind: &'static str) {
        let old: Vec<String> = self.lines[row..row + count].to_vec();
        let merge = !self.no_merge && matches!(kind, "type" | "bs" | "del") && self.undo.last().map(|u|
            u.kind == kind && u.row == row && u.new_count == 1 && count == 1 && new.len() == 1).unwrap_or(false);
        if !merge {
            self.undo.push(Undo { kind, row, old, new_count: new.len(), crow: self.row, ccol: self.col });
            if self.undo.len() > 2000 { self.undo.remove(0); self.saved_depth = self.saved_depth.wrapping_sub(1); }
        }
        self.redo.clear();
        self.no_merge = false;
        self.lines.splice(row..row + count, new);
        if self.lines.is_empty() { self.lines.push(String::new()); }
        self.hl.invalidate(row);
        self.dirty = true;
    }

    fn step_history(&mut self, redo: bool) {
        let u = if redo { self.redo.pop() } else { self.undo.pop() };
        let Some(u) = u else { self.say(if redo { "Nothing to redo" } else { "Nothing to undo" }, false); return };
        let cnt = u.new_count.min(self.lines.len() - u.row);
        let now: Vec<String> = self.lines[u.row..u.row + cnt].to_vec();
        let n_old = u.old.len();
        self.lines.splice(u.row..u.row + cnt, u.old);
        if self.lines.is_empty() { self.lines.push(String::new()); }
        let back = Undo { kind: "hist", row: u.row, old: now, new_count: n_old, crow: self.row, ccol: self.col };
        if redo { self.undo.push(back) } else { self.redo.push(back) }
        self.anchor = None;
        self.set_cursor(u.crow, u.ccol);
        self.hl.invalidate(u.row);
        self.dirty = self.undo.len() != self.saved_depth;
    }

    pub fn save(&mut self) -> bool {
        if !self.writable() { return false; }
        let mut text = self.lines.join(self.eol);
        if self.end_eol { text.push_str(self.eol); }
        if let Err(e) = std::fs::write(&self.path, textdoc::encode(&text, self.enc)) {
            self.say(&format!("Save failed: {e}"), true);
            return false;
        }
        self.dirty = false; self.saved_depth = self.undo.len(); self.no_merge = true;
        let m = format!("Saved {}  ·  {} lines  ·  {}", self.name, self.lines.len(), self.enc.name());
        self.say(&m, false);
        true
    }

    pub fn set_cursor(&mut self, row: usize, col: usize) {
        let row = row.min(self.lines.len() - 1);
        self.row = row;
        self.col = col.min(nchars(&self.lines[row]));
        self.want = None;
        self.no_merge = true;
    }

    pub fn wraps(&self, row: usize) -> Vec<usize> {
        if !self.wrap { return vec![0]; }
        textdoc::wrap_starts(&self.lines[row], self.text_w)
    }

    fn move_vertical(&mut self, delta: isize) {
        if !self.wrap {
            let want = *self.want.get_or_insert_with(|| disp_col(&self.lines[self.row], self.col));
            let row = (self.row as isize + delta).clamp(0, self.lines.len() as isize - 1) as usize;
            self.row = row;
            self.col = index_at_col(&self.lines[row], want);
            self.no_merge = true;
            return;
        }
        let mut r = self.row;
        let mut st = self.wraps(r);
        let dc = disp_col(&self.lines[r], self.col);
        let mut seg = textdoc::seg_of(&st, dc);
        let want = *self.want.get_or_insert(dc - st[seg]);
        for _ in 0..delta.unsigned_abs() {
            if delta > 0 {
                if seg + 1 < st.len() { seg += 1 } else if r + 1 < self.lines.len() { r += 1; st = self.wraps(r); seg = 0 } else { break }
            } else if seg > 0 { seg -= 1 } else if r > 0 { r -= 1; st = self.wraps(r); seg = st.len() - 1 } else { break }
        }
        let line = &self.lines[r];
        let seg_end = if seg + 1 < st.len() { st[seg + 1] - 1 } else { textdoc::line_width(line) };
        self.row = r;
        self.col = index_at_col(line, (st[seg] + want).min(seg_end));
        self.no_merge = true;
    }

    /// Scroll by n visual rows.
    pub fn step_top(&mut self, n: isize) {
        if !self.wrap { self.top = (self.top as isize + n).clamp(0, self.lines.len() as isize - 1) as usize; return; }
        let (mut r, mut s) = (self.top, self.top_sub);
        for _ in 0..n.unsigned_abs() {
            if n > 0 {
                if s + 1 < self.wraps(r).len() { s += 1 } else if r + 1 < self.lines.len() { r += 1; s = 0 } else { break }
            } else if s > 0 { s -= 1 } else if r > 0 { r -= 1; s = self.wraps(r).len() - 1 } else { break }
        }
        self.top = r; self.top_sub = s;
    }

    /// Selection as (row1, col1, row2, col2) in order.
    pub fn sel(&self) -> Option<(usize, usize, usize, usize)> {
        let (ar, ac) = self.anchor?;
        let ar = ar.min(self.lines.len() - 1);
        let ac = ac.min(nchars(&self.lines[ar]));
        if ar == self.row && ac == self.col { return None; }
        if (ar, ac) < (self.row, self.col) { Some((ar, ac, self.row, self.col)) } else { Some((self.row, self.col, ar, ac)) }
    }

    pub fn sel_text(&self) -> Option<String> {
        let (r1, c1, r2, c2) = self.sel()?;
        if r1 == r2 { let l = &self.lines[r1]; return Some(skip(take(l, c2), c1).to_string()); }
        let mut parts = vec![skip(&self.lines[r1], c1).to_string()];
        for r in r1 + 1..r2 { parts.push(self.lines[r].clone()); }
        parts.push(take(&self.lines[r2], c2).to_string());
        Some(parts.join("\r\n"))
    }

    fn remove_sel(&mut self) -> bool {
        let s = self.sel();
        self.anchor = None;
        let Some((r1, c1, r2, c2)) = s else { return false };
        let new = format!("{}{}", take(&self.lines[r1], c1), skip(&self.lines[r2], c2));
        self.set_lines(r1, r2 - r1 + 1, vec![new], "cut");
        self.set_cursor(r1, c1);
        true
    }

    pub fn insert_text(&mut self, s: &str) {
        if !self.writable() { return; }
        self.remove_sel();
        let norm = s.replace("\r\n", "\n").replace('\r', "\n");
        let parts: Vec<&str> = norm.split('\n').collect();
        let line = self.lines[self.row].clone();
        let (before, after) = (take(&line, self.col).to_string(), skip(&line, self.col).to_string());
        if parts.len() == 1 {
            self.set_lines(self.row, 1, vec![format!("{before}{s}{after}")], "type");
            self.col += nchars(s);
        } else {
            let mut new = vec![format!("{before}{}", parts[0])];
            for p in &parts[1..parts.len() - 1] { new.push(p.to_string()); }
            let lastp = parts[parts.len() - 1];
            new.push(format!("{lastp}{after}"));
            self.set_lines(self.row, 1, new, "paste");
            self.row += parts.len() - 1;
            self.col = nchars(lastp);
            self.no_merge = true;
        }
        self.want = None;
    }

    fn copy_text(&mut self, cut: bool) {
        let (text, whole) = match self.sel_text() { Some(t) => (t, false), None => (format!("{}\r\n", self.lines[self.row]), true) };
        if !crate::winapi::set_clipboard(&text) { self.say("Could not use the clipboard", true); return; }
        if !cut { self.say(if whole { "Line copied" } else { "Copied" }, false); return; }
        if !self.writable() { return; }
        if whole {
            if self.lines.len() == 1 { self.set_lines(0, 1, vec![String::new()], "cut") } else { self.set_lines(self.row, 1, vec![], "cut") }
            let r = self.row;
            self.set_cursor(r, 0);
        } else { self.remove_sel(); }
    }

    fn set_indent(&mut self, out: bool) {
        if !self.writable() { return; }
        let s = self.sel();
        let (mut r1, mut r2) = (self.row, self.row);
        if let Some((a, _, b, bc)) = s { r1 = a; r2 = b; if bc == 0 && r2 > r1 { r2 -= 1; } }
        let unit = if self.indent_tab { "\t".to_string() } else { "    ".to_string() };
        let mut new = Vec::new();
        let mut shift: isize = 0;
        for r in r1..=r2 {
            let l = &self.lines[r];
            if out {
                let cut = if l.starts_with('\t') { 1 } else { l.chars().take(4).take_while(|c| *c == ' ').count() };
                new.push(skip(l, cut).to_string());
                if r == self.row { shift = -(cut as isize); }
            } else {
                new.push(format!("{unit}{l}"));
                if r == self.row { shift = nchars(&unit) as isize; }
            }
        }
        let anchor = self.anchor;
        self.set_lines(r1, r2 - r1 + 1, new, "indent");
        self.col = (self.col as isize + shift).max(0) as usize;
        if s.is_some() { if let Some((ar, _)) = anchor { self.anchor = Some((ar, 0)); } }
        self.no_merge = true;
    }

    pub fn find_next(&mut self, back: bool) {
        let q = self.last_find.clone();
        if q.is_empty() { self.say("Nothing to find - press Ctrl+F first", true); return; }
        let n = self.lines.len();
        let qn = nchars(&q);
        let s = self.sel();
        if back {
            let (r, c) = match s { Some((a, b, _, _)) => (a, b), None => (self.row, self.col) };
            for k in 0..=n {
                let row = (r + n * 2 - k) % n;
                let line = &self.lines[row];
                let limit = if k == 0 { if c == 0 { continue } else { c - 1 } } else { nchars(line) };
                if let Some(i) = textdoc::rfind_ci(line, &q, limit) {
                    self.set_cursor(row, i + qn); self.anchor = Some((row, i));
                    self.say(&format!("Found at line {}", row + 1), false); return;
                }
            }
        } else {
            let (r, c) = match s { Some((_, _, a, b)) => (a, b), None => (self.row, self.col) };
            for k in 0..=n {
                let row = (r + k) % n;
                let from = if k == 0 { c } else { 0 };
                if let Some(i) = textdoc::find_ci(&self.lines[row], &q, from) {
                    self.set_cursor(row, i + qn); self.anchor = Some((row, i));
                    self.say(&format!("Found at line {}", row + 1), false); return;
                }
            }
        }
        self.say(&format!("'{q}' not found"), true);
    }

    pub fn goto_line(&mut self, n: usize) {
        self.anchor = None;
        self.set_cursor(n.saturating_sub(1), 0);
        self.top = self.row.saturating_sub(4);
        self.top_sub = 0;
        self.follow = true;
    }

    pub fn select_all(&mut self) {
        let last = self.lines.len() - 1;
        let len = nchars(&self.lines[last]);
        self.set_cursor(last, len);
        self.anchor = Some((0, 0));
    }

    /// Select the word at (row, col) - double-click.
    pub fn select_word(&mut self, row: usize, col: usize) {
        let l = self.lines[row].clone();
        let n = nchars(&l);
        let a = textdoc::word_left(&l, (col + 1).min(n));
        let mut b = textdoc::word_right(&l, a);
        let chars: Vec<char> = l.chars().collect();
        while b > a && b <= n && b > 0 && chars[b - 1].is_whitespace() { b -= 1; }
        self.set_cursor(row, b);
        self.anchor = Some((row, a));
    }

    /// Keys. `pasting` = more input is already waiting (no auto-indent while text is pasted).
    pub fn on_key(&mut self, k: KeyEvent, pasting: bool) -> Out {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl_only = ctrl && !alt;
        let confirm = self.confirm_close;
        self.confirm_close = false;
        self.follow = true;
        let last = self.lines.len() - 1;
        let page = self.page.saturating_sub(4).max(1) as isize;

        if alt && !ctrl && matches!(k.code, KeyCode::Char('z') | KeyCode::Char('Z')) { return Out::NotHandled; }   // Alt+Z: app toggles wrap
        if alt && !ctrl && matches!(k.code, KeyCode::Char('v') | KeyCode::Char('V')) {       // Alt+V: paste (Ctrl+V may be taken by the terminal)
            if let Some(t) = crate::winapi::get_clipboard() { if !t.is_empty() { self.insert_text(&t); } }
            return Out::Handled;
        }
        if alt && !ctrl { if let KeyCode::Char(_) = k.code { return Out::Handled; } }         // other Alt+letter: nothing to type

        let is_move = matches!(k.code, KeyCode::Up | KeyCode::Down | KeyCode::Left | KeyCode::Right | KeyCode::Home | KeyCode::End | KeyCode::PageUp | KeyCode::PageDown);
        if is_move {
            if shift { if self.anchor.is_none() { self.anchor = Some((self.row, self.col)); } }
            else {
                let s = self.sel();
                self.anchor = None;
                if let Some((r1, c1, r2, c2)) = s {
                    if !ctrl_only && matches!(k.code, KeyCode::Left | KeyCode::Right) {
                        if k.code == KeyCode::Left { self.set_cursor(r1, c1) } else { self.set_cursor(r2, c2) }
                        return Out::Handled;
                    }
                }
            }
        }
        let line = self.lines[self.row].clone();
        let len = nchars(&line);

        if ctrl_only {
            match k.code {
                KeyCode::Char(c) => match c.to_ascii_lowercase() {
                    's' => { self.save(); }
                    'z' => self.step_history(shift),
                    'y' => self.step_history(true),
                    'a' => self.select_all(),
                    'c' => self.copy_text(false),
                    'x' => self.copy_text(true),
                    'v' => { if let Some(t) = crate::winapi::get_clipboard() { if !t.is_empty() { self.insert_text(&t); } } }
                    'f' => {
                        let init = self.sel_text().filter(|t| !t.contains('\n')).unwrap_or_else(|| self.last_find.clone());
                        return Out::AskFind(init);
                    }
                    'g' => return Out::AskGoto,
                    _ => return Out::NotHandled,
                },
                KeyCode::Home => self.set_cursor(0, 0),
                KeyCode::End => { let l = nchars(&self.lines[last]); self.set_cursor(last, l); }
                KeyCode::Left => {
                    if self.col > 0 { let c = textdoc::word_left(&line, self.col); self.set_cursor(self.row, c) }
                    else if self.row > 0 { let r = self.row - 1; let l = nchars(&self.lines[r]); self.set_cursor(r, l) }
                }
                KeyCode::Right => {
                    if self.col < len { let c = textdoc::word_right(&line, self.col); self.set_cursor(self.row, c) }
                    else if self.row < last { let r = self.row + 1; self.set_cursor(r, 0) }
                }
                KeyCode::Up => { self.follow = false; self.step_top(-1); }
                KeyCode::Down => { self.follow = false; self.step_top(1); }
                _ => return Out::NotHandled,     // Ctrl+Tab etc. are global
            }
            return Out::Handled;
        }

        match k.code {
            KeyCode::Up => self.move_vertical(-1),
            KeyCode::Down => self.move_vertical(1),
            KeyCode::PageUp => self.move_vertical(-page),
            KeyCode::PageDown => self.move_vertical(page),
            KeyCode::Left => {
                if self.col > 0 { let c = self.col - 1; self.set_cursor(self.row, c) }
                else if self.row > 0 { let r = self.row - 1; let l = nchars(&self.lines[r]); self.set_cursor(r, l) }
            }
            KeyCode::Right => {
                if self.col < len { let c = self.col + 1; self.set_cursor(self.row, c) }
                else if self.row < last { let r = self.row + 1; self.set_cursor(r, 0) }
            }
            KeyCode::Home => {
                let ind = line.chars().take_while(|c| c.is_whitespace()).count();
                if self.col != ind { self.set_cursor(self.row, ind) } else { self.set_cursor(self.row, 0) }
            }
            KeyCode::End => self.set_cursor(self.row, len),
            KeyCode::Esc => {
                if self.anchor.is_some() { self.anchor = None; return Out::Handled; }
                if self.dirty && !confirm {
                    self.confirm_close = true;
                    self.say("Unsaved changes - Ctrl+S to save, Esc again to discard", true);
                    return Out::Handled;
                }
                if self.dirty { let m = format!("Changes to {} discarded", self.name); self.say(&m, true); }
                return Out::Close;
            }
            KeyCode::F(3) => self.find_next(shift),
            KeyCode::Enter => {
                if !self.writable() { return Out::Handled; }
                self.remove_sel();
                let line = self.lines[self.row].clone();
                let mut indent = String::new();
                if !pasting {
                    indent = line.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                    if nchars(&indent) > self.col { indent = take(&indent, self.col).to_string(); }
                }
                let (a, b) = (take(&line, self.col).to_string(), skip(&line, self.col).to_string());
                self.set_lines(self.row, 1, vec![a, format!("{indent}{b}")], "enter");
                self.row += 1; self.col = nchars(&indent); self.want = None; self.no_merge = true;
            }
            KeyCode::Backspace => {
                if !self.writable() || self.remove_sel() { return Out::Handled; }
                if self.col > 0 {
                    let new = format!("{}{}", take(&line, self.col - 1), skip(&line, self.col));
                    self.set_lines(self.row, 1, vec![new], "bs");
                    self.col -= 1;
                } else if self.row > 0 {
                    let prev = self.lines[self.row - 1].clone();
                    self.set_lines(self.row - 1, 2, vec![format!("{prev}{line}")], "join");
                    self.row -= 1; self.col = nchars(&prev); self.no_merge = true;
                }
                self.want = None;
            }
            KeyCode::Delete => {
                if !self.writable() || self.remove_sel() { return Out::Handled; }
                if self.col < len {
                    let new = format!("{}{}", take(&line, self.col), skip(&line, self.col + 1));
                    self.set_lines(self.row, 1, vec![new], "del");
                } else if self.row < last {
                    let next = self.lines[self.row + 1].clone();
                    self.set_lines(self.row, 2, vec![format!("{line}{next}")], "join");
                    self.no_merge = true;
                }
                self.want = None;
            }
            KeyCode::BackTab => self.set_indent(true),
            KeyCode::Tab => {
                let s = self.sel();
                if shift { self.set_indent(true) }
                else if matches!(s, Some((a, _, b, _)) if a != b) { self.set_indent(false) }
                else if self.indent_tab { self.insert_text("\t") }
                else { let n = 4 - disp_col(&line, self.col) % 4; self.insert_text(&" ".repeat(n)) }
            }
            KeyCode::F(1) | KeyCode::F(5) | KeyCode::F(6) | KeyCode::F(9) => return Out::NotHandled,
            KeyCode::F(_) => {}
            KeyCode::Char(c) if !(ctrl && !alt) => {
                if c != '\u{7f}' && !c.is_control() { self.insert_text(&c.to_string()); }
            }
            _ => {}
        }
        Out::Handled
    }

    /// Work out the visual rows for a text area of `text_w` x `body_rows`, scrolling so the cursor is visible.
    pub fn layout(&mut self, text_w: usize, body_rows: usize) -> Vec<VRow> {
        self.text_w = text_w.max(4);
        self.page = body_rows;
        let n = self.lines.len();
        self.top = self.top.min(n - 1);
        let dc = disp_col(&self.lines[self.row], self.col);
        if self.wrap {
            self.left = 0;
            let cst = self.wraps(self.row);
            let cseg = textdoc::seg_of(&cst, dc);
            let tcount = self.wraps(self.top).len();
            self.top_sub = self.top_sub.min(tcount - 1);
            if self.follow {
                if self.row < self.top || (self.row == self.top && cseg < self.top_sub) { self.top = self.row; self.top_sub = cseg; }
                else {
                    let (mut d, mut r, mut s) = (0usize, self.top, self.top_sub);
                    while r < self.row && d < body_rows { d += self.wraps(r).len() - s; s = 0; r += 1; }
                    if r == self.row { d += cseg.saturating_sub(s); }
                    if d >= body_rows {
                        let (mut r, mut s, mut left) = (self.row, cseg, body_rows.saturating_sub(1));
                        while left > 0 {
                            if s > 0 { s -= 1; left -= 1 } else if r > 0 { r -= 1; s = self.wraps(r).len() - 1; left -= 1 } else { break }
                        }
                        self.top = r; self.top_sub = s;
                    }
                }
            }
        } else {
            self.top_sub = 0;
            if self.follow {
                let margin = 8.min(text_w / 4);
                if self.row < self.top { self.top = self.row; }
                if self.row >= self.top + body_rows { self.top = self.row + 1 - body_rows; }
                if dc < self.left { self.left = dc.saturating_sub(margin); }
                if dc >= self.left + text_w { self.left = dc + 1 + margin - text_w; }
            }
        }
        let mut rows = Vec::new();
        let (mut r, mut s) = (self.top, self.top_sub);
        while rows.len() < body_rows && r < n {
            if self.wrap {
                let st = self.wraps(r);
                let mut j = s;
                while j < st.len() && rows.len() < body_rows {
                    let cells = if j + 1 < st.len() { st[j + 1] - st[j] } else { text_w };
                    rows.push(VRow { row: r, start: st[j], cells, first: j == 0, last: j + 1 == st.len() });
                    j += 1;
                }
            } else {
                rows.push(VRow { row: r, start: self.left, cells: text_w, first: true, last: true });
            }
            s = 0; r += 1;
        }
        self.rows_map = rows.clone();
        rows
    }

    /// Screen row (0-based within the text area) and display column of the cursor.
    pub fn cursor_cell(&self) -> Option<(usize, usize)> {
        let dc = disp_col(&self.lines[self.row], self.col);
        let start = if self.wrap { let st = self.wraps(self.row); st[textdoc::seg_of(&st, dc)] } else { self.left };
        let i = self.rows_map.iter().position(|v| v.row == self.row && v.start == start)?;
        let vc = dc.checked_sub(start)?;
        Some((i, vc.min(self.text_w.saturating_sub(1))))
    }

    /// Line / column under a mouse position in the text area.
    pub fn pos_at(&self, tr: usize, cx: usize) -> Option<(usize, usize)> {
        if self.rows_map.is_empty() { return None; }
        if tr >= self.rows_map.len() { let last = self.lines.len() - 1; return Some((last, nchars(&self.lines[last]))); }
        let v = self.rows_map[tr];
        let mut target = v.start + cx;
        if !v.last && cx >= v.cells { target = v.start + v.cells - 1; }
        Some((v.row, index_at_col(&self.lines[v.row], target)))
    }

    pub fn meta(&self) -> String {
        let dc = disp_col(&self.lines[self.row], self.col);
        let eol = if self.eol == "\n" { "LF" } else if self.eol == "\r" { "CR" } else { "CRLF" };
        let mut m = format!("Ln {}, Col {}  ·  {} lines  ·  {}  ·  {eol}  ·  {}", self.row + 1, dc + 1, self.lines.len(), self.enc.name(), if self.wrap { "wrap" } else { "wrap off" });
        if let Some((r1, _, r2, _)) = self.sel() { m += &format!("  ·  {} line(s) selected", r2 - r1 + 1); }
        m
    }

    pub fn gutter(&self) -> usize { self.lines.len().to_string().len() + 1 }
}
