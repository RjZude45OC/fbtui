//! Local AI chat (Alt+A, or "Local AI" in Spotlight): talk to a model running on this PC or the network
//! (Ollama, LM Studio, llama.cpp server, anything OpenAI-compatible over plain http).
//! The model gets `ai-skill.md` (what the app is and which actions it can run) plus live context:
//! the time, today's tasks, upcoming tasks, days off and the latest work-log lines.
//! Settings: [ai] url = http://localhost:11434   model = qwen2.5:7b   context = 8192

use crate::config::{Config, Theme};
use crate::tasks::{self, Store};
use chrono::{Datelike, Local, NaiveDateTime};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Duration;

pub const DEFAULT_SKILL: &str = include_str!("../res/ai-skill.md");

pub fn skill_path() -> std::path::PathBuf { tasks::default_path().with_file_name("ai-skill.md") }

/// The skill file, created with the default text the first time.
pub fn skill_text() -> String {
    let p = skill_path();
    match std::fs::read_to_string(&p) {
        Ok(s) if !s.trim().is_empty() => s,
        _ => { let _ = std::fs::write(&p, DEFAULT_SKILL); DEFAULT_SKILL.to_string() }
    }
}

/// Is Ollama installed here (or a model server set in [ai] url)? Without it the Local AI tool and AI fill are greyed out.
pub fn available(cfg: &Config) -> bool {
    if cfg.ini.get("ai", "url").is_some() { return true; }
    static FOUND: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FOUND.get_or_init(ollama_on_disk)
}

fn ollama_on_disk() -> bool {
    let exe = if cfg!(windows) { "ollama.exe" } else { "ollama" };
    let mut dirs: Vec<std::path::PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(l) = std::env::var_os("LOCALAPPDATA") { dirs.push(std::path::PathBuf::from(l).join("Programs").join("Ollama")); }
    if let Some(p) = std::env::var_os("ProgramFiles") { dirs.push(std::path::PathBuf::from(p).join("Ollama")); }
    dirs.iter().any(|d| d.join(exe).is_file())
}

pub const NOT_INSTALLED: &str = "Ollama is not installed - get it from ollama.com, then: ollama pull qwen2.5:3b";

/// AI fill for the task form: turn "call the bank tomorrow at 10" into title / date / time / repeat.
pub fn fill_task(sentence: &str) -> Result<Value, String> {
    let cfg = Config::load();
    let mut server = Server::from(&cfg);
    if server.model.is_empty() { server.model = server.models()?.into_iter().next().ok_or("the model server has no models - e.g. ollama pull qwen2.5:3b")?; }
    let now = now();
    let sys = format!("You fill in a to-do form. Now is {}. Reply with JSON only, no other text:\n\
        {{\"title\": short task title without the date and time words, \"date\": \"\" or YYYY-MM-DD, \"time\": \"\" or HH:MM (24 h), \
        \"repeat\": \"\" or a repeat rule such as every day, mon, mon, wed, fri, mon-fri, weekends, 15th}}\n\
        Use \"repeat\" (and no date) only for tasks that come back, like \"every monday\". Keep the user's language for the title.\n\
        Example: \"llamar al banco mañana a las 10\" -> {{\"title\":\"Llamar al banco\",\"date\":\"{}\",\"time\":\"10:00\",\"repeat\":\"\"}}",
        now.format("%A %Y-%m-%d %H:%M"), (now.date() + chrono::Duration::days(1)).format("%Y-%m-%d"));
    let msgs = json!([{ "role": "system", "content": sys }, { "role": "user", "content": sentence }]);
    let (url, body) = if server.openai {
        (format!("{}/chat/completions", server.base), json!({ "model": server.model, "messages": msgs, "stream": false, "temperature": 0 }))
    } else {
        (format!("{}/api/chat", server.base), json!({ "model": server.model, "messages": msgs, "stream": false, "format": "json", "options": { "temperature": 0 } }))
    };
    let mut r = request(&url, "POST", Some(&body.to_string()), Duration::from_secs(120))?;
    let mut text = String::new();
    r.read_to_string(&mut text).map_err(|e| e.to_string())?;
    let v: Value = serde_json::from_str(&text).map_err(|_| "the model server sent something unexpected".to_string())?;
    let content = if server.openai { v["choices"][0]["message"]["content"].as_str() } else { v["message"]["content"].as_str() }.unwrap_or("").to_string();
    let content = strip_think(&content);
    let (a, b) = (content.find('{'), content.rfind('}'));
    let json_part = match (a, b) { (Some(a), Some(b)) if b > a => &content[a..=b], _ => return Err("the model did not answer with the fields".into()) };
    serde_json::from_str(json_part).map_err(|_| "the model's answer could not be read".to_string())
}

// ---------------------------------------------------------------- tiny http client (plain http, enough for a local server)

struct Url { host: String, port: u16, path: String }

fn parse_url(u: &str) -> Result<Url, String> {
    let rest = u.trim().strip_prefix("http://").ok_or_else(|| {
        if u.trim().starts_with("https://") { "only http:// addresses are supported (a local server)".to_string() } else { format!("bad address: {u}") }
    })?;
    let (hp, path) = match rest.find('/') { Some(i) => (&rest[..i], &rest[i..]), None => (rest, "") };
    let (host, port) = match hp.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().map_err(|_| format!("bad port in {u}"))?),
        None => (hp.to_string(), 80),
    };
    Ok(Url { host, port, path: path.trim_end_matches('/').to_string() })
}

/// Body reader that undoes "Transfer-Encoding: chunked".
struct Chunked<R: BufRead> { inner: R, left: usize, done: bool }

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.done { return Ok(0); }
        if self.left == 0 {
            let mut line = String::new();
            loop {
                line.clear();
                if self.inner.read_line(&mut line)? == 0 { self.done = true; return Ok(0); }
                if !line.trim().is_empty() { break; }
            }
            let size = usize::from_str_radix(line.trim().split(';').next().unwrap_or("0"), 16).unwrap_or(0);
            if size == 0 { self.done = true; return Ok(0); }
            self.left = size;
        }
        let n = buf.len().min(self.left);
        let got = self.inner.read(&mut buf[..n])?;
        if got == 0 { self.done = true; }
        self.left -= got;
        Ok(got)
    }
}

/// Send a request and return a line reader over the body (status must be 2xx).
fn request(url: &str, method: &str, body: Option<&str>, timeout: Duration) -> Result<Box<dyn BufRead + Send>, String> {
    let u = parse_url(url)?;
    let addr = format!("{}:{}", u.host, u.port);
    let addrs: Vec<_> = std::net::ToSocketAddrs::to_socket_addrs(&addr).map_err(|e| format!("{addr}: {e}"))?.collect();
    let sock = addrs.iter().find_map(|a| TcpStream::connect_timeout(a, Duration::from_secs(3)).ok())
        .ok_or_else(|| format!("no answer from {addr} - is the model server running?"))?;
    sock.set_read_timeout(Some(timeout)).ok();
    let mut w = sock.try_clone().map_err(|e| e.to_string())?;
    let path = if u.path.is_empty() { "/".to_string() } else { u.path.clone() };
    let body = body.unwrap_or("");
    let head = format!("{method} {path} HTTP/1.1\r\nHost: {}\r\nUser-Agent: file-browser\r\nAccept: */*\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n", u.host, body.len());
    w.write_all(head.as_bytes()).and_then(|_| w.write_all(body.as_bytes())).map_err(|e| e.to_string())?;
    let mut r = BufReader::new(sock);
    let mut status = String::new();
    r.read_line(&mut status).map_err(|e| e.to_string())?;
    let code: u16 = status.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let mut chunked = false;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).map_err(|e| e.to_string())? == 0 { break; }
        let h = h.trim();
        if h.is_empty() { break; }
        let low = h.to_ascii_lowercase();
        if low.starts_with("transfer-encoding:") && low.contains("chunked") { chunked = true; }
    }
    let mut reader: Box<dyn BufRead + Send> = if chunked { Box::new(BufReader::new(Chunked { inner: r, left: 0, done: false })) } else { Box::new(r) };
    if !(200..300).contains(&code) {
        let mut text = String::new();
        let _ = reader.read_to_string(&mut text);
        let msg = serde_json::from_str::<Value>(&text).ok()
            .and_then(|v| v.get("error").map(|e| e.get("message").and_then(|m| m.as_str()).or(e.as_str()).unwrap_or("").to_string()))
            .filter(|m| !m.is_empty()).unwrap_or_else(|| text.chars().take(200).collect());
        return Err(format!("{} {}", if code == 0 { "bad answer".to_string() } else { format!("HTTP {code}") }, msg.trim()));
    }
    Ok(reader)
}

// ---------------------------------------------------------------- the model server

#[derive(Clone)]
struct Server { base: String, openai: bool, model: String, ctx: u32 }

impl Server {
    fn from(cfg: &Config) -> Server {
        let url = cfg.ini.get("ai", "url").unwrap_or("http://localhost:11434").trim().trim_end_matches('/').to_string();
        let openai = cfg.ini.get("ai", "api").map(|a| a.eq_ignore_ascii_case("openai")).unwrap_or(url.ends_with("/v1"));
        let model = cfg.ini.get("ai", "model").unwrap_or("").trim().to_string();
        let ctx = cfg.ini.get("ai", "context").and_then(|c| c.trim().parse().ok()).unwrap_or(8192);
        Server { base: url, openai, model, ctx }
    }

    fn models(&self) -> Result<Vec<String>, String> {
        let url = if self.openai { format!("{}/models", self.base) } else { format!("{}/api/tags", self.base) };
        let mut r = request(&url, "GET", None, Duration::from_secs(10))?;
        let mut text = String::new();
        r.read_to_string(&mut text).map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("not a model server answer: {e}"))?;
        let list = if self.openai { v.get("data") } else { v.get("models") };
        let key = if self.openai { "id" } else { "name" };
        Ok(list.and_then(|l| l.as_array()).map(|a| a.iter().filter_map(|m| m.get(key).and_then(|n| n.as_str()).map(|s| s.to_string())).collect()).unwrap_or_default())
    }

    /// Stream the answer, sending each piece of text as it arrives.
    fn chat(&self, messages: Vec<Value>, tx: &Sender<Ev>) -> Result<(), String> {
        let (url, body) = if self.openai {
            (format!("{}/chat/completions", self.base), json!({ "model": self.model, "messages": messages, "stream": true }))
        } else {
            (format!("{}/api/chat", self.base), json!({ "model": self.model, "messages": messages, "stream": true, "options": { "num_ctx": self.ctx } }))
        };
        let mut r = request(&url, "POST", Some(&body.to_string()), Duration::from_secs(300))?;
        let mut line = String::new();
        loop {
            line.clear();
            if r.read_line(&mut line).map_err(|e| format!("connection lost: {e}"))? == 0 { break; }
            let l = line.trim();
            let l = l.strip_prefix("data:").map(|s| s.trim()).unwrap_or(l);
            if l.is_empty() { continue; }
            if l == "[DONE]" { break; }
            let Ok(v) = serde_json::from_str::<Value>(l) else { continue };
            if let Some(e) = v.get("error") { return Err(e.get("message").and_then(|m| m.as_str()).or(e.as_str()).unwrap_or("error").to_string()); }
            let (piece, think) = if self.openai {
                let d = &v["choices"][0]["delta"];
                (d.get("content").and_then(|c| c.as_str()).unwrap_or(""), d.get("reasoning_content").or(d.get("reasoning")).and_then(|c| c.as_str()).unwrap_or(""))
            } else {
                (v["message"].get("content").and_then(|c| c.as_str()).unwrap_or(""), v["message"].get("thinking").and_then(|c| c.as_str()).unwrap_or(""))
            };
            if !think.is_empty() { let _ = tx.send(Ev::Thinking); }
            if !piece.is_empty() && tx.send(Ev::Text(piece.to_string())).is_err() { return Ok(()); }   // chat closed
            if v.get("done").and_then(|d| d.as_bool()).unwrap_or(false) { break; }
        }
        Ok(())
    }
}

enum Ev { Models(Result<Vec<String>, String>), Text(String), Thinking, Done, Error(String) }

// ---------------------------------------------------------------- live context for the model

fn now() -> NaiveDateTime { Local::now().naive_local() }

pub fn context(now: NaiveDateTime) -> String {
    let today = now.date();
    let mut s = format!("## Live context\n\nNow: {} (week {}).\n", now.format("%A %Y-%m-%d %H:%M"), today.iso_week().week());
    let store = Store::open(tasks::default_path());
    let mut due_today = Vec::new();
    let mut later = Vec::new();
    let mut nodate = Vec::new();
    let mut late = Vec::new();
    for t in &store.tasks {
        let when = tasks::fmt_when(t, today);
        if t.daily {
            if t.applies(today) {
                let st = if t.is_done(today) { "done today" } else if t.overdue(now) { "LATE" } else { "to do" };
                due_today.push(format!("- [repeating, {}] {} — {st}", when.trim(), t.title));
            } else if let Some(d) = t.next_on(today) {
                if (d - today).num_days() <= 7 { later.push((d, format!("- {} [repeating {}] {}", d.format("%a %Y-%m-%d"), tasks::fmt_repeat(t), t.title))); }
            }
            continue;
        }
        if t.done { continue; }
        match t.date {
            Some(_) if t.overdue(now) => late.push(format!("- {} {} — LATE ({})", when.trim(), t.title, tasks::fmt_relative(t, now))),
            Some(d) if d == today => due_today.push(format!("- {} {} — to do", t.time.map(|x| x.format("%H:%M").to_string()).unwrap_or("any time".into()), t.title)),
            Some(d) if (d - today).num_days() <= 14 => later.push((d, format!("- {} {} {}", d.format("%a %Y-%m-%d"), t.time.map(|x| x.format("%H:%M").to_string()).unwrap_or_default(), t.title))),
            Some(_) => {}
            None => nodate.push(format!("- {} (no date)", t.title)),
        }
    }
    let done_today: Vec<String> = store.tasks.iter().filter(|t| !t.daily && t.done && t.done_at.map(|d| d.date() == today).unwrap_or(false)).map(|t| format!("- {}", t.title)).collect();
    later.sort_by_key(|x| x.0);
    let sec = |s: &mut String, title: &str, v: &[String]| { s.push_str(&format!("\n{title}:\n")); if v.is_empty() { s.push_str("- none\n"); } else { for l in v { s.push_str(l); s.push('\n'); } } };
    sec(&mut s, "Late", &late);
    sec(&mut s, "Today", &due_today);
    sec(&mut s, "Finished today", &done_today);
    sec(&mut s, "Next two weeks", &later.into_iter().map(|x| x.1).collect::<Vec<_>>());
    sec(&mut s, "Without a date", &nodate);
    let mut off = Vec::new();
    if let Some(n) = crate::calendar::day_off(today) { off.push(format!("- today is a day off: {n}")); }
    let (list, _) = crate::calendar::load();
    let mut ev: Vec<_> = list.iter().filter(|e| e.to >= today && (e.from - today).num_days() <= 60).collect();
    ev.sort_by_key(|e| e.from);
    for e in ev.iter().take(8) {
        let span = if e.to != e.from { format!("{} to {}", e.from.format("%a %Y-%m-%d"), e.to.format("%a %Y-%m-%d")) } else { e.from.format("%a %Y-%m-%d").to_string() };
        off.push(format!("- {span}: {} ({})", e.title, format!("{:?}", e.kind).to_lowercase()));
    }
    sec(&mut s, "Calendar (next 60 days)", &off);
    let cfg = Config::load();
    let wd = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"][today.weekday().num_days_from_monday() as usize];
    if let Some(h) = cfg.ini.get("tasks", &format!("countdown_{wd}")).or(cfg.ini.get("tasks", "countdown")) { s.push_str(&format!("\nWork hours today: {h}\n")); }
    let log = crate::worklog::read();
    let tail: Vec<String> = log.iter().filter(|l| !l.trim().is_empty()).rev().take(12).cloned().collect::<Vec<_>>().into_iter().rev().collect();
    if !tail.is_empty() { s.push_str("\nLatest work log lines:\n"); for l in tail { s.push_str(&l); s.push('\n'); } }
    s
}

// ---------------------------------------------------------------- actions written by the model

pub enum After { Stay, Open(crate::tools::Tool) }

/// Run one <action>{...}</action>. Returns the line shown in the chat.
fn run_action(json_text: &str, after: &mut After) -> (bool, String) {
    let v: Value = match serde_json::from_str(json_text.trim()) { Ok(v) => v, Err(_) => return (false, format!("could not read the action {json_text}")) };
    let g = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|s| s.trim().to_string()).unwrap_or_default();
    let now = now();
    match g("do").as_str() {
        "add_task" => {
            let title = g("title");
            if title.is_empty() { return (false, "add_task without a title".into()); }
            let repeat = g("repeat");
            let daily = !repeat.is_empty();
            let (date, time) = match tasks::resolve(&g("date"), &g("time"), daily, now) { Ok(x) => x, Err(e) => return (false, format!("add_task {title}: {e}")) };
            let mut t = tasks::new_task(&title, daily, date, time);
            t.link = tasks::clean_link(&g("link"));
            if daily {
                match tasks::parse_repeat(&repeat) { Ok((days, md)) => { t.days = days; t.month_day = md; } Err(_) => return (false, format!("add_task {title}: repeat '{repeat}' not understood")) }
            }
            let mut store = Store::open(tasks::default_path());
            let when = tasks::fmt_when(&t, now.date());
            store.add(t);
            match store.save() { Ok(()) => (true, format!("Added task: {title}  ·  {}", if when.trim().is_empty() { "no date".into() } else { when.trim().to_string() })), Err(e) => (false, e) }
        }
        "done_task" => {
            let q = g("title").to_lowercase();
            let mut store = Store::open(tasks::default_path());
            let today = now.date();
            let pick = store.tasks.iter().position(|t| t.title.to_lowercase() == q && !t.is_done(today))
                .or_else(|| store.tasks.iter().position(|t| t.title.to_lowercase().contains(&q) && !t.is_done(today) && !q.is_empty()));
            let Some(i) = pick else { return (false, format!("no open task matches '{}'", g("title"))) };
            let t = &mut store.tasks[i];
            if t.daily { t.done_on = Some(today); } else { t.done = true; }
            t.done_at = Some(now);
            let title = t.title.clone();
            match store.save() {
                Ok(()) => { let _ = crate::worklog::append(&crate::worklog::finished(&title, ""), now); (true, format!("Done: {title}  ✓")) }
                Err(e) => (false, e),
            }
        }
        "log" => {
            let text = g("text");
            if text.is_empty() { return (false, "log without text".into()); }
            match crate::worklog::append(&text, now) { Ok(_) => (true, format!("Logged: {text}")), Err(e) => (false, e) }
        }
        "add_event" => {
            let title = g("title");
            let kind = crate::calendar::Kind::from(&g("kind").to_lowercase());
            let from = match tasks::parse_date(&g("from"), now.date()) { Ok(Some(d)) => d, _ => return (false, format!("add_event {title}: date '{}' not understood", g("from"))) };
            let to = match tasks::parse_date(&g("to"), now.date()) { Ok(Some(d)) => d.max(from), _ => from };
            let (mut list, next) = crate::calendar::load();
            list.push(crate::calendar::Entry { id: next, title: if title.is_empty() { "Event".into() } else { title.clone() }, kind, from, to });
            match crate::calendar::save(&list, next + 1) {
                Ok(()) => (true, format!("Calendar: {title}  ·  {}{}", from.format("%a %d %b"), if to != from { format!(" – {}", to.format("%a %d %b")) } else { String::new() })),
                Err(e) => (false, e),
            }
        }
        "open" => {
            let want = g("tool").to_lowercase();
            match crate::tools::ALL.iter().find(|f| f.key == want || f.name.to_lowercase().starts_with(&want)) {
                Some(f) => { *after = After::Open(f.tool); (true, format!("Opening {}", f.name)) }
                None => (false, format!("no tool called '{want}'")),
            }
        }
        other => (false, format!("unknown action '{other}'")),
    }
}

/// Split the model's text into what is shown and the actions in it.
fn split_actions(text: &str) -> (String, Vec<String>) {
    let mut shown = String::new();
    let mut acts = Vec::new();
    let mut rest = text;
    while let Some(i) = rest.find("<action>") {
        shown.push_str(&rest[..i]);
        let after = &rest[i + 8..];
        match after.find("</action>") {
            Some(j) => { acts.push(after[..j].to_string()); rest = &after[j + 9..]; }
            None => { rest = ""; }                 // still arriving
        }
    }
    shown.push_str(rest);
    (strip_think(&shown), acts)
}

/// Drop <think>…</think> blocks (reasoning models); an open one hides the rest while it is being written.
fn strip_think(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("<think>") {
        out.push_str(&rest[..i]);
        match rest[i..].find("</think>") { Some(j) => rest = &rest[i + j + 8..], None => { rest = ""; } }
    }
    out.push_str(rest);
    // collapse the blank lines left behind
    let mut res = String::new();
    let mut blank = 0;
    for l in out.trim().lines() {
        if l.trim().is_empty() { blank += 1; if blank > 1 { continue; } } else { blank = 0; }
        res.push_str(l.trim_end());
        res.push('\n');
    }
    res.trim_end().to_string()
}

// ---------------------------------------------------------------- the chat screen

#[derive(Clone, PartialEq)]
enum Who { You, Ai, Did(bool), Note }

struct Chat {
    server: Server,
    models: Vec<String>,
    msgs: Vec<(Who, String)>,
    history: Vec<Value>,             // what the model sees (without the system prompt)
    input: String,
    cur: usize,
    rx: Option<Receiver<Ev>>,
    answer: String,
    thinking: bool,
    streaming: bool,
    scroll: Option<usize>,           // None = follow the end
    status: String,
    rows: usize,
    total: usize,
}

impl Chat {
    fn system(&self) -> Value { json!({ "role": "system", "content": format!("{}\n\n{}", skill_text(), context(now())) }) }

    fn send(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() || self.rx.is_some() { return; }
        if self.server.model.is_empty() { self.status = "No model yet - F2 chooses one (or set model = … in [ai] in config.ini)".into(); return; }
        self.input.clear();
        self.cur = 0;
        self.msgs.push((Who::You, text.clone()));
        self.history.push(json!({ "role": "user", "content": text }));
        let mut messages = vec![self.system()];
        // keep the conversation short enough for small context windows
        let keep = self.history.len().saturating_sub(20);
        messages.extend(self.history[keep..].iter().cloned());
        let (tx, rx) = channel();
        let server = self.server.clone();
        std::thread::spawn(move || {
            let r = server.chat(messages, &tx);
            let _ = tx.send(match r { Ok(()) => Ev::Done, Err(e) => Ev::Error(e) });
        });
        self.rx = Some(rx);
        self.answer.clear();
        self.thinking = false;
        self.streaming = true;
        self.scroll = None;
        self.status.clear();
    }

    fn load_models(&mut self) {
        let (tx, rx) = channel();
        let server = self.server.clone();
        std::thread::spawn(move || { let _ = tx.send(Ev::Models(server.models())); });
        self.rx = Some(rx);
        self.status = format!("Connecting to {} …", self.server.base);
    }

    /// Handle what the worker sent. Returns Some when an action asks to open a tool.
    fn poll(&mut self, cfg: &mut Config) -> Option<crate::tools::Tool> {
        let mut open = None;
        let Some(rx) = &self.rx else { return None };
        let mut finished = false;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                Ev::Models(Ok(list)) => {
                    finished = true;
                    self.status.clear();
                    if list.is_empty() { self.status = "The server has no models - e.g. run: ollama pull qwen2.5:7b".into(); }
                    else if self.server.model.is_empty() || !list.iter().any(|m| *m == self.server.model) {
                        let was = self.server.model.clone();
                        self.server.model = list[0].clone();
                        let _ = cfg.set_value("ai", "model", Some(&self.server.model));
                        if !was.is_empty() { self.status = format!("Model '{was}' not found - using {}", self.server.model); }
                    }
                    self.models = list;
                }
                Ev::Models(Err(e)) => { finished = true; self.status = e; }
                Ev::Thinking => self.thinking = true,
                Ev::Text(t) => { self.answer.push_str(&t); }
                Ev::Done | Ev::Error(_) => {
                    finished = true;
                    self.streaming = false;
                    if let Ev::Error(e) = &ev { self.status = e.clone(); }
                    let full = std::mem::take(&mut self.answer);
                    let (shown, acts) = split_actions(&full);
                    if !full.trim().is_empty() { self.history.push(json!({ "role": "assistant", "content": full })); }
                    if !shown.is_empty() { self.msgs.push((Who::Ai, shown)); }
                    for a in acts {
                        let mut after = After::Stay;
                        let (ok, line) = run_action(&a, &mut after);
                        self.msgs.push((Who::Did(ok), line));
                        if let After::Open(t) = after { open = Some(t); }
                    }
                }
            }
        }
        if finished { self.rx = None; }
        open
    }

    fn draw(&mut self, f: &mut Frame, t: &Theme) {
        let area = f.area();
        let base = match t.background { Some(bg) => Style::default().bg(bg).fg(t.text), None => Style::default().fg(t.text) };
        f.render_widget(Block::default().style(base), area);
        let dim = Style::default().fg(t.dim);
        let acc = Style::default().fg(t.accent).add_modifier(Modifier::BOLD);
        let model = if self.server.model.is_empty() { "no model".to_string() } else { self.server.model.clone() };
        f.render_widget(Paragraph::new(vec![
            Line::from(vec![Span::styled("▌ ", acc), Span::styled("LOCAL AI", Style::default().fg(t.text).add_modifier(Modifier::BOLD)),
                Span::styled(format!("   {model}"), Style::default().fg(t.accent)), Span::styled(format!("  ·  {}", self.server.base), dim)]),
            Line::from(vec![Span::styled("▌ ", acc), Span::styled("knows your tasks, calendar and work log  ·  can add tasks, tick them off, log work and add days off", dim)]),
        ]), Rect { x: area.x + 1, y: area.y + 1, width: area.width.saturating_sub(2), height: 2 });
        // conversation
        let w = area.width.saturating_sub(6).max(20) as usize;
        let mut lines: Vec<Line> = Vec::new();
        let mut show = self.msgs.clone();
        if self.streaming {
            let (s, _) = split_actions(&self.answer);
            let s = if s.is_empty() { if self.thinking || self.answer.contains("<think>") { "thinking…".to_string() } else { "…".to_string() } } else { format!("{s} ▍") };
            show.push((Who::Note, s));
        }
        if show.is_empty() {
            for l in ["Ask anything, for example:", "", "  what do I have today?", "  add a task to call the bank tomorrow at 10",
                "  remind me every monday at 9 to send the weekly report", "  log that I fixed the printer on the 2nd floor",
                "  I am on vacation from 22 to 31 December", "  how do I bookmark a folder?"] {
                lines.push(Line::styled(format!("  {l}"), dim));
            }
        }
        for (who, text) in &show {
            let (label, st) = match who {
                Who::You => ("you ›", Style::default().fg(t.accent).add_modifier(Modifier::BOLD)),
                Who::Ai => ("ai  ›", Style::default().fg(t.ok).add_modifier(Modifier::BOLD)),
                Who::Note => ("ai  ›", Style::default().fg(t.dim).add_modifier(Modifier::BOLD)),
                Who::Did(true) => ("  ✔  ", Style::default().fg(t.ok)),
                Who::Did(false) => ("  ✕  ", Style::default().fg(t.error)),
            };
            let body = match who { Who::You => Style::default().fg(t.text).add_modifier(Modifier::BOLD), Who::Note => Style::default().fg(t.dim), Who::Did(true) => Style::default().fg(t.ok), Who::Did(false) => Style::default().fg(t.error), _ => Style::default().fg(t.text) };
            let mut first = true;
            for para in text.lines() {
                for piece in crate::ui::wrap_text(para, w.saturating_sub(6)) {
                    lines.push(Line::from(vec![Span::styled(if first { format!(" {label} ") } else { "       ".into() }, st), Span::styled(piece, body)]));
                    first = false;
                }
                if para.is_empty() { lines.push(Line::raw("")); }
            }
            if first { lines.push(Line::from(Span::styled(format!(" {label} "), st))); }
            if !matches!(who, Who::Did(_)) { lines.push(Line::raw("")); }
        }
        let body = Rect { x: area.x + 1, y: area.y + 4, width: area.width.saturating_sub(2), height: area.height.saturating_sub(9) };
        self.rows = body.height as usize;
        self.total = lines.len();
        let max = self.total.saturating_sub(self.rows);
        let top = self.scroll.map(|s| s.min(max)).unwrap_or(max);
        f.render_widget(Paragraph::new(lines.into_iter().skip(top).take(self.rows).collect::<Vec<_>>()), body);
        // input box
        let iy = area.y + area.height.saturating_sub(4);
        f.render_widget(Paragraph::new(Span::styled("─".repeat(area.width.saturating_sub(2) as usize), dim)), Rect { x: area.x + 1, y: iy, width: area.width.saturating_sub(2), height: 1 });
        let room = area.width.saturating_sub(8) as usize;
        let chars: Vec<char> = self.input.chars().collect();
        let start = self.cur.saturating_sub(room.saturating_sub(1));
        let shown: String = chars[start.min(chars.len())..].iter().take(room).collect();
        let prompt = if self.input.is_empty() { Span::styled(if self.streaming { "answering… Esc stops" } else { "type a message, Enter sends" }, dim) } else { Span::styled(shown, Style::default().fg(t.text)) };
        f.render_widget(Paragraph::new(Line::from(vec![Span::styled(" › ", acc), prompt])), Rect { x: area.x + 1, y: iy + 1, width: area.width.saturating_sub(2), height: 1 });
        if self.rx.is_none() || !self.input.is_empty() { f.set_cursor_position((area.x + 4 + (self.cur - start) as u16, iy + 1)); }
        let st = if self.status.is_empty() { Span::raw("") } else { Span::styled(format!(" {}", self.status), Style::default().fg(t.error)) };
        f.render_widget(Paragraph::new(st), Rect { x: area.x + 1, y: iy + 2, width: area.width.saturating_sub(2), height: 1 });
        let foot = crate::ui::hint_line(&[("Enter", "send"), ("Esc", "stop / back"), ("F2", "model"), ("Ctrl+L", "new chat"), ("Ctrl+O", "edit the skill"), ("PgUp PgDn", "scroll")], area.width, t);
        f.render_widget(Paragraph::new(foot), Rect { x: area.x, y: area.y + area.height.saturating_sub(1), width: area.width, height: 1 });
    }
}

/// Returns a tool to open when the model asked for one.
pub fn run(terminal: &mut ratatui::DefaultTerminal) -> std::io::Result<Option<crate::tools::Tool>> {
    let mut cfg = Config::load();
    let _ = skill_text();
    let mut c = Chat { server: Server::from(&cfg), models: Vec::new(), msgs: Vec::new(), history: Vec::new(), input: String::new(), cur: 0,
        rx: None, answer: String::new(), thinking: false, streaming: false, scroll: None, status: String::new(), rows: 10, total: 0 };
    c.load_models();
    let _ = terminal.clear();
    loop {
        if let Some(tool) = c.poll(&mut cfg) { let _ = terminal.clear(); return Ok(Some(tool)); }
        let t = cfg.theme.clone();
        terminal.draw(|f| c.draw(f, &t))?;
        if !event::poll(Duration::from_millis(if c.rx.is_some() { 50 } else { 500 }))? { continue; }
        match event::read()? {
            Event::Mouse(m) => match m.kind {
                MouseEventKind::ScrollUp => { let top = c.scroll.unwrap_or(c.total.saturating_sub(c.rows)); c.scroll = Some(top.saturating_sub(3)); }
                MouseEventKind::ScrollDown => { let top = c.scroll.unwrap_or(usize::MAX) + 3; c.scroll = if top >= c.total.saturating_sub(c.rows) { None } else { Some(top) }; }
                _ => {}
            },
            Event::Paste(s) => { for ch in s.chars().filter(|c| *c != '\r') { let ch = if ch == '\n' { ' ' } else { ch }; let b = byte(&c.input, c.cur); c.input.insert(b, ch); c.cur += 1; } }
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                let ctrl = k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT);
                let n = c.input.chars().count();
                match k.code {
                    KeyCode::Esc => {
                        if c.streaming {
                            c.streaming = false;
                            // stop: drop the receiver, the worker ends at its next piece
                            c.rx = None;
                            let partial = std::mem::take(&mut c.answer);
                            let (shown, _) = split_actions(&partial);
                            if !shown.is_empty() { c.msgs.push((Who::Ai, format!("{shown} (stopped)"))); c.history.push(json!({ "role": "assistant", "content": partial })); }
                        } else { break; }
                    }
                    KeyCode::Enter => c.send(),
                    KeyCode::F(2) => {
                        if c.models.is_empty() { if c.rx.is_none() { c.load_models(); } }
                        else {
                            let i = c.models.iter().position(|m| *m == c.server.model).map(|i| (i + 1) % c.models.len()).unwrap_or(0);
                            c.server.model = c.models[i].clone();
                            let _ = cfg.set_value("ai", "model", Some(&c.server.model));
                            c.status = format!("Model: {}  ({} of {})", c.server.model, i + 1, c.models.len());
                        }
                    }
                    KeyCode::Char('l') | KeyCode::Char('L') if ctrl => { if c.rx.is_none() { c.msgs.clear(); c.history.clear(); c.scroll = None; c.status = "New chat".into(); } }
                    KeyCode::Char('o') | KeyCode::Char('O') if ctrl => {
                        if let Some(m) = crate::ui::edit_file(terminal, &skill_path(), false)? { c.status = m; }
                        let _ = terminal.clear();
                    }
                    KeyCode::Char('u') | KeyCode::Char('U') if ctrl => { c.input.clear(); c.cur = 0; }
                    KeyCode::PageUp => { let top = c.scroll.unwrap_or(c.total.saturating_sub(c.rows)); c.scroll = Some(top.saturating_sub(c.rows.max(2) - 1)); }
                    KeyCode::PageDown => { let top = c.scroll.unwrap_or(usize::MAX).saturating_add(c.rows.max(2) - 1); c.scroll = if top >= c.total.saturating_sub(c.rows) { None } else { Some(top) }; }
                    KeyCode::Left => c.cur = c.cur.saturating_sub(1),
                    KeyCode::Right => c.cur = (c.cur + 1).min(n),
                    KeyCode::Home => c.cur = 0,
                    KeyCode::End => c.cur = n,
                    KeyCode::Up | KeyCode::Down if c.input.is_empty() => {
                        // recall the last message
                        if let Some((_, last)) = c.msgs.iter().rev().find(|m| m.0 == Who::You) { c.input = last.clone(); c.cur = c.input.chars().count(); }
                    }
                    KeyCode::Backspace if c.cur > 0 => { let b = byte(&c.input, c.cur - 1); c.input.remove(b); c.cur -= 1; }
                    KeyCode::Delete if c.cur < n => { let b = byte(&c.input, c.cur); c.input.remove(b); }
                    KeyCode::Char(ch) if !ctrl => { let b = byte(&c.input, c.cur); c.input.insert(b, ch); c.cur += 1; }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    let _ = terminal.clear();
    Ok(None)
}

fn byte(s: &str, ci: usize) -> usize { s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len()) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actions_and_think() {
        let (s, a) = split_actions("<think>hmm</think>Sure.\n<action>{\"do\":\"log\",\"text\":\"x\"}</action>\nDone");
        assert_eq!(s, "Sure.\n\nDone");
        assert_eq!(a, vec!["{\"do\":\"log\",\"text\":\"x\"}".to_string()]);
        let (s, a) = split_actions("Adding <action>{\"do\":");
        assert_eq!(s, "Adding");
        assert!(a.is_empty());
        let u = parse_url("http://localhost:11434/").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.path.as_str()), ("localhost", 11434, ""));
        let u = parse_url("http://192.168.1.5:1234/v1").unwrap();
        assert_eq!((u.port, u.path.as_str()), (1234, "/v1"));
    }
}
