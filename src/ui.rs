//! Drawing: header, file list, preview pane / editor, footer - same layout as the PowerShell version.

use crate::app::{App, View};
use crate::fsutil::fmt_time;
use crate::text::format_size;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use ratatui_image::Image;
use unicode_width::UnicodeWidthStr;

const HELP: &[&str] = &[
    "BROWSE",
    "  Up Down PgUp PgDn Home End   move",
    "  Right / Enter                open folder / edit text file / open file",
    "  Left / Backspace             back to the parent folder",
    "  type letters                 filter this folder (Esc clears)",
    "  Ctrl+Up / Ctrl+Down, wheel   scroll the preview",
    "  Ctrl+H                       show / hide hidden files",
    "  F5                           refresh (and reload config.ini)",
    "",
    "FILES",
    "  Space / Insert               select or unselect (Ctrl+A = all)",
    "  F2                           rename",
    "  Ctrl+N / F7                  new file / new folder",
    "  Delete                       to the Recycle Bin (Shift+Delete = permanently)",
    "  Ctrl+C  Ctrl+X               copy / cut files",
    "  Ctrl+V / Alt+V               paste here (also files copied in Explorer)",
    "  Ctrl+P                       copy the full path as text",
    "  Ctrl+O                       show in Explorer",
    "",
    "COMMAND PROMPT",
    "  F4                           run a command here (output in the preview, Esc stops / closes)",
    "                               Up / Down = earlier commands,  cd <folder> / D: = go there",
    "  Ctrl+T                       open a full Command Prompt here (type exit to come back)",
    "",
    "SEARCH",
    "  Ctrl+F                       search names and / or text in this folder and all subfolders",
    "    in the search line:        Tab = names + text / names / folders only / text   Alt+F = folders",
    "                               Alt+R = regex   Alt+C = match case",
    "                               Alt+G = all drives instead of this folder",
    "  F3 / Shift+F3                next / previous match in the results (also while searching)",
    "  F6 / Shift+F6                next / previous match from anywhere (from the editor: opens it)",
    "  Esc                          stop a running search, then back to the folder",
    "",
    "BOOKMARKS AND QUICK OPEN",
    "  Ctrl+D                       bookmark the file under the cursor, or else this folder (again = remove)",
    "  Ctrl+B                       list bookmarks (Enter opens, Delete removes)",
    "  Ctrl+1..9                    open bookmark 1-9 from anywhere",
    "  Tab, Ctrl+Space / Ctrl+K     Spotlight: type part of a tool (Tasks & clock, Typing test…) or a bookmarked app, file or folder, Enter opens it, → goes to it in the browser, Delete removes the bookmark, Tab closes",
    "",
    "EDITOR",
    "  Shift + arrows, mouse drag   select text (Ctrl+A = all)",
    "  Ctrl+C / X / V, Alt+V        copy / cut / paste (no selection = whole line)",
    "  Ctrl+Z / Ctrl+Y              undo / redo",
    "  Ctrl+F, F3, Shift+F3         find, next, previous      Ctrl+G  go to line",
    "  Tab / Shift+Tab              indent / unindent the selected lines",
    "  Alt+Z                        word wrap on / off",
    "  Ctrl+S                       save          Esc  close",
    "",
    "VIEWER  (Right or F3 on a picture, PDF, video or audio file)",
    "  Left Right                   PDF page / seek video / previous-next picture",
    "  PgUp PgDn                    10 pages back-forward / previous-next file",
    "  + - 0, mouse wheel           zoom in / out / fit   (arrows pan when zoomed)",
    "  Space                        play video / GIF in the terminal (no sound)",
    "  P                            play with sound (ffplay window)",
    "  1..9                         jump to 10%..90% of a video",
    "  Enter                        open in its app     Esc  back to the list",
    "",
    "TOOLS  (Esc, F10 or the ≡ Tools button: the list with check boxes, Space turns a tool on / off; Tab searches them)",
    "  Alt+C                        calendar: holidays, vacations, events, tasks per day",
    "  Alt+Y                        typing test (words, WPM, accuracy, history)",
    "  Alt+L                        new task linked to the file or folder under the cursor (G in Tasks opens it)",
    "  Alt+A                        local AI chat (Ollama, LM Studio…): knows your tasks, calendar and work log, adds tasks and logs work for you",
    "  Alt+P                        themes: built-in and every monkeytype theme, with a live preview",
    "  Alt+I                        image gallery of this folder (thumbnails, Enter views)",
    "  Ctrl+G                       git: changes, diff, stage, commit, history, branches, pull / push, init",
    "  Alt+D                        compare: mark two folders / files with Space, or Alt+D on one, then on the other",
    "",
    "TASKS & CLOCK",
    "  F8 / Alt+T, or click         tasks, daily tasks, reminders, clock and time left today (Esc comes back)",
    "  the ◷ Tasks button            (top right) does the same",
    "",
    "OTHER",
    "  Ctrl+Tab / F9                next colour theme (Shift = previous)",
    "  F1                           this help (F1 or Esc closes it)",
    "  Esc                          clear filter / selection, leave a list, then the tools list (check boxes: Space on / off); type exit in Spotlight (Tab) to quit",
];

pub fn fit(s: &str, w: usize) -> String {
    if s.width() <= w { let pad = w - s.width(); return format!("{s}{}", " ".repeat(pad)); }
    let mut out = String::new();
    let mut cw = 0;
    for ch in s.chars() {
        let c = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if cw + c + 1 > w { break; }
        out.push(ch); cw += c;
    }
    out.push('…'); cw += 1;
    while cw < w { out.push(' '); cw += 1; }
    out
}

fn fit_left(s: &str, w: usize) -> String {
    if s.width() <= w { return s.to_string(); }
    let chars: Vec<char> = s.chars().collect();
    let mut cw = 1; let mut i = chars.len();
    while i > 0 {
        let c = unicode_width::UnicodeWidthChar::width(chars[i - 1]).unwrap_or(0);
        if cw + c > w { break; }
        cw += c; i -= 1;
    }
    format!("…{}", chars[i..].iter().collect::<String>())
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let t = app.theme.clone();
    let area = f.area();
    let base = match t.background { Some(bg) => Style::default().bg(bg).fg(t.text), None => Style::default().fg(t.text) };
    f.render_widget(Block::default().style(base), area);
    if app.viewer.is_some() {
        draw_viewer(f, app);
        draw_status(f, app, area.height.saturating_sub(1));
        if let Some(sp) = &app.spotlight { sp.draw(f, &t); }
        if let Some(w) = &app.tools { w.draw(f, &t, &app.cfg); }
        return;
    }
    if let Some(egg) = app.egg.as_mut() {
        let dim = t.dim;
        egg.draw(f.buffer_mut(), area, t.bg_rgb, dim);
        return;
    }
    let (w, h) = (area.width, area.height);

    let rows = Layout::default().direction(Direction::Vertical).constraints([
        Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Length(1), Constraint::Length(1),
        Constraint::Length(1), Constraint::Min(1), Constraint::Length(1), Constraint::Length(1), Constraint::Length(1),
    ]).split(area);

    // ---- header
    let bar = Span::styled("  ▌ ", Style::default().fg(t.accent));
    let dim = Style::default().fg(t.dim);
    let (title, where_) = match &app.view {
        View::Grep(title) => ("SEARCH RESULTS", title.clone()),
        View::Bookmarks => ("BOOKMARKS", "Ctrl+1..9 jumps to a bookmark from anywhere".to_string()),
        View::Dir if app.pick.is_some() => ("PICK A FILE OR FOLDER FOR THE TASK", app.cwd.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "This PC".into())),
        View::Dir => ("FILE BROWSER", app.cwd.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "This PC".into())),
    };
    f.render_widget(Paragraph::new(Line::from(vec![bar.clone(), Span::styled(title, Style::default().fg(t.text).add_modifier(Modifier::BOLD))])), rows[1]);
    let tasks_on = crate::tools::enabled(&app.cfg, crate::tools::Tool::Tasks);
    app.tasks_button = if tasks_on { tasks_button(f, rows[1], &t, app.tasks_info.as_deref(), 4 + title.len() as u16 + 2) } else { Rect::default() };
    app.tools_button = tools_button(f, rows[1], &t, if tasks_on { app.tasks_button.x } else { rows[1].x + rows[1].width - 1 }, 4 + title.len() as u16 + 2);
    f.render_widget(Paragraph::new(Line::from(vec![bar.clone(), Span::raw(fit_left(&where_, w.saturating_sub(6) as usize))])), rows[2]);
    let n = app.entries.len();
    let nd = app.entries.iter().filter(|e| e.is_dir).count();
    let pos = if n > 0 { app.sel + 1 } else { 0 };
    let pos_txt = if app.filter.is_empty() { format!("{pos}/{n}") } else { format!("{n} of {} match", app.all.len()) };
    let img = if app.sixel_capable { "pictures: sixel" } else { "pictures: blocks" };
    let info = match &app.view {
        View::Grep(_) => format!("{n} matches  ·  {pos_txt}  ·  {}", app.cfg.label()),
        View::Bookmarks => format!("{n} bookmarks  ·  {}", app.cfg.label()),
        View::Dir => format!("{nd} folders  ·  {} files  ·  {pos_txt}  ·  hidden: {}  ·  {}  ·  {img}", n - nd, if app.show_hidden { "on" } else { "off" }, app.cfg.label()),
    };
    let mut spans = vec![bar.clone()];
    if !app.marks.is_empty() {
        spans.push(Span::styled(format!("{} selected", app.marks.len()), Style::default().fg(t.highlight)));
        spans.push(Span::styled("  ·  ", dim));
    }
    spans.push(Span::styled(info, dim));
    f.render_widget(Paragraph::new(Line::from(spans)), rows[3]);
    let tip = if !app.filter.is_empty() {
        Line::from(vec![bar.clone(), Span::styled("filter: ", dim), Span::styled(app.filter.clone(), Style::default().add_modifier(Modifier::BOLD)), Span::styled("_", Style::default().fg(t.accent))])
    } else if let Some((p, mv)) = &app.clip {
        Line::from(vec![bar.clone(), Span::styled(format!("{} item(s) {}  ·  Ctrl+V pastes into the current folder", p.len(), if *mv { "cut" } else { "copied" }), dim)])
    } else {
        Line::from(vec![bar.clone(), Span::styled("type to filter  ·  F1 all shortcuts", dim)])
    };
    f.render_widget(Paragraph::new(tip), rows[4]);
    let rule = format!("  {}", "─".repeat(w.saturating_sub(4) as usize));
    f.render_widget(Paragraph::new(Span::styled(rule.clone(), dim)), rows[5]);
    f.render_widget(Paragraph::new(Span::styled(rule, dim)), rows[7]);

    // ---- body: list | preview or editor
    let body = rows[6];
    let editing = app.editor.is_some();
    let show_prev = w >= 100 || editing;
    let show_list = !(editing && w < 100);
    let list_w = if !show_list { 0 } else if show_prev { app.list_width(w) } else { w };
    let list_area = Rect { x: body.x, y: body.y, width: list_w, height: body.height };
    let prev_x = if show_list { body.x + list_w + 2 } else { body.x + 1 };
    let prev_area = Rect { x: prev_x, y: body.y, width: w.saturating_sub(prev_x), height: body.height };
    app.list_area = if show_list { list_area } else { Rect::default() };
    app.preview_area = if show_prev { prev_area } else { Rect::default() };
    app.visible_rows = body.height as usize;
    app.fix_scroll();

    if show_list && !show_prev && app.cmd_run.is_some() {
        draw_cmd(f, app, list_area);                       // narrow window: the output takes the list's place
    } else if show_list {
        let show_date = list_w >= 62;
        let date_w = if show_date { 18 } else { 0 };
        let name_w = (list_w as usize).saturating_sub(14 + date_w).max(8);
        let mut lines: Vec<Line> = Vec::new();
        for k in 0..body.height as usize {
            let i = app.top + k;
            if let Some(e) = app.entries.get(i) {
                let is_sel = i == app.sel;
                let marked = !app.marks.is_empty() && app.marks.contains(&e.path);
                let bg = if is_sel { Style::default().bg(t.select_bg) } else { Style::default() };
                let mut name_col = if e.is_dir { t.accent } else if e.hidden { t.dim } else { t.text };
                if app.view == View::Bookmarks && e.hidden { name_col = t.dim; }
                if marked { name_col = t.highlight; }
                let mut name_style = bg.fg(name_col);
                if is_sel { name_style = name_style.add_modifier(Modifier::BOLD); }
                let size = if e.is_drive || e.is_dir { String::new() } else { format_size(e.size) };
                let date = if show_date { format!("  {:<16}", fmt_time(e.modified)) } else { String::new() };
                let lead = if marked { Span::styled("•", bg.fg(t.highlight)) } else { Span::styled(" ", bg) };
                let marker = if is_sel { Span::styled("► ", bg.fg(t.accent)) } else { Span::styled("  ", bg) };
                let mut spans = vec![lead, marker];
                let disp = fit(&e.display(), name_w);
                let mut done = false;
                if !app.filter.is_empty() {
                    let low = disp.to_lowercase();
                    if let Some(pos) = low.find(&app.filter.to_lowercase()) {
                        let end = (pos + app.filter.len()).min(disp.len());
                        if disp.is_char_boundary(pos) && disp.is_char_boundary(end) {
                            spans.push(Span::styled(disp[..pos].to_string(), name_style));
                            spans.push(Span::styled(disp[pos..end].to_string(), bg.fg(t.highlight).add_modifier(Modifier::BOLD)));
                            spans.push(Span::styled(disp[end..].to_string(), name_style));
                            done = true;
                        }
                    }
                }
                if !done { spans.push(Span::styled(disp, name_style)); }
                spans.push(Span::styled(format!(" {:>9}{date} ", size), bg.fg(t.dim)));
                lines.push(Line::from(spans));
            } else if n == 0 && k == 0 {
                let m = if !app.filter.is_empty() { "  (no matches)" } else if matches!(app.view, View::Grep(_)) { "  (nothing found)" } else { "  (empty folder)" };
                lines.push(Line::styled(m, dim));
            } else {
                lines.push(Line::raw(""));
            }
        }
        f.render_widget(Paragraph::new(lines), list_area);
    }

    if show_prev {
        if show_list {
            let div = Rect { x: body.x + list_w, y: body.y, width: 1, height: body.height };
            let dcol = if editing { Style::default().fg(t.accent) } else { dim };     // accent divider = editor has the keyboard
            f.render_widget(Paragraph::new(vec![Line::styled("│", dcol); body.height as usize]), div);
        }
        if editing { draw_editor(f, app, prev_area); }
        else if app.cmd_run.is_some() { draw_cmd(f, app, prev_area); }
        else { draw_preview(f, app, prev_area, w, h); }
    }

    // ---- footer
    let esc = if app.show_help { "close help" } else if !app.filter.is_empty() { "clear filter" } else if !app.marks.is_empty() { "clear selection" } else if app.view != View::Dir { "back" } else { "tools" };
    let searching_prompt = app.prompt.as_ref().map(|p| matches!(p.kind, crate::app::PromptKind::Grep)).unwrap_or(false);
    let segs: Vec<(&str, &str)> = if searching_prompt {
        vec![("Enter", "search"), ("Tab", "names / folders / text / both"), ("Alt+R", "regex"), ("Alt+C", "match case"), ("Alt+G", "here / all drives"), ("Esc", "cancel")]
    } else if editing {
        vec![("Ctrl+S", "save"), ("Esc", "close"), ("Shift+arrows", "select"), ("Ctrl+C/X/V", "copy/cut/paste"), ("Ctrl+Z/Y", "undo/redo"),
             ("Ctrl+F", "find"), ("F3", "next"), ("Ctrl+G", "line"), ("Alt+Z", "wrap"), ("F1", "help")]
    } else {
        match app.view {
            _ if app.pick.is_some() => vec![("Enter", "pick the selected file / folder"), ("→", "open folder"), ("←", "back"), ("Ctrl+B", "bookmarks"), ("Tab", "search bookmarks"), ("type", "filter"), ("Esc", "cancel")],
            View::Grep(_) => vec![("F3 / Shift+F3", "next / prev match"), ("Enter", "open"), ("Esc", if app.search.is_some() { "stop search" } else { "back" }),
                ("F6", "next match from anywhere"), ("type", "filter"), ("Ctrl+F", "new search"), ("F5", "again"), ("F1", "help")],
            View::Bookmarks => vec![("Enter", "open"), ("Del", "remove"), ("Esc", "back"), ("Ctrl+1..9", "jump"), ("F1", "help")],
            View::Dir if app.cmd_run.is_some() => vec![("Esc", if app.cmd_run.as_ref().map(|r| r.running()).unwrap_or(false) { "stop command" } else { "close output" }),
                ("F4", "another command"), ("Ctrl+↑↓ / wheel", "scroll output"), ("Ctrl+T", "full prompt"), ("F1", "help")],
            View::Dir => vec![("F1", "help"), ("Esc", esc), ("→", "open/edit/view"), ("←", "back"), ("F4", "cmd"), ("Ctrl+T", "prompt"), ("Alt+T", "tasks"), ("Tab", "search"), ("Space", "select"), ("F2", "rename"), ("Del", "delete"),
                ("Ctrl+C/X/V", "copy/cut/paste"), ("Ctrl+N", "new file"), ("F7", "new folder"), ("Ctrl+F", "find in files"), ("Ctrl+D", "bookmark"), ("Ctrl+B", "bookmarks")],
        }
    };
    f.render_widget(Paragraph::new(hint_line(&segs, w, &t)), rows[8]);
    draw_status(f, app, rows[9].y);
    if let Some(sp) = &app.spotlight { sp.draw(f, &t); }
    if let Some(w) = &app.tools { w.draw(f, &t, &app.cfg); }
}

pub fn hint_line(segs: &[(&str, &str)], w: u16, t: &crate::config::Theme) -> Line<'static> {
    let dim = Style::default().fg(t.dim);
    let mut spans = vec![Span::raw("  ")];
    let mut used = 2usize;
    for (i, (k, l)) in segs.iter().enumerate() {
        let len = k.width() + 1 + l.width() + if i > 0 { 5 } else { 0 };
        if used + len > (w as usize).saturating_sub(2) { break; }
        if i > 0 { spans.push(Span::styled("  ·  ", dim)); }
        spans.push(Span::styled(k.to_string(), Style::default().fg(t.accent)));
        spans.push(Span::styled(format!(" {l}"), dim));
        used += len;
    }
    Line::from(spans)
}

/// Last row: a prompt (with the cursor), a y/n question, or the status message.
fn draw_status(f: &mut Frame, app: &mut App, y: u16) {
    let t = app.theme.clone();
    let w = f.area().width;
    let row = Rect { x: 0, y, width: w, height: 1 };
    if let Some(p) = &app.prompt {
        let label = if matches!(p.kind, crate::app::PromptKind::Grep) {
            let o = &app.search_opts;
            format!("Find  [{}]  [{}]{}{}  ›", o.what.label(), if o.global { "all drives" } else { "here + subfolders" },
                if o.regex { "  [regex]" } else { "" }, if o.case { "  [Aa]" } else { "" })
        } else { p.label.clone() };
        let avail = (w as usize).saturating_sub(6 + label.width()).max(10);
        let chars: Vec<char> = p.text.chars().collect();
        let start = p.cursor.saturating_sub(avail.saturating_sub(1));
        let vis: String = chars[start..].iter().collect();
        let vis = fit(&vis, avail).trim_end().to_string();
        let before: String = chars[start..p.cursor].iter().collect();
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::raw("  "), Span::styled(label.clone(), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::raw(" "), Span::styled(vis, if p.fresh { Style::default().fg(t.text).bg(t.select_bg) } else { Style::default().fg(t.text) }),
        ])), row);
        f.set_cursor_position((2 + label.width() as u16 + 1 + before.width() as u16, y));
        return;
    }
    if let Some(c) = &app.confirm {
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::raw("  "), Span::styled(c.question.clone(), Style::default().fg(t.error).add_modifier(Modifier::BOLD)),
            Span::styled("  (y / n)", Style::default().fg(t.dim)),
        ])), row);
        return;
    }
    if let Some(s) = app.search.as_ref().filter(|_| app.status.is_empty()) {
        // search progress: bar, percentage, counts, the folder being searched
        let bar_w = 24usize;
        let head = format!("  {} {:>3.0}%  ", crate::grep::bar(s.pct, bar_w), s.pct * 100.0);
        let counts = format!("{} files  ·  {} matches  ·  {:.0}s  ·  Esc stops  ·  ", s.files, s.hits.len(), s.start.elapsed().as_secs_f64());
        let room = (w as usize).saturating_sub(head.width() + counts.width() + 2);
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled(head, Style::default().fg(t.accent)),
            Span::styled(counts, Style::default().fg(t.text)),
            Span::styled(fit_left(&s.current, room), Style::default().fg(t.dim)),
        ])), row);
        return;
    }
    let st = Style::default().fg(if app.status_err { t.error } else { t.ok });
    f.render_widget(Paragraph::new(Span::styled(format!("  {}", fit(&app.status, (w as usize).saturating_sub(4)).trim_end()), st)), row);
}

fn rgb((r, g, b): (u8, u8, u8)) -> Color { Color::Rgb(r, g, b) }

fn draw_editor(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let show_cursor = app.prompt.is_none() && app.confirm.is_none();
    let Some(ed) = app.editor.as_mut() else { return };
    app.editor_area = draw_editor_body(f, ed, &t, area, show_cursor);
}

/// The editor (title, info line, numbered text with highlighting and selection) in `area`.
/// Returns the text area (for mouse clicks). Used by the browser and by stand-alone editing.
pub fn draw_editor_body(f: &mut Frame, ed: &mut crate::editor::Editor, t: &crate::config::Theme, area: Rect, show_cursor: bool) -> Rect {
    let dim = Style::default().fg(t.dim);
    let line = |y: u16| Rect { x: area.x, y: area.y + y, width: area.width, height: 1 };
    let mut title = ed.name.clone();
    if ed.dirty { title += "  [modified]"; }
    if ed.read_only { title += "  [read-only]"; }
    f.render_widget(Paragraph::new(Span::styled(title, Style::default().fg(t.text).add_modifier(Modifier::BOLD))), line(0));
    let body_rows = area.height.saturating_sub(3) as usize;
    let gutter = ed.gutter();
    let text_w = (area.width as usize).saturating_sub(gutter).max(4);
    let rows = ed.layout(text_w, body_rows);
    f.render_widget(Paragraph::new(Span::styled(ed.meta(), dim)), line(1));
    let sel = ed.sel();
    let mut out: Vec<Line> = Vec::with_capacity(rows.len());
    for v in &rows {
        let li = v.row;
        let cur_line = li == ed.row;
        let row_bg = if cur_line { Some(t.select_bg) } else { None };
        let num = if v.first { format!("{:>w$} ", li + 1, w = gutter - 1) } else { " ".repeat(gutter) };
        let mut spans = vec![Span::styled(num, Style::default().fg(if cur_line { t.accent } else { t.dim }).bg(row_bg.or(t.background).unwrap_or(Color::Reset)))];
        // selected columns on this line
        let (sf, st) = match sel {
            Some((r1, c1, r2, c2)) if li >= r1 && li <= r2 => {
                let a = if li == r1 { crate::textdoc::disp_col(&ed.lines[li], c1) } else { 0 };
                let b = if li == r2 { crate::textdoc::disp_col(&ed.lines[li], c2) } else { usize::MAX };
                (a, b)
            }
            _ => (usize::MAX, usize::MAX),
        };
        let pieces = ed.hl.line(&ed.lines, li);
        let (from, to) = (v.start, v.start + v.cells.min(text_w));
        let mut col = 0usize;
        for (c, text) in pieces {
            for ch in text.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if col >= from && col + cw <= to {
                    let mut stl = Style::default().fg(rgb(c));
                    if col >= sf && col < st { stl = stl.bg(t.highlight).fg(Color::Rgb(t.bg_rgb.0, t.bg_rgb.1, t.bg_rgb.2)); }
                    else if let Some(b) = row_bg { stl = stl.bg(b); }
                    spans.push(Span::styled(ch.to_string(), stl));
                }
                col += cw;
                if col >= to { break; }
            }
            if col >= to { break; }
        }
        // selected line break (selection continues onto the next line)
        if sf != usize::MAX && st == usize::MAX && v.last && col >= from && col < to { spans.push(Span::styled(" ", Style::default().bg(t.highlight))); col += 1; }
        if let Some(b) = row_bg { let used = col.saturating_sub(from).min(text_w); spans.push(Span::styled(" ".repeat(text_w.saturating_sub(used)), Style::default().bg(b))); }
        out.push(Line::from(merge_spans(spans)));
    }
    let text_area = Rect { x: area.x, y: area.y + 3, width: area.width, height: body_rows as u16 };
    f.render_widget(Paragraph::new(out), text_area);
    if show_cursor {
        if let Some((r, c)) = ed.cursor_cell() {
            f.set_cursor_position((area.x + (gutter + c) as u16, area.y + 3 + r as u16));
        }
    }
    Rect { x: area.x + gutter as u16, y: area.y + 3, width: text_w as u16, height: body_rows as u16 }
}

/// Edit one file full screen with the built-in editor (Tasks & clock uses it for the work log).
/// `end`: start at the last line. Returns when the editor is closed (Esc).
pub fn edit_file(terminal: &mut ratatui::DefaultTerminal, path: &std::path::Path, end: bool) -> std::io::Result<Option<String>> {
    use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseEventKind};
    let cfg = crate::config::Config::load();
    let t = cfg.theme.clone();
    let mut ed = match crate::editor::Editor::open(path, &t, cfg.editor_wrap(), 0, "") { Ok(e) => e, Err(m) => return Ok(Some(m)) };
    if end { let n = ed.lines.len(); ed.goto_line(n); }
    let mut status: (String, bool) = ed.msg.take().unwrap_or_default();
    let mut prompt: Option<(bool, String)> = None;           // (find?, text)  Ctrl+F / Ctrl+G
    let _ = terminal.clear();
    loop {
        terminal.draw(|f| {
            let area = f.area();
            let base = match t.background { Some(bg) => Style::default().bg(bg).fg(t.text), None => Style::default().fg(t.text) };
            f.render_widget(Block::default().style(base), area);
            let body = Rect { x: area.x + 2, y: area.y + 1, width: area.width.saturating_sub(4), height: area.height.saturating_sub(3) };
            draw_editor_body(f, &mut ed, &t, body, prompt.is_none());
            let sy = area.y + area.height.saturating_sub(2);
            if let Some((find, text)) = &prompt {
                let label = if *find { "  Find: " } else { "  Go to line: " };
                f.render_widget(Paragraph::new(Line::from(vec![Span::styled(label, Style::default().fg(t.accent).add_modifier(Modifier::BOLD)), Span::styled(text.clone(), Style::default().fg(t.text))])), Rect { x: area.x, y: sy, width: area.width, height: 1 });
                f.set_cursor_position((area.x + label.width() as u16 + text.width() as u16, sy));
            } else {
                f.buffer_mut().set_string(area.x + 2, sy, fit(&status.0, area.width.saturating_sub(4) as usize).trim_end(), Style::default().fg(if status.1 { t.error } else { t.ok }));
            }
            let segs = [("Ctrl+S", "save"), ("Ctrl+Z / Y", "undo / redo"), ("Ctrl+F F3", "find"), ("Ctrl+G", "go to line"), ("Alt+Z", "wrap"), ("Esc", "close")];
            f.render_widget(Paragraph::new(hint_line(&segs, area.width, &t)), Rect { x: area.x, y: area.y + area.height.saturating_sub(1), width: area.width, height: 1 });
        })?;
        if !event::poll(std::time::Duration::from_millis(500))? { continue; }
        match event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                if let Some((find, text)) = prompt.as_mut() {
                    match k.code {
                        KeyCode::Esc => { prompt = None; }
                        KeyCode::Enter => {
                            let (find, text) = (*find, text.clone());
                            prompt = None;
                            if find { ed.last_find = text; ed.anchor = None; ed.find_next(false); }
                            else if let Ok(n) = text.trim().parse::<usize>() { ed.goto_line(n.clamp(1, ed.lines.len())); }
                        }
                        KeyCode::Backspace => { text.pop(); }
                        KeyCode::Char(c) => text.push(c),
                        _ => {}
                    }
                } else {
                    let pasting = event::poll(std::time::Duration::from_millis(0))?;
                    match ed.on_key(k, pasting) {
                        crate::editor::Out::Close => return Ok(None),
                        crate::editor::Out::AskFind(init) => prompt = Some((true, init)),
                        crate::editor::Out::AskGoto => prompt = Some((false, String::new())),
                        _ => {}
                    }
                }
                if let Some(m) = ed.msg.take() { status = m; }
            }
            Event::Mouse(m) => match m.kind {
                MouseEventKind::ScrollDown => { ed.follow = false; ed.step_top(3); }
                MouseEventKind::ScrollUp => { ed.follow = false; ed.step_top(-3); }
                _ => {}
            },
            Event::Resize(_, _) => { let _ = terminal.clear(); }
            _ => {}
        }
    }
}

/// Join neighbouring one-character spans with the same style (fewer spans to draw).
fn merge_spans(v: Vec<Span<'static>>) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::with_capacity(v.len() / 4 + 1);
    for s in v {
        if let Some(last) = out.last_mut() {
            if last.style == s.style { let mut c = last.content.to_string(); c.push_str(&s.content); last.content = c.into(); continue; }
        }
        out.push(s);
    }
    out
}

fn draw_preview(f: &mut Frame, app: &mut App, area: Rect, w: u16, h: u16) {
    let t = app.theme.clone();
    let dim = Style::default().fg(t.dim);
    let title_style = Style::default().fg(t.text).add_modifier(Modifier::BOLD);
    let line = |y: u16| Rect { x: area.x, y: area.y + y, width: area.width, height: 1 };
    let body = Rect { x: area.x, y: area.y + 3, width: area.width, height: area.height.saturating_sub(3) };

    if app.show_help {
        f.render_widget(Paragraph::new(Span::styled("HELP", title_style)), line(0));
        f.render_widget(Paragraph::new(Span::styled("Keyboard shortcuts  ·  wheel / Ctrl+↓ scrolls".to_string(), dim)), line(1));
        let mut wrapped = wrap_help(body.width as usize);
        wrapped.push((String::new(), String::new(), false));
        wrapped.push(("HELPERS".into(), String::new(), true));
        for p in wrap_text(&crate::deps::status(), (body.width as usize).saturating_sub(2)) { wrapped.push(("  ".into(), p, false)); }
        let max = wrapped.len().saturating_sub(body.height as usize);
        if app.pv_scroll > max { app.pv_scroll = max; }
        let lines: Vec<Line> = wrapped.into_iter().skip(app.pv_scroll).map(|(key, desc, head)| {
            if head { Line::styled(key, Style::default().fg(t.accent)) }
            else { Line::from(vec![Span::styled(key, Style::default().fg(t.text).add_modifier(Modifier::BOLD)), Span::styled(desc, Style::default().fg(t.text))]) }
        }).collect();
        f.render_widget(Paragraph::new(lines), body);
        return;
    }
    let Some(e) = app.current().cloned() else { return };
    let title_col = if e.is_dir { t.accent } else { t.text };
    f.render_widget(Paragraph::new(Span::styled(if e.line > 0 { e.name.clone() } else { e.display() }, title_style.fg(title_col))), line(0));
    let key = app.current_key(w, h).unwrap_or_default();
    let Some(pv) = app.engine.get(&key) else {
        f.render_widget(Paragraph::new(Span::styled("loading…", dim)), line(1));
        return;
    };
    let mut meta = pv.meta.clone();
    let body_rows = body.height as usize;
    let mut y = body.y;
    // a picture under the quick-open box would show through it (Sixel draws on top), so leave it out meanwhile
    if let Some(proto) = pv.image.as_ref().filter(|_| app.spotlight.is_none()) {
        let ih = pv.image_rows.min(body.height);
        f.render_widget(Image::new(proto), Rect { x: body.x, y, width: body.width, height: ih });
        y += ih + if pv.lines.is_empty() { 0 } else { 1 };
    }
    let rest = body.height.saturating_sub(y - body.y) as usize;
    let total = pv.lines.len();
    if pv.scrollable {
        let max = total.saturating_sub(rest);
        if app.pv_scroll > max { app.pv_scroll = max; }
        if app.pv_scroll > 0 { meta += &format!("  ·  lines {}-{} of {total}", app.pv_scroll + 1, (app.pv_scroll + rest).min(total)); }
    } else { app.pv_scroll = 0; }
    f.render_widget(Paragraph::new(Span::styled(meta, dim)), line(1));
    if rest > 0 && total > 0 {
        let start = app.pv_scroll.min(total);
        let end = (start + rest.min(body_rows)).min(total);
        let hit = app.current().map(|e| e.line).unwrap_or(0);
        let lines: Vec<Line> = pv.lines[start..end].iter().enumerate().map(|(k, l)| {
            if pv.scrollable && hit > 0 && start + k + 1 == hit { l.clone().style(Style::default().bg(t.select_bg)) } else { l.clone() }
        }).collect();
        f.render_widget(Paragraph::new(lines), Rect { x: body.x, y, width: body.width, height: rest as u16 });
    }
}

fn draw_viewer(f: &mut Frame, app: &mut App) {
    let t = app.theme.clone();
    let area = f.area();
    let (w, h) = (area.width, area.height);
    let dim = Style::default().fg(t.dim);
    let bar = Span::styled("  ▌ ", Style::default().fg(t.accent));
    let spot_open = app.spotlight.is_some();
    let Some(v) = app.viewer.as_mut() else { return };
    let row = |y: u16| Rect { x: 0, y, width: w, height: 1 };
    let kind = match v.kind { crate::media::Kind::Pdf => "PDF", crate::media::Kind::Video => "VIDEO", crate::media::Kind::Audio => "AUDIO", _ => "PICTURE" };
    f.render_widget(Paragraph::new(Line::from(vec![bar.clone(), Span::styled(format!("{kind} VIEWER"), Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
        Span::styled(format!("   {}", v.position()), dim)])), row(1));
    f.render_widget(Paragraph::new(Line::from(vec![bar.clone(), Span::styled(fit_left(&v.path.display().to_string(), w.saturating_sub(6) as usize), Style::default().fg(t.accent))])), row(2));
    let rule = format!("  {}", "─".repeat(w.saturating_sub(4) as usize));
    f.render_widget(Paragraph::new(Span::styled(rule.clone(), dim)), row(3));
    let body_h = h.saturating_sub(7);
    let body = Rect { x: 2, y: 4, width: w.saturating_sub(4), height: body_h };
    f.render_widget(Paragraph::new(Span::styled(rule, dim)), row(4 + body_h));

    // info panel on the right when there is room
    let panel_w = if w >= 110 && !v.info.is_empty() { 36 } else { 0 };
    let img_area = Rect { x: body.x, y: body.y, width: body.width.saturating_sub(if panel_w > 0 { panel_w + 2 } else { 0 }), height: body.height };
    v.cells = (img_area.width, img_area.height);
    if panel_w > 0 {
        let px = body.x + body.width - panel_w;
        let div = Rect { x: px - 2, y: body.y, width: 1, height: body.height };
        f.render_widget(Paragraph::new(vec![Line::styled("│", dim); body.height as usize]), div);
        let mut lines: Vec<Line> = Vec::new();
        for l in &v.info {
            // "Label:      value" -> label dim, value text; wrap long values
            let (lab, val) = if l.len() > 12 && l.is_char_boundary(12) && l[..12].trim_end().ends_with(':') { (l[..12].to_string(), l[12..].to_string()) } else { (String::new(), l.clone()) };
            let mut first = true;
            let width = (panel_w as usize).saturating_sub(12).max(8);
            let chars: Vec<char> = val.chars().collect();
            let mut i = 0;
            loop {
                let chunk: String = chars.iter().skip(i).take(width).collect();
                i += width;
                let label = if first { lab.clone() } else { " ".repeat(12) };
                lines.push(Line::from(vec![Span::styled(if lab.is_empty() && first { String::new() } else { label }, dim), Span::styled(chunk, Style::default().fg(t.text))]));
                first = false;
                if i >= chars.len() { break; }
            }
        }
        f.render_widget(Paragraph::new(lines), Rect { x: px, y: body.y, width: panel_w, height: body.height });
    }
    if let Some(proto) = v.proto.as_ref().filter(|_| !spot_open) {
        let sz = proto.size();
        let x = img_area.x + img_area.width.saturating_sub(sz.width) / 2;
        let y = img_area.y + img_area.height.saturating_sub(sz.height) / 2;
        f.render_widget(Image::new(proto), Rect { x, y, width: sz.width.min(img_area.width), height: sz.height.min(img_area.height) });
    } else if let Some(e) = &v.error {
        f.render_widget(Paragraph::new(Span::styled(e.clone(), Style::default().fg(t.error))), img_area);
    } else {
        f.render_widget(Paragraph::new(Span::styled("loading…", dim)), img_area);
    }

    // footer
    let segs: Vec<(&str, &str)> = match v.kind {
        crate::media::Kind::Pdf => vec![("← →", "page"), ("PgUp PgDn", "±10 pages"), ("+ - 0", "zoom"), ("Enter", "open in app"), ("Esc", "back")],
        crate::media::Kind::Video => vec![("← →", "seek"), ("Space", "play / pause"), ("P", "with sound"), ("1-9", "jump"), ("PgUp PgDn", "prev / next file"), ("+ - 0", "zoom"), ("Esc", "back")],
        crate::media::Kind::Audio => vec![("P", "play with sound"), ("PgUp PgDn", "prev / next file"), ("Enter", "open in app"), ("Esc", "back")],
        _ if v.anim_frames > 1 => vec![("Space", "play / pause"), ("← →", "frame (paused)"), ("PgUp PgDn", "prev / next file"), ("+ - 0", "zoom"), ("Esc", "back")],
        _ => vec![("← →", "prev / next picture"), ("+ - 0", "zoom"), ("arrows", "pan when zoomed"), ("Enter", "open in app"), ("Esc", "back")],
    };
    let mut spans = vec![Span::raw("  ")];
    let mut used = 2usize;
    for (i, (k, l)) in segs.iter().enumerate() {
        let len = k.width() + 1 + l.width() + if i > 0 { 5 } else { 0 };
        if used + len > w as usize - 2 { break; }
        if i > 0 { spans.push(Span::styled("  ·  ", dim)); }
        spans.push(Span::styled(*k, Style::default().fg(t.accent)));
        spans.push(Span::styled(format!(" {l}"), dim));
        used += len;
    }
    f.render_widget(Paragraph::new(Line::from(spans)), row(5 + body_h));
}

/// Output of an F4 command.
fn draw_cmd(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme.clone();
    let dim = Style::default().fg(t.dim);
    let Some(r) = app.cmd_run.as_mut() else { return };
    let line = |y: u16| Rect { x: area.x, y: area.y + y, width: area.width, height: 1 };
    f.render_widget(Paragraph::new(Line::from(vec![
        Span::styled("cmd> ", Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
        Span::styled(r.cmd.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
    ])), line(0));
    let body = Rect { x: area.x, y: area.y + 3, width: area.width, height: area.height.saturating_sub(3) };
    let rows = body.height as usize;
    // wrap long lines to the pane width (breaks after spaces when it can)
    let width = (body.width as usize).max(10);
    let mut vis: Vec<String> = Vec::with_capacity(r.lines.len());
    for l in &r.lines {
        let starts = crate::textdoc::wrap_starts(l, width);
        if starts.len() == 1 { vis.push(l.clone()); continue; }
        let chars: Vec<char> = l.chars().collect();
        let mut col = 0usize;
        let mut seg = String::new();
        let mut k = 1;
        for ch in chars {
            if k < starts.len() && col >= starts[k] { vis.push(std::mem::take(&mut seg)); k += 1; }
            seg.push(ch);
            col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        }
        vis.push(seg);
    }
    let n = vis.len();
    let max = n.saturating_sub(rows);
    if r.follow || r.scroll > max { r.scroll = max; }
    if r.scroll >= max { r.follow = true; }
    let state = match r.exit {
        None => format!("running  ·  {:.0}s  ·  Esc stops", r.start.elapsed().as_secs_f64()),
        Some(Some(0)) => format!("done  ·  {:.1}s  ·  Esc closes", r.took),
        Some(Some(c)) => format!("exit code {c}  ·  {:.1}s  ·  Esc closes", r.took),
        Some(None) => if r.killed { "stopped  ·  Esc closes".into() } else { "ended  ·  Esc closes".into() },
    };
    let pos = if n > rows { format!("  ·  lines {}-{} of {n}", r.scroll + 1, (r.scroll + rows).min(n)) } else { String::new() };
    let col = match r.exit { Some(Some(0)) => t.ok, Some(_) => t.error, None => t.dim };
    f.render_widget(Paragraph::new(Line::from(vec![Span::styled(state, Style::default().fg(col)), Span::styled(pos, dim)])), line(1));
    let lines: Vec<Line> = if n == 0 && r.exit.is_some() { vec![Line::styled("(no output)", dim)] }
        else { vis[r.scroll..(r.scroll + rows).min(n)].iter().map(|l| Line::styled(l.clone(), Style::default().fg(t.text))).collect() };
    f.render_widget(Paragraph::new(lines), body);
}

/// Split text into pieces of at most `width` columns, breaking after spaces when possible.
pub fn wrap_text(s: &str, width: usize) -> Vec<String> {
    let starts = crate::textdoc::wrap_starts(s, width.max(4));
    if starts.len() == 1 { return vec![s.to_string()]; }
    let mut out = Vec::new();
    let mut seg = String::new();
    let (mut col, mut k) = (0usize, 1usize);
    for ch in s.chars() {
        if k < starts.len() && col >= starts[k] { out.push(std::mem::take(&mut seg).trim_end().to_string()); k += 1; }
        seg.push(ch);
        col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
    }
    out.push(seg);
    out
}

/// The help text wrapped to the pane: (key column, description, is a heading).
/// Long descriptions continue under the description column; in a narrow pane the description goes below the key.
fn wrap_help(width: usize) -> Vec<(String, String, bool)> {
    let mut out = Vec::new();
    for l in HELP {
        if l.is_empty() { out.push((String::new(), String::new(), false)); continue; }
        if !l.starts_with(' ') {
            for p in wrap_text(l, width) { out.push((p, String::new(), true)); }
            continue;
        }
        // "  key<2+ spaces>description": the description starts after the first run of 2+ spaces
        let b = l.as_bytes();
        let mut desc_at = None;
        let mut i = 2;
        while i + 1 < b.len() {
            if b[i] == b' ' && b[i + 1] == b' ' {
                let mut j = i;
                while j < b.len() && b[j] == b' ' { j += 1; }
                if l[..i].trim().is_empty() { i = j; continue; }      // a continuation line: leading spaces only
                desc_at = Some(j);
                break;
            }
            i += 1;
        }
        let lead = l.len() - l.trim_start().len();
        let (key, desc, col) = match desc_at {
            Some(j) => (l[..j].to_string(), l[j..].to_string(), j),
            None if lead > 4 => (" ".repeat(lead), l.trim_start().to_string(), lead),     // continuation of the line above
            None => (String::new(), l.to_string(), 0),
        };
        if col > 0 && width >= col + 24 {
            for (n, p) in wrap_text(&desc, width - col).into_iter().enumerate() {
                out.push((if n == 0 { key.clone() } else { " ".repeat(col) }, p, false));
            }
        } else {
            // narrow pane: key on its own line, description below with a small indent
            if !key.trim().is_empty() { out.push((key.trim_end().to_string(), String::new(), false)); }
            for p in wrap_text(&desc, width.saturating_sub(6)) { out.push(("      ".to_string(), p, false)); }
        }
    }
    out
}

/// "◷ 14:32 │ Tasks · 2 today" button at the right end of a header row. Returns where it is (for clicks).
pub fn tasks_button(f: &mut Frame, row: Rect, t: &crate::config::Theme, info: Option<&str>, min_x: u16) -> Rect {
    let now = chrono::Local::now().format("%H:%M");
    let mut label = match info { Some(i) => format!(" ◷ {now}  │  Tasks · {i} "), None => format!(" ◷ {now}  │  Tasks ") };
    if row.width < min_x + label.width() as u16 + 2 { label = " ◷ Tasks ".into(); }
    let w = label.width() as u16;
    if row.width < min_x + w + 2 { return Rect::default(); }
    let x = row.x + row.width - w - 2;
    let late = info.map(|i| i.contains("late")).unwrap_or(false);
    let fg = match t.background { Some(bg) => bg, None => Color::Black };
    let style = Style::default().bg(if late { t.error } else { t.accent }).fg(fg).add_modifier(Modifier::BOLD);
    f.buffer_mut().set_string(x, row.y, &label, style);
    Rect { x, y: row.y, width: w, height: 1 }
}

/// "≡ Tools" button just left of `right_x` on a header row. Returns where it is (for clicks).
pub fn tools_button(f: &mut Frame, row: Rect, t: &crate::config::Theme, right_x: u16, min_x: u16) -> Rect {
    let label = " ≡ Tools ";
    let w = label.width() as u16;
    if right_x < min_x + w + 2 { return Rect::default(); }
    let x = right_x - w - 1;
    f.buffer_mut().set_string(x, row.y, label, Style::default().bg(t.select_bg).fg(t.text).add_modifier(Modifier::BOLD));
    Rect { x, y: row.y, width: w, height: 1 }
}
