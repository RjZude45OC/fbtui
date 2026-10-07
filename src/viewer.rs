//! Full-screen viewer (→ on a picture, PDF, video or audio file):
//! PDF pages, zoom and pan, video seek + playback in the terminal, animated GIF / WebP / APNG.
//! Pictures are prepared on a background thread so the keys stay instant.

use crate::media::{self, Kind};
use image::DynamicImage;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Size;
use ratatui_image::picker::{Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::Resize;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const ZOOMS: &[f32] = &[1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0];

#[derive(Clone)]
struct Req {
    key: String,
    path: PathBuf,
    kind: Kind,
    page: u32,         // PDF page / animation frame
    pos: f64,          // video position (s)
    px: (u32, u32),    // target area in pixels
    cells: (u16, u16),
    zoom: f32,
    center: (f32, f32),
    picker: Picker,
    wave_rgb: (u8, u8, u8),
}

enum Msg {
    Image { key: String, proto: Protocol, src: (u32, u32) },
    Error { key: String, err: String },
    Frames(usize),                                 // animation: number of frames and their delays
    Delays(Vec<Duration>),
}

pub enum Action { None, Close, Prev, Next, Open, PlayExternal }

pub struct Viewer {
    pub path: PathBuf,
    pub kind: Kind,
    pub info: Vec<String>,
    pub pages: u32,
    pub page: u32,
    pub duration: f64,
    pub pos: f64,
    pub zoom_i: usize,
    pub center: (f32, f32),
    pub src_size: (u32, u32),
    pub proto: Option<Protocol>,
    pub shown_key: String,
    pub error: Option<String>,
    pub loading: bool,
    pub playing: bool,
    pub cells: (u16, u16),
    // animation
    pub anim_frames: usize,
    anim_delays: Vec<Duration>,
    next_tick: Instant,
    // video playback
    player: Option<Player>,
    pub play_fps: f64,
    // worker
    tx: Sender<Req>,
    rx: Receiver<Msg>,
    last_req: String,
    picker: Option<Picker>,
    wave_rgb: (u8, u8, u8),
    pub message: String,
}

impl Viewer {
    pub fn open(path: &Path, picker: Option<Picker>, wave_rgb: (u8, u8, u8)) -> Result<Viewer, String> {
        let kind = media::kind_of(path);
        if kind == Kind::None { return Err("No viewer for this file type".into()); }
        let mut v = {
            let (tx, rrx) = channel::<Req>();
            let (mtx, rx) = channel::<Msg>();
            std::thread::spawn(move || worker(rrx, mtx));
            Viewer {
                path: path.to_path_buf(),
                kind, info: Vec::new(), pages: 1, page: 0, duration: 0.0, pos: 0.0, zoom_i: 0, center: (0.5, 0.5), src_size: (0, 0),
                proto: None, shown_key: String::new(), error: None, loading: true, playing: false, cells: (0, 0),
                anim_frames: 0, anim_delays: Vec::new(), next_tick: Instant::now(), player: None, play_fps: 0.0,
                tx, rx, last_req: String::new(), picker, wave_rgb, message: String::new(),
            }
        };
        match kind {
            Kind::Pdf => {
                let i = media::pdf_info(path)?;
                v.pages = i.pages.max(1);
                v.info = i.lines;
            }
            Kind::Video | Kind::Audio => {
                let m = media::probe(path).ok_or("FFmpeg not found (run migration\\get-deps.bat)")?;
                if kind == Kind::Video && !m.has_video { return Err("No video stream in this file".into()); }
                v.duration = m.duration;
                v.pos = if kind == Kind::Video && m.duration > 4.0 { (m.duration * 0.2).floor() } else { 0.0 };
                v.info = m.lines;
            }
            Kind::Picture | Kind::Svg => {
                if let Some(x) = media::exif(path) { v.info = x.lines; }
                v.playing = true;      // animated pictures start playing
            }
            Kind::None => {}
        }
        Ok(v)
    }

    pub fn zoom(&self) -> f32 { ZOOMS[self.zoom_i] }

    fn key(&self) -> String {
        format!("{}|{}|{:.2}|{}|{:.3},{:.3}|{}x{}", self.path.display(), self.page, self.pos, self.zoom_i, self.center.0, self.center.1, self.cells.0, self.cells.1)
    }

    /// Called every loop: send the current request, collect results, drive animation / playback.
    /// Returns true if the screen must be redrawn.
    pub fn update(&mut self) -> bool {
        let mut dirty = false;
        let Some(picker) = self.picker.clone() else { return false };
        if self.cells.0 == 0 { return false; }
        // playback of a video: frames come from the player thread
        if let Some(p) = &mut self.player {
            let mut got = None;
            while let Ok(f) = p.frames.try_recv() { got = Some(f); }
            let none = got.is_none();
            if let Some((pos, proto)) = got { self.pos = pos; self.proto = Some(proto); dirty = true; }
            if p.done.load(Ordering::Relaxed) && none {
                self.player = None; self.playing = false; self.last_req.clear(); dirty = true;
                if self.pos >= self.duration - 1.0 { self.pos = 0.0; }
            }
            return dirty;
        }
        // animation frames
        if self.anim_frames > 1 && self.playing && Instant::now() >= self.next_tick {
            let d = self.anim_delays.get(self.page as usize).copied().unwrap_or(Duration::from_millis(100));
            self.page = (self.page + 1) % self.anim_frames as u32;
            self.next_tick = Instant::now() + d;
        }
        let key = self.key();
        if key != self.last_req {
            self.last_req = key.clone();
            self.loading = true;
            let fs = picker.font_size();
            let px = (self.cells.0 as u32 * fs.width as u32, self.cells.1 as u32 * fs.height as u32);
            let _ = self.tx.send(Req { key, path: self.path.clone(), kind: self.kind, page: self.page, pos: self.pos, px, cells: self.cells,
                zoom: self.zoom(), center: self.center, picker, wave_rgb: self.wave_rgb });
        }
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Image { key, proto, src } => {
                    self.proto = Some(proto); self.src_size = src; self.error = None;
                    if key == self.last_req { self.loading = false; }
                    self.shown_key = key;
                    dirty = true;
                }
                Msg::Error { key, err } => { if key == self.last_req { self.loading = false; self.error = Some(err); self.proto = None; dirty = true; } }
                Msg::Frames(n) => { self.anim_frames = n; if n > 1 { self.pages = n as u32; self.next_tick = Instant::now(); } dirty = true; }
                Msg::Delays(d) => { self.anim_delays = d; }
            }
        }
        dirty
    }

    /// How soon the main loop should wake up again.
    pub fn wake_in(&self) -> Duration {
        if self.player.is_some() { return Duration::from_millis(8); }
        if self.anim_frames > 1 && self.playing { return self.next_tick.saturating_duration_since(Instant::now()).min(Duration::from_millis(30)); }
        if self.loading { return Duration::from_millis(15); }
        Duration::from_millis(30)
    }

    fn stop_player(&mut self) {
        if let Some(p) = self.player.take() { p.stop.store(true, Ordering::Relaxed); }
        self.playing = false;
        self.last_req.clear();
    }

    fn start_player(&mut self) {
        let Some(picker) = self.picker.clone() else { return };
        let Some(m) = media::probe(&self.path) else { return };
        if !m.has_video { return; }
        let fs = picker.font_size();
        let (aw, ah) = (self.cells.0 as f64 * fs.width as f64, self.cells.1 as f64 * fs.height as f64);
        let (vw, vh) = (m.width.max(1) as f64, m.height.max(1) as f64);
        let s = (aw / vw).min(ah / vh).min(1.0);
        let sixel = picker.protocol_type() != ProtocolType::Halfblocks;
        // keep the pictures small enough to encode in time
        let cap = if sixel { 640.0 } else { 320.0 };
        let s = s.min(cap / vw.max(vh) * (vw.max(vh) / vw.max(vh)));
        let w = ((vw * s) as u32 / 2 * 2).max(16);
        let h = ((vh * s) as u32 / 2 * 2).max(16);
        let fps = if sixel { 12.0 } else { 15.0 };
        if self.pos >= self.duration - 0.5 { self.pos = 0.0; }
        match Player::start(&self.path, self.pos, w, h, fps, picker, self.cells) {
            Some(p) => { self.player = Some(p); self.playing = true; self.play_fps = fps; }
            None => self.message = "Cannot start playback (FFmpeg missing?)".into(),
        }
    }

    pub fn on_key(&mut self, k: KeyEvent) -> Action {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        self.message.clear();
        let zoomed = self.zoom_i > 0;
        let pan = 0.15 / self.zoom();
        let step = if self.duration > 600.0 { 30.0 } else if self.duration > 60.0 { 10.0 } else { (self.duration / 10.0).max(1.0) };
        let is_video = self.kind == Kind::Video;
        if self.player.is_some() && !matches!(k.code, KeyCode::Char(' ') | KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q')) {
            // any other key during playback: pause first, then act
            self.stop_player();
        }
        match k.code {
            KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q') => { if self.player.is_some() { self.stop_player(); } else { return Action::Close; } }
            KeyCode::Left if zoomed => self.center.0 -= pan,
            KeyCode::Right if zoomed => self.center.0 += pan,
            KeyCode::Up if zoomed => self.center.1 -= pan,
            KeyCode::Down if zoomed => self.center.1 += pan,
            KeyCode::Left | KeyCode::Up => {
                if self.kind == Kind::Pdf { if self.page > 0 { self.page -= 1 } }
                else if is_video { self.pos = (self.pos - step).max(0.0) }
                else if self.anim_frames > 1 && !self.playing { self.page = (self.page + self.pages - 1) % self.pages }
                else if k.code == KeyCode::Left { return Action::Prev }
            }
            KeyCode::Right | KeyCode::Down => {
                if self.kind == Kind::Pdf { if self.page + 1 < self.pages { self.page += 1 } }
                else if is_video { self.pos = (self.pos + step).min((self.duration - 0.5).max(0.0)) }
                else if self.anim_frames > 1 && !self.playing { self.page = (self.page + 1) % self.pages }
                else if k.code == KeyCode::Right { return Action::Next }
            }
            KeyCode::PageUp => { if self.kind == Kind::Pdf && !ctrl { self.page = self.page.saturating_sub(10) } else { return Action::Prev } }
            KeyCode::PageDown => { if self.kind == Kind::Pdf && !ctrl { self.page = (self.page + 10).min(self.pages - 1) } else { return Action::Next } }
            KeyCode::Home => { self.page = 0; self.pos = 0.0; self.center = (0.5, 0.5); }
            KeyCode::End => { self.page = self.pages.saturating_sub(1); if is_video { self.pos = (self.duration - 1.0).max(0.0); } }
            KeyCode::Char('+') | KeyCode::Char('=') => self.zoom_in(),
            KeyCode::Char('-') | KeyCode::Char('_') => self.zoom_out(),
            KeyCode::Char('0') => { self.zoom_i = 0; self.center = (0.5, 0.5); }
            KeyCode::Char(' ') => {
                if is_video { if self.player.is_some() { self.stop_player() } else { self.start_player() } }
                else if self.anim_frames > 1 { self.playing = !self.playing; self.next_tick = Instant::now(); }
                else if self.kind == Kind::Pdf { if self.page + 1 < self.pages { self.page += 1 } }
            }
            KeyCode::Char('p') | KeyCode::Char('P') => return Action::PlayExternal,
            KeyCode::Enter => return Action::Open,
            KeyCode::Char(c) if c.is_ascii_digit() && is_video => { self.pos = self.duration * (c as u8 - b'0') as f64 / 10.0; }
            _ => {}
        }
        self.center.0 = self.center.0.clamp(0.0, 1.0);
        self.center.1 = self.center.1.clamp(0.0, 1.0);
        Action::None
    }

    pub fn zoom_in(&mut self) { if self.zoom_i + 1 < ZOOMS.len() { self.zoom_i += 1; } }
    pub fn zoom_out(&mut self) { if self.zoom_i > 0 { self.zoom_i -= 1; } if self.zoom_i == 0 { self.center = (0.5, 0.5); } }

    /// One-line position text for the header.
    pub fn position(&self) -> String {
        let mut s = match self.kind {
            Kind::Pdf => format!("page {} of {}", self.page + 1, self.pages),
            Kind::Video | Kind::Audio => format!("{} / {}", media::fmt_dur(self.pos), media::fmt_dur(self.duration)),
            _ if self.anim_frames > 1 => format!("frame {} of {}{}", self.page + 1, self.anim_frames, if self.playing { "  ▶" } else { "  ❚❚" }),
            _ => String::new(),
        };
        if self.player.is_some() { s += &format!("  ▶ playing ({} fps, no sound)", self.play_fps); }
        if self.src_size.0 > 0 && matches!(self.kind, Kind::Picture | Kind::Svg) && self.anim_frames <= 1 { s = format!("{} x {}", self.src_size.0, self.src_size.1); }
        if self.zoom_i > 0 { s += &format!("  ·  zoom {}x", self.zoom()); }
        if self.loading && self.player.is_none() { s += "  ·  loading…"; }
        s
    }

}

impl Drop for Viewer {
    fn drop(&mut self) { if let Some(p) = &self.player { p.stop.store(true, Ordering::Relaxed); } }
}

// ------------------------------------------------------------------ worker

struct SrcCache { key: String, img: Option<DynamicImage> }

fn worker(rx: Receiver<Req>, tx: Sender<Msg>) {
    crate::winapi::com_init();
    let mut cache = SrcCache { key: String::new(), img: None };
    let mut anim: Option<(PathBuf, Vec<DynamicImage>)> = None;
    let mut anim_checked: Option<PathBuf> = None;
    while let Ok(mut r) = rx.recv() {
        while let Ok(newer) = rx.try_recv() { r = newer; }          // only the latest request matters
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // animated pictures: decode all frames once
            if r.kind == Kind::Picture && anim_checked.as_ref() != Some(&r.path) {
                anim_checked = Some(r.path.clone());
                anim = None;
                if let Some(frames) = media::animation_frames(&r.path, 600) {
                    let _ = tx.send(Msg::Delays(frames.iter().map(|f| f.1).collect()));
                    let _ = tx.send(Msg::Frames(frames.len()));
                    anim = Some((r.path.clone(), frames.into_iter().map(|f| f.0).collect()));
                } else { let _ = tx.send(Msg::Frames(1)); }
            }
            let src: Option<DynamicImage> = if let Some((p, frames)) = anim.as_ref().filter(|(p, _)| p == &r.path) {
                let _ = p;
                frames.get(r.page as usize).cloned()
            } else {
                // sources that must be re-rendered for sharpness when zooming carry the zoom in their key
                let (fw, fh) = r.px;
                let z = r.zoom;
                let skey = match r.kind {
                    Kind::Pdf => format!("{}|{}|{}", r.path.display(), r.page, z),
                    Kind::Svg => format!("{}|{}", r.path.display(), z),
                    Kind::Video => format!("{}|{:.2}", r.path.display(), r.pos),
                    _ => format!("{}", r.path.display()),
                } + &format!("|{fw}x{fh}");
                if skey != cache.key {
                    let want = |a: u32| ((a as f32 * z) as u32).min(9000);
                    cache.img = match r.kind {
                        Kind::Pdf => match media::pdf_render(&r.path, r.page, want(fw), want(fh)) { Ok(i) => Some(i), Err(e) => { let _ = tx.send(Msg::Error { key: r.key.clone(), err: e }); return; } },
                        Kind::Svg => media::svg_render(&r.path, want(fw), want(fh)),
                        Kind::Video => media::video_frame(&r.path, r.pos, 3840, 2160),
                        Kind::Audio => {
                            media::cover_art(&r.path, fw, fh).or_else(|| media::waveform(&r.path, fw, fh / 2, r.wave_rgb))
                        }
                        _ => media::load_picture(&r.path, fw.max(fh) * 2).map(|x| x.0),
                    };
                    cache.key = skey;
                }
                cache.img.clone()
            };
            let Some(src) = src else { let _ = tx.send(Msg::Error { key: r.key.clone(), err: "Cannot decode this file".into() }); return; };
            let (sw, sh) = (src.width(), src.height());
            // PDF / SVG sources are rendered at zoom size already, so fit*zoom is ~1 for them
            let (aw, ah) = (r.px.0 as f32, r.px.1 as f32);
            let fit = (aw / sw as f32).min(ah / sh as f32);
            let s = fit.min(8.0) * r.zoom;
            let vw = (aw / s).min(sw as f32).max(1.0);
            let vh = (ah / s).min(sh as f32).max(1.0);
            let cx = (r.center.0 * sw as f32 - vw / 2.0).clamp(0.0, sw as f32 - vw);
            let cy = (r.center.1 * sh as f32 - vh / 2.0).clamp(0.0, sh as f32 - vh);
            let crop = if vw < sw as f32 - 0.5 || vh < sh as f32 - 0.5 { src.crop_imm(cx as u32, cy as u32, vw as u32, vh as u32) } else { src };
            let (tw, th) = (((crop.width() as f32 * s) as u32).max(1), ((crop.height() as f32 * s) as u32).max(1));
            let out = if s > 3.0 && crop.width() < 200 {
                crop.resize_exact(tw, th, image::imageops::FilterType::Nearest)
            } else { crate::media::fit_sharp(crop, tw, th, 64.0) };
            match r.picker.new_protocol(out, Size::new(r.cells.0, r.cells.1), Resize::Fit(None)) {
                Ok(proto) => { let _ = tx.send(Msg::Image { key: r.key.clone(), proto, src: (sw, sh) }); }
                Err(e) => { let _ = tx.send(Msg::Error { key: r.key.clone(), err: format!("Cannot draw: {e}") }); }
            }
        }));
        if res.is_err() { let _ = tx.send(Msg::Error { key: r.key.clone(), err: "Viewer failed on this file".into() }); }
    }
}

// ------------------------------------------------------------------ video playback

struct Player {
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    frames: Receiver<(f64, Protocol)>,
}

impl Player {
    fn start(path: &Path, from: f64, w: u32, h: u32, fps: f64, picker: Picker, cells: (u16, u16)) -> Option<Player> {
        let exe = crate::deps::ffmpeg()?;
        let mut c = crate::deps::command(exe);
        c.args(["-v", "error", "-nostdin"]);
        if from > 0.0 { c.args(["-ss", &format!("{from:.3}")]); }
        c.arg("-i").arg(path).args(["-map", "0:V:0", "-an", "-sn", "-vf", &format!("fps={fps},scale={w}:{h}:flags=bilinear"), "-f", "rawvideo", "-pix_fmt", "rgb24", "-"]);
        c.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
        let mut child = c.spawn().ok()?;
        let mut out = child.stdout.take()?;
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let (tx, rx) = sync_channel::<(f64, Protocol)>(2);
        let (s2, d2) = (stop.clone(), done.clone());
        std::thread::spawn(move || {
            let n = (w * h * 3) as usize;
            let mut buf = vec![0u8; n];
            let start = Instant::now();
            let mut i = 0u64;
            while !s2.load(Ordering::Relaxed) {
                if out.read_exact(&mut buf).is_err() { break; }
                let due = Duration::from_secs_f64(i as f64 / fps);
                i += 1;
                let now = start.elapsed();
                if now > due + Duration::from_secs_f64(1.5 / fps) { continue; }         // behind: drop this frame
                let Some(img) = image::RgbImage::from_raw(w, h, buf.clone()) else { break };
                let Ok(proto) = picker.new_protocol(DynamicImage::ImageRgb8(img), Size::new(cells.0, cells.1), Resize::Fit(None)) else { break };
                let wait = due.saturating_sub(start.elapsed());
                if !wait.is_zero() { std::thread::sleep(wait); }
                if tx.send((from + i as f64 / fps, proto)).is_err() { break; }
            }
            let _ = child.kill();
            let _ = child.wait();
            d2.store(true, Ordering::Relaxed);
        });
        Some(Player { stop, done, frames: rx })
    }
}
