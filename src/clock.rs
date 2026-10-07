//! Clock widgets for the Tasks screen: an analog clock face, big digits, and the time left today.

use chrono::{Datelike, NaiveDateTime, NaiveTime, Timelike, Weekday};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Circle, Line as CLine};
use ratatui::widgets::{Paragraph, Widget};

const FONT: [[&str; 5]; 11] = [
    ["███", "█ █", "█ █", "█ █", "███"],
    ["  █", "  █", "  █", "  █", "  █"],
    ["███", "  █", "███", "█  ", "███"],
    ["███", "  █", "███", "  █", "███"],
    ["█ █", "█ █", "███", "  █", "  █"],
    ["███", "█  ", "███", "  █", "███"],
    ["███", "█  ", "███", "█ █", "███"],
    ["███", "  █", "  █", "  █", "  █"],
    ["███", "█ █", "███", "█ █", "███"],
    ["███", "█ █", "███", "  █", "███"],
    [" ", "▀", " ", "▀", " "],
];

/// Width in cells of `text` (digits and ':') drawn with big_text.
pub fn big_width(text: &str) -> u16 {
    text.chars().map(|c| if c == ':' { 2 } else { 4 }).sum::<u16>().saturating_sub(1)
}

/// Five rows tall digits, centred in `area`.
pub fn big_text(buf: &mut Buffer, area: Rect, text: &str, style: Style) {
    let w = big_width(text);
    if area.height < 5 || area.width < w {
        // too small: plain text instead
        let x = area.x + area.width.saturating_sub(text.len() as u16) / 2;
        buf.set_string(x, area.y + area.height.saturating_sub(1) / 2, text, style.add_modifier(Modifier::BOLD));
        return;
    }
    let mut x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - 5) / 2;
    for c in text.chars() {
        let glyph = match c { '0'..='9' => &FONT[(c as u8 - b'0') as usize], ':' => &FONT[10], _ => { x += 4; continue } };
        for (r, row) in glyph.iter().enumerate() { buf.set_string(x, y + r as u16, row, style); }
        x += if c == ':' { 2 } else { 4 };
    }
}

/// Round clock face with hour ticks and three hands.
pub fn analog(buf: &mut Buffer, area: Rect, now: NaiveDateTime, face: Color, hands: Color, second: Color, bg: Option<Color>) {
    if area.width < 8 || area.height < 4 { return; }
    // a cell is about twice as tall as it is wide: make the coordinate space match so the circle is round
    let (w, h) = (area.width as f64, area.height as f64 * 2.0);
    let r = (w.min(h) / 2.0) * 0.92;
    let secs = now.second() as f64;
    let mins = now.minute() as f64 + secs / 60.0;
    let hours = (now.hour() % 12) as f64 + mins / 60.0;
    let at = |frac: f64, len: f64| { let a = std::f64::consts::FRAC_PI_2 - frac * std::f64::consts::TAU; (len * a.cos(), len * a.sin()) };
    let mut c = Canvas::default().marker(Marker::Braille).x_bounds([-w / 2.0, w / 2.0]).y_bounds([-h / 2.0, h / 2.0])
        .paint(move |ctx| {
            ctx.draw(&Circle { x: 0.0, y: 0.0, radius: r, color: face });
            for i in 0..60 {
                let big = i % 5 == 0;
                if !big && r < 9.0 { continue; }
                let (x1, y1) = at(i as f64 / 60.0, r * if i % 15 == 0 { 0.80 } else if big { 0.87 } else { 0.95 });
                let (x2, y2) = at(i as f64 / 60.0, r);
                ctx.draw(&CLine::new(x1, y1, x2, y2, face));
            }
            ctx.layer();
            // hour hand (drawn three times, slightly apart, to look thicker)
            let (hx, hy) = at(hours / 12.0, r * 0.5);
            for o in [-0.25, 0.0, 0.25] { ctx.draw(&CLine::new(o, o, hx + o, hy + o, hands)); }
            let (mx, my) = at(mins / 60.0, r * 0.78);
            for o in [-0.15, 0.15] { ctx.draw(&CLine::new(o, 0.0, mx + o, my, hands)); }
            ctx.layer();
            let (sx, sy) = at(secs / 60.0, r * 0.88);
            let (tx, ty) = at(secs / 60.0 + 0.5, r * 0.15);
            ctx.draw(&CLine::new(tx, ty, sx, sy, second));
            ctx.draw(&Circle { x: 0.0, y: 0.0, radius: (r * 0.04).max(0.5), color: second });
        });
    if let Some(bg) = bg { c = c.background_color(bg); }
    c.render(area, buf);
}

/// Seconds left until midnight.
pub fn left_today(now: NaiveDateTime) -> i64 { 86_400 - now.num_seconds_from_midnight() as i64 }

pub fn hms(secs: i64) -> String { let s = secs.max(0); format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) }

/// Work periods for the countdown, from [tasks] in config.ini:
///   countdown = 8-15                        every day (or the days in countdown_days)
///   countdown_tue = 8-13:30, 15:30-18:30    a different schedule on Tuesday ("off" = no work that day)
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Periods {
    pub spans: Vec<(NaiveTime, NaiveTime)>,
    pub days: Vec<Weekday>,
    pub per_day: Vec<(Weekday, Vec<(NaiveTime, NaiveTime)>)>,
}

pub const DAY_KEYS: [(&str, Weekday); 7] = [("mon", Weekday::Mon), ("tue", Weekday::Tue), ("wed", Weekday::Wed), ("thu", Weekday::Thu), ("fri", Weekday::Fri), ("sat", Weekday::Sat), ("sun", Weekday::Sun)];

pub fn day_key(d: Weekday) -> &'static str { DAY_KEYS.iter().find(|(_, x)| *x == d).map(|(k, _)| *k).unwrap_or("mon") }

/// "8-12, 14-18", "08:00 - 12:00; 14:30-18" ("" / "day" / "midnight" = the whole day).
pub fn parse_periods(s: &str) -> Result<Vec<(NaiveTime, NaiveTime)>, String> {
    let s = s.trim().to_lowercase();
    if s.is_empty() || s == "day" || s == "midnight" || s == "none" || s == "off" { return Ok(Vec::new()); }
    let mut v = Vec::new();
    for part in s.split([',', ';']).map(|p| p.trim()).filter(|p| !p.is_empty()) {
        let (a, b) = part.split_once(" to ").or_else(|| part.split_once('-')).or_else(|| part.split_once('–'))
            .ok_or_else(|| format!("“{part}” needs a start and an end, like 8-12"))?;
        let t = |x: &str| match crate::tasks::parse_time(x) { Ok(crate::tasks::TimeIn::At(t)) => Ok(t), _ => Err(format!("“{}” is not a time", x.trim())) };
        let (a2, mut b2) = (t(a)?, t(b)?);
        if b2 == NaiveTime::MIN { b2 = NaiveTime::from_hms_opt(23, 59, 59).unwrap(); }   // 18-24 / 18-0
        if b2 <= a2 { return Err(format!("“{part}” ends before it starts")); }
        v.push((a2, b2));
    }
    v.sort();
    // join overlapping periods
    let mut out: Vec<(NaiveTime, NaiveTime)> = Vec::new();
    for (a, b) in v { match out.last_mut() { Some(l) if a <= l.1 => l.1 = l.1.max(b), _ => out.push((a, b)) } }
    Ok(out)
}

pub fn fmt_periods(v: &[(NaiveTime, NaiveTime)]) -> String {
    v.iter().map(|(a, b)| format!("{}-{}", a.format("%H:%M"), b.format("%H:%M"))).collect::<Vec<_>>().join(", ")
}

/// "mon-fri", "mon,tue,thu", "tue thu", "" = every day.
pub fn parse_days(s: &str) -> Vec<Weekday> {
    let all = [Weekday::Mon, Weekday::Tue, Weekday::Wed, Weekday::Thu, Weekday::Fri, Weekday::Sat, Weekday::Sun];
    let day = |x: &str| -> Option<usize> {
        let x = x.trim().to_lowercase();
        let names = [["mon", "lun"], ["tue", "mar"], ["wed", "mie"], ["thu", "jue"], ["fri", "vie"], ["sat", "sab"], ["sun", "dom"]];
        names.iter().position(|n| n.iter().any(|p| x.starts_with(p)) || (x.starts_with("mié") && n[1] == "mie") || (x.starts_with("sáb") && n[1] == "sab"))
    };
    let mut v = Vec::new();
    for part in s.split([',', ' ', '/', '&']).map(|p| p.trim()).filter(|p| !p.is_empty() && *p != "and" && *p != "y") {
        if let Some((a, b)) = part.split_once('-') {
            if let (Some(a), Some(b)) = (day(a), day(b)) { let mut i = a; loop { v.push(all[i]); if i == b { break; } i = (i + 1) % 7; } }
        } else if let Some(d) = day(part) { v.push(all[d]); }
    }
    v.sort_by_key(|d| d.num_days_from_monday());
    v.dedup();
    v
}

/// What P in Tasks & clock was given: "8-15" (every day) or "tue,thu: 8-13:30, 15:30-18:30" (those days).
/// "tue: off" = no work periods that day, "tue: default" = back to the normal schedule.
pub enum PeriodInput { All(Vec<(NaiveTime, NaiveTime)>), Days(Vec<Weekday>, Option<Vec<(NaiveTime, NaiveTime)>>, bool) }

pub fn parse_input(s: &str) -> Result<PeriodInput, String> {
    if let Some((d, rest)) = s.split_once(':').filter(|(d, _)| d.chars().any(|c| c.is_alphabetic())) {
        let days = parse_days(d);
        if days.is_empty() { return Err(format!("“{}” is not a day (use mon tue wed thu fri sat sun)", d.trim())); }
        let r = rest.trim().to_lowercase();
        if r == "default" || r == "normal" || r == "same" { return Ok(PeriodInput::Days(days, None, false)); }
        if r == "off" || r == "free" || r == "none" { return Ok(PeriodInput::Days(days, Some(Vec::new()), true)); }
        let v = parse_periods(rest)?;
        if v.is_empty() { return Ok(PeriodInput::Days(days, None, false)); }
        return Ok(PeriodInput::Days(days, Some(v), false));
    }
    parse_periods(s).map(PeriodInput::All)
}

impl Periods {
    pub fn from_config(ini: &crate::config::Ini) -> (Periods, Option<String>) {
        let mut err = None;
        let spans = match parse_periods(ini.get("tasks", "countdown").unwrap_or("")) { Ok(v) => v, Err(e) => { err = Some(format!("[tasks] countdown: {e}")); Vec::new() } };
        let mut per_day = Vec::new();
        for (k, d) in DAY_KEYS {
            let Some(v) = ini.get("tasks", &format!("countdown_{k}")) else { continue };
            match parse_periods(v) { Ok(x) => per_day.push((d, x)), Err(e) => err = Some(format!("[tasks] countdown_{k}: {e}")) }
        }
        (Periods { spans, days: parse_days(ini.get("tasks", "countdown_days").unwrap_or("")), per_day }, err)
    }
    /// The periods for one day of the week (empty = count down to midnight).
    pub fn for_day(&self, d: Weekday) -> &[(NaiveTime, NaiveTime)] {
        if let Some((_, v)) = self.per_day.iter().find(|(x, _)| *x == d) { return v; }
        if self.days.is_empty() || self.days.contains(&d) { &self.spans } else { &[] }
    }
    pub fn works_on(&self, d: Weekday) -> bool { !self.for_day(d).is_empty() }
    pub fn any(&self) -> bool { !self.spans.is_empty() || self.per_day.iter().any(|(_, v)| !v.is_empty()) }
    /// "Mon 08:00-15:00 · Tue 08:00-13:30, 15:30-18:30 · ..." (days with the same hours grouped)
    pub fn week_text(&self) -> String {
        let mut groups: Vec<(Vec<&str>, String)> = Vec::new();
        for (k, d) in DAY_KEYS {
            let t = fmt_periods(self.for_day(d));
            let t = if t.is_empty() { "off".to_string() } else { t };
            match groups.iter_mut().find(|(_, x)| *x == t) { Some(g) => g.0.push(k), None => groups.push((vec![k], t)) }
        }
        groups.iter().map(|(d, t)| format!("{} {t}", d.join(","))).collect::<Vec<_>>().join("  ·  ")
    }
}

pub struct Countdown {
    pub label: String,      // "TIME LEFT TODAY", "WORK TIME LEFT · until 12:00", "BREAK · back at 14:00" ...
    pub secs: i64,          // what the big digits count down
    pub active: bool,       // inside a period (or whole-day mode): the countdown runs in colour
    pub urgent: bool,       // last 15 minutes of a period / last hour of the day
    pub progress: f64,      // 0..1 for the bar
    pub bar_label: String,
    pub detail: String,     // one line under the bar
}

/// What the countdown shows right now.
pub fn countdown(p: &Periods, now: NaiveDateTime) -> Countdown {
    let t = now.time();
    let secs = |a: NaiveTime, b: NaiveTime| (b - a).num_seconds().max(0);
    let spans = p.for_day(now.weekday());
    if spans.is_empty() {
        let left = left_today(now);
        let gone = now.num_seconds_from_midnight() as f64 / 86_400.0;
        let detail = if !p.any() { String::new() } else { format!("no work periods today  ·  {}", next_start(p, now)) };
        return Countdown { label: "TIME LEFT TODAY".into(), secs: left, active: true, urgent: left < 3600, progress: gone, bar_label: format!("{:.0}% of the day gone", gone * 100.0), detail };
    }
    let total: i64 = spans.iter().map(|(a, b)| secs(*a, *b)).sum();
    let left_work: i64 = spans.iter().map(|(a, b)| if t >= *b { 0 } else if t <= *a { secs(*a, *b) } else { secs(t, *b) }).sum();
    let done = 1.0 - left_work as f64 / total.max(1) as f64;
    let bar_label = format!("{:.0}% of the work day done", done * 100.0);
    let work_left = format!("work left today {} of {}", fmt_hm(left_work), fmt_hm(total));
    if let Some((a, b)) = spans.iter().find(|(a, b)| t >= *a && t < *b) {
        let left = secs(t, *b);
        let urgent = left <= 15 * 60;
        return Countdown { label: format!("WORK TIME LEFT  ·  {}–{}", a.format("%H:%M"), b.format("%H:%M")), secs: left, active: true, urgent, progress: done, bar_label, detail: work_left };
    }
    if let Some((a, _)) = spans.iter().find(|(a, _)| t < *a) {
        let first = spans[0].0 == *a;
        let label = if first { format!("STARTS AT {}", a.format("%H:%M")) } else { format!("BREAK  ·  back at {}", a.format("%H:%M")) };
        return Countdown { label, secs: secs(t, *a), active: false, urgent: false, progress: done, bar_label, detail: if first { format!("today  {}", fmt_periods(spans).replace(", ", "  ")) } else { work_left } };
    }
    Countdown { label: "DONE FOR TODAY".into(), secs: 0, active: false, urgent: false, progress: 1.0, bar_label, detail: next_start(p, now) }
}

fn next_start(p: &Periods, now: NaiveDateTime) -> String {
    for i in 1..=7 {
        let d = now.date() + chrono::Duration::days(i);
        if p.works_on(d.weekday()) {
            let when = if i == 1 { "tomorrow".to_string() } else { d.format("%A").to_string() };
            return format!("next: {when} at {}", p.for_day(d.weekday())[0].0.format("%H:%M"));
        }
    }
    String::new()
}

fn fmt_hm(s: i64) -> String { format!("{}h {:02}m", s / 3600, (s / 60) % 60) }

/// "████████░░░░  62% of the work day done"
pub fn bar(buf: &mut Buffer, area: Rect, progress: f64, label: &str, fill: Color, empty: Color, dim: Color) {
    let label = format!("  {label}");
    let bar_w = (area.width as usize).saturating_sub(label.chars().count() + 2).max(4);
    let full = ((bar_w as f64) * progress.clamp(0.0, 1.0)).round() as usize;
    let line = Line::from(vec![
        Span::raw(" "),
        Span::styled("█".repeat(full), Style::default().fg(fill)),
        Span::styled("░".repeat(bar_w - full.min(bar_w)), Style::default().fg(empty)),
        Span::styled(label, Style::default().fg(dim)),
    ]);
    Paragraph::new(line).render(area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tm(h: u32, m: u32) -> NaiveTime { NaiveTime::from_hms_opt(h, m, 0).unwrap() }
    #[test]
    fn periods() {
        assert_eq!(parse_periods("8-12, 14-18").unwrap(), vec![(tm(8, 0), tm(12, 0)), (tm(14, 0), tm(18, 0))]);
        assert_eq!(parse_periods("14:30 - 18; 08:00-12:00").unwrap(), vec![(tm(8, 0), tm(12, 0)), (tm(14, 30), tm(18, 0))]);
        assert_eq!(parse_periods("8-12,11-13").unwrap(), vec![(tm(8, 0), tm(13, 0))]);
        assert_eq!(parse_periods("9am to 5pm").unwrap(), vec![(tm(9, 0), tm(17, 0))]);
        assert!(parse_periods("12-8").is_err());
        assert_eq!(parse_periods("8-13:30, 15:30-18:30").unwrap(), vec![(tm(8, 0), tm(13, 30)), (tm(15, 30), tm(18, 30))]);
        // the user's week: 8-15, Tuesday and Thursday split
        let ini = crate::config::parse_ini("[tasks]\ncountdown = 8-15\ncountdown_tue = 8-13:30, 15:30-18:30\ncountdown_thu = 8-13:30, 15:30-18:30\ncountdown_sat = off\ncountdown_sun = off\n");
        let (w, err) = Periods::from_config(&ini);
        assert!(err.is_none());
        assert_eq!(w.for_day(Weekday::Mon), &[(tm(8, 0), tm(15, 0))]);
        assert_eq!(w.for_day(Weekday::Thu).len(), 2);
        assert!(w.for_day(Weekday::Sun).is_empty());
        assert_eq!(w.week_text(), "mon,wed,fri 08:00-15:00  ·  tue,thu 08:00-13:30, 15:30-18:30  ·  sat,sun off");
        let tue = chrono::NaiveDate::from_ymd_opt(2026, 10, 6).unwrap();
        let c = countdown(&w, tue.and_time(tm(14, 0)));
        assert!(c.label.contains("back at 15:30") && c.detail.contains("3h 00m of 8h 30m"), "{} {}", c.label, c.detail);
        let mon = chrono::NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let c = countdown(&w, mon.and_time(tm(14, 0)));
        assert!(c.active && c.secs == 3600, "{}", c.label);
        let fri = chrono::NaiveDate::from_ymd_opt(2026, 10, 9).unwrap();
        let c = countdown(&w, fri.and_time(tm(16, 0)));
        assert!(c.label == "DONE FOR TODAY" && c.detail.contains("Monday at 08:00"), "{}", c.detail);
        assert!(matches!(parse_input("tue, thu: 8-13:30, 15:30-18:30"), Ok(PeriodInput::Days(d, Some(v), false)) if d.len() == 2 && v.len() == 2));
        assert!(matches!(parse_input("sat sun: off"), Ok(PeriodInput::Days(d, Some(v), true)) if d.len() == 2 && v.is_empty()));
        assert!(matches!(parse_input("8-15"), Ok(PeriodInput::All(v)) if v.len() == 1));
        assert!(parse_periods("8").is_err());
        assert!(parse_periods("").unwrap().is_empty());
        assert_eq!(parse_days("mon-fri").len(), 5);
        assert_eq!(parse_days("sat-mon"), vec![Weekday::Mon, Weekday::Sat, Weekday::Sun]);
        assert_eq!(parse_days("lunes, miércoles"), vec![Weekday::Mon, Weekday::Wed]);
        let p = Periods { spans: parse_periods("8-12, 14-18").unwrap(), days: vec![], per_day: vec![] };
        let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let c = countdown(&p, day.and_time(tm(7, 30)));
        assert!(c.label.starts_with("STARTS AT 08:00") && !c.active && c.secs == 1800, "{}", c.label);
        let c = countdown(&p, day.and_time(tm(10, 0)));
        assert!(c.active && c.secs == 7200 && c.detail.contains("6h 00m of 8h 00m"), "{} {}", c.label, c.detail);
        let c = countdown(&p, day.and_time(tm(12, 30)));
        assert!(c.label.contains("back at 14:00") && c.secs == 5400 && !c.active);
        let c = countdown(&p, day.and_time(tm(17, 50)));
        assert!(c.active && c.urgent && c.secs == 600);
        let c = countdown(&p, day.and_time(tm(19, 0)));
        assert!(c.label == "DONE FOR TODAY" && c.secs == 0 && c.detail.contains("tomorrow at 08:00"));
        // not a work day: whole-day countdown
        let p2 = Periods { spans: p.spans.clone(), days: parse_days("mon-fri"), per_day: vec![] };
        let sat = chrono::NaiveDate::from_ymd_opt(2026, 10, 10).unwrap();
        let c = countdown(&p2, sat.and_time(tm(10, 0)));
        assert!(c.label == "TIME LEFT TODAY" && c.detail.contains("Monday"), "{}", c.detail);
    }
}
