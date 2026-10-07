//! Tasks & clock screen: to-do list with dates / times and daily tasks (done ones are crossed out),
//! an analog + digital clock and a countdown of the time left today.
//! F8 in the file browser, T in the launcher, or `file-browser.exe --tasks` (tasks.bat).

use crate::clock;
use crate::config::{Config, Theme};
use crate::tasks::{self, Store, Task};
use chrono::{Duration as CDur, Local, NaiveDateTime, NaiveTime, Timelike};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use ratatui::Frame;
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

enum Row { Header(String, String), Task(u64), Note(String), Blank }

enum Confirm { Delete(u64), ClearDone }

#[derive(Default, Clone)]
struct Edit { text: String, cur: usize }

impl Edit {
    fn new(s: &str) -> Edit { Edit { text: s.to_string(), cur: s.chars().count() } }
    fn key(&mut self, k: &KeyEvent) -> bool {
        let n = self.text.chars().count();
        let byte = |s: &str, i: usize| s.char_indices().nth(i).map(|(b, _)| b).unwrap_or(s.len());
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Char('u') | KeyCode::Char('U') if ctrl => { self.text.clear(); self.cur = 0; }
            KeyCode::Char(c) if !ctrl => { let b = byte(&self.text, self.cur); self.text.insert(b, c); self.cur += 1; }
            KeyCode::Backspace if self.cur > 0 => { let b = byte(&self.text, self.cur - 1); self.text.remove(b); self.cur -= 1; }
            KeyCode::Delete if self.cur < n => { let b = byte(&self.text, self.cur); self.text.remove(b); }
            KeyCode::Left => self.cur = self.cur.saturating_sub(1),
            KeyCode::Right => self.cur = (self.cur + 1).min(n),
            KeyCode::Home => self.cur = 0,
            KeyCode::End => self.cur = n,
            _ => return false,
        }
        true
    }
}

struct Form { editing: Option<u64>, daily: bool, field: usize, title: Edit, date: Edit, on: Edit, time: Edit, link: Edit, error: Option<String> }

impl Form {
    // fields: 0 title, 1 once / repeats, 2 date (once) or the repeat days (repeats), 3 time, 4 link
    fn fields(&self) -> Vec<usize> { vec![0, 1, 2, 3, 4] }
    fn step(&mut self, d: isize) {
        let f = self.fields();
        let i = f.iter().position(|x| *x == self.field).unwrap_or(0) as isize;
        self.field = f[((i + d).rem_euclid(f.len() as isize)) as usize];
    }
}

pub struct Planner {
    cfg: Config,
    store: Store,
    rows: Vec<Row>,
    sel: Option<u64>,
    top: usize,
    visible: usize,
    list: Rect,
    done_view: u8,                               // DONE section: 0 = finished today, 1 = all finished, 2 = hidden (H cycles)
    big_clock: bool,
    form: Option<Form>,
    confirm: Option<Confirm>,
    status: String,
    status_err: bool,
    last_click: Option<(u64, Instant)>,
    notifier: bool,
    autostart: bool,
    ai_fill: Option<std::sync::mpsc::Receiver<Result<serde_json::Value, String>>>,   // Ctrl+Space in the form: the model is filling it
    periods_edit: Option<Edit>,                  // P: the countdown periods being typed
    log_edit: Option<(Option<String>, Edit)>,    // "what did you do?" (Some(title) = for a finished task)
    log_view: bool,                              // V: show the work log instead of the list
    heatmap: Option<crate::heatmap::Heatmap>,    // M: the activity heatmap instead of the list
    log_top: Option<usize>,                      // scroll position in the log view (None = the end)
    logged_today: usize,
    picker: Option<ratatui_image::picker::Picker>,
    watch: bool,                                 // C: picture watch (true) or line clock
    watch_shown: bool,                           // the picture was drawn in the last frame
    pub repaint: bool,                           // main loop: clear the screen (pictures need a full repaint)
    edit_request: Option<std::path::PathBuf>,
    open_request: Option<String>,
    pick_request: bool,                          // Ctrl+O in the form: choose the link in the file browser                // G: open the selected task's link (needs the terminal)
    calendar_request: bool,                     // K: open the calendar (the run loop does it)       // O: edit this file with the built-in editor
    pub watch_full: bool,                        // next watch frame: send the whole picture (after a clear)
    watch_prev: Option<(Rect, image::RgbaImage)>,  // last watch frame, to send only what changed
}

fn now() -> NaiveDateTime { Local::now().naive_local() }

impl Planner {
    pub fn new(picker: Option<ratatui_image::picker::Picker>) -> Planner {
        let cfg = Config::load();
        let store = Store::open(tasks::default_path());
        let mut p = Planner { cfg, store, rows: Vec::new(), sel: None, top: 0, visible: 10, list: Rect::default(), done_view: 0, big_clock: false,
            form: None, confirm: None, status: String::new(), status_err: false, last_click: None, notifier: false, autostart: false, ai_fill: None, periods_edit: None, log_edit: None, log_view: false, heatmap: None, log_top: None, logged_today: crate::worklog::today_count(now()), picker, watch: true, watch_shown: false, repaint: false, watch_full: true, watch_prev: None, edit_request: None, open_request: None, pick_request: false, calendar_request: false };
        // [tasks] watch_render = blocks: draw the watch with coloured blocks (works in every console)
        if p.cfg.ini.get("tasks", "watch_render").map(|v| v.eq_ignore_ascii_case("blocks")).unwrap_or(false) {
            let mut hb = ratatui_image::picker::Picker::halfblocks();
            let (r, g, b) = p.cfg.theme.bg_rgb;
            hb.set_background_color(Some(image::Rgba([r, g, b, 255])));
            p.picker = Some(hb);
        }
        let sixel = p.picker.as_ref().map(|x| x.protocol_type() != ratatui_image::picker::ProtocolType::Halfblocks).unwrap_or(false);
        p.watch = match p.cfg.ini.get("tasks", "clock") { Some(v) => v.eq_ignore_ascii_case("watch"), None => sixel };
        if let Some(e) = p.store.error.clone() { p.set_status(&e, true); }
        // reminders: start them with Windows the first time (L turns it off), and make sure they are running now
        if p.cfg.ini.get("tasks", "autostart").is_none() {
            let on = crate::notify::set_autostart(true).is_ok();
            let _ = p.cfg.set_value("tasks", "autostart", Some(if on { "yes" } else { "no" }));
            p.cfg.reload();
            if on { p.set_status("Reminders now start with Windows (L turns that off)", false); }
        }
        if p.cfg.ini.get("tasks", "reminders").map(|v| !v.eq_ignore_ascii_case("no")).unwrap_or(true) {
            if let Err(e) = crate::notify::ensure_running() { p.set_status(&format!("Reminders could not start: {e}"), true); }
        }
        p.refresh_flags();
        p.rebuild();
        p
    }

    fn refresh_flags(&mut self) {
        self.notifier = crate::notify::is_running();
        self.autostart = crate::notify::autostart_enabled();
    }

    fn set_status(&mut self, s: &str, err: bool) { self.status = s.to_string(); self.status_err = err; }

    fn task(&self, id: u64) -> Option<&Task> { self.store.tasks.iter().find(|t| t.id == id) }

    /// Daily tasks, then what is still to do (by date), then what is done.
    fn rebuild(&mut self) {
        let now = now();
        let today = now.date();
        let ts = &self.store.tasks;
        let mut daily: Vec<&Task> = ts.iter().filter(|t| t.daily).collect();
        daily.sort_by_key(|t| (t.time.is_none(), t.time, t.created));
        let mut todo: Vec<&Task> = ts.iter().filter(|t| !t.daily && !t.done).collect();
        todo.sort_by_key(|t| (t.date.is_none(), t.date, t.time.unwrap_or(NaiveTime::from_hms_opt(23, 59, 59).unwrap()), t.created));
        let mut done: Vec<&Task> = ts.iter().filter(|t| !t.daily && t.done).collect();
        done.sort_by_key(|t| std::cmp::Reverse(t.done_at));
        let mut rows = Vec::new();
        // repeating: the ones due today first
        daily.sort_by_key(|t| (!t.applies(today), t.time.is_none(), t.time, t.created));
        let due: Vec<&&Task> = daily.iter().filter(|t| t.applies(today)).collect();
        let ddone = due.iter().filter(|t| t.is_done(today)).count();
        rows.push(Row::Header("REPEATING".into(), if daily.is_empty() { String::new() } else { format!("{ddone}/{} due today done", due.len()) }));
        if daily.is_empty() { rows.push(Row::Note("No repeating tasks yet  ·  D adds one (every day, every Monday, the 15th …)".into())); }
        rows.extend(daily.iter().map(|t| Row::Task(t.id)));
        rows.push(Row::Blank);
        let late = todo.iter().filter(|t| t.overdue(now)).count();
        rows.push(Row::Header("TO DO".into(), match (todo.len(), late) { (0, _) => String::new(), (n, 0) => format!("{n} pending"), (n, l) => format!("{n} pending  ·  {l} late") }));
        if todo.is_empty() { rows.push(Row::Note("Nothing to do  ·  A adds a task".into())); }
        rows.extend(todo.iter().map(|t| Row::Task(t.id)));
        if !done.is_empty() {
            // only what was finished today, unless H asks for all of them
            let today_done: Vec<&&Task> = done.iter().filter(|t| t.done_at.map(|d| d.date() == today).unwrap_or(false)).collect();
            let earlier = done.len() - today_done.len();
            rows.push(Row::Blank);
            match self.done_view {
                0 => {
                    let more = if earlier > 0 { format!("  ·  {earlier} earlier: H shows all") } else { String::new() };
                    rows.push(Row::Header("DONE TODAY".into(), format!("{}{more}", today_done.len())));
                    if today_done.is_empty() { rows.push(Row::Note("Nothing finished yet today".into())); }
                    rows.extend(today_done.iter().map(|t| Row::Task(t.id)));
                }
                1 => {
                    rows.push(Row::Header("DONE".into(), format!("{} today  ·  {} in all  ·  H hides them", today_done.len(), done.len())));
                    rows.extend(done.iter().map(|t| Row::Task(t.id)));
                }
                _ => rows.push(Row::Header("DONE".into(), format!("{} hidden  ·  H shows today's", done.len()))),
            }
        }
        self.rows = rows;
        if self.sel.map(|id| !self.rows.iter().any(|r| matches!(r, Row::Task(x) if *x == id))).unwrap_or(true) {
            self.sel = self.rows.iter().find_map(|r| if let Row::Task(id) = r { Some(*id) } else { None });
        }
    }

    fn task_rows(&self) -> Vec<u64> { self.rows.iter().filter_map(|r| if let Row::Task(id) = r { Some(*id) } else { None }).collect() }

    fn move_sel(&mut self, d: isize) {
        let ids = self.task_rows();
        if ids.is_empty() { return; }
        let i = self.sel.and_then(|s| ids.iter().position(|x| *x == s)).unwrap_or(0) as isize;
        self.sel = Some(ids[(i + d).clamp(0, ids.len() as isize - 1) as usize]);
    }

    fn save(&mut self) -> bool {
        match self.store.save() { Ok(()) => { self.rebuild(); true } Err(e) => { self.set_status(&e, true); false } }
    }

    fn toggle_done(&mut self) {
        let Some(id) = self.sel else { return };
        self.store.reload_if_changed();
        let now = now();
        let Some(t) = self.store.get_mut(id) else { return };
        let title = t.title.clone();
        if t.daily && !t.applies(now.date()) && t.done_on != Some(now.date()) {
            let next = t.next_on(now.date()).map(|d| tasks::fmt_day(d, now.date())).unwrap_or_default();
            self.set_status(&format!("{title} is not due today  ·  next: {next}"), true);
            return;
        }
        let done = if t.daily {
            if t.done_on == Some(now.date()) { t.done_on = None; false } else { t.done_on = Some(now.date()); t.done_at = Some(now); true }
        } else {
            t.done = !t.done; t.done_at = if t.done { Some(now) } else { None }; t.done
        };
        if self.save() {
            self.set_status(&if done { format!("Done: {title}  ✓") } else { format!("Back to do: {title}") }, false);
            if done {
                // finished: ask what was done, for the work log
                if self.cfg.ini.get("tasks", "log_prompt").map(|v| v.eq_ignore_ascii_case("no")).unwrap_or(false) { self.write_log(&crate::worklog::finished(&title, "")); }
                else { self.log_edit = Some((Some(title), Edit::default())); }
            }
        }
    }

    fn write_log(&mut self, line: &str) {
        match crate::worklog::append(line, now()) {
            Ok(_) => {
                self.logged_today = crate::worklog::today_count(now());
                if self.heatmap.is_some() { self.heatmap = Some(crate::heatmap::Heatmap::load()); }
                self.set_status(&format!("Logged: {line}"), false);
            }
            Err(e) => self.set_status(&e, true),
        }
    }

    fn log_key(&mut self, k: KeyEvent) {
        let Some((task, e)) = self.log_edit.as_mut() else { return };
        match k.code {
            KeyCode::Enter | KeyCode::Esc => {
                let note = if k.code == KeyCode::Enter { e.text.trim().to_string() } else { String::new() };
                let task = task.clone();
                self.log_edit = None;
                match task {
                    Some(title) => self.write_log(&crate::worklog::finished(&title, &note)),
                    None if note.is_empty() => self.set_status("Nothing logged", false),
                    None => self.write_log(&note),
                }
            }
            _ => { e.key(&k); }
        }
    }

    fn open_log(&mut self) {
        let p = crate::worklog::path();
        if !p.exists() { if let Err(e) = std::fs::write(&p, "# Work log\n") { self.set_status(&format!("Cannot create {}: {e}", p.display()), true); return; } }
        // the app's own editor (the run loop opens it: it needs the terminal)
        self.edit_request = Some(p);
    }

    fn snooze(&mut self) {
        let Some(id) = self.sel else { return };
        self.store.reload_if_changed();
        let mut when = now() + CDur::minutes(10);
        when = when.with_second(0).unwrap_or(when);
        let Some(t) = self.store.get_mut(id) else { return };
        if t.daily || t.done { self.set_status("Snooze works on tasks that are still to do (not daily ones)", true); return; }
        t.date = Some(when.date()); t.time = Some(when.time());
        let title = t.title.clone();
        if self.save() { self.set_status(&format!("Snoozed to {}: {title}", when.format("%H:%M")), false); }
    }

    fn open_form(&mut self, edit: bool, daily: bool) {
        let today = now().date();
        if edit {
            let Some(t) = self.sel.and_then(|id| self.task(id)).cloned() else { return };
            let date = t.date.map(|d| if d == today { "today".to_string() } else { d.format("%d/%m/%Y").to_string() }).unwrap_or_default();
            let time = t.time.map(|x| x.format("%H:%M").to_string()).unwrap_or_default();
            let on = if t.daily { tasks::repeat_text(&t) } else { "every day".into() };
            self.form = Some(Form { editing: Some(t.id), daily: t.daily, field: 0, title: Edit::new(&t.title), date: Edit::new(&date), on: Edit::new(&on), time: Edit::new(&time), link: Edit::new(t.link.as_deref().unwrap_or("")), error: None });
        } else {
            self.form = Some(Form { editing: None, daily, field: 0, title: Edit::default(), date: Edit::new("today"), on: Edit::new("every day"), time: Edit::default(), link: Edit::default(), error: None });
        }
    }

    fn submit_form(&mut self) {
        let Some(f) = self.form.as_mut() else { return };
        let title = f.title.text.trim().to_string();
        if title.is_empty() { f.error = Some("Type a title first".into()); f.field = 0; return; }
        let (date, time) = match tasks::resolve(&f.date.text, &f.time.text, f.daily, now()) {
            Ok(x) => x,
            Err(e) => { f.error = Some(if e.starts_with("date") { "Date not understood".into() } else { "Time not understood".into() }); f.field = if e.starts_with("date") { 2 } else { 3 }; return; }
        };
        let (days, month_day) = if f.daily {
            match tasks::parse_repeat(&f.on.text) {
                Ok(x) => x,
                Err(_) => { f.error = Some("Repeat days not understood  ·  e.g. mon · mon, wed · mon-fri · weekends · 15th".into()); f.field = 2; return; }
            }
        } else { (Vec::new(), None) };
        let (editing, daily) = (f.editing, f.daily);
        let link = tasks::clean_link(&f.link.text);
        self.form = None;
        self.store.reload_if_changed();
        let id = match editing {
            Some(id) => {
                let Some(t) = self.store.get_mut(id) else { self.set_status("That task was deleted meanwhile", true); return };
                if t.daily != daily { t.done = false; t.done_on = None; t.done_at = None; }
                t.title = title.clone(); t.daily = daily; t.date = if daily { None } else { date }; t.time = time;
                t.days = days; t.month_day = month_day; t.link = link;
                id
            }
            None => {
                let mut t = tasks::new_task(&title, daily, if daily { None } else { date }, time);
                t.days = days; t.month_day = month_day; t.link = link;
                self.store.add(t)
            }
        };
        self.sel = Some(id);
        if self.save() {
            let when = self.task(id).map(|t| tasks::fmt_when(t, now().date())).unwrap_or_default();
            let what = if editing.is_some() { "Saved" } else { "Added" };
            self.set_status(&if when.is_empty() { format!("{what}: {title}") } else { format!("{what}: {title}  ·  {when}") }, false);
        }
    }

    fn form_key(&mut self, k: KeyEvent) {
        let Some(f) = self.form.as_mut() else { return };
        f.error = None;
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char(' ') | KeyCode::Char('@') if ctrl => {
                // AI fill: the sentence in the Task field becomes title, date / repeat and time
                if !crate::chat::available(&self.cfg) { self.set_status(crate::chat::NOT_INSTALLED, true); return; }
                let sentence = [f.title.text.trim(), f.date.text.trim(), f.time.text.trim()].iter().filter(|s| !s.is_empty() && **s != "today").cloned().collect::<Vec<_>>().join(" ");
                if f.title.text.trim().is_empty() { f.error = Some("Type the task as a sentence first, e.g. call the bank tomorrow at 10".into()); return; }
                let (tx, rx) = std::sync::mpsc::channel();
                std::thread::spawn(move || { let _ = tx.send(crate::chat::fill_task(&sentence)); });
                self.ai_fill = Some(rx);
                self.set_status("AI is filling in the form…", false);
            }
            KeyCode::Char('o') | KeyCode::Char('O') if ctrl => { f.field = 4; self.pick_request = true; }
            KeyCode::Esc => { self.form = None; self.ai_fill = None; }
            KeyCode::Enter => self.submit_form(),
            KeyCode::Tab => f.step(if shift { -1 } else { 1 }),
            KeyCode::BackTab | KeyCode::Up => f.step(-1),
            KeyCode::Down => f.step(1),
            _ if f.field == 1 => {
                if matches!(k.code, KeyCode::Left | KeyCode::Right | KeyCode::Char(' ')) { f.daily = !f.daily; }
                else if let KeyCode::Char(c) = k.code {
                    match c.to_ascii_lowercase() { 'o' => f.daily = false, 'd' | 'e' => f.daily = true, _ => {} }
                }
            }
            _ => {
                let e = match f.field { 0 => &mut f.title, 2 if f.daily => &mut f.on, 2 => &mut f.date, 4 => &mut f.link, _ => &mut f.time };
                e.key(&k);
            }
        }
    }

    /// The AI fill answer arrived: put it in the form for checking (Enter saves).
    fn poll_ai(&mut self) {
        let Some(rx) = &self.ai_fill else { return };
        let Ok(res) = rx.try_recv() else { return };
        self.ai_fill = None;
        let v = match res { Ok(v) => v, Err(e) => { self.set_status(&format!("AI fill: {e}"), true); return; } };
        let Some(f) = self.form.as_mut() else { return };
        let g = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|s| s.trim().to_string()).unwrap_or_default();
        if !g("title").is_empty() { f.title = Edit::new(&g("title")); }
        let repeat = g("repeat");
        f.daily = !repeat.is_empty();
        if f.daily { f.on = Edit::new(&repeat); }
        let date = g("date");
        let date = chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").map(|d| if d == now().date() { "today".to_string() } else { d.format("%d/%m/%Y").to_string() }).unwrap_or(date);
        f.date = Edit::new(&date);
        f.time = Edit::new(&g("time"));
        f.error = None;
        self.set_status("Filled in by the AI - check it and press Enter to save", false);
    }

    /// false = leave the screen
    fn on_key(&mut self, k: KeyEvent) -> bool {
        if self.form.is_some() { self.form_key(k); return true; }
        if self.log_edit.is_some() { self.log_key(k); return true; }
        if let Some(h) = self.heatmap.as_mut() {
            if h.on_key(k) { return true; }
            if matches!(k.code, KeyCode::Esc | KeyCode::Char('m') | KeyCode::Char('M') | KeyCode::Char('q') | KeyCode::Char('Q')) { self.heatmap = None; self.repaint = true; return true; }
            if !matches!(k.code, KeyCode::Char('w') | KeyCode::Char('W')) { return true; }
        }
        if self.log_view {
            let page = self.visible.max(1);
            let len = crate::worklog::read().len();
            let max = len.saturating_sub(page);
            let top = self.log_top.unwrap_or(max);
            match k.code {
                KeyCode::Up => self.log_top = Some(top.saturating_sub(1)),
                KeyCode::Down => self.log_top = Some((top + 1).min(max)),
                KeyCode::PageUp => self.log_top = Some(top.saturating_sub(page)),
                KeyCode::PageDown => self.log_top = Some((top + page).min(max)),
                KeyCode::Home => self.log_top = Some(0),
                KeyCode::End => self.log_top = None,
                KeyCode::Esc | KeyCode::Left | KeyCode::Char('v') | KeyCode::Char('V') | KeyCode::Char('q') | KeyCode::Char('Q') => { self.log_view = false; }
                KeyCode::Char('w') | KeyCode::Char('W') => { self.log_edit = Some((None, Edit::default())); self.log_top = None; }
                KeyCode::Char('o') | KeyCode::Char('O') | KeyCode::Right => self.open_log(),
                _ => {}
            }
            return true;
        }
        if let Some(e) = self.periods_edit.as_mut() {
            match k.code {
                KeyCode::Esc => { self.periods_edit = None; self.set_status("Cancelled", false); }
                KeyCode::Enter => {
                    let text = e.text.trim().to_string();
                    let saved = match clock::parse_input(&text) {
                        Ok(clock::PeriodInput::All(v)) => {
                            let val = clock::fmt_periods(&v);
                            self.cfg.set_value("tasks", "countdown", if val.is_empty() { None } else { Some(&val) }).map(|_| ())
                        }
                        Ok(clock::PeriodInput::Days(days, v, off)) => {
                            let val = match (&v, off) { (_, true) => Some("off".to_string()), (Some(v), _) => Some(clock::fmt_periods(v)), (None, _) => None };
                            days.iter().try_for_each(|d| self.cfg.set_value("tasks", &format!("countdown_{}", clock::day_key(*d)), val.as_deref()))
                        }
                        Err(err) => { self.set_status(&err, true); return true; }
                    };
                    match saved {
                        Ok(()) => {
                            self.cfg.reload();
                            self.periods_edit = None;
                            let p = clock::Periods::from_config(&self.cfg.ini).0;
                            self.set_status(&if p.any() { format!("Countdown  ·  {}", p.week_text()) } else { "Countdown: time left until midnight".to_string() }, false);
                        }
                        Err(e) => self.set_status(&format!("Could not save config.ini: {e}"), true),
                    }
                }
                _ => { e.key(&k); self.status.clear(); }
            }
            return true;
        }
        if let Some(c) = self.confirm.take() {
            if matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Enter) {
                self.store.reload_if_changed();
                match c {
                    Confirm::Delete(id) => {
                        let title = self.task(id).map(|t| t.title.clone()).unwrap_or_default();
                        self.move_sel(1);
                        if self.sel == Some(id) { self.move_sel(-1); }
                        self.store.tasks.retain(|t| t.id != id);
                        if self.save() { self.set_status(&format!("Deleted: {title}"), false); }
                    }
                    Confirm::ClearDone => {
                        let n = self.store.tasks.iter().filter(|t| !t.daily && t.done).count();
                        self.store.tasks.retain(|t| t.daily || !t.done);
                        if self.save() { self.set_status(&format!("Cleared {n} finished tasks"), false); }
                    }
                }
            } else { self.set_status("Cancelled", false); }
            return true;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        self.status.clear();
        match k.code {
            KeyCode::Tab | KeyCode::BackTab if ctrl => { self.cfg.cycle_theme(if shift || k.code == KeyCode::BackTab { -1 } else { 1 }); }
            KeyCode::F(9) => { self.cfg.cycle_theme(if shift { -1 } else { 1 }); }
            KeyCode::Char('c') if ctrl => return false,
            KeyCode::Esc => { if self.big_clock { self.big_clock = false } else { return false } }
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::F(8) => return false,
            KeyCode::Up => self.move_sel(-1),
            KeyCode::Down => self.move_sel(1),
            KeyCode::PageUp => self.move_sel(-(self.visible as isize)),
            KeyCode::PageDown => self.move_sel(self.visible as isize),
            KeyCode::Home => self.move_sel(-100_000),
            KeyCode::End => self.move_sel(100_000),
            KeyCode::Char(' ') | KeyCode::Enter => self.toggle_done(),
            KeyCode::Char('a') | KeyCode::Char('A') | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('+') | KeyCode::Insert => self.open_form(false, false),
            KeyCode::Char('d') | KeyCode::Char('D') => self.open_form(false, true),
            KeyCode::Char('e') | KeyCode::Char('E') | KeyCode::F(2) | KeyCode::Right => self.open_form(true, false),
            KeyCode::Char('s') | KeyCode::Char('S') => self.snooze(),
            KeyCode::Char('g') | KeyCode::Char('G') => {
                match self.sel.and_then(|id| self.task(id)).map(|t| (t.title.clone(), t.link.clone())) {
                    Some((_, Some(l))) => self.open_request = Some(l),
                    Some((title, None)) => self.set_status(&format!("{title} has no link  ·  → edits it: put a folder, file or web address in Link"), true),
                    None => {}
                }
            }
            KeyCode::Delete | KeyCode::Char('-') => {
                if let Some(t) = self.sel.and_then(|id| self.task(id)) {
                    let msg = format!("Delete “{}”?  Y / N", t.title);
                    self.confirm = Some(Confirm::Delete(t.id));
                    self.set_status(&msg, true);
                }
            }
            KeyCode::Char('x') | KeyCode::Char('X') => {
                let n = self.store.tasks.iter().filter(|t| !t.daily && t.done).count();
                if n == 0 { self.set_status("No finished tasks to clear", false); }
                else { self.confirm = Some(Confirm::ClearDone); self.set_status(&format!("Remove the {n} finished tasks from the list?  Y / N"), true); }
            }
            KeyCode::Char('h') | KeyCode::Char('H') => {
                self.done_view = (self.done_view + 1) % 3;
                self.set_status(match self.done_view { 0 => "Done: today's only", 1 => "Done: all finished tasks", _ => "Done: hidden" }, false);
                self.rebuild();
            }
            KeyCode::Char('z') | KeyCode::Char('Z') => { self.big_clock = !self.big_clock; self.repaint = true; }
            KeyCode::Char('c') | KeyCode::Char('C') if !ctrl => {
                self.watch = !self.watch;
                self.repaint = true;
                let _ = self.cfg.set_value("tasks", "clock", Some(if self.watch { "watch" } else { "lines" }));
                self.cfg.reload();
                if self.watch && self.picker.as_ref().map(|x| x.protocol_type() == ratatui_image::picker::ProtocolType::Halfblocks).unwrap_or(true) {
                    self.set_status("Watch: this terminal shows pictures as blocks - Windows Terminal draws it sharp (Sixel)", false);
                } else if self.watch {
                    let fs = self.picker.as_ref().map(|p| p.font_size()).map(|f| (f.width, f.height)).unwrap_or((0, 0));
                    let msg = format!("Clock: watch  ·  sixel, cell {}×{} px", fs.0, fs.1);
                    self.set_status(&msg, false);
                } else { self.set_status("Clock: lines", false); }
            }
            KeyCode::Char('w') | KeyCode::Char('W') => { self.log_edit = Some((None, Edit::default())); }
            KeyCode::Char('k') | KeyCode::Char('K') => {
                if crate::tools::enabled(&self.cfg, crate::tools::Tool::Calendar) { self.calendar_request = true; }
                else { self.set_status("The calendar is off  ·  turn it on in ≡ Tools (Tab)", true); }
            }
            KeyCode::Char('m') | KeyCode::Char('M') => {
                if crate::tools::enabled(&self.cfg, crate::tools::Tool::Heatmap) { self.heatmap = Some(crate::heatmap::Heatmap::load()); self.repaint = true; }
                else { self.set_status("The heatmap is off  ·  turn it on in ≡ Tools (Tab)", true); }
            }
            KeyCode::Char('v') | KeyCode::Char('V') => { self.log_view = true; self.big_clock = false; self.log_top = None; }
            KeyCode::Char('o') | KeyCode::Char('O') => self.open_log(),
            KeyCode::Char('p') | KeyCode::Char('P') => {
                let cur = self.cfg.ini.get("tasks", "countdown").unwrap_or("").to_string();
                self.periods_edit = Some(Edit::new(&cur));
            }
            KeyCode::Char('l') | KeyCode::Char('L') => {
                let on = !self.autostart;
                match crate::notify::set_autostart(on) {
                    Ok(()) => {
                        let _ = self.cfg.set_value("tasks", "autostart", Some(if on { "yes" } else { "no" }));
                        self.set_status(if on { "Reminders start with Windows" } else { "Reminders no longer start with Windows (they run while this app is open)" }, false);
                    }
                    Err(e) => self.set_status(&format!("Could not change the Startup entry: {e}"), true),
                }
                self.refresh_flags();
            }
            KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::F(5) => {
                match crate::notify::ensure_running() {
                    Ok(()) => self.set_status("Reminders are running", false),
                    Err(e) => self.set_status(&format!("Reminders could not start: {e}"), true),
                }
                self.store.load(); self.rebuild(); self.refresh_flags();
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                match crate::notify::test_notification() {
                    Ok(()) => self.set_status("Test notification sent  ·  if nothing shows, check Windows notification settings / Focus", false),
                    Err(e) => self.set_status(&e, true),
                }
            }
            _ => {}
        }
        true
    }

    fn on_mouse(&mut self, m: event::MouseEvent) {
        if self.form.is_some() || self.confirm.is_some() { return; }
        let l = self.list;
        let inside = m.column >= l.x && m.column < l.x + l.width && m.row >= l.y && m.row < l.y + l.height;
        match m.kind {
            MouseEventKind::ScrollDown => self.move_sel(1),
            MouseEventKind::ScrollUp => self.move_sel(-1),
            MouseEventKind::Down(MouseButton::Left) if inside => {
                let i = self.top + (m.row - l.y) as usize;
                if let Some(Row::Task(id)) = self.rows.get(i) {
                    let id = *id;
                    let now = Instant::now();
                    if m.column < l.x + 7 { self.sel = Some(id); self.toggle_done(); return; }
                    if matches!(self.last_click, Some((x, t)) if x == id && now.duration_since(t).as_millis() < 450) { self.last_click = None; self.sel = Some(id); self.open_form(true, false); }
                    else { self.sel = Some(id); self.last_click = Some((id, now)); }
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ drawing

    pub fn draw(&mut self, f: &mut Frame) {
        let t = self.cfg.theme.clone();
        let area = f.area();
        let (w, h) = (area.width, area.height);
        let base = match t.background { Some(bg) => Style::default().bg(bg).fg(t.text), None => Style::default().fg(t.text) };
        f.render_widget(Block::default().style(base), area);
        let now = now();
        let dim = Style::default().fg(t.dim);
        let row = |y: u16| Rect { x: 0, y, width: w, height: 1 };
        let bar = Span::styled("  ▌ ", Style::default().fg(t.accent));
        let wide = w >= 96;
        f.render_widget(Paragraph::new(Line::from(vec![bar.clone(), Span::styled("TASKS & CLOCK", Style::default().fg(t.text).add_modifier(Modifier::BOLD))])), row(1));
        let mut info = vec![bar.clone(), Span::styled(now.format("%A %-d %B %Y").to_string(), dim)];
        if !wide && !self.big_clock {
            let cd = self.countdown(now);
            let what = if cd.active { "left".to_string() } else { cd.label.to_lowercase() };
            info.push(Span::styled(format!("  ·  {}  ·  {} {what}", now.format("%H:%M:%S"), clock::hms(cd.secs)), Style::default().fg(if cd.active { t.accent } else { t.dim })));
        }
        if self.logged_today > 0 { info.push(Span::styled(format!("  ·  {} logged today", self.logged_today), dim)); }
        let rem = if self.notifier { if self.autostart { "reminders on  ·  start with Windows" } else { "reminders on" } } else { "reminders off  ·  R starts them" };
        info.push(Span::styled(format!("  ·  {rem}"), Style::default().fg(if self.notifier { t.dim } else { t.error })));
        f.render_widget(Paragraph::new(Line::from(info)), row(2));
        if let Some((task, _)) = &self.log_edit {
            let label = match task { Some(tt) => format!("  ✔ {}  ·  what did you do?", crate::ui::fit(tt, (w as usize).saturating_sub(30)).trim_end()), None => "  What did you do?".to_string() };
            f.render_widget(Paragraph::new(Span::styled(label, Style::default().fg(t.accent).add_modifier(Modifier::BOLD))), row(3));
        }
        if let Some(e) = &self.periods_edit {
            let label = "  Countdown hours (8-15  ·  tue,thu: 8-13:30, 15:30-18:30  ·  sat,sun: off  ·  empty = midnight) › ";
            f.render_widget(Paragraph::new(Line::from(vec![Span::styled(label, Style::default().fg(t.accent)), Span::styled(e.text.clone(), Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
                Span::styled(if self.status_err && !self.status.is_empty() { format!("   ✗ {}", self.status) } else { String::new() }, Style::default().fg(t.error))])), row(3));
            let before: String = e.text.chars().take(e.cur).collect();
            f.set_cursor_position(((label.width() + before.width()) as u16, 3));
        }
        let msg = self.cfg.warning.clone().or_else(|| clock::Periods::from_config(&self.cfg.ini).1).unwrap_or_else(|| self.status.clone());
        let err = self.cfg.warning.is_some() || self.status_err;
        let err = err || (self.cfg.warning.is_none() && clock::Periods::from_config(&self.cfg.ini).1.is_some());
        if self.periods_edit.is_none() && self.log_edit.is_none() { f.render_widget(Paragraph::new(Span::styled(format!("  {msg}"), Style::default().fg(if err { t.error } else { t.ok }))), row(3)); }
        let rule = format!("  {}", "─".repeat(w.saturating_sub(4) as usize));
        f.render_widget(Paragraph::new(Span::styled(rule.clone(), dim)), row(4));
        let body = Rect { x: 0, y: 5, width: w, height: h.saturating_sub(7) };
        if let Some(hm) = &self.heatmap {
            hm.draw(f, body, &t);
        } else if self.big_clock {
            self.draw_clock(f, body, &t, now, true);
        } else if wide {
            let cw = 46u16;
            let list = Rect { x: 0, y: body.y, width: w - cw - 1, height: body.height };
            if self.log_view { self.draw_log(f, list, &t); } else { self.draw_list(f, list, &t, now); }
            for y in body.y..body.y + body.height { f.buffer_mut().set_string(w - cw - 1, y, "│", dim); }
            self.draw_clock(f, Rect { x: w - cw, y: body.y, width: cw, height: body.height }, &t, now, false);
        } else if self.log_view {
            self.draw_log(f, body, &t);
        } else {
            self.draw_list(f, body, &t, now);
        }
        f.render_widget(Paragraph::new(Span::styled(rule, dim)), row(h.saturating_sub(2)));
        let week = { let p = clock::Periods::from_config(&self.cfg.ini).0; if p.any() { p.week_text() } else { "until midnight every day".to_string() } };
        let segs: Vec<(&str, &str)> = if self.form.is_some() { if crate::chat::available(&self.cfg) { vec![("Enter", "save"), ("Ctrl+O", "pick link"), ("Ctrl+Space", "AI fill from a sentence"), ("Tab ↑↓", "field"), ("← →", "once / repeats"), ("Ctrl+U", "clear"), ("Esc", "cancel")] } else { vec![("Enter", "save"), ("Ctrl+O", "pick link"), ("Tab ↑↓", "field"), ("← →", "once / repeats"), ("Ctrl+U", "clear"), ("Esc", "cancel")] } }
            else if let Some((task, _)) = &self.log_edit { if task.is_some() { vec![("Enter", "save to the work log"), ("Esc", "log just the task")] } else { vec![("Enter", "save to the work log"), ("Esc", "cancel")] } }
            else if self.heatmap.is_some() { vec![("← →", "week"), ("↑ ↓", "day"), ("Home", "today"), ("W", "log what you did"), ("M / Esc", "back to the tasks")] }
            else if self.log_view { vec![("↑↓ PgUp PgDn", "scroll"), ("W", "log what you did"), ("→ / O", "edit the file"), ("← / Esc", "back to the tasks")] }
            else if self.periods_edit.is_some() { vec![("Enter", "save"), ("Esc", "cancel"), ("now:", week.as_str())] }
            else if self.big_clock { vec![("Z", "back to the list"), ("C", "watch / line clock"), ("P", "countdown periods"), ("Esc", "back"), ("Ctrl+Tab", "theme")] }
            else { vec![("Space", "done / not done"), ("A", "add"), ("D", "add repeating"), ("→ / E", "edit"), ("G", "open link"), ("W", "log what you did"), ("V", "work log"), ("M", "heatmap"), ("K", "calendar"), ("Del", "delete"), ("S", "snooze 10m"), ("H", "done: today / all / hide"), ("X", "clear done"), ("Z", "big clock"), ("C", "watch / line clock"), ("P", "countdown periods"), ("T", "test notification"), ("L", "start with Windows"), ("Esc", "back")] };
        f.render_widget(Paragraph::new(crate::ui::hint_line(&segs, w, &t)), row(h.saturating_sub(1)));
        if self.form.is_some() { self.draw_form(f, &t, now); }
        // the work-log note: a box over the list that grows as the text wraps
        if let Some((_, e)) = &self.log_edit {
            let lw = if wide && self.heatmap.is_none() && !self.big_clock { w - 46 - 1 } else { w };
            let bw = lw.saturating_sub(1).max(20);
            let iw = bw.saturating_sub(4).max(10) as usize;
            let lines = wrap_input(&e.text, iw);
            let max_rows = body.height.saturating_sub(2).max(1) as usize;
            let (cl, cc) = cursor_in(&e.text, &lines, e.cur, iw);
            let top = cl.saturating_sub(max_rows - 1);
            let shown = lines.len().max(cl + 1).min(max_rows).max(1);
            let r = Rect { x: 1, y: body.y, width: bw.min(w.saturating_sub(1)), height: shown as u16 + 2 };
            f.render_widget(ratatui::widgets::Clear, r);
            let block = Block::default().borders(ratatui::widgets::Borders::ALL).border_type(BorderType::Rounded).border_style(Style::default().fg(t.accent)).style(base)
                .title(Span::styled(format!(" work log · {} ", now.format("%H:%M")), Style::default().fg(t.accent)));
            let inner = block.inner(r);
            f.render_widget(block, r);
            let chars: Vec<char> = e.text.chars().collect();
            let text_lines: Vec<Line> = if e.text.is_empty() { vec![Line::styled(" type what you did · Enter saves", dim)] } else {
                lines.iter().skip(top).take(shown).map(|&(a, b)| Line::from(Span::styled(format!(" {}", chars[a..b].iter().collect::<String>()), Style::default().fg(t.text).add_modifier(Modifier::BOLD)))).collect() };
            f.render_widget(Paragraph::new(text_lines), inner);
            f.set_cursor_position((inner.x + 1 + cc as u16, inner.y + (cl - top) as u16));
        }
    }

    /// The work log file, newest at the bottom; day headings in the accent colour.
    fn draw_log(&mut self, f: &mut Frame, area: Rect, t: &Theme) {
        self.visible = area.height.saturating_sub(1) as usize;
        let lines = crate::worklog::read();
        let path = crate::worklog::path();
        f.buffer_mut().set_string(area.x + 2, area.y, crate::ui::fit(&format!("WORK LOG  ·  {}", path.display()), area.width.saturating_sub(3) as usize), Style::default().fg(t.accent).add_modifier(Modifier::BOLD));
        let body = Rect { x: area.x, y: area.y + 1, width: area.width, height: area.height.saturating_sub(1) };
        if lines.iter().all(|l| l.trim().is_empty() || l.starts_with("# ")) {
            f.buffer_mut().set_string(body.x + 4, body.y + 1, "Nothing logged yet  ·  finish a task (Space) or press W to write what you did", Style::default().fg(t.dim));
            return;
        }
        let max = lines.len().saturating_sub(self.visible);
        let top = self.log_top.unwrap_or(max).min(max);
        let w = body.width.saturating_sub(4) as usize;
        for (k, l) in lines.iter().skip(top).take(self.visible).enumerate() {
            let y = body.y + k as u16;
            let (txt, st) = if let Some(h) = l.strip_prefix("## ") { (h.to_string(), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)) }
                else if l.starts_with("# ") { continue }
                else if let Some(e) = l.strip_prefix("- ") {
                    // "09:14  ✔ title — note": time dim, the rest normal
                    let (time, rest) = e.split_at(e.find("  ").unwrap_or(0));
                    f.buffer_mut().set_string(body.x + 4, y, time, Style::default().fg(t.dim));
                    let rest = rest.trim_start();
                    let x = body.x + 4 + time.width() as u16 + 2;
                    let done = rest.starts_with('✔');
                    f.buffer_mut().set_string(x, y, crate::ui::fit(rest, w.saturating_sub(time.width() + 2)).trim_end(), Style::default().fg(if done { t.ok } else { t.text }));
                    continue;
                } else { (l.clone(), Style::default().fg(t.text)) };
            f.buffer_mut().set_string(body.x + 2, y, crate::ui::fit(&txt, w + 2).trim_end(), st);
        }
        if lines.len() > self.visible {
            let more = format!(" {}-{} of {} ", top + 1, (top + self.visible).min(lines.len()), lines.len());
            f.buffer_mut().set_string(area.x + area.width.saturating_sub(more.len() as u16 + 1), area.y, &more, Style::default().fg(t.dim));
        }
    }

    fn draw_list(&mut self, f: &mut Frame, area: Rect, t: &Theme, now: NaiveDateTime) {
        self.list = area;
        self.visible = area.height as usize;
        let today = now.date();
        // keep the selection on screen
        if let Some(i) = self.sel.and_then(|id| self.rows.iter().position(|r| matches!(r, Row::Task(x) if *x == id))) {
            if i < self.top { self.top = i.saturating_sub(1); }
            if i >= self.top + self.visible { self.top = i + 1 - self.visible; }
        }
        self.top = self.top.min(self.rows.len().saturating_sub(self.visible));
        let w = area.width as usize;
        let dim = Style::default().fg(t.dim);
        for k in 0..self.visible {
            let Some(r) = self.rows.get(self.top + k) else { break };
            let y = area.y + k as u16;
            let rect = Rect { x: area.x, y, width: area.width, height: 1 };
            let line = match r {
                Row::Blank => continue,
                Row::Note(s) => Line::from(Span::styled(format!("      {s}"), dim)),
                Row::Header(name, info) => Line::from(vec![
                    Span::styled(format!("  {name}"), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
                    Span::styled(if info.is_empty() { String::new() } else { format!("   {info}") }, dim),
                ]),
                Row::Task(id) => {
                    let Some(task) = self.task(*id) else { continue };
                    let sel = self.sel == Some(*id);
                    let done = task.is_done(today);
                    let late = task.overdue(now);
                    let bg = if sel { Style::default().bg(t.select_bg) } else { Style::default() };
                    let when = if task.daily && task.days.is_empty() && task.month_day.is_none() { task.time.map(|x| x.format("%H:%M").to_string()).unwrap_or_default() } else { tasks::fmt_when(task, today) };
                    let resting = task.daily && !task.applies(today) && !done;      // repeating, not due today
                    let right = if done {
                        let at = task.done_at.map(|d| if d.date() == today { d.format("%H:%M").to_string() } else { tasks::fmt_day(d.date(), today) }).unwrap_or_default();
                        if at.is_empty() { "done".to_string() } else { format!("done {at}") }
                    } else { tasks::fmt_relative(task, now) };
                    let when_w = 20usize.min(w / 4);
                    let right_w = right.width().max(12);
                    let title_w = w.saturating_sub(7 + when_w + 2 + right_w + 2);
                    let strike = if done { Modifier::CROSSED_OUT } else { Modifier::empty() };
                    let soon = !done && !late && task.next_time(now).map(|x| (x - now).num_minutes() < 60 && x >= now).unwrap_or(false);
                    let title_style = if done { bg.fg(t.dim).add_modifier(strike) } else if resting { bg.fg(t.dim) } else if sel { bg.fg(t.text).add_modifier(Modifier::BOLD) } else { bg.fg(t.text) };
                    // crossed out text, but not the padding after it
                    let split = |s: &str, w: usize| { let f = crate::ui::fit(s, w); let txt = f.trim_end().to_string(); let pad = f.width() - txt.width(); (txt, " ".repeat(pad)) };
                    let (when, no_when) = if when.is_empty() { (if task.daily { "any time".to_string() } else { "no date".to_string() }, true) } else { (when, false) };
                    let (when_txt, when_pad) = split(&when, when_w);
                    // a linked folder / file / site: its name after the title
                    let link_txt = task.link.as_deref().map(|l| format!("  ↗ {}", tasks::link_label(l))).unwrap_or_default();
                    let link_w = link_txt.width().min(title_w / 2);
                    let (title_txt, _) = split(&task.title, title_w.saturating_sub(link_w));
                    let link_txt = crate::ui::fit(&link_txt, link_w).trim_end().to_string();
                    let title_pad = " ".repeat(title_w.saturating_sub(title_txt.width() + link_txt.width()));
                    let mut spans = vec![
                        Span::styled(if sel { "  ► " } else { "    " }, bg.fg(t.accent)),
                        Span::styled(if done { "✔" } else if resting { "·" } else { "○" }, bg.fg(if done { t.ok } else if late { t.error } else { t.dim })),
                        Span::styled("  ", bg),
                        Span::styled(when_txt, bg.fg(if done || no_when { t.dim } else if late { t.error } else { t.accent }).add_modifier(strike)),
                        Span::styled(when_pad, bg),
                        Span::styled("  ", bg),
                        Span::styled(title_txt, title_style),
                        Span::styled(link_txt, bg.fg(if done { t.dim } else { t.accent })),
                        Span::styled(title_pad, bg),
                        Span::styled("  ", bg),
                        Span::styled(format!("{right:>right_w$}"), bg.fg(if done { t.ok } else if late { t.error } else if soon { t.accent } else { t.dim })),
                    ];
                    let used: usize = spans.iter().map(|s| s.content.width()).sum();
                    spans.push(Span::styled(" ".repeat(w.saturating_sub(used)), bg));
                    Line::from(spans)
                }
            };
            f.render_widget(Paragraph::new(line), rect);
        }
        if self.rows.len() > self.visible {
            let more = format!(" {}-{} of {} ", self.top + 1, (self.top + self.visible).min(self.rows.len()), self.rows.len());
            f.buffer_mut().set_string(area.x + area.width.saturating_sub(more.len() as u16 + 1), area.y + area.height.saturating_sub(1), &more, dim);
        }
    }

    fn draw_clock(&mut self, f: &mut Frame, area: Rect, t: &Theme, now: NaiveDateTime, big: bool) {
        if area.height < 3 { return; }
        let cd = self.countdown(now);
        let second = t.rgb.get("error").map(|&(r, g, b)| Color::Rgb(r, g, b)).unwrap_or(t.error);
        let (digits, countdown) = (now.format("%H:%M:%S").to_string(), clock::hms(cd.secs));
        let cd_color = if !cd.active { t.dim } else if cd.urgent { t.error } else { t.accent };
        let next = self.next_line(now);
        let center = |f: &mut Frame, y: u16, x: u16, wdt: u16, s: &str, st: Style| {
            let sw = s.width() as u16;
            if sw <= wdt { f.buffer_mut().set_string(x + (wdt - sw) / 2, y, s, st); } else { f.buffer_mut().set_string(x, y, crate::ui::fit(s, wdt as usize), st); }
        };
        // the text part: big time, date, "time left today", big countdown, day bar, next task
        // the watch shows the time, the weekday and the date itself: no big digits / date line under it
        let use_watch = self.watch && self.picker.is_some();
        let text_h: u16 = if use_watch { 1 + 5 + 2 + 1 + 3 } else { 5 + 1 + 2 + 1 + 5 + 2 + 1 + 3 };
        let side = big && area.width >= 90;
        let (face, text) = if side {
            let fw = (area.width / 2).min(area.height.saturating_mul(2) + 4);
            (Rect { x: area.x + 1, y: area.y, width: fw, height: area.height }, Rect { x: area.x + fw + 2, y: area.y, width: area.width.saturating_sub(fw + 3), height: area.height })
        } else {
            let mut fh = area.height.saturating_sub(text_h + 1);
            if use_watch {
                // the watch is as wide as the panel allows: no taller than that, so the countdown sits right under it
                if let Some(pk) = &self.picker {
                    let fs = pk.font_size();
                    let px = ((area.width.saturating_sub(3) as u32 * fs.width.max(1) as u32) as f32 * 0.94) as u32;
                    fh = fh.min((px / fs.height.max(1) as u32) as u16 + 1);
                }
            }
            let fw = (fh * 2 + 2).min(area.width);
            (Rect { x: area.x + (area.width - fw) / 2, y: area.y, width: fw, height: fh }, Rect { x: area.x, y: area.y + fh + if fh > 0 { 1 } else { 0 }, width: area.width, height: area.height.saturating_sub(fh + 1) })
        };
        self.watch_shown = false;
        if face.height >= 6 {
            if self.watch && self.picker.is_some() {
                // a form box may cover the clock: pictures cannot be partly covered, so leave it out meanwhile
                if self.form.is_none() { self.draw_watch(f, face, t, now); }
            } else {
                clock::analog(f.buffer_mut(), face, now, t.dim, t.text, second, t.background);
            }
        }
        let mut y = text.y + if side { text.height.saturating_sub(text_h) / 2 } else { 0 };
        let bottom = text.y + text.height;
        let fits = |y: u16, n: u16| y + n <= bottom;
        if !use_watch {
            if fits(y, 5) { clock::big_text(f.buffer_mut(), Rect { x: text.x, y, width: text.width, height: 5 }, &digits, Style::default().fg(t.text)); y += 5; }
            else { center(f, y, text.x, text.width, &digits, Style::default().fg(t.text).add_modifier(Modifier::BOLD)); y += 1; }
            if fits(y, 1) { center(f, y, text.x, text.width, &now.format("%A %-d %B").to_string(), Style::default().fg(t.dim)); y += 2; }
        }
        if fits(y, 1) { center(f, y, text.x, text.width, &cd.label, Style::default().fg(if cd.active { t.text } else { t.dim }).add_modifier(Modifier::BOLD)); y += 1; }
        if fits(y, 5) { clock::big_text(f.buffer_mut(), Rect { x: text.x, y, width: text.width, height: 5 }, &countdown, Style::default().fg(cd_color)); y += 5; }
        else if fits(y, 1) { center(f, y, text.x, text.width, &countdown, Style::default().fg(cd_color).add_modifier(Modifier::BOLD)); y += 1; }
        if fits(y, 2) { y += 1; clock::bar(f.buffer_mut(), Rect { x: text.x + 1, y, width: text.width.saturating_sub(2), height: 1 }, cd.progress, &cd.bar_label, if cd.active { cd_color } else { t.accent }, t.dim, t.dim); y += 1; }
        if fits(y, 1) && !cd.detail.is_empty() { center(f, y, text.x, text.width, &cd.detail, Style::default().fg(t.dim)); y += 1; }
        if fits(y, 2) { y += 1; if let Some((s, late)) = next { center(f, y, text.x, text.width, &s, Style::default().fg(if late { t.error } else { t.text })); } }
        if fits(y, 2) {
            if let Some((d, name)) = crate::calendar::next_day_off(now.date()) {
                let days = (d - now.date()).num_days();
                let when = if days == 1 { "tomorrow".to_string() } else { format!("in {days} days") };
                center(f, y + 1, text.x, text.width, &format!("next day off: {name}  ·  {when}"), Style::default().fg(t.ok));
            }
        }
    }

    fn countdown(&self, now: NaiveDateTime) -> clock::Countdown {
        let p = clock::Periods::from_config(&self.cfg.ini).0;
        // a holiday / vacation day in the calendar: no work countdown, just the rest of the day
        if p.any() {
            if let Some(name) = crate::calendar::day_off(now.date()) {
                let left = clock::left_today(now);
                let gone = 1.0 - left as f64 / 86_400.0;
                return clock::Countdown { label: format!("DAY OFF  ·  {name}"), secs: left, active: false, urgent: false, progress: gone,
                    bar_label: format!("{:.0}% of the day gone", gone * 100.0), detail: "enjoy it".into() };
            }
        }
        clock::countdown(&p, now)
    }

    /// The picture watch, as big as fits in `area`, centred.

    fn draw_watch(&mut self, f: &mut Frame, area: Rect, t: &Theme, now: NaiveDateTime) {
        let Some(picker) = self.picker.clone() else { return };
        let (cw, ch) = { let fs = picker.font_size(); (fs.width.max(1) as u32, fs.height.max(1) as u32) };
        // keep a margin: if the cell size the terminal reported is a little off, the picture must still fit
        // (Windows Terminal drops a Sixel picture that would go past the edge of the window)
        let area = Rect { x: area.x + 1, y: area.y, width: area.width.saturating_sub(3), height: area.height };
        let px = ((area.width as u32 * cw).min(area.height as u32 * ch) as f32 * 0.94) as u32;
        let px = px.min(900);

        if px < 40 { return; }
        let rgb = |c: Color, d: (u8, u8, u8)| match c { Color::Rgb(r, g, b) => (r, g, b), _ => d };
        let bg = t.bg_rgb;
        let colors = crate::watch::Colors { dial: crate::watch::dial_color(bg), hands: rgb(t.text, (235, 235, 230)), second: rgb(t.error, (230, 70, 60)), accent: rgb(t.accent, (99, 179, 237)), bg };
        let Some(img) = crate::watch::render(now, px, &colors) else { return };
        let cols = ((px + cw - 1) / cw) as u16;
        let rows = ((px + ch - 1) / ch) as u16;
        let rect = Rect { x: area.x + area.width.saturating_sub(cols) / 2, y: area.y + area.height.saturating_sub(rows) / 2, width: cols.min(area.width), height: rows.min(area.height) };
        // the picture on a canvas that lines up exactly with the character cells
        let (w, h) = (rect.width as u32 * cw, rect.height as u32 * ch);
        let mut canvas = image::RgbaImage::from_pixel(w, h, image::Rgba([bg.0, bg.1, bg.2, 255]));
        image::imageops::overlay(&mut canvas, &img.to_rgba8(), ((w as i64 - px as i64) / 2).max(0), ((h as i64 - px as i64) / 2).max(0));
        // cells under the picture are left alone by the text drawing (anything written there would erase it)
        for y in rect.top()..rect.bottom() {
            for x in rect.left()..rect.right() {
                if let Some(c) = f.buffer_mut().cell_mut((x, y)) { c.set_diff_option(ratatui::buffer::CellDiffOption::Skip); }
            }
        }
        // piece by piece only when the cell size is known exactly (else the pieces would not line up)
        let exact = crate::app::CELL_MEASURED.load(std::sync::atomic::Ordering::Relaxed) || picker.protocol_type() == ratatui_image::picker::ProtocolType::Halfblocks;
        let full = !exact || self.watch_full || self.watch_prev.as_ref().map(|(r, _)| *r != rect).unwrap_or(true);
        if full {
            if let Some(proto) = watch_proto(&picker, image::DynamicImage::ImageRgba8(canvas.clone()), rect.width, rect.height) {
                f.render_widget(ratatui_image::Image::new(&proto), rect);
            }
        } else if let Some((_, prev)) = &self.watch_prev {
            // only the part where a hand moved: the box around every 2 x 2 cell block that differs from the last frame,
            // sent as ONE picture so the terminal never shows half a frame
            let (tc, tr) = (2u16, 2u16);
            let (mut x1, mut y1, mut x2, mut y2) = (u16::MAX, u16::MAX, 0u16, 0u16);
            let mut ty = 0u16;
            while ty < rect.height {
                let th = tr.min(rect.height - ty);
                let mut tx = 0u16;
                while tx < rect.width {
                    let tw = tc.min(rect.width - tx);
                    let (x0, y0, pw, ph) = (tx as u32 * cw, ty as u32 * ch, tw as u32 * cw, th as u32 * ch);
                    let changed = (y0..y0 + ph).any(|yy| {
                        let a = (yy * w + x0) as usize * 4;
                        canvas.as_raw()[a..a + pw as usize * 4] != prev.as_raw()[a..a + pw as usize * 4]
                    });
                    if changed { x1 = x1.min(tx); y1 = y1.min(ty); x2 = x2.max(tx + tw); y2 = y2.max(ty + th); }
                    tx += tc;
                }
                ty += tr;
            }
            if x2 > x1 && y2 > y1 {
                let (tw, th) = (x2 - x1, y2 - y1);
                let part = image::imageops::crop_imm(&canvas, x1 as u32 * cw, y1 as u32 * ch, tw as u32 * cw, th as u32 * ch).to_image();
                if let Some(proto) = watch_proto(&picker, image::DynamicImage::ImageRgba8(part), tw, th) {
                    f.render_widget(ratatui_image::Image::new(&proto), Rect { x: rect.x + x1, y: rect.y + y1, width: tw, height: th });
                }
            }
        }
        self.watch_prev = Some((rect, canvas));
        self.watch_full = false;
        self.watch_shown = true;
    }

    /// "Next: Call the bank · in 2h 05m"
    fn next_line(&self, now: NaiveDateTime) -> Option<(String, bool)> {
        let ts = &self.store.tasks;
        let late = ts.iter().filter(|t| t.overdue(now)).count();
        let next = ts.iter().filter(|t| !t.is_done(now.date()) && !t.overdue(now)).filter_map(|t| t.next_time(now).filter(|x| *x >= now).map(|x| (x, t))).min_by_key(|(x, _)| *x);
        match next {
            Some((x, t)) => {
                let mins = (x - now).num_minutes();
                let s = format!("Next: {}  ·  {}", t.title, if mins < 1 { "now".to_string() } else { format!("in {}", tasks::fmt_span(mins)) });
                Some(if late > 0 { (format!("{late} late  ·  {s}"), true) } else { (s, false) })
            }
            None if late > 0 => Some((format!("{late} {} late", if late == 1 { "task" } else { "tasks" }), true)),
            None => None,
        }
    }

    fn draw_form(&self, f: &mut Frame, t: &Theme, now: NaiveDateTime) {
        let Some(form) = &self.form else { return };
        let area = f.area();
        let bw = 66u16.min(area.width.saturating_sub(2));
        let bh = 16u16.min(area.height);
        let r = Rect { x: area.x + (area.width - bw) / 2, y: area.y + area.height.saturating_sub(bh) / 2, width: bw, height: bh };
        f.render_widget(Clear, r);
        let bg = t.background.unwrap_or(Color::Reset);
        let title = match (form.editing.is_some(), form.daily) { (true, _) => " Edit task ", (false, true) => " New repeating task ", (false, false) => " New task " };
        let block = Block::bordered().border_type(BorderType::Rounded).title(Span::styled(title, Style::default().fg(t.accent).add_modifier(Modifier::BOLD)))
            .border_style(Style::default().fg(t.accent)).style(Style::default().bg(bg).fg(t.text));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let lw = 9u16;
        let fw = inner.width.saturating_sub(lw + 2);
        let preview = tasks::resolve(&form.date.text, &form.time.text, form.daily, now);
        let mut cursor = None;
        let mut y = inner.y + 1;
        let field = |f: &mut Frame, y: u16, label: &str, active: bool| {
            f.buffer_mut().set_string(inner.x + 1, y, label, Style::default().fg(if active { t.accent } else { t.dim }).add_modifier(if active { Modifier::BOLD } else { Modifier::empty() }));
        };
        let input = |f: &mut Frame, y: u16, e: &Edit, active: bool, width: u16| -> Option<(u16, u16)> {
            let st = if active { Style::default().bg(t.select_bg).fg(t.text) } else { Style::default().fg(t.text) };
            let chars: Vec<char> = e.text.chars().collect();
            let before: String = chars[..e.cur].iter().collect();
            let skip = before.width().saturating_sub(width.saturating_sub(2) as usize);
            let mut shown = String::new(); let mut acc = 0;
            for c in e.text.chars() { let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0); if acc >= skip { shown.push(c); } acc += cw; }
            f.buffer_mut().set_string(inner.x + lw + 1, y, crate::ui::fit(&shown, width as usize), st);
            if active { Some((inner.x + lw + 1 + (before.width() - skip) as u16, y)) } else { None }
        };
        // title
        field(f, y, "Task", form.field == 0);
        if let Some(c) = input(f, y, &form.title, form.field == 0, fw) { cursor = Some(c); }
        y += 2;
        // repeat
        field(f, y, "Repeat", form.field == 1);
        let opt = |on: bool, s: &str| Span::styled(format!("{} {s}   ", if on { "◉" } else { "○" }), Style::default().fg(if on { t.accent } else { t.dim }).add_modifier(if on && form.field == 1 { Modifier::BOLD } else { Modifier::empty() }));
        f.render_widget(Paragraph::new(Line::from(vec![opt(!form.daily, "Once"), opt(form.daily, "Repeats")])), Rect { x: inner.x + lw + 1, y, width: fw, height: 1 });
        y += 2;
        // date
        let iw = 16u16.min(fw);
        field(f, y, if form.daily { "On" } else { "Date" }, form.field == 2);
        if let Some(c) = input(f, y, if form.daily { &form.on } else { &form.date }, form.field == 2, iw) { cursor = Some(c); }
        y += 1;
        field(f, y, "Time", form.field == 3);
        if let Some(c) = input(f, y, &form.time, form.field == 3, iw) { cursor = Some(c); }
        // what it means
        let px = inner.x + lw + 1 + iw + 2;
        let pw = inner.width.saturating_sub(lw + iw + 4) as usize;
        let (ptext, pst) = match &preview {
            Ok(_) if form.daily && tasks::parse_repeat(&form.on.text).is_err() => ("? days not understood".to_string(), Style::default().fg(t.error)),
            Ok((d, tm)) => {
                let mut tmp = tasks::new_task("", form.daily, *d, *tm);
                tmp.date = *d;
                if let Ok((days, md)) = tasks::parse_repeat(&form.on.text) { if form.daily { tmp.days = days; tmp.month_day = md; } }
                let when = tasks::fmt_when(&tmp, now.date());
                let rel = tasks::fmt_relative(&tmp, now);
                let s = if when.is_empty() { "no date".to_string() } else if rel.is_empty() { format!("→ {when}") } else { format!("→ {when}  ·  {rel}") };
                (s, Style::default().fg(t.ok))
            }
            Err(e) => (if e.starts_with("date") { "? date not understood".into() } else { "? time not understood".into() }, Style::default().fg(t.error)),
        };
        f.buffer_mut().set_string(px, y - 1, crate::ui::fit(&ptext, pw), pst);
        y += 2;
        // link: a folder, file or web address that G opens
        field(f, y, "Link", form.field == 4);
        if let Some(c) = input(f, y, &form.link, form.field == 4, fw) { cursor = Some(c); }
        y += 1;
        let (ltext, lst) = match tasks::clean_link(&form.link.text) {
            None => ("Ctrl+O picks a file or folder (or type / paste a path or web address)".to_string(), Style::default().fg(t.dim)),
            Some(l) => match tasks::link_kind(&l) {
                "not found" => ("? not found on this PC (saved anyway)".to_string(), Style::default().fg(t.error)),
                k => (format!("→ {k}: {}", tasks::link_label(&l)), Style::default().fg(t.ok)),
            },
        };
        f.buffer_mut().set_string(inner.x + lw + 1, y, crate::ui::fit(&ltext, fw as usize), lst);
        y += 2;
        let help = if form.daily { "on: every day · mon · mon, wed, fri · mon-fri · weekends · 15th (monthly)    time: 8:00 · 9pm · empty" } else { "date: today · tmr · fri · +3 · 20/10 · 20/10/27    time: 14:30 · 9pm · +30m · +2h" };
        f.buffer_mut().set_string(inner.x + 1, y, crate::ui::fit(help, inner.width.saturating_sub(2) as usize), Style::default().fg(t.dim));
        y += 2;
        if let Some(e) = &form.error { f.buffer_mut().set_string(inner.x + 1, y, crate::ui::fit(e, inner.width.saturating_sub(2) as usize), Style::default().fg(t.error).add_modifier(Modifier::BOLD)); }
        if let Some(c) = cursor { f.set_cursor_position(c); }
    }
}

/// Run the screen until Esc / Q. The caller owns the terminal (full-screen mode, mouse on).
/// A watch picture for the terminal. For Sixel the "erase these cells first" that ratatui-image puts before
/// every picture is left out: the watch picture covers its cells completely, and erasing first is what made
/// the moving hands flicker (the terminal could show the blank cells for a moment before the new picture).
fn watch_proto(picker: &ratatui_image::picker::Picker, img: image::DynamicImage, w: u16, h: u16) -> Option<ratatui_image::protocol::Protocol> {
    let proto = picker.new_protocol(img, ratatui::layout::Size::new(w, h), ratatui_image::Resize::Fit(None)).ok()?;
    Some(match proto {
        ratatui_image::protocol::Protocol::Sixel(mut sx) if !sx.is_tmux => {
            if let Some(i) = sx.data.find("\x1bP") { sx.data.drain(..i); }
            ratatui_image::protocol::Protocol::Sixel(sx)
        }
        other => other,
    })
}

/// Synchronized output (DEC 2026): the terminal shows the whole frame at once instead of while it arrives.
/// Terminals that do not know it ignore it.
fn sync_frame(begin: bool) {
    use std::io::Write;
    let mut o = std::io::stdout();
    let _ = o.write_all(if begin { b"\x1b[?2026h" } else { b"\x1b[?2026l" });
    let _ = o.flush();
}

/// Word-wrap a one-line input into rows of at most `width` columns: (first char, end char) of each row.
/// Spaces stay at the end of their row, so every character belongs to exactly one row (for the cursor).
fn wrap_input(text: &str, width: usize) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let cw = |c: char| unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
    let mut rows = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let (mut i, mut col, mut last_space) = (start, 0usize, None);
        while i < chars.len() && col + cw(chars[i]) <= width {
            if chars[i] == ' ' { last_space = Some(i); }
            col += cw(chars[i]);
            i += 1;
        }
        if i >= chars.len() { rows.push((start, chars.len())); break; }
        // break after the last space on this row, if there is one; a space right at the edge also belongs here
        let end = if chars[i] == ' ' { i + 1 } else { match last_space { Some(sp) if sp > start => sp + 1, _ => i.max(start + 1) } };
        rows.push((start, end));
        start = end;
    }
    if rows.is_empty() { rows.push((0, 0)); }
    rows
}

/// Row and column of the cursor (char index `cur`) in the wrapped rows.
fn cursor_in(text: &str, rows: &[(usize, usize)], cur: usize, width: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    let cw = |c: &char| unicode_width::UnicodeWidthChar::width(*c).unwrap_or(0);
    for (r, &(a, b)) in rows.iter().enumerate() {
        if cur < b || r == rows.len() - 1 {
            let col: usize = chars[a..cur.min(chars.len()).max(a)].iter().map(cw).sum();
            // at the very end of a full row: show the cursor at the start of the next one
            if col >= width { return (r + 1, 0); }
            return (r, col);
        }
    }
    (0, 0)
}

/// `picker`: the file browser's picture settings (None = find out here).
pub fn run(terminal: &mut ratatui::DefaultTerminal, picker: Option<ratatui_image::picker::Picker>) -> std::io::Result<()> {
    run_with(terminal, picker, false)
}

/// Open a task's link: a folder in the file browser (back here when it closes), a text file in the app's
/// editor, anything else (pictures, documents, web addresses) in its own app. Returns (status, is error).
fn open_link(terminal: &mut ratatui::DefaultTerminal, link: &str) -> std::io::Result<(String, bool)> {
    use ratatui::crossterm::{event::{DisableMouseCapture, EnableMouseCapture}, execute};
    let label = tasks::link_label(link);
    if tasks::is_web(link) {
        let url = if link.to_ascii_lowercase().starts_with("www.") { format!("https://{link}") } else { link.to_string() };
        return Ok(match crate::winapi::open_default(std::path::Path::new(&url)) { Ok(_) => (format!("Opened {label}"), false), Err(e) => (format!("Cannot open {label}: {e}"), true) });
    }
    let path = std::path::PathBuf::from(link);
    if path.is_dir() {
        let Ok(exe) = std::env::current_exe() else { return Ok(("Cannot find file-browser.exe".into(), true)) };
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
        ratatui::restore();
        let r = std::process::Command::new(exe).arg(&path).status();
        *terminal = ratatui::init();
        execute!(std::io::stdout(), EnableMouseCapture)?;
        let _ = terminal.clear();
        return Ok(match r { Ok(_) => (format!("Back from {label}"), false), Err(e) => (format!("Cannot open {label}: {e}"), true) });
    }
    if !path.is_file() { return Ok((format!("{link} is not there any more  ·  → edits the task"), true)); }
    let mut b = vec![0u8; 8192];
    let n = std::fs::File::open(&path).and_then(|mut f| std::io::Read::read(&mut f, &mut b)).unwrap_or(0);
    let ext = crate::fsutil::ext_of(&path);
    let office = crate::preview::OFFICE_EXTS.contains(&ext.as_str()) || crate::preview::ZIP_EXTS.contains(&ext.as_str()) || crate::preview::EXE_EXTS.contains(&ext.as_str());
    if !office && (n == 0 || !crate::text::looks_binary(&b[..n])) {
        return Ok(match crate::ui::edit_file(terminal, &path, false)? { Some(err) => (err, true), None => (format!("Closed {label}"), false) });
    }
    Ok(match crate::winapi::open_default(&path) { Ok(_) => (format!("Opened {label}"), false), Err(e) => (format!("Cannot open {label}: {e}"), true) })
}

/// Ctrl+O in the task form: the file browser as a picker (bookmarks, search and all). Ok(None) = cancelled.
fn pick_link(terminal: &mut ratatui::DefaultTerminal, start: Option<std::path::PathBuf>) -> std::io::Result<Result<Option<String>, String>> {
    use ratatui::crossterm::{event::{DisableMouseCapture, EnableMouseCapture}, execute};
    let Ok(exe) = std::env::current_exe() else { return Ok(Err("Cannot find file-browser.exe".into())) };
    let answer = crate::notify::data_dir().join(format!("pick-{}.txt", std::process::id()));
    let _ = std::fs::create_dir_all(crate::notify::data_dir());
    let _ = std::fs::remove_file(&answer);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--pick").arg(&answer);
    if let Some(d) = start { cmd.arg(d); }
    let r = cmd.status();
    *terminal = ratatui::init();
    execute!(std::io::stdout(), EnableMouseCapture)?;
    let _ = terminal.clear();
    if let Err(e) = r { return Ok(Err(format!("Cannot start the file browser: {e}"))); }
    let picked = std::fs::read_to_string(&answer).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let _ = std::fs::remove_file(&answer);
    Ok(Ok(picked))
}

/// From the file browser (Alt+L): a new task linked to this file or folder, its name as the title.
pub fn run_link(terminal: &mut ratatui::DefaultTerminal, picker: Option<ratatui_image::picker::Picker>, link: std::path::PathBuf) -> std::io::Result<()> {
    run_inner(terminal, picker, false, Some(link))
}

/// `heatmap`: open straight on the activity heatmap.
pub fn run_with(terminal: &mut ratatui::DefaultTerminal, picker: Option<ratatui_image::picker::Picker>, heatmap: bool) -> std::io::Result<()> {
    run_inner(terminal, picker, heatmap, None)
}

fn run_inner(terminal: &mut ratatui::DefaultTerminal, picker: Option<ratatui_image::picker::Picker>, heatmap: bool, link: Option<std::path::PathBuf>) -> std::io::Result<()> {
    let picker = picker.or_else(|| {
        let cfg = Config::load();
        let (mut p, _) = crate::app::build_picker(&cfg.image_mode());
        let (r, g, b) = cfg.theme.bg_rgb;
        p.set_background_color(Some(image::Rgba([r, g, b, 255])));
        Some(p)
    });
    let mut p = Planner::new(picker);
    if heatmap { p.heatmap = Some(crate::heatmap::Heatmap::load()); }
    if let Some(l) = link {
        p.open_form(false, false);
        if let Some(f) = p.form.as_mut() {
            let name = l.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            f.title = Edit::new(&name);
            f.link = Edit::new(&l.to_string_lossy());
            f.date = Edit::default();
        }
        p.set_status("New task linked to this file / folder: type what to do, Enter saves", false);
    }
    let fps: u64 = p.cfg.ini.get("tasks", "watch_fps").and_then(|v| v.trim().parse().ok()).unwrap_or(6).clamp(1, 20);
    let mut overlay = false;
    let _ = terminal.clear();
    let mut last_check = Instant::now();
    let mut theme = p.cfg.theme.name.clone();
    loop {
        // a form box opening / closing over a picture: repaint everything
        if p.calendar_request {
            p.calendar_request = false;
            crate::calendar::run(terminal)?;
            p.repaint = true;
        }
        if p.pick_request {
            p.pick_request = false;
            let start = p.form.as_ref().and_then(|f| tasks::clean_link(&f.link.text)).map(std::path::PathBuf::from)
                .map(|l| if l.is_dir() { l } else { l.parent().map(|x| x.to_path_buf()).unwrap_or(l) }).filter(|d| d.is_dir())
                // else where the last pick was, else the browser's start folder
                .or_else(|| std::fs::read_to_string(crate::notify::data_dir().join("last-pick.txt")).ok().map(|s| std::path::PathBuf::from(s.trim())).filter(|d| d.is_dir()))
                .or_else(|| p.cfg.start_dir().filter(|d| d.is_dir()));
            match pick_link(terminal, start)? {
                Ok(Some(path)) => {
                    let pp = std::path::Path::new(&path);
                    if let Some(dir) = pp.parent() { let _ = std::fs::write(crate::notify::data_dir().join("last-pick.txt"), dir.to_string_lossy().as_bytes()); }
                    if let Some(f) = p.form.as_mut() {
                        f.link = Edit::new(&path);
                        // a new task without a title yet: the file / folder name
                        if f.title.text.trim().is_empty() { f.title = Edit::new(&tasks::link_label(&path).trim_end_matches(['\\', '/']).to_string()); }
                    }
                    p.set_status("Link picked  ·  Enter saves the task", false);
                }
                Ok(None) => p.set_status("No link picked", false),
                Err(e) => p.set_status(&e, true),
            }
            p.repaint = true;
        }
        if let Some(link) = p.open_request.take() {
            let msg = open_link(terminal, &link)?;
            p.set_status(&msg.0, msg.1);
            p.repaint = true;
        }
        if let Some(path) = p.edit_request.take() {
            if let Some(err) = crate::ui::edit_file(terminal, &path, true)? { p.set_status(&err, true); }
            else { p.set_status(&format!("Closed {}", path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()), false); }
            p.logged_today = crate::worklog::today_count(now());
            if p.heatmap.is_some() { p.heatmap = Some(crate::heatmap::Heatmap::load()); }
            p.repaint = true;
        }
        if p.form.is_some() != overlay || p.repaint { overlay = p.form.is_some(); p.repaint = false; { let _ = terminal.clear(); p.watch_full = true; } }
        sync_frame(true);
        let drawn = terminal.draw(|f| p.draw(f));
        sync_frame(false);
        drawn?;
        // wake up at the next full second so the clock ticks on time (the watch sweeps: several frames a second)
        // the watch: frames on an even beat of the wall clock, so the second hand steps evenly
        let ms = if p.watch_shown { let per = 1000 / fps; per - (Local::now().timestamp_subsec_millis() as u64 % per) } else { 1000 - (Local::now().timestamp_subsec_millis() % 1000) as u64 };
        if event::poll(Duration::from_millis(ms.max(5)))? {
            loop {
                match event::read()? {
                    Event::Key(k) if k.kind != KeyEventKind::Release => { if !p.on_key(k) { return Ok(()); } }
                    Event::Mouse(m) => p.on_mouse(m),
                    Event::Resize(_, _) => { { let _ = terminal.clear(); p.watch_full = true; } }
                    _ => {}
                }
                if !event::poll(Duration::from_millis(0))? { break; }
            }
        }
        p.poll_ai();
        if p.cfg.theme.name != theme { theme = p.cfg.theme.name.clone(); { let _ = terminal.clear(); p.watch_full = true; } }
        if last_check.elapsed() > Duration::from_secs(1) {
            last_check = Instant::now();
            if p.store.reload_if_changed() { p.rebuild(); }
            if p.cfg.changed() { p.cfg.reload(); { let _ = terminal.clear(); p.watch_full = true; } }
            // the day changed or a task became late: re-sort
            p.rebuild();
        }
    }
}

/// "2 late · 3 to do today" for the launcher header (None = nothing to say).
pub fn summary() -> Option<String> {
    let s = Store::open(tasks::default_path());
    let now = now();
    let late = s.tasks.iter().filter(|t| t.overdue(now)).count();
    let today = s.tasks.iter().filter(|t| !t.is_done(now.date()) && !t.overdue(now) && (t.applies(now.date()) || (!t.daily && t.date == Some(now.date())))).count();
    match (late, today) {
        (0, 0) => None,
        (0, n) => Some(format!("{n} {} today", if n == 1 { "task" } else { "tasks" })),
        (l, 0) => Some(format!("{l} late")),
        (l, n) => Some(format!("{l} late  ·  {n} today")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wraps_input() {
        let t = "fixed the printer on the second floor";
        let rows = wrap_input(t, 12);
        let parts: Vec<String> = rows.iter().map(|&(a, b)| t.chars().skip(a).take(b - a).collect()).collect();
        assert_eq!(parts, vec!["fixed the ", "printer on ", "the second ", "floor"]);
        assert_eq!(cursor_in(t, &rows, 0, 12), (0, 0));
        assert_eq!(cursor_in(t, &rows, 10, 12), (1, 0));
        assert_eq!(cursor_in(t, &rows, t.chars().count(), 12), (3, 5));
        let long = "abcdefghijklmnop";
        let rows = wrap_input(long, 5);
        assert_eq!(rows, vec![(0, 5), (5, 10), (10, 15), (15, 16)]);
        assert_eq!(cursor_in(long, &wrap_input("abcde", 5), 5, 5), (1, 0));
    }
}
