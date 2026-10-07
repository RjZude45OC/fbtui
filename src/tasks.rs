//! Task list shared by the Tasks screen and the background notifier.
//! Stored as tasks.json next to config.ini (script\tasks.json).

use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Weekday};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: u64,
    pub title: String,
    pub daily: bool,                      // repeats (done resets the next day it is due)
    pub days: Vec<Weekday>,               // repeating: on these weekdays (empty = every day)
    pub month_day: Option<u32>,           // repeating: on this day of every month instead
    pub date: Option<NaiveDate>,          // one-off tasks: the day it is due (None = no date)
    pub time: Option<NaiveTime>,          // due time (one-off) or time of day (daily)
    pub done: bool,                       // one-off tasks
    pub done_at: Option<NaiveDateTime>,
    pub done_on: Option<NaiveDate>,       // daily tasks: the day it was last ticked off
    pub created: NaiveDateTime,
    pub link: Option<String>,             // a folder, file or web address to open with the task (G)
}

impl Task {
    /// A repeating task: is it due on this day?
    pub fn applies(&self, d: NaiveDate) -> bool {
        if !self.daily { return false; }
        if let Some(n) = self.month_day {
            let last = last_day_of_month(d);
            return d.day() == n.min(last);
        }
        self.days.is_empty() || self.days.contains(&d.weekday())
    }
    /// A repeating task: the first day from `d` on that it is due.
    pub fn next_on(&self, d: NaiveDate) -> Option<NaiveDate> {
        (0..400).map(|i| d + Duration::days(i)).find(|x| self.applies(*x))
    }
    pub fn is_done(&self, today: NaiveDate) -> bool {
        if self.daily { self.done_on == Some(today) } else { self.done }
    }
    /// When a one-off task is due (date-only tasks: the end of that day).
    pub fn due(&self) -> Option<NaiveDateTime> {
        if self.daily { return None; }
        self.date.map(|d| d.and_time(self.time.unwrap_or_else(|| NaiveTime::from_hms_opt(23, 59, 59).unwrap())))
    }
    pub fn overdue(&self, now: NaiveDateTime) -> bool {
        if self.daily {
            return self.applies(now.date()) && !self.is_done(now.date()) && self.time.map(|t| now.time() > t).unwrap_or(false);
        }
        !self.done && self.due().map(|d| now > d).unwrap_or(false)
    }
    /// The next moment this task wants attention (for sorting and "next task in ...").
    pub fn next_time(&self, now: NaiveDateTime) -> Option<NaiveDateTime> {
        if self.daily {
            let t = self.time?;
            let today = now.date();
            let from = if self.applies(today) && !self.is_done(today) { today } else { today + Duration::days(1) };
            return self.next_on(from).map(|d| d.and_time(t));
        }
        if self.done { return None; }
        self.date.map(|d| d.and_time(self.time.unwrap_or(NaiveTime::MIN)))
    }

    fn to_json(&self) -> Value {
        let fd = |d: &Option<NaiveDate>| d.map(|d| d.format("%Y-%m-%d").to_string());
        let fdt = |d: &Option<NaiveDateTime>| d.map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string());
        json!({
            "id": self.id, "title": self.title, "daily": self.daily,
            "days": self.days.iter().map(|d| crate::clock::day_key(*d)).collect::<Vec<_>>(), "month_day": self.month_day,
            "date": fd(&self.date), "time": self.time.map(|t| t.format("%H:%M").to_string()),
            "done": self.done, "done_at": fdt(&self.done_at), "done_on": fd(&self.done_on),
            "created": self.created.format("%Y-%m-%d %H:%M:%S").to_string(),
            "link": self.link,
        })
    }
    fn from_json(v: &Value) -> Option<Task> {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str());
        let date = |k: &str| s(k).and_then(|x| NaiveDate::parse_from_str(x, "%Y-%m-%d").ok());
        let dt = |k: &str| s(k).and_then(|x| NaiveDateTime::parse_from_str(x, "%Y-%m-%d %H:%M:%S").ok());
        Some(Task {
            id: v.get("id")?.as_u64()?,
            title: s("title")?.to_string(),
            daily: v.get("daily").and_then(|x| x.as_bool()).unwrap_or(false),
            days: v.get("days").and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|d| d.as_str()).flat_map(|d| crate::clock::parse_days(d)).collect()).unwrap_or_default(),
            month_day: v.get("month_day").and_then(|x| x.as_u64()).map(|n| n.clamp(1, 31) as u32),
            date: date("date"),
            time: s("time").and_then(|x| NaiveTime::parse_from_str(x, "%H:%M").ok()),
            done: v.get("done").and_then(|x| x.as_bool()).unwrap_or(false),
            done_at: dt("done_at"),
            done_on: date("done_on"),
            created: dt("created").unwrap_or_else(|| Local::now().naive_local()),
            link: s("link").map(|x| x.to_string()).filter(|x| !x.trim().is_empty()),
        })
    }
}

pub struct Store {
    pub path: PathBuf,
    pub tasks: Vec<Task>,
    pub next_id: u64,
    stamp: Option<SystemTime>,
    pub error: Option<String>,
}

/// script\tasks.json (the folder that holds config.ini).
pub fn default_path() -> PathBuf {
    let cfg = crate::config::Config::load();
    let dir = cfg.file.as_ref().unwrap_or(&cfg.default_file).parent().map(|p| p.to_path_buf()).unwrap_or_default();
    dir.join("tasks.json")
}

impl Store {
    pub fn open(path: PathBuf) -> Store {
        let mut s = Store { path, tasks: Vec::new(), next_id: 1, stamp: None, error: None };
        s.load();
        s
    }

    fn mtime(&self) -> Option<SystemTime> { std::fs::metadata(&self.path).and_then(|m| m.modified()).ok() }

    pub fn load(&mut self) {
        self.stamp = self.mtime();
        self.error = None;
        let text = match std::fs::read_to_string(&self.path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => { self.tasks.clear(); return; }
            Err(e) => { self.error = Some(format!("Cannot read {}: {e}", self.path.display())); return; }
        };
        match serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) {
            Ok(v) => {
                self.tasks = v.get("tasks").and_then(|t| t.as_array()).map(|a| a.iter().filter_map(Task::from_json).collect()).unwrap_or_default();
                let max = self.tasks.iter().map(|t| t.id).max().unwrap_or(0);
                self.next_id = v.get("next_id").and_then(|x| x.as_u64()).unwrap_or(1).max(max + 1);
            }
            Err(e) => self.error = Some(format!("tasks.json is not valid JSON ({e}) - fix it or delete it")),
        }
    }

    /// Re-read the file if something else changed it. True if it was reloaded.
    pub fn reload_if_changed(&mut self) -> bool {
        if self.mtime() != self.stamp { self.load(); true } else { false }
    }

    pub fn save(&mut self) -> Result<(), String> {
        if self.error.is_some() { return Err("tasks.json could not be read, so it was not overwritten".into()); }
        let v = json!({ "version": 1, "next_id": self.next_id, "tasks": self.tasks.iter().map(|t| t.to_json()).collect::<Vec<_>>() });
        let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, text).map_err(|e| format!("Cannot save tasks: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| format!("Cannot save tasks: {e}"))?;
        self.stamp = self.mtime();
        Ok(())
    }

    pub fn add(&mut self, mut t: Task) -> u64 {
        t.id = self.next_id;
        self.next_id += 1;
        let id = t.id;
        self.tasks.push(t);
        id
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Task> { self.tasks.iter_mut().find(|t| t.id == id) }
}

pub fn new_task(title: &str, daily: bool, date: Option<NaiveDate>, time: Option<NaiveTime>) -> Task {
    Task { id: 0, title: title.to_string(), daily, days: Vec::new(), month_day: None, date, time, done: false, done_at: None, done_on: None, created: Local::now().naive_local(), link: None }
}

/// What was typed in the Link field: quotes from Explorer's "Copy as path" removed, empty = no link.
pub fn clean_link(s: &str) -> Option<String> {
    let t = s.trim().trim_matches('"').trim();
    if t.is_empty() { None } else { Some(t.to_string()) }
}

pub fn is_web(link: &str) -> bool { let l = link.to_ascii_lowercase(); l.starts_with("http://") || l.starts_with("https://") || l.starts_with("www.") }

/// Short name of a link for the list: the file or folder name, or the web site.
pub fn link_label(link: &str) -> String {
    if is_web(link) {
        let l = link.split("://").nth(1).unwrap_or(link);
        return l.split('/').next().unwrap_or(l).to_string();
    }
    let t = link.trim_end_matches(['\\', '/']);
    let name = t.rsplit(['\\', '/']).next().unwrap_or(t);
    if name.is_empty() { link.to_string() } else if std::path::Path::new(link).is_dir() { format!("{name}\\") } else { name.to_string() }
}

/// "folder", "file", "web address" or "not found" (for the form).
pub fn link_kind(link: &str) -> &'static str {
    if is_web(link) { return "web address"; }
    let p = std::path::Path::new(link);
    if p.is_dir() { "folder" } else if p.is_file() { "file" } else { "not found" }
}

pub fn last_day_of_month(d: NaiveDate) -> u32 {
    (28..=31).rev().find(|&x| NaiveDate::from_ymd_opt(d.year(), d.month(), x).is_some()).unwrap_or(28)
}

const WEEK: [Weekday; 7] = [Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri, Weekday::Sat, Weekday::Sun];

/// "every day", "mon", "mon, wed, fri", "mon-fri", "weekdays", "weekends", "lunes",
/// "15", "15th", "monthly 15", "day 1"  ->  (weekdays, day of month)
pub fn parse_repeat(s: &str) -> Result<(Vec<Weekday>, Option<u32>), ()> {
    let s = s.trim().to_lowercase();
    let s = s.trim_start_matches("every").trim_start_matches("on").trim().to_string();
    match s.as_str() {
        "" | "day" | "daily" | "days" | "everyday" | "every day" | "dia" | "día" | "todos los dias" | "todos los días" => return Ok((Vec::new(), None)),
        "weekday" | "weekdays" | "workdays" | "work days" | "laborables" => return Ok((WEEK[..5].to_vec(), None)),
        "weekend" | "weekends" | "fin de semana" => return Ok((WEEK[5..].to_vec(), None)),
        _ => {}
    }
    // a day of the month
    let digits: String = s.trim_start_matches("monthly").trim_start_matches("month").trim_start_matches("day").trim()
        .trim_end_matches("st").trim_end_matches("nd").trim_end_matches("rd").trim_end_matches("th").trim().to_string();
    if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
        let n: u32 = digits.parse().map_err(|_| ())?;
        return if (1..=31).contains(&n) { Ok((Vec::new(), Some(n))) } else { Err(()) };
    }
    let days = crate::clock::parse_days(&s);
    if days.is_empty() { return Err(()); }
    Ok((if days.len() == 7 { Vec::new() } else { days }, None))
}

fn ordinal(n: u32) -> String {
    let suf = match (n % 10, n % 100) { (1, x) if x != 11 => "st", (2, x) if x != 12 => "nd", (3, x) if x != 13 => "rd", _ => "th" };
    format!("{n}{suf}")
}

/// "Daily", "Mon", "Mon, Wed, Fri", "Mon–Fri", "Weekends", "Monthly 15th"
pub fn fmt_repeat(t: &Task) -> String {
    if let Some(n) = t.month_day { return format!("Monthly {}", ordinal(n)); }
    let d = &t.days;
    if d.is_empty() || d.len() == 7 { return "Daily".into(); }
    if *d == WEEK[..5].to_vec() { return "Mon–Fri".into(); }
    if *d == WEEK[5..].to_vec() { return "Weekends".into(); }
    // a run of 3 or more days: "Tue–Fri"
    let idx: Vec<u32> = d.iter().map(|x| x.num_days_from_monday()).collect();
    if idx.len() >= 3 && idx.windows(2).all(|w| w[1] == w[0] + 1) {
        return format!("{}–{}", WEEK[idx[0] as usize].to_string(), WEEK[*idx.last().unwrap() as usize].to_string());
    }
    d.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", ")
}

/// The text to put in the form's "On" field for a repeating task.
pub fn repeat_text(t: &Task) -> String {
    if let Some(n) = t.month_day { return ordinal(n); }
    if t.days.is_empty() { return "every day".into(); }
    t.days.iter().map(|d| crate::clock::day_key(*d)).collect::<Vec<_>>().join(", ")
}

// ---------------------------------------------------------------- input parsing

const DAYS: &[(&str, Weekday)] = &[
    ("monday", Weekday::Mon), ("mon", Weekday::Mon), ("lunes", Weekday::Mon), ("lun", Weekday::Mon),
    ("tuesday", Weekday::Tue), ("tue", Weekday::Tue), ("tues", Weekday::Tue), ("martes", Weekday::Tue),
    ("wednesday", Weekday::Wed), ("wed", Weekday::Wed), ("miercoles", Weekday::Wed), ("miércoles", Weekday::Wed), ("mie", Weekday::Wed), ("mié", Weekday::Wed),
    ("thursday", Weekday::Thu), ("thu", Weekday::Thu), ("thur", Weekday::Thu), ("thurs", Weekday::Thu), ("jueves", Weekday::Thu), ("jue", Weekday::Thu),
    ("friday", Weekday::Fri), ("fri", Weekday::Fri), ("viernes", Weekday::Fri), ("vie", Weekday::Fri),
    ("saturday", Weekday::Sat), ("sat", Weekday::Sat), ("sabado", Weekday::Sat), ("sábado", Weekday::Sat), ("sab", Weekday::Sat), ("sáb", Weekday::Sat),
    ("sunday", Weekday::Sun), ("sun", Weekday::Sun), ("domingo", Weekday::Sun), ("dom", Weekday::Sun),
];

/// "today", "tomorrow", "fri", "+3", "+2w", "5", "5/10", "5/10/26", "2026-10-05" (day before month).
/// Ok(None) = no date.
pub fn parse_date(s: &str, today: NaiveDate) -> Result<Option<NaiveDate>, ()> {
    let s = s.trim().to_lowercase();
    if s.is_empty() || s == "-" || s == "none" { return Ok(None); }
    match s.as_str() {
        "today" | "tod" | "hoy" | "t" => return Ok(Some(today)),
        "tomorrow" | "tmr" | "tmrw" | "tom" | "mañana" | "manana" => return Ok(Some(today + Duration::days(1))),
        "yesterday" | "ayer" => return Ok(Some(today - Duration::days(1))),
        _ => {}
    }
    if let Some(&(_, wd)) = DAYS.iter().find(|(n, _)| *n == s) {
        let ahead = (wd.num_days_from_monday() as i64 - today.weekday().num_days_from_monday() as i64).rem_euclid(7);
        return Ok(Some(today + Duration::days(ahead)));
    }
    if let Some(r) = s.strip_prefix('+') {
        let (num, unit) = r.split_at(r.find(|c: char| !c.is_ascii_digit()).unwrap_or(r.len()));
        let n: i64 = num.parse().map_err(|_| ())?;
        let days = match unit.trim() { "" | "d" | "day" | "days" => n, "w" | "week" | "weeks" => n * 7, _ => return Err(()) };
        return Ok(Some(today + Duration::days(days)));
    }
    if let Ok(d) = NaiveDate::parse_from_str(&s, "%Y-%m-%d") { return Ok(Some(d)); }
    let parts: Vec<&str> = s.split(['/', '.', '-']).collect();
    let num = |x: &str| x.trim().parse::<u32>().map_err(|_| ());
    match parts.len() {
        1 => {
            // a day of the month: this month, or the next one if it has passed
            let d = num(parts[0])?;
            let mut c = NaiveDate::from_ymd_opt(today.year(), today.month(), d);
            if c.map(|c| c < today).unwrap_or(true) {
                let (y, m) = if today.month() == 12 { (today.year() + 1, 1) } else { (today.year(), today.month() + 1) };
                c = NaiveDate::from_ymd_opt(y, m, d);
            }
            c.map(Some).ok_or(())
        }
        2 => {
            let (d, m) = (num(parts[0])?, num(parts[1])?);
            let c = NaiveDate::from_ymd_opt(today.year(), m, d).ok_or(())?;
            Ok(Some(if c < today { NaiveDate::from_ymd_opt(today.year() + 1, m, d).ok_or(())? } else { c }))
        }
        3 => {
            let (d, m, mut y) = (num(parts[0])?, num(parts[1])?, num(parts[2])? as i32);
            if y < 100 { y += 2000; }
            NaiveDate::from_ymd_opt(y, m, d).map(Some).ok_or(())
        }
        _ => Err(()),
    }
}

pub enum TimeIn { None, At(NaiveTime), In(Duration) }

/// "14:30", "1430", "9", "9pm", "9:30am", "+30m", "+2h", "+1h30m", "+45" (minutes).
pub fn parse_time(s: &str) -> Result<TimeIn, ()> {
    let s = s.trim().to_lowercase().replace(' ', "");
    if s.is_empty() || s == "-" { return Ok(TimeIn::None); }
    if let Some(r) = s.strip_prefix('+').or_else(|| s.strip_prefix("in")) {
        let mut mins = 0i64;
        let mut num = String::new();
        let mut any = false;
        for c in r.chars() {
            if c.is_ascii_digit() { num.push(c); continue; }
            let n: i64 = num.parse().map_err(|_| ())?;
            num.clear();
            match c { 'h' => mins += n * 60, 'm' => mins += n, 'd' => mins += n * 1440, _ => return Err(()) }
            any = true;
        }
        if !num.is_empty() { mins += num.parse::<i64>().map_err(|_| ())?; any = true; }
        if !any || mins <= 0 { return Err(()); }
        return Ok(TimeIn::In(Duration::minutes(mins)));
    }
    let (body, ampm) = if let Some(b) = s.strip_suffix("am") { (b, Some(false)) } else if let Some(b) = s.strip_suffix("pm") { (b, Some(true)) }
        else if let Some(b) = s.strip_suffix('h') { (b, None) } else { (s.as_str(), None) };
    let (h, m) = if let Some((h, m)) = body.split_once([':', '.', 'h']) {
        (h.parse::<u32>().map_err(|_| ())?, if m.is_empty() { 0 } else { m.parse::<u32>().map_err(|_| ())? })
    } else if body.len() >= 3 && body.chars().all(|c| c.is_ascii_digit()) {
        let n: u32 = body.parse().map_err(|_| ())?;
        (n / 100, n % 100)
    } else {
        (body.parse::<u32>().map_err(|_| ())?, 0)
    };
    let h = match ampm {
        Some(pm) => { if h == 0 || h > 12 { return Err(()); } (h % 12) + if pm { 12 } else { 0 } }
        None => h,
    };
    NaiveTime::from_hms_opt(h, m, 0).map(TimeIn::At).ok_or(())
}

/// Date + time fields of the form -> (date, time).
pub fn resolve(date_s: &str, time_s: &str, daily: bool, now: NaiveDateTime) -> Result<(Option<NaiveDate>, Option<NaiveTime>), String> {
    let time = parse_time(time_s).map_err(|_| "time?".to_string())?;
    if daily {
        return match time {
            TimeIn::None => Ok((None, None)),
            TimeIn::At(t) => Ok((None, Some(t))),
            TimeIn::In(d) => { let t = now + d; Ok((None, Some(NaiveTime::from_hms_opt(t.hour(), t.minute(), 0).unwrap()))) }
        };
    }
    let date = parse_date(date_s, now.date()).map_err(|_| "date?".to_string())?;
    match time {
        TimeIn::None => Ok((date, None)),
        TimeIn::At(t) => Ok((Some(date.unwrap_or_else(|| if t > now.time() { now.date() } else { now.date() + Duration::days(1) })), Some(t))),
        TimeIn::In(d) => { let t = now + d; Ok((Some(t.date()), Some(NaiveTime::from_hms_opt(t.hour(), t.minute(), 0).unwrap()))) }
    }
}

// ---------------------------------------------------------------- display

/// "Today", "Tomorrow", "Yesterday", "Thu", "Mon 12 Oct", "Mon 12 Oct 2027".
pub fn fmt_day(d: NaiveDate, today: NaiveDate) -> String {
    let diff = (d - today).num_days();
    match diff {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        -1 => "Yesterday".into(),
        2..=6 => d.format("%a").to_string(),
        _ if d.year() == today.year() => d.format("%a %-d %b").to_string(),
        _ => d.format("%a %-d %b %Y").to_string(),
    }
}

pub fn fmt_when(t: &Task, today: NaiveDate) -> String {
    let time = t.time.map(|x| x.format("%H:%M").to_string());
    if t.daily { let r = fmt_repeat(t); return time.map(|x| format!("{r} {x}")).unwrap_or(r); }
    match (t.date, time) {
        (Some(d), Some(x)) => format!("{} {x}", fmt_day(d, today)),
        (Some(d), None) => fmt_day(d, today),
        (None, _) => String::new(),
    }
}

/// "3m", "2h 05m", "3d 4h".
pub fn fmt_span(mins: i64) -> String {
    let m = mins.abs();
    if m < 60 { format!("{m}m") } else if m < 24 * 60 { format!("{}h {:02}m", m / 60, m % 60) } else { format!("{}d {}h", m / 1440, (m % 1440) / 60) }
}

/// "in 2h 05m" / "5m ago" / "" for a one-off task (or a daily task today).
pub fn fmt_relative(t: &Task, now: NaiveDateTime) -> String {
    if t.is_done(now.date()) { return String::new(); }
    if t.daily && !t.applies(now.date()) {
        // not due today: when is it next
        return t.next_on(now.date()).map(|d| if (d - now.date()).num_days() == 1 { "tomorrow".to_string() } else { format!("next {}", fmt_day(d, now.date())) }).unwrap_or_default();
    }
    let target = if t.daily { t.time.map(|x| now.date().and_time(x)) } else if t.time.is_some() { t.due() } else { None };
    if let Some(x) = target {
        let mins = (x - now).num_minutes();
        if (x - now).num_seconds() >= 0 { if mins == 0 { "now".into() } else { format!("in {}", fmt_span(mins)) } } else { format!("{} late", fmt_span(mins.abs().max(1))) }
    } else if !t.daily {
        match t.date { Some(d) if d < now.date() => format!("{} late", fmt_span((now.date() - d).num_days() * 1440)), _ => String::new() }
    } else { String::new() }
}

pub fn exists(path: &Path) -> bool { path.is_file() }

#[cfg(test)]
mod tests {
    use super::*;
    fn d(y: i32, m: u32, dd: u32) -> NaiveDate { NaiveDate::from_ymd_opt(y, m, dd).unwrap() }
    #[test]
    fn dates() {
        let today = d(2026, 10, 5); // Monday
        assert_eq!(parse_date("today", today), Ok(Some(today)));
        assert_eq!(parse_date("tmr", today), Ok(Some(d(2026, 10, 6))));
        assert_eq!(parse_date("fri", today), Ok(Some(d(2026, 10, 9))));
        assert_eq!(parse_date("lunes", today), Ok(Some(today)));
        assert_eq!(parse_date("+2w", today), Ok(Some(d(2026, 10, 19))));
        assert_eq!(parse_date("3", today), Ok(Some(d(2026, 11, 3))));
        assert_eq!(parse_date("20/10", today), Ok(Some(d(2026, 10, 20))));
        assert_eq!(parse_date("1/2", today), Ok(Some(d(2027, 2, 1))));
        assert_eq!(parse_date("5.10.27", today), Ok(Some(d(2027, 10, 5))));
        assert_eq!(parse_date("2026-12-24", today), Ok(Some(d(2026, 12, 24))));
        assert_eq!(parse_date("", today), Ok(None));
        assert!(parse_date("31/2", today).is_err());
        assert!(parse_date("blah", today).is_err());
    }
    #[test]
    fn repeats() {
        let r = |s: &str| parse_repeat(s).map(|(d, m)| (d.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(","), m));
        assert_eq!(r("every monday"), Ok(("Mon".into(), None)));
        assert_eq!(r("mon, wed, fri"), Ok(("Mon,Wed,Fri".into(), None)));
        assert_eq!(r("weekdays"), Ok(("Mon,Tue,Wed,Thu,Fri".into(), None)));
        assert_eq!(r("lunes"), Ok(("Mon".into(), None)));
        assert_eq!(r("every day"), Ok(("".into(), None)));
        assert_eq!(r("15th"), Ok(("".into(), Some(15))));
        assert_eq!(r("monthly 1"), Ok(("".into(), Some(1))));
        assert!(r("blah").is_err());
        assert!(r("32").is_err());
        let mut t = new_task("Weekly report", true, None, NaiveTime::from_hms_opt(9, 0, 0));
        t.days = vec![Weekday::Mon];
        assert_eq!(fmt_repeat(&t), "Mon");
        let tue = d(2026, 10, 6);
        assert!(!t.applies(tue));
        assert!(!t.overdue(tue.and_hms_opt(12, 0, 0).unwrap()));
        assert_eq!(t.next_on(tue), Some(d(2026, 10, 12)));
        assert_eq!(t.next_time(tue.and_hms_opt(12, 0, 0).unwrap()), Some(d(2026, 10, 12).and_hms_opt(9, 0, 0).unwrap()));
        assert_eq!(fmt_relative(&t, tue.and_hms_opt(12, 0, 0).unwrap()), "next Mon");
        let mon = d(2026, 10, 12).and_hms_opt(10, 0, 0).unwrap();
        assert!(t.overdue(mon));
        t.days = vec![Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri];
        assert_eq!(fmt_repeat(&t), "Tue–Fri");
        t.days.clear(); t.month_day = Some(31);
        assert!(t.applies(d(2026, 11, 30)), "the 31st falls on the last day of shorter months");
        assert_eq!(fmt_repeat(&t), "Monthly 31st");
        // saved and read back
        let dir = std::env::temp_dir().join(format!("fb_rep_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = Store::open(dir.join("tasks.json"));
        let mut w = new_task("Gym", true, None, None); w.days = vec![Weekday::Mon, Weekday::Thu];
        s.add(w); s.save().unwrap();
        let s2 = Store::open(dir.join("tasks.json"));
        assert_eq!(s2.tasks[0].days, vec![Weekday::Mon, Weekday::Thu]);
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn times() {
        let t = |s: &str| match parse_time(s) { Ok(TimeIn::At(t)) => t.format("%H:%M").to_string(), Ok(TimeIn::In(d)) => format!("+{}", d.num_minutes()), Ok(TimeIn::None) => "-".into(), Err(_) => "ERR".into() };
        assert_eq!(t("14:30"), "14:30");
        assert_eq!(t("1430"), "14:30");
        assert_eq!(t("930"), "09:30");
        assert_eq!(t("9"), "09:00");
        assert_eq!(t("9pm"), "21:00");
        assert_eq!(t("12am"), "00:00");
        assert_eq!(t("9:15 pm"), "21:15");
        assert_eq!(t("18h"), "18:00");
        assert_eq!(t("+30m"), "+30");
        assert_eq!(t("+1h30m"), "+90");
        assert_eq!(t("+45"), "+45");
        assert_eq!(t(""), "-");
        assert_eq!(t("25:00"), "ERR");
    }
    #[test]
    fn resolve_and_store() {
        let now = d(2026, 10, 5).and_hms_opt(15, 0, 0).unwrap();
        // a time earlier than now with no date -> tomorrow
        assert_eq!(resolve("", "9", false, now), Ok((Some(d(2026, 10, 6)), NaiveTime::from_hms_opt(9, 0, 0))));
        assert_eq!(resolve("", "+2h", false, now), Ok((Some(d(2026, 10, 5)), NaiveTime::from_hms_opt(17, 0, 0))));
        assert_eq!(resolve("whatever", "8", true, now), Ok((None, NaiveTime::from_hms_opt(8, 0, 0))));
        let dir = std::env::temp_dir().join(format!("fb_tasks_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("tasks.json");
        let mut s = Store::open(p.clone());
        let id = s.add(new_task("Call the bank", false, Some(d(2026, 10, 5)), NaiveTime::from_hms_opt(14, 30, 0)));
        s.add(new_task("Gym", true, None, NaiveTime::from_hms_opt(8, 0, 0)));
        s.save().unwrap();
        let s2 = Store::open(p);
        assert_eq!(s2.tasks.len(), 2);
        assert_eq!(s2.tasks[0].title, "Call the bank");
        assert!(s2.tasks[0].overdue(now));
        assert_eq!(fmt_relative(&s2.tasks[0], now), "30m late");
        assert_eq!(s2.next_id, id + 2);
        let _ = std::fs::remove_dir_all(dir);
    }
}
