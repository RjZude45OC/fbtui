//! Typing test in the style of monkeytype: random common English words, time or word-count mode,
//! live caret and colouring, then WPM, raw, accuracy, consistency, a WPM graph and personal bests.
//! History is kept in typing.json next to tasks.json.

use crate::config::{Config, Theme};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

const WORDS: &str = "the be of and a to in he have it that for they with as not on she at by this we you do but from or which one would all will there say who make when can more if no man out other so what time up go about than into could state only new year some take come these know see use get like then first any work now may such give over think most even find day also after way many must look before great back through long where much should well people down own just because good each those feel seem how high too place little world very still nation hand old life tell write become here show house both between need mean call develop under last right move thing general school never same another begin while number part turn real leave might want point form off child few small since against ask late home interest large person end open public follow during present without again hold govern around possible head consider word program problem however lead system set order eye plan run keep face fact group play stand increase early course change help line";

#[derive(Clone, Copy, PartialEq)]
pub enum Mode { Time(u32), Words(usize) }

const MODES: &[Mode] = &[Mode::Time(15), Mode::Time(30), Mode::Time(60), Mode::Time(120), Mode::Words(10), Mode::Words(25), Mode::Words(50), Mode::Words(100)];

impl Mode {
    fn label(&self) -> String { match self { Mode::Time(s) => format!("time {s}"), Mode::Words(n) => format!("words {n}") } }
    fn key(&self) -> String { match self { Mode::Time(s) => format!("time{s}"), Mode::Words(n) => format!("words{n}") } }
}

fn rand_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    h.finish()
}

fn words(n: usize) -> Vec<String> {
    let list: Vec<&str> = WORDS.split_whitespace().collect();
    let mut seed = rand_u64() | 1;
    let mut out: Vec<String> = Vec::with_capacity(n);
    while out.len() < n {
        seed ^= seed << 13; seed ^= seed >> 7; seed ^= seed << 17;
        let w = list[(seed % list.len() as u64) as usize];
        if out.last().map(|l| l == w).unwrap_or(false) { continue; }
        out.push(w.to_string());
    }
    out
}

pub struct Result_ { pub wpm: f64, pub raw: f64, pub acc: f64, pub consistency: f64, pub correct: usize, pub incorrect: usize, pub extra: usize, pub missed: usize, pub secs: f64, pub per_sec: Vec<(f64, usize)> }

pub struct Test {
    mode: Mode,
    words: Vec<String>,
    typed: Vec<String>,
    cur: usize,
    start: Option<Instant>,
    keys_ok: usize,
    keys_bad: usize,
    // per second: chars typed and errors, for the graph and consistency
    sec_chars: Vec<usize>,
    sec_errs: Vec<usize>,
    result: Option<Result_>,
    best: Option<f64>,
    new_best: bool,
    history_note: String,
}

fn history_path() -> std::path::PathBuf { crate::tasks::default_path().with_file_name("typing.json") }

fn load_history() -> Vec<Value> {
    std::fs::read_to_string(history_path()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("results").and_then(|r| r.as_array()).cloned()).unwrap_or_default()
}

fn best_for(hist: &[Value], mode: Mode) -> Option<f64> {
    hist.iter().filter(|r| r.get("mode").and_then(|m| m.as_str()) == Some(mode.key().as_str()))
        .filter_map(|r| r.get("wpm").and_then(|w| w.as_f64())).fold(None, |a: Option<f64>, b| Some(a.map_or(b, |a| a.max(b))))
}

impl Test {
    pub fn new(mode: Mode) -> Test {
        let n = match mode { Mode::Words(n) => n, Mode::Time(s) => (s as usize * 4).max(60) };
        let best = best_for(&load_history(), mode);
        Test { mode, words: words(n), typed: vec![String::new()], cur: 0, start: None, keys_ok: 0, keys_bad: 0, sec_chars: Vec::new(), sec_errs: Vec::new(), result: None, best, new_best: false, history_note: String::new() }
    }

    fn elapsed(&self) -> f64 { self.start.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0) }

    fn bump(&mut self, ok: bool) {
        let sec = self.elapsed() as usize;
        while self.sec_chars.len() <= sec { self.sec_chars.push(0); self.sec_errs.push(0); }
        self.sec_chars[sec] += 1;
        if !ok { self.sec_errs[sec] += 1; }
        if ok { self.keys_ok += 1 } else { self.keys_bad += 1 }
    }

    /// Typing keys. Returns true when the test just finished.
    fn key(&mut self, k: KeyEvent) -> bool {
        if self.result.is_some() { return false; }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL) || k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Char(' ') => {
                if self.typed[self.cur].is_empty() { return false; }
                let ok = self.typed[self.cur] == self.words[self.cur];
                self.bump(ok);
                if self.cur + 1 >= self.words.len() { return self.finish(); }
                self.cur += 1;
                self.typed.push(String::new());
                if let Mode::Time(_) = self.mode { if self.words.len() - self.cur < 40 { let more = words(60); self.words.extend(more); } }
            }
            KeyCode::Backspace if ctrl => {
                if self.typed[self.cur].is_empty() { self.back_word(); }
                self.typed[self.cur].clear();
            }
            KeyCode::Char('h') | KeyCode::Char('w') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.typed[self.cur].is_empty() { self.back_word(); }
                self.typed[self.cur].clear();
            }
            KeyCode::Backspace => {
                if self.typed[self.cur].pop().is_none() { self.back_word(); }
            }
            KeyCode::Char(c) if !c.is_whitespace() => {
                if self.start.is_none() { self.start = Some(Instant::now()); }
                let w = &self.words[self.cur];
                let i = self.typed[self.cur].chars().count();
                if i >= w.chars().count() + 12 { return false; }
                let ok = w.chars().nth(i) == Some(c);
                self.typed[self.cur].push(c);
                self.bump(ok);
                // words mode: the last word typed correctly ends the test without a space
                if let Mode::Words(_) = self.mode { if self.cur + 1 == self.words.len() && self.typed[self.cur] == self.words[self.cur] { return self.finish(); } }
            }
            _ => {}
        }
        false
    }

    /// Like monkeytype: only a wrong word can be gone back into.
    fn back_word(&mut self) {
        if self.cur > 0 && self.typed[self.cur - 1] != self.words[self.cur - 1] {
            self.typed.pop();
            self.cur -= 1;
        }
    }

    pub fn tick(&mut self) -> bool {
        if let (Mode::Time(s), Some(_), None) = (self.mode, self.start, &self.result) {
            if self.elapsed() >= s as f64 { return self.finish(); }
        }
        false
    }

    fn finish(&mut self) -> bool {
        let secs = match self.mode { Mode::Time(s) => (s as f64).min(self.elapsed().max(0.5)), Mode::Words(_) => self.elapsed().max(0.5) };
        let (mut correct, mut incorrect, mut extra, mut missed, mut word_chars) = (0, 0, 0, 0, 0);
        let done = self.cur + 1;
        for i in 0..done.min(self.typed.len()) {
            let (w, t): (Vec<char>, Vec<char>) = (self.words[i].chars().collect(), self.typed[i].chars().collect());
            for (j, c) in t.iter().enumerate() {
                match w.get(j) { Some(x) if x == c => correct += 1, Some(_) => incorrect += 1, None => extra += 1 }
            }
            let last_partial = i == self.cur && t.len() < w.len();
            if !last_partial { missed += w.len().saturating_sub(t.len()); }
            if t == w { word_chars += w.len() + if i + 1 < done { 1 } else { 0 }; }
        }
        let typed_chars: usize = self.typed.iter().map(|t| t.chars().count()).sum::<usize>() + self.cur;
        let mins = secs / 60.0;
        let wpm = word_chars as f64 / 5.0 / mins;
        let raw = typed_chars as f64 / 5.0 / mins;
        let acc = if self.keys_ok + self.keys_bad == 0 { 0.0 } else { self.keys_ok as f64 * 100.0 / (self.keys_ok + self.keys_bad) as f64 };
        let n = (secs.ceil() as usize).max(1);
        let per_sec: Vec<(f64, usize)> = (0..n).map(|i| (self.sec_chars.get(i).copied().unwrap_or(0) as f64 * 12.0, self.sec_errs.get(i).copied().unwrap_or(0))).collect();
        let vals: Vec<f64> = per_sec.iter().map(|p| p.0).collect();
        let mean = vals.iter().sum::<f64>() / vals.len() as f64;
        let sd = (vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / vals.len() as f64).sqrt();
        let consistency = if mean > 0.0 { (100.0 * (1.0 - (sd / mean).tanh())).clamp(0.0, 100.0) } else { 0.0 };
        self.new_best = self.best.map(|b| wpm > b).unwrap_or(wpm > 0.0);
        // history
        let mut hist = load_history();
        hist.push(json!({ "date": chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(), "mode": self.mode.key(), "wpm": (wpm * 100.0).round() / 100.0,
            "raw": (raw * 100.0).round() / 100.0, "acc": (acc * 10.0).round() / 10.0, "consistency": consistency.round() }));
        let saved = std::fs::write(history_path(), serde_json::to_string_pretty(&json!({ "results": hist })).unwrap_or_default());
        self.history_note = match saved { Ok(()) => format!("{} tests in typing.json", hist.len()), Err(e) => format!("could not save typing.json: {e}") };
        self.result = Some(Result_ { wpm, raw, acc, consistency, correct, incorrect, extra, missed, secs, per_sec });
        true
    }

    fn draw(&self, f: &mut Frame, t: &Theme, cfg: &Config) {
        let area = f.area();
        let base = match t.background { Some(bg) => Style::default().bg(bg).fg(t.text), None => Style::default().fg(t.text) };
        f.render_widget(Block::default().style(base), area);
        let dim = Style::default().fg(t.dim);
        let w = area.width.saturating_sub(8).min(84);
        let x0 = area.x + (area.width.saturating_sub(w)) / 2;
        f.buffer_mut().set_string(area.x + 2, area.y + 1, "▌ ", Style::default().fg(t.accent));
        f.buffer_mut().set_string(area.x + 4, area.y + 1, "TYPING TEST", Style::default().fg(t.text).add_modifier(Modifier::BOLD));
        // mode bar
        let mut spans = vec![Span::styled("  ", dim)];
        for m in MODES {
            let on = *m == self.mode;
            spans.push(Span::styled(format!(" {} ", m.label()), if on { Style::default().fg(t.accent).add_modifier(Modifier::BOLD | Modifier::UNDERLINED) } else { dim }));
        }
        if let Some(b) = self.best { spans.push(Span::styled(format!("   best {:.0} wpm", b), dim)); }
        f.render_widget(Paragraph::new(Line::from(spans)), Rect { x: area.x + 2, y: area.y + 2, width: area.width.saturating_sub(4), height: 1 });
        let _ = cfg;
        if let Some(r) = &self.result { self.draw_result(f, t, r, x0, w); return; }

        // progress: seconds left or words done
        let top = area.y + area.height.saturating_sub(5) / 2;
        let prog = match self.mode {
            Mode::Time(s) => format!("{}", (s as f64 - self.elapsed()).ceil().max(0.0) as i64),
            Mode::Words(n) => format!("{}/{}", self.cur, n),
        };
        let live = if self.start.is_some() && self.elapsed() > 1.0 {
            let ok_chars: usize = (0..self.cur).filter(|&i| self.typed[i] == self.words[i]).map(|i| self.words[i].len() + 1).sum();
            format!("   {:.0} wpm", ok_chars as f64 / 5.0 / (self.elapsed() / 60.0))
        } else { String::new() };
        f.buffer_mut().set_string(x0, top.saturating_sub(2), &prog, Style::default().fg(t.accent).add_modifier(Modifier::BOLD));
        f.buffer_mut().set_string(x0 + prog.width() as u16, top.saturating_sub(2), &live, dim);
        // lay the words out in lines
        let mut lines: Vec<Vec<usize>> = vec![Vec::new()];
        let mut lw = 0usize;
        for (i, wd) in self.words.iter().enumerate() {
            let len = wd.chars().count().max(self.typed.get(i).map(|t| t.chars().count()).unwrap_or(0)) + 1;
            if lw + len > w as usize && lw > 0 { lines.push(Vec::new()); lw = 0; }
            lines.last_mut().unwrap().push(i);
            lw += len;
            if lines.len() > self.cur / 3 + 200 { break; }
        }
        let cur_line = lines.iter().position(|l| l.contains(&self.cur)).unwrap_or(0);
        let first = cur_line.saturating_sub(1);
        let mut cursor = None;
        for (k, li) in lines.iter().skip(first).take(3).enumerate() {
            let y = top + k as u16;
            let mut x = x0;
            for &i in li {
                let word: Vec<char> = self.words[i].chars().collect();
                let typed: Vec<char> = self.typed.get(i).map(|t| t.chars().collect()).unwrap_or_default();
                let done = i < self.cur;
                let wrong_word = done && typed != word;
                for j in 0..word.len().max(typed.len()) {
                    let (ch, st) = match (word.get(j), typed.get(j)) {
                        (Some(w), Some(c)) if w == c => (*w, Style::default().fg(t.text)),
                        (Some(w), Some(_)) => (*w, Style::default().fg(t.error)),
                        (None, Some(c)) => (*c, Style::default().fg(t.error).add_modifier(Modifier::DIM)),
                        (Some(w), None) => (*w, if done { Style::default().fg(t.error).add_modifier(Modifier::DIM) } else { dim }),
                        (None, None) => (' ', dim),
                    };
                    let st = if wrong_word { st.add_modifier(Modifier::UNDERLINED).underline_color(t.error) } else { st };
                    f.buffer_mut().set_string(x, y, ch.to_string(), st);
                    if i == self.cur && j == typed.len() { cursor = Some((x, y)); }
                    x += 1;
                }
                if i == self.cur && typed.len() >= word.len() { cursor = Some((x, y)); }
                x += 1;
            }
        }
        if let Some(c) = cursor { f.set_cursor_position(c); }
        let hint = if self.start.is_none() { vec![("start typing", ""), ("← →", "mode"), ("Tab", "new words"), ("Esc", "back")] }
            else { vec![("Tab", "restart"), ("Ctrl+Backspace", "delete word"), ("Esc", "back")] };
        f.render_widget(Paragraph::new(crate::ui::hint_line(&hint, area.width, t)), Rect { x: area.x, y: area.y + area.height.saturating_sub(1), width: area.width, height: 1 });
    }

    fn draw_result(&self, f: &mut Frame, t: &Theme, r: &Result_, x0: u16, w: u16) {
        let area = f.area();
        let dim = Style::default().fg(t.dim);
        let y = area.y + 5;
        let big = |f: &mut Frame, x: u16, y: u16, label: &str, val: String, col: Color| {
            f.buffer_mut().set_string(x, y, label, dim);
            crate::clock::big_text(f.buffer_mut(), Rect { x, y: y + 1, width: crate::clock::big_width(&val) + 1, height: 5 }, &val, Style::default().fg(col));
        };
        big(f, x0, y, "wpm", format!("{:.0}", r.wpm), t.accent);
        big(f, x0 + 18, y, "acc", format!("{:.0}", r.acc), t.text);
        let mut info = vec![
            Line::from(vec![Span::styled("raw          ", dim), Span::styled(format!("{:.0}", r.raw), Style::default().fg(t.text))]),
            Line::from(vec![Span::styled("consistency  ", dim), Span::styled(format!("{:.0}%", r.consistency), Style::default().fg(t.text))]),
            Line::from(vec![Span::styled("characters   ", dim), Span::styled(format!("{}", r.correct), Style::default().fg(t.ok)), Span::styled("/", dim),
                Span::styled(format!("{}", r.incorrect), Style::default().fg(t.error)), Span::styled("/", dim), Span::styled(format!("{}", r.extra), Style::default().fg(t.error)), Span::styled("/", dim), Span::styled(format!("{}", r.missed), dim)]),
            Line::styled("             correct / incorrect / extra / missed", dim),
            Line::from(vec![Span::styled("time         ", dim), Span::styled(format!("{:.1}s", r.secs), Style::default().fg(t.text)), Span::styled(format!("   {}", self.mode.label()), dim)]),
        ];
        if self.new_best { info.push(Line::styled("new personal best!", Style::default().fg(t.ok).add_modifier(Modifier::BOLD))); }
        else if let Some(b) = self.best { info.push(Line::styled(format!("personal best {:.0} wpm", b), dim)); }
        f.render_widget(Paragraph::new(info), Rect { x: x0 + 38, y, width: w.saturating_sub(38), height: 7 });
        // wpm per second graph (bars) with errors marked under it
        let gy = y + 8;
        let gh = 8u16.min(area.height.saturating_sub(gy + 4));
        if gh >= 3 {
            let max = r.per_sec.iter().map(|p| p.0).fold(r.wpm.max(10.0), f64::max);
            let n = r.per_sec.len().max(1);
            let colw = ((w as usize) / n).clamp(1, 4) as u16;
            f.buffer_mut().set_string(x0, gy - 1, format!("wpm per second  (max {:.0})", max), dim);
            const BARS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
            for (i, (v, errs)) in r.per_sec.iter().enumerate() {
                let x = x0 + i as u16 * colw;
                if x + colw > x0 + w { break; }
                let eighths = ((v / max) * (gh as f64 * 8.0)).round() as u16;
                for row in 0..gh {
                    let fill = eighths.saturating_sub((gh - 1 - row) * 8).min(8) as usize;
                    let s = BARS[fill].repeat(colw.saturating_sub(if colw > 1 { 1 } else { 0 }) as usize);
                    f.buffer_mut().set_string(x, gy + row, s, Style::default().fg(t.accent));
                }
                if *errs > 0 { f.buffer_mut().set_string(x, gy + gh, "×", Style::default().fg(t.error)); }
            }
        }
        f.buffer_mut().set_string(x0, area.y + area.height.saturating_sub(3), &self.history_note, dim);
        f.render_widget(Paragraph::new(crate::ui::hint_line(&[("Tab / Enter", "next test"), ("← →", "mode"), ("Esc", "back")], area.width, t)),
            Rect { x: area.x, y: area.y + area.height.saturating_sub(1), width: area.width, height: 1 });
    }
}

/// Full-screen typing test until Esc.
pub fn run(terminal: &mut ratatui::DefaultTerminal) -> std::io::Result<()> {
    let mut cfg = Config::load();
    let saved = cfg.ini.get("typing", "mode").map(|s| s.to_string()).unwrap_or_default();
    let mut mi = MODES.iter().position(|m| m.key() == saved).unwrap_or(1);
    let mut test = Test::new(MODES[mi]);
    let _ = terminal.clear();
    loop {
        terminal.draw(|f| test.draw(f, &cfg.theme, &cfg))?;
        test.tick();
        let wait = if test.start.is_some() && test.result.is_none() { 100 } else { 500 };
        if !event::poll(Duration::from_millis(wait))? { continue; }
        if let Event::Key(k) = event::read()? {
            if k.kind == KeyEventKind::Release { continue; }
            match k.code {
                KeyCode::Esc => return Ok(()),
                KeyCode::Tab | KeyCode::BackTab => { test = Test::new(MODES[mi]); }
                KeyCode::Enter if test.result.is_some() => { test = Test::new(MODES[mi]); }
                KeyCode::Left | KeyCode::Right if test.start.is_none() || test.result.is_some() => {
                    mi = if k.code == KeyCode::Left { (mi + MODES.len() - 1) % MODES.len() } else { (mi + 1) % MODES.len() };
                    let _ = cfg.set_value("typing", "mode", Some(&MODES[mi].key()));
                    cfg.reload();
                    test = Test::new(MODES[mi]);
                }
                KeyCode::F(9) => { cfg.cycle_theme(1); let _ = terminal.clear(); }
                _ => { test.key(k); }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn press(t: &mut Test, s: &str) { for c in s.chars() { t.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)); } }
    #[test]
    fn scoring() {
        let mut t = Test::new(Mode::Words(3));
        t.words = vec!["the".into(), "word".into(), "end".into()];
        press(&mut t, "teh");                 // 2 wrong chars
        t.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        t.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        press(&mut t, "he word en");
        assert!(t.result.is_none());
        press(&mut t, "d");                   // last word right: finished without a space
        let r = t.result.as_ref().expect("finished");
        assert_eq!((r.correct, r.incorrect, r.extra, r.missed), (10, 0, 0, 0));
        assert!(r.acc < 100.0 && r.acc > 80.0, "{}", r.acc);
        // going back into a correct word is not allowed, into a wrong one it is
        let mut t = Test::new(Mode::Words(3));
        t.words = vec!["a".into(), "b".into(), "c".into()];
        press(&mut t, "a x ");
        t.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(t.cur, 1);
        t.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(t.cur, 1);
        assert!(t.typed[1].is_empty());
        t.key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
        assert_eq!(t.cur, 1, "word 'a' was right: cannot go back into it");
    }
}
