//! Calendar (Alt+C in the browser, K in Tasks & clock): a month grid with holidays, vacations and
//! events (calendar.json next to tasks.json), the tasks that are due each day, the next days off,
//! and how many work days off you have taken this year. Holidays and vacations also stop the
//! work-hours countdown for that day.

use crate::config::{Config, Theme};
use chrono::{Datelike, Duration, Local, NaiveDate, Weekday};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use ratatui::Frame;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration as StdDur, SystemTime};
use unicode_width::UnicodeWidthStr;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind { Holiday, Vacation, Event }

impl Kind {
    fn key(&self) -> &'static str { match self { Kind::Holiday => "holiday", Kind::Vacation => "vacation", Kind::Event => "event" } }
    fn label(&self) -> &'static str { match self { Kind::Holiday => "Holiday", Kind::Vacation => "Vacation", Kind::Event => "Event" } }
    pub fn from(s: &str) -> Kind { match s { "holiday" => Kind::Holiday, "vacation" => Kind::Vacation, _ => Kind::Event } }
    /// a day off: no work countdown
    pub fn off(&self) -> bool { matches!(self, Kind::Holiday | Kind::Vacation) }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry { pub id: u64, pub title: String, pub kind: Kind, pub from: NaiveDate, pub to: NaiveDate }

impl Entry {
    pub fn covers(&self, d: NaiveDate) -> bool { d >= self.from && d <= self.to }
}

pub fn path() -> PathBuf { crate::tasks::default_path().with_file_name("calendar.json") }

pub fn load() -> (Vec<Entry>, u64) {
    let Ok(t) = std::fs::read_to_string(path()) else { return (Vec::new(), 1) };
    let Ok(v) = serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')) else { return (Vec::new(), 1) };
    let d = |x: &Value, k: &str| x.get(k).and_then(|s| s.as_str()).and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
    let list: Vec<Entry> = v.get("events").and_then(|e| e.as_array()).map(|a| a.iter().filter_map(|x| {
        let from = d(x, "from")?;
        Some(Entry { id: x.get("id")?.as_u64()?, title: x.get("title")?.as_str()?.to_string(), kind: Kind::from(x.get("kind").and_then(|k| k.as_str()).unwrap_or("event")),
            from, to: d(x, "to").unwrap_or(from).max(from) })
    }).collect()).unwrap_or_default();
    let next = v.get("next_id").and_then(|n| n.as_u64()).unwrap_or(1).max(list.iter().map(|e| e.id).max().unwrap_or(0) + 1);
    (list, next)
}

pub fn save(list: &[Entry], next: u64) -> Result<(), String> {
    let mut sorted = list.to_vec();
    sorted.sort_by_key(|e| (e.from, e.id));
    let v = json!({ "next_id": next, "events": sorted.iter().map(|e| json!({ "id": e.id, "title": e.title, "kind": e.kind.key(),
        "from": e.from.format("%Y-%m-%d").to_string(), "to": e.to.format("%Y-%m-%d").to_string() })).collect::<Vec<_>>() });
    let p = path();
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&v).unwrap_or_default()).map_err(|e| format!("Cannot save calendar.json: {e}"))?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("Cannot save calendar.json: {e}"))
}

// the clock asks several times a second: re-read the file only when it changed
static CACHE: Mutex<Option<(Option<SystemTime>, Vec<Entry>)>> = Mutex::new(None);

fn cached() -> Vec<Entry> {
    let m = std::fs::metadata(path()).and_then(|m| m.modified()).ok();
    let mut c = CACHE.lock().unwrap();
    if c.as_ref().map(|(t, _)| *t != m).unwrap_or(true) { *c = Some((m, load().0)); }
    c.as_ref().map(|(_, v)| v.clone()).unwrap_or_default()
}

/// A holiday / vacation on this day (the work countdown is off).
pub fn day_off(d: NaiveDate) -> Option<String> {
    cached().into_iter().find(|e| e.kind.off() && e.covers(d)).map(|e| e.title)
}

/// The next day off after `d`: (first day, title).
pub fn next_day_off(d: NaiveDate) -> Option<(NaiveDate, String)> {
    cached().into_iter().filter(|e| e.kind.off() && e.to > d).map(|e| (e.from.max(d + Duration::days(1)), e.title)).min_by_key(|(x, _)| *x)
}

/// Easter Sunday (Gregorian, anonymous algorithm).
pub fn easter(y: i32) -> NaiveDate {
    let a = y % 19; let b = y / 100; let c = y % 100; let d = b / 4; let e = b % 4;
    let f = (b + 8) / 25; let g = (b - f + 1) / 3; let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4; let k = c % 4; let l = (32 + 2 * e + 2 * i - h - k) % 7; let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31; let day = (h + l - 7 * m + 114) % 31 + 1;
    NaiveDate::from_ymd_opt(y, month as u32, day as u32).unwrap()
}

/// Spain's national holidays for a year (regional and local ones differ: add those yourself).
pub fn spain_national(y: i32) -> Vec<(NaiveDate, &'static str)> {
    let d = |m, day| NaiveDate::from_ymd_opt(y, m, day).unwrap();
    vec![(d(1, 1), "Año Nuevo"), (d(1, 6), "Reyes"), (easter(y) - Duration::days(2), "Viernes Santo"), (d(5, 1), "Día del Trabajo"),
         (d(8, 15), "Asunción de la Virgen"), (d(10, 12), "Fiesta Nacional de España"), (d(11, 1), "Todos los Santos"),
         (d(12, 6), "Día de la Constitución"), (d(12, 8), "Inmaculada Concepción"), (d(12, 25), "Navidad")]
}

fn first_of_month(d: NaiveDate) -> NaiveDate { NaiveDate::from_ymd_opt(d.year(), d.month(), 1).unwrap() }
fn add_months(d: NaiveDate, n: i32) -> NaiveDate {
    let m0 = d.year() * 12 + d.month() as i32 - 1 + n;
    let (y, m) = (m0.div_euclid(12), m0.rem_euclid(12) as u32 + 1);
    let last = (1..=31).rev().find(|&x| NaiveDate::from_ymd_opt(y, m, x).is_some()).unwrap();
    NaiveDate::from_ymd_opt(y, m, d.day().min(last)).unwrap()
}

/// Work days (Mon-Fri) taken off in `year` by holidays and vacations.
pub fn days_off_in_year(list: &[Entry], year: i32) -> (usize, usize) {
    let (mut hol, mut vac) = (0, 0);
    let mut d = NaiveDate::from_ymd_opt(year, 1, 1).unwrap();
    while d.year() == year {
        if d.weekday().num_days_from_monday() < 5 {
            if list.iter().any(|e| e.kind == Kind::Holiday && e.covers(d)) { hol += 1; }
            else if list.iter().any(|e| e.kind == Kind::Vacation && e.covers(d)) { vac += 1; }
        }
        d += Duration::days(1);
    }
    (hol, vac)
}

struct Form { editing: Option<u64>, field: usize, title: String, from: String, to: String, kind: Kind, error: String }

pub struct Cal {
    cfg: Config,
    list: Vec<Entry>,
    next_id: u64,
    tasks: Vec<crate::tasks::Task>,
    sel: NaiveDate,
    today: NaiveDate,
    form: Option<Form>,
    confirm: Option<(String, Vec<u64>, Vec<(NaiveDate, String)>)>,   // question, ids to delete / holidays to add
    status: (String, bool),
    grid: Rect,
    cell: (u16, u16),
    first: NaiveDate,
}

impl Cal {
    pub fn new() -> Cal {
        let (list, next_id) = load();
        let today = Local::now().date_naive();
        let tasks = crate::tasks::Store::open(crate::tasks::default_path()).tasks;
        Cal { cfg: Config::load(), list, next_id, tasks, sel: today, today, form: None, confirm: None, status: (String::new(), false), grid: Rect::default(), cell: (1, 1), first: today }
    }

    fn say(&mut self, s: &str, err: bool) { self.status = (s.to_string(), err); }

    fn on(&self, d: NaiveDate) -> Vec<&Entry> { self.list.iter().filter(|e| e.covers(d)).collect() }

    /// Tasks due that day: one-off ones, and repeating ones that are not every day (every day would fill every cell).
    fn tasks_on(&self, d: NaiveDate) -> Vec<&crate::tasks::Task> {
        self.tasks.iter().filter(|t| if t.daily { (t.month_day.is_some() || !t.days.is_empty()) && t.applies(d) } else { t.date == Some(d) }).collect()
    }

    fn persist(&mut self) -> bool {
        match save(&self.list, self.next_id) { Ok(()) => true, Err(e) => { self.say(&e, true); false } }
    }

    fn open_form(&mut self, edit: Option<Entry>) {
        let f = |d: NaiveDate| d.format("%d/%m/%Y").to_string();
        self.form = Some(match edit {
            Some(e) => Form { editing: Some(e.id), field: 0, title: e.title.clone(), from: f(e.from), to: f(e.to), kind: e.kind, error: String::new() },
            None => Form { editing: None, field: 0, title: String::new(), from: f(self.sel), to: f(self.sel), kind: Kind::Vacation, error: String::new() },
        });
    }

    fn submit(&mut self) {
        let Some(f) = self.form.as_mut() else { return };
        let title = f.title.trim().to_string();
        if title.is_empty() { f.error = "Type a name first".into(); f.field = 0; return; }
        let from = match crate::tasks::parse_date(&f.from, self.today) { Ok(Some(d)) => d, _ => { f.error = "From: date not understood".into(); f.field = 2; return; } };
        let to = match crate::tasks::parse_date(&f.to, from) { Ok(Some(d)) => d, Ok(None) => from, Err(_) => { f.error = "To: date not understood".into(); f.field = 3; return; } };
        if to < from { f.error = "To is before From".into(); f.field = 3; return; }
        let (editing, kind) = (f.editing, f.kind);
        self.form = None;
        match editing {
            Some(id) => if let Some(e) = self.list.iter_mut().find(|e| e.id == id) { e.title = title.clone(); e.from = from; e.to = to; e.kind = kind; },
            None => { self.list.push(Entry { id: self.next_id, title: title.clone(), kind, from, to }); self.next_id += 1; }
        }
        if self.persist() {
            let days = (to - from).num_days() + 1;
            let span = if days == 1 { from.format("%a %-d %b %Y").to_string() } else { format!("{} – {}  ({days} days)", from.format("%a %-d %b"), to.format("%a %-d %b %Y")) };
            self.say(&format!("Saved: {title}  ·  {}  ·  {span}", kind.label()), false);
            self.sel = from;
        }
    }

    fn form_key(&mut self, k: KeyEvent) {
        let Some(f) = self.form.as_mut() else { return };
        f.error.clear();
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        match k.code {
            KeyCode::Esc => self.form = None,
            KeyCode::Enter => self.submit(),
            KeyCode::Tab | KeyCode::Down => f.field = if shift { (f.field + 3) % 4 } else { (f.field + 1) % 4 },
            KeyCode::BackTab | KeyCode::Up => f.field = (f.field + 3) % 4,
            _ if f.field == 1 => {
                if matches!(k.code, KeyCode::Left) { f.kind = match f.kind { Kind::Holiday => Kind::Event, Kind::Vacation => Kind::Holiday, Kind::Event => Kind::Vacation }; }
                if matches!(k.code, KeyCode::Right | KeyCode::Char(' ')) { f.kind = match f.kind { Kind::Holiday => Kind::Vacation, Kind::Vacation => Kind::Event, Kind::Event => Kind::Holiday }; }
            }
            KeyCode::Char('u') if k.modifiers.contains(KeyModifiers::CONTROL) => { let t = match f.field { 0 => &mut f.title, 2 => &mut f.from, _ => &mut f.to }; t.clear(); }
            KeyCode::Backspace => { let t = match f.field { 0 => &mut f.title, 2 => &mut f.from, _ => &mut f.to }; t.pop(); }
            KeyCode::Char(c) => { let t = match f.field { 0 => &mut f.title, 2 => &mut f.from, _ => &mut f.to }; t.push(c); }
            _ => {}
        }
    }

    /// false = close
    pub fn on_key(&mut self, k: KeyEvent) -> bool {
        if self.form.is_some() { self.form_key(k); return true; }
        if let Some((_, ids, add)) = self.confirm.take() {
            if matches!(k.code, KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Char('s') | KeyCode::Char('S') | KeyCode::Enter) {
                if !ids.is_empty() { self.list.retain(|e| !ids.contains(&e.id)); if self.persist() { self.say("Deleted", false); } }
                if !add.is_empty() {
                    let mut n = 0;
                    for (d, name) in add {
                        if self.list.iter().any(|e| e.kind == Kind::Holiday && e.covers(d)) { continue; }
                        self.list.push(Entry { id: self.next_id, title: name.to_string(), kind: Kind::Holiday, from: d, to: d });
                        self.next_id += 1; n += 1;
                    }
                    if self.persist() { self.say(&format!("Added {n} national holidays  ·  add your regional and local ones with A"), false); }
                }
            } else { self.say("Cancelled", false); }
            return true;
        }
        self.status.0.clear();
        let move_to = |c: &mut Cal, d: NaiveDate| c.sel = d;
        match k.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => return false,
            KeyCode::Left => move_to(self, self.sel - Duration::days(1)),
            KeyCode::Right => move_to(self, self.sel + Duration::days(1)),
            KeyCode::Up => move_to(self, self.sel - Duration::days(7)),
            KeyCode::Down => move_to(self, self.sel + Duration::days(7)),
            KeyCode::PageUp => move_to(self, add_months(self.sel, -1)),
            KeyCode::PageDown => move_to(self, add_months(self.sel, 1)),
            KeyCode::Home | KeyCode::Char('t') | KeyCode::Char('T') => move_to(self, self.today),
            KeyCode::Char('a') | KeyCode::Char('A') | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Insert => self.open_form(None),
            KeyCode::Enter | KeyCode::Char('e') | KeyCode::Char('E') | KeyCode::F(2) => {
                let first = self.on(self.sel).first().map(|e| (*e).clone());
                if first.is_none() && k.code != KeyCode::Enter { self.say("Nothing on this day to edit  ·  A adds", true); } else { self.open_form(first); }
            }
            KeyCode::Delete | KeyCode::Char('d') | KeyCode::Char('D') => {
                let on: Vec<(u64, String)> = self.on(self.sel).iter().map(|e| (e.id, e.title.clone())).collect();
                if on.is_empty() { self.say("Nothing on this day", true); }
                else {
                    let names: Vec<String> = on.iter().map(|x| x.1.clone()).collect();
                    let q = format!("Delete “{}”?  Y / N", names.join("”, “"));
                    self.say(&q, true);
                    self.confirm = Some((q, on.into_iter().map(|x| x.0).collect(), Vec::new()));
                }
            }
            KeyCode::Char('i') | KeyCode::Char('I') => {
                let y = self.sel.year();
                let add: Vec<(NaiveDate, String)> = spain_national(y).into_iter().map(|(d, n)| (d, n.to_string())).collect();
                let q = format!("Add Spain's {} national holidays for {y}?  Y / N", add.len());
                self.say(&q, false);
                self.confirm = Some((q, Vec::new(), add));
            }
            KeyCode::F(9) => self.cfg.cycle_theme(1),
            _ => {}
        }
        true
    }

    pub fn on_mouse(&mut self, m: event::MouseEvent) {
        if self.form.is_some() { return; }
        match m.kind {
            MouseEventKind::ScrollDown => self.sel = add_months(self.sel, 1),
            MouseEventKind::ScrollUp => self.sel = add_months(self.sel, -1),
            MouseEventKind::Down(MouseButton::Left) => {
                let g = self.grid;
                if m.column >= g.x && m.row >= g.y + 1 && m.column < g.x + g.width && m.row < g.y + g.height {
                    let col = ((m.column - g.x) / self.cell.0) as i64;
                    let row = ((m.row - g.y - 1) / self.cell.1) as i64;
                    if col < 7 { let d = self.first + Duration::days(row * 7 + col); if d == self.sel { self.open_form(self.on(d).first().map(|e| (*e).clone())); } else { self.sel = d; } }
                }
            }
            _ => {}
        }
    }

    pub fn draw(&mut self, f: &mut Frame) {
        let t = self.cfg.theme.clone();
        let area = f.area();
        let base = match t.background { Some(bg) => Style::default().bg(bg).fg(t.text), None => Style::default().fg(t.text) };
        f.render_widget(Block::default().style(base), area);
        let dim = Style::default().fg(t.dim);
        let bar = Span::styled("  ▌ ", Style::default().fg(t.accent));
        let row = |y: u16| Rect { x: area.x, y, width: area.width, height: 1 };
        f.render_widget(Paragraph::new(Line::from(vec![bar.clone(), Span::styled("CALENDAR", Style::default().fg(t.text).add_modifier(Modifier::BOLD))])), row(area.y + 1));
        f.render_widget(Paragraph::new(Line::from(vec![bar.clone(), Span::styled(self.sel.format("%B %Y").to_string(), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
            Span::styled(format!("   today {}", self.today.format("%A %-d %B")), dim)])), row(area.y + 2));
        f.render_widget(Paragraph::new(Span::styled(format!("  {}", self.status.0), Style::default().fg(if self.status.1 { t.error } else { t.ok }))), row(area.y + 3));
        // layout: month grid, side panel on the right when there is room
        let side_w = if area.width >= 110 { 38 } else { 0 };
        let gw = area.width.saturating_sub(4 + side_w + if side_w > 0 { 2 } else { 0 });
        let gh = area.height.saturating_sub(7);
        let cw = (gw / 7).max(4);
        let rows_needed = 6u16;
        let ch = ((gh.saturating_sub(1)) / rows_needed).max(1);
        self.cell = (cw, ch);
        self.grid = Rect { x: area.x + 2, y: area.y + 5, width: cw * 7, height: 1 + ch * rows_needed };
        let g = self.grid;
        let month_first = first_of_month(self.sel);
        self.first = month_first - Duration::days(month_first.weekday().num_days_from_monday() as i64);
        let buf = f.buffer_mut();
        for (i, n) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].iter().enumerate() {
            buf.set_string(g.x + i as u16 * cw + 1, g.y, *n, if i >= 5 { dim } else { Style::default().fg(t.text).add_modifier(Modifier::BOLD) });
        }
        let rgb = |c: Color, d: (u8, u8, u8)| match c { Color::Rgb(r, g, b) => (r, g, b), _ => d };
        let mix = |a: (u8, u8, u8), b: (u8, u8, u8), k: f32| { let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k) as u8; Color::Rgb(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2)) };
        let bg = t.bg_rgb;
        let hol_bg = mix(bg, rgb(t.error, (230, 70, 60)), 0.28);
        let vac_bg = mix(bg, rgb(t.ok, (72, 199, 116)), 0.25);
        let evt_fg = t.highlight;
        for k in 0..42i64 {
            let d = self.first + Duration::days(k);
            let (cx, cy) = (g.x + (k % 7) as u16 * cw, g.y + 1 + (k / 7) as u16 * ch);
            let on = self.on(d);
            let in_month = d.month() == self.sel.month();
            let weekend = d.weekday() == Weekday::Sat || d.weekday() == Weekday::Sun;
            let cell_bg = if on.iter().any(|e| e.kind == Kind::Holiday) { Some(hol_bg) } else if on.iter().any(|e| e.kind == Kind::Vacation) { Some(vac_bg) } else { None };
            let buf = f.buffer_mut();
            if let Some(b) = cell_bg { for y in 0..ch { buf.set_string(cx, cy + y, " ".repeat(cw.saturating_sub(1) as usize), Style::default().bg(b)); } }
            let sel = d == self.sel;
            let today = d == self.today;
            let num = format!("{:>2}", d.day());
            let mut st = Style::default().fg(if !in_month { t.dim } else if weekend { t.dim } else { t.text });
            if let Some(b) = cell_bg { st = st.bg(b); }
            if today { st = st.fg(t.accent).add_modifier(Modifier::BOLD | Modifier::UNDERLINED); }
            if sel { st = Style::default().bg(t.accent).fg(Color::Rgb(bg.0, bg.1, bg.2)).add_modifier(Modifier::BOLD); }
            buf.set_string(cx, cy, &num, st);
            if !in_month && cell_bg.is_none() { continue; }
            // what is on: titles, then the tasks due that day
            let mut lines: Vec<(String, Style)> = on.iter().map(|e| {
                let c = match e.kind { Kind::Holiday => t.error, Kind::Vacation => t.ok, Kind::Event => evt_fg };
                let mut s = Style::default().fg(c);
                if let Some(b) = cell_bg { s = s.bg(b); }
                // a vacation shows its name on the first day (and on Mondays), a line elsewhere
                let name = if e.kind == Kind::Vacation && d != e.from && d.weekday() != Weekday::Mon { "·".to_string() } else { e.title.clone() };
                (name, s)
            }).collect();
            let nt = self.tasks_on(d).len();
            if nt > 0 { lines.push((format!("• {nt} {}", if nt == 1 { "task" } else { "tasks" }), { let s = Style::default().fg(t.accent); if let Some(b) = cell_bg { s.bg(b) } else { s } })); }
            let room = cw.saturating_sub(4) as usize;
            if ch == 1 {
                if let Some((s, stl)) = lines.first() { buf.set_string(cx + 3, cy, crate::ui::fit(s, room).trim_end(), *stl); }
            } else {
                for (i, (s, stl)) in lines.iter().take(ch.saturating_sub(1) as usize).enumerate() {
                    let w = cw.saturating_sub(2) as usize;
                    buf.set_string(cx + 1, cy + 1 + i as u16, crate::ui::fit(s, w).trim_end(), *stl);
                }
                if let Some((s, stl)) = lines.get(0).filter(|_| ch == 1) { buf.set_string(cx + 3, cy, crate::ui::fit(s, room).trim_end(), *stl); }
            }
        }
        // side panel
        if side_w > 0 {
            let sx = g.x + g.width + 2;
            let mut lines: Vec<Line> = Vec::new();
            let head = |s: &str| Line::styled(s.to_string(), Style::default().fg(t.accent).add_modifier(Modifier::BOLD));
            lines.push(head(&self.sel.format("%A %-d %B %Y").to_string()));
            let on = self.on(self.sel);
            let tk = self.tasks_on(self.sel);
            if on.is_empty() && tk.is_empty() { lines.push(Line::styled("  nothing planned  ·  A adds", dim)); }
            for e in &on {
                let c = match e.kind { Kind::Holiday => t.error, Kind::Vacation => t.ok, Kind::Event => t.highlight };
                let span = if e.from == e.to { String::new() } else { format!("  {}–{}", e.from.format("%-d %b"), e.to.format("%-d %b")) };
                lines.push(Line::from(vec![Span::styled(format!("  {} ", e.kind.label()), Style::default().fg(c)), Span::styled(crate::ui::fit(&e.title, 20).trim_end().to_string(), Style::default().fg(t.text)), Span::styled(span, dim)]));
            }
            for x in &tk {
                let when = x.time.map(|h| h.format("%H:%M").to_string()).unwrap_or_default();
                lines.push(Line::from(vec![Span::styled(format!("  {} ", if x.done { "✔" } else { "○" }), Style::default().fg(if x.done { t.ok } else { t.accent })), Span::styled(format!("{when} "), dim), Span::styled(crate::ui::fit(&x.title, 26).trim_end().to_string(), Style::default().fg(t.text))]));
            }
            lines.push(Line::raw(""));
            lines.push(head("NEXT DAYS OFF"));
            let mut up: Vec<&Entry> = self.list.iter().filter(|e| e.to >= self.today).collect();
            up.sort_by_key(|e| e.from);
            if up.is_empty() { lines.push(Line::styled("  none planned", dim)); }
            for e in up.iter().take(8) {
                let days = (e.from - self.today).num_days();
                let when = if days <= 0 { "now".to_string() } else if days == 1 { "tomorrow".into() } else { format!("in {days} d") };
                let c = match e.kind { Kind::Holiday => t.error, Kind::Vacation => t.ok, Kind::Event => t.highlight };
                lines.push(Line::from(vec![Span::styled(format!("  {:<11}", e.from.format("%a %-d %b")), Style::default().fg(c)), Span::styled(crate::ui::fit(&e.title, 16), Style::default().fg(t.text)), Span::styled(format!(" {when}"), dim)]));
            }
            lines.push(Line::raw(""));
            let y = self.sel.year();
            let (h, v) = days_off_in_year(&self.list, y);
            lines.push(head(&format!("{y}")));
            lines.push(Line::from(vec![Span::styled("  holidays    ", dim), Span::styled(format!("{h} work days"), Style::default().fg(t.error))]));
            lines.push(Line::from(vec![Span::styled("  vacation    ", dim), Span::styled(format!("{v} work days"), Style::default().fg(t.ok))]));
            f.render_widget(Paragraph::new(lines), Rect { x: sx, y: g.y, width: side_w, height: area.height.saturating_sub(g.y + 2) });
        }
        let hints: Vec<(&str, &str)> = if self.form.is_some() { vec![("Enter", "save"), ("Tab ↑↓", "field"), ("← →", "type"), ("Esc", "cancel")] }
            else { vec![("arrows", "day"), ("PgUp PgDn", "month"), ("Home", "today"), ("A", "add"), ("Enter / E", "edit"), ("Del", "delete"), ("I", "Spain's holidays"), ("Esc", "back")] };
        f.render_widget(Paragraph::new(crate::ui::hint_line(&hints, area.width, &t)), row(area.y + area.height.saturating_sub(1)));
        if self.form.is_some() { self.draw_form(f, &t); }
    }

    fn draw_form(&self, f: &mut Frame, t: &Theme) {
        let Some(fm) = &self.form else { return };
        let area = f.area();
        let (bw, bh) = (64u16.min(area.width.saturating_sub(2)), 12u16.min(area.height));
        let r = Rect { x: area.x + (area.width - bw) / 2, y: area.y + area.height.saturating_sub(bh) / 2, width: bw, height: bh };
        f.render_widget(Clear, r);
        let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::default().fg(t.accent))
            .title(Span::styled(if fm.editing.is_some() { " Edit " } else { " Add to the calendar " }, Style::default().fg(t.accent).add_modifier(Modifier::BOLD)))
            .style(Style::default().bg(t.background.unwrap_or(Color::Reset)).fg(t.text));
        let inner = block.inner(r);
        f.render_widget(block, r);
        let lab = |f: &mut Frame, y: u16, s: &str, on: bool| f.buffer_mut().set_string(inner.x + 1, y, s, Style::default().fg(if on { t.accent } else { t.dim }).add_modifier(if on { Modifier::BOLD } else { Modifier::empty() }));
        let field = |f: &mut Frame, y: u16, s: &str, on: bool, w: u16| {
            let st = if on { Style::default().bg(t.select_bg).fg(t.text) } else { Style::default().fg(t.text) };
            f.buffer_mut().set_string(inner.x + 9, y, crate::ui::fit(s, w as usize), st);
            if on { f.set_cursor_position((inner.x + 9 + s.width().min(w as usize - 1) as u16, y)); }
        };
        let mut y = inner.y + 1;
        lab(f, y, "Name", fm.field == 0); field(f, y, &fm.title, fm.field == 0, inner.width.saturating_sub(11)); y += 2;
        lab(f, y, "Type", fm.field == 1);
        let opt = |k: Kind| Span::styled(format!("{} {}   ", if fm.kind == k { "◉" } else { "○" }, k.label()), Style::default().fg(if fm.kind == k { match k { Kind::Holiday => t.error, Kind::Vacation => t.ok, Kind::Event => t.highlight } } else { t.dim }));
        f.render_widget(Paragraph::new(Line::from(vec![opt(Kind::Holiday), opt(Kind::Vacation), opt(Kind::Event)])), Rect { x: inner.x + 9, y, width: inner.width.saturating_sub(10), height: 1 });
        y += 2;
        lab(f, y, "From", fm.field == 2); field(f, y, &fm.from, fm.field == 2, 16);
        let show = |s: &str, base: NaiveDate| match crate::tasks::parse_date(s, base) { Ok(Some(d)) => (d.format("→ %a %-d %b %Y").to_string(), t.ok), Ok(None) => (String::new(), t.dim), Err(_) => ("? not understood".into(), t.error) };
        let (a, ca) = show(&fm.from, self.today);
        f.buffer_mut().set_string(inner.x + 27, y, a, Style::default().fg(ca));
        y += 1;
        lab(f, y, "To", fm.field == 3); field(f, y, &fm.to, fm.field == 3, 16);
        let base = crate::tasks::parse_date(&fm.from, self.today).ok().flatten().unwrap_or(self.today);
        let (b, cb) = show(&fm.to, base);
        let days = crate::tasks::parse_date(&fm.to, base).ok().flatten().map(|to| (to - base).num_days() + 1).filter(|n| *n > 1).map(|n| format!("  ({n} days)")).unwrap_or_default();
        f.buffer_mut().set_string(inner.x + 27, y, format!("{b}{days}"), Style::default().fg(cb));
        y += 2;
        f.buffer_mut().set_string(inner.x + 1, y, crate::ui::fit("dates: 24/12 · 24/12/26 · fri · +7 · 2026-12-24   (To: +14 = two weeks)", inner.width.saturating_sub(2) as usize), Style::default().fg(t.dim));
        y += 1;
        if !fm.error.is_empty() { f.buffer_mut().set_string(inner.x + 1, y, &fm.error, Style::default().fg(t.error).add_modifier(Modifier::BOLD)); }
    }
}

pub fn run(terminal: &mut ratatui::DefaultTerminal) -> std::io::Result<()> {
    let mut c = Cal::new();
    let _ = terminal.clear();
    let mut theme = c.cfg.theme.name.clone();
    loop {
        terminal.draw(|f| c.draw(f))?;
        if !event::poll(StdDur::from_millis(1000))? { continue; }
        match event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => { if !c.on_key(k) { return Ok(()); } }
            Event::Mouse(m) => c.on_mouse(m),
            Event::Resize(_, _) => { let _ = terminal.clear(); }
            _ => {}
        }
        if c.cfg.theme.name != theme { theme = c.cfg.theme.name.clone(); let _ = terminal.clear(); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dates() {
        assert_eq!(easter(2026), NaiveDate::from_ymd_opt(2026, 4, 5).unwrap());
        assert_eq!(easter(2027), NaiveDate::from_ymd_opt(2027, 3, 28).unwrap());
        assert_eq!(easter(2025), NaiveDate::from_ymd_opt(2025, 4, 20).unwrap());
        let es = spain_national(2026);
        assert_eq!(es.len(), 10);
        assert!(es.contains(&(NaiveDate::from_ymd_opt(2026, 4, 3).unwrap(), "Viernes Santo")));
        assert_eq!(add_months(NaiveDate::from_ymd_opt(2026, 1, 31).unwrap(), 1), NaiveDate::from_ymd_opt(2026, 2, 28).unwrap());
        assert_eq!(add_months(NaiveDate::from_ymd_opt(2026, 1, 15).unwrap(), -1), NaiveDate::from_ymd_opt(2025, 12, 15).unwrap());
        let d = |m, x| NaiveDate::from_ymd_opt(2026, m, x).unwrap();
        let list = vec![Entry { id: 1, title: "Navidad".into(), kind: Kind::Holiday, from: d(12, 25), to: d(12, 25) },
                        Entry { id: 2, title: "Summer".into(), kind: Kind::Vacation, from: d(8, 3), to: d(8, 16) },
                        Entry { id: 3, title: "Dentist".into(), kind: Kind::Event, from: d(8, 5), to: d(8, 5) }];
        // 25 Dec 2026 is a Friday; 3-16 Aug is two full weeks = 10 work days
        assert_eq!(days_off_in_year(&list, 2026), (1, 10));
    }
}
