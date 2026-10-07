//! Activity heatmap: one square per day for the last year, coloured by how many work-log entries
//! (finished tasks and notes) that day has, like a GitHub contribution graph. Streaks and stats below,
//! and the entries of the day under the cursor.

use crate::config::Theme;
use chrono::{Datelike, Duration, NaiveDate, Weekday};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use std::collections::BTreeMap;

pub struct Heatmap {
    pub days: BTreeMap<NaiveDate, Vec<String>>,   // entries per day ("09:14  ✔ Call the bank — …")
    pub cursor: NaiveDate,
    today: NaiveDate,
}

/// Work-log entries per day.
pub fn parse(text: &str) -> BTreeMap<NaiveDate, Vec<String>> {
    let mut days: BTreeMap<NaiveDate, Vec<String>> = BTreeMap::new();
    let mut cur: Option<NaiveDate> = None;
    for l in text.lines() {
        if let Some(h) = l.strip_prefix("## ") {
            cur = h.get(..10).and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
            if let Some(d) = cur { days.entry(d).or_default(); }
        } else if let (Some(d), Some(e)) = (cur, l.strip_prefix("- ")) {
            days.entry(d).or_default().push(e.trim().to_string());
        }
    }
    days.retain(|_, v| !v.is_empty());
    days
}

pub struct Stats { pub total: usize, pub active: usize, pub streak: usize, pub best_streak: usize, pub best_day: Option<(NaiveDate, usize)>, pub week: usize, pub done: usize }

pub fn stats(days: &BTreeMap<NaiveDate, Vec<String>>, today: NaiveDate) -> Stats {
    let year_ago = today - Duration::days(365);
    let in_year = days.range(year_ago..=today);
    let mut st = Stats { total: 0, active: 0, streak: 0, best_streak: 0, best_day: None, week: 0, done: 0 };
    for (d, v) in in_year {
        st.total += v.len();
        st.active += 1;
        st.done += v.iter().filter(|e| e.contains('✔')).count();
        if st.best_day.map(|(_, n)| v.len() > n).unwrap_or(true) { st.best_day = Some((*d, v.len())); }
    }
    let monday = today - Duration::days(today.weekday().num_days_from_monday() as i64);
    st.week = days.range(monday..=today).map(|(_, v)| v.len()).sum();
    // current streak: days in a row with entries, ending today (or yesterday if nothing yet today)
    let mut d = if days.contains_key(&today) { today } else { today - Duration::days(1) };
    while days.contains_key(&d) { st.streak += 1; d -= Duration::days(1); }
    let mut run = 0;
    let mut prev: Option<NaiveDate> = None;
    for d in days.keys() {
        run = if prev.map(|p| *d - p == Duration::days(1)).unwrap_or(false) { run + 1 } else { 1 };
        st.best_streak = st.best_streak.max(run);
        prev = Some(*d);
    }
    st
}

fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> Color {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color::Rgb(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

impl Heatmap {
    pub fn load() -> Heatmap {
        let text = std::fs::read_to_string(crate::worklog::path()).unwrap_or_default();
        let today = chrono::Local::now().date_naive();
        Heatmap { days: parse(&text), cursor: today, today }
    }

    pub fn on_key(&mut self, k: KeyEvent) -> bool {
        let d = match k.code {
            KeyCode::Left => -7, KeyCode::Right => 7, KeyCode::Up => -1, KeyCode::Down => 1,
            KeyCode::PageUp => -28, KeyCode::PageDown => 28,
            KeyCode::Home => { self.cursor = self.today; return true; }
            KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::F(5) => { let c = self.cursor; *self = Heatmap::load(); self.cursor = c; return true; }
            _ => return false,
        };
        let n = self.cursor + Duration::days(d);
        if n <= self.today && n > self.today - Duration::days(371) { self.cursor = n; }
        true
    }

    pub fn draw(&self, f: &mut Frame, area: Rect, t: &Theme) {
        let buf = f.buffer_mut();
        let bg = t.bg_rgb;
        let acc = match t.accent { Color::Rgb(r, g, b) => (r, g, b), _ => (99, 179, 237) };
        let levels = [mix(bg, (128, 128, 128), 0.18), mix(bg, acc, 0.30), mix(bg, acc, 0.55), mix(bg, acc, 0.80), mix(bg, acc, 1.0)];
        let dim = Style::default().fg(t.dim);
        let max = self.days.values().map(|v| v.len()).max().unwrap_or(1).max(4);
        let level = |n: usize| if n == 0 { 0 } else { (((n as f32 / max as f32) * 4.0).ceil() as usize).clamp(1, 4) };
        // how many weeks fit: 2 cells per week after a 5-cell day label
        let weeks = (((area.width as usize).saturating_sub(8)) / 2).clamp(4, 53) as i64;
        let this_monday = self.today - Duration::days(self.today.weekday().num_days_from_monday() as i64);
        let first = this_monday - Duration::days(7 * (weeks - 1));
        let x0 = area.x + 7;
        let y0 = area.y + 2;
        buf.set_string(area.x + 2, area.y, "ACTIVITY", Style::default().fg(t.accent).add_modifier(Modifier::BOLD));
        buf.set_string(area.x + 12, area.y, format!("work-log entries per day  ·  last {weeks} weeks"), dim);
        // month names over the first week of each month
        let mut last_month = 0;
        for w in 0..weeks {
            let d = first + Duration::days(7 * w);
            if d.month() != last_month {
                last_month = d.month();
                let x = x0 + (w as u16) * 2;
                if x + 3 < area.x + area.width { buf.set_string(x, area.y + 1, d.format("%b").to_string(), dim); }
            }
        }
        for (i, name) in ["Mon", "", "Wed", "", "Fri", "", "Sun"].iter().enumerate() {
            buf.set_string(area.x + 2, y0 + i as u16, *name, dim);
        }
        for w in 0..weeks {
            for wd in 0..7 {
                let d = first + Duration::days(7 * w + wd);
                if d > self.today { continue; }
                let n = self.days.get(&d).map(|v| v.len()).unwrap_or(0);
                let x = x0 + (w as u16) * 2;
                let y = y0 + wd as u16;
                if y >= area.y + area.height { continue; }
                let cur = d == self.cursor;
                let st = Style::default().fg(levels[level(n)]);
                buf.set_string(x, y, if cur { "▣" } else { "■" }, if cur { st.add_modifier(Modifier::BOLD).fg(if n == 0 { t.text } else { levels[level(n)] }) } else { st });
            }
        }
        // legend
        let ly = y0 + 8;
        if ly < area.y + area.height {
            buf.set_string(x0, ly, "less ", dim);
            for (i, c) in levels.iter().enumerate() { buf.set_string(x0 + 5 + i as u16 * 2, ly, "■", Style::default().fg(*c)); }
            buf.set_string(x0 + 15, ly, " more", dim);
        }
        // stats
        let st = stats(&self.days, self.today);
        let sy = y0 + 10;
        let best = st.best_day.map(|(d, n)| format!("{n} on {}", d.format("%a %-d %b"))).unwrap_or_else(|| "-".into());
        let row = |label: &str, val: String, col: Color| vec![Span::styled(format!("  {label:<18}"), dim), Span::styled(val, Style::default().fg(col).add_modifier(Modifier::BOLD))];
        let lines: Vec<Line> = vec![
            Line::from(row("current streak", format!("{} {}", st.streak, if st.streak == 1 { "day" } else { "days" }), if st.streak > 0 { t.ok } else { t.dim })),
            Line::from(row("longest streak", format!("{} days", st.best_streak), t.text)),
            Line::from(row("this week", format!("{} entries", st.week), t.text)),
            Line::from(row("last 12 months", format!("{} entries · {} tasks done · {} active days", st.total, st.done, st.active), t.text)),
            Line::from(row("busiest day", best, t.text)),
        ];
        if sy < area.y + area.height {
            f.render_widget(Paragraph::new(lines), Rect { x: area.x, y: sy, width: area.width, height: (area.y + area.height).saturating_sub(sy).min(5) });
        }
        // the selected day
        let dy = sy + 6;
        if dy + 1 < area.y + area.height {
            let entries = self.days.get(&self.cursor).cloned().unwrap_or_default();
            let mut lines = vec![Line::from(vec![
                Span::styled(format!("  {}", self.cursor.format("%A %-d %B %Y")), Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
                Span::styled(format!("   {} {}", entries.len(), if entries.len() == 1 { "entry" } else { "entries" }), dim),
            ])];
            if entries.is_empty() { lines.push(Line::styled("    nothing logged", dim)); }
            for e in &entries {
                let (time, rest) = e.split_at(e.find("  ").unwrap_or(0));
                lines.push(Line::from(vec![Span::styled(format!("    {time}"), dim), Span::styled(format!("  {}", rest.trim()), Style::default().fg(if rest.contains('✔') { t.ok } else { t.text }))]));
            }
            f.render_widget(Paragraph::new(lines), Rect { x: area.x, y: dy, width: area.width, height: (area.y + area.height).saturating_sub(dy) });
        }
        let _ = Weekday::Mon;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_and_stats() {
        let text = "# Work log\n\n## 2026-10-03 · Saturday\n\n- 09:00  a\n\n## 2026-10-04 · Sunday\n\n- 10:00  ✔ b\n- 11:00  c\n\n## 2026-10-05 · Monday\n\n- 09:14  ✔ d — x\n";
        let days = parse(text);
        assert_eq!(days.len(), 3);
        let today = NaiveDate::from_ymd_opt(2026, 10, 5).unwrap();
        let s = stats(&days, today);
        assert_eq!((s.total, s.active, s.streak, s.best_streak, s.done), (4, 3, 3, 3, 2));
        assert_eq!(s.best_day.unwrap().1, 2);
        assert_eq!(s.week, 1);
    }
}
