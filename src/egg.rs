//! A small secret: type "slavcat" in the file list.
//! Random pixels fly in from the edges, form a squatting cat in a hat and a tracksuit,
//! and then the picture waves like a flag on a pole.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use std::time::Instant;

const ART: &[&str] = &[
    "........KK......................KK......",
    ".......KGGK....................KGGK.....",
    ".......KGPGK..KKKKKKKKKKKK....KGPGK.....",
    ".......KGPPGKKHHHHHHHHHHHHKKKKGPPGK.....",
    ".........KKHHHHHHHHHHHHHHHHHHKK.........",
    "........KHHHHHHHHHHHRHHHHHHHHHHK........",
    ".......KhhhhhhhhhhhhhhhhhhhhhhhhK.......",
    "......KhhhhhhhhhhhhhhhhhhhhhhhhhhK......",
    "......KhhhKKKKKKKKKKKKKKKKKKKhhhhK......",
    "......KhhKGGGGGGGGGGGGGGGGGGGGKhhK......",
    "......KhhKGGGGGGGGGGGGGGGGGGGGKhhK......",
    "......KhhKGGKKKGGGGGGGGGGKKKGGKhhK......",
    "......KhhKGKYYKGGGGGGGGGKYYKGGKhhK......",
    "......KhhKGKYKKGGGGGGGGGKYKKGGKhhK......",
    "......KhhKGGKKGGGGGggGGGGKKGGGKhhK......",
    "......KhhKGGGGGGGggggggGGGGGGGKhhK......",
    ".......KhKGWWGGggggPPggggGGWWGKhK.......",
    ".......KhKWGGGGgggKKKKgggGGGGWKhK.......",
    "........KKGGGGGGggKggKggGGGGGGKK........",
    "..........KGGGGGGGGGGGGGGGGGGK..........",
    "...........KKGGGGGGGGGGGGGGKK...........",
    ".........KKBBKKKKGGGGGGKKKKBBKK.........",
    ".......KKBBBBBBBBKWWWWKBBBBBBBBKK.......",
    "......KBBBBBBBBBBKWBBWKBBBBBBBBBBK......",
    ".....KBBWBBBBBBBBBBWBBBBBBBBBBWBBBK.....",
    "....KBBWBKBBBBBBBBBWBBBBBBBBBKBWBBK.....",
    "...KBBWBBKKBBBBBBBBWBBBBBBBBKKBBWBBK....",
    "..KBBBBBBGGKBBBBBBBWBBBBBBBKGGBBBBBBK...",
    "..KbBBBBKGGGKbBBBBBWBBBBBbKGGGKBBBBbK...",
    "..KbbBBBKggGKbbBBBBWBBBBbbKGggKBBBbbK...",
    "...KbbbBBKKKbbbbBBBBBBBbbbbKKKBBbbbK....",
    "...KbbbbbbbbbbbKbbbbbbbKbbbbbbbbbbbK....",
    "....KbbbbbbbbbK.KbbbbbK.KbbbbbbbbbK.....",
    ".....KbbbbbbbK...KKKKK...KbbbbbbbK......",
    "....KWWWWWWWWWK.........KWWWWWWWWWK.....",
    "....KKKKKKKKKKK.........KKKKKKKKKKK.....",
];

fn pal(c: u8) -> (u8, u8, u8) {
    match c {
        b'K' => (18, 18, 22), b'H' => (92, 58, 34), b'h' => (186, 150, 110), b'R' => (200, 40, 40),
        b'G' => (128, 128, 136), b'g' => (196, 196, 204), b'W' => (240, 240, 240), b'P' => (240, 150, 170),
        b'Y' => (250, 210, 60), b'B' => (40, 90, 200), b'b' => (24, 56, 140),
        _ => (52, 58, 78),                                   // the flag cloth
    }
}

const SPARKS: [(u8, u8, u8); 6] = [(255, 80, 80), (80, 255, 140), (90, 160, 255), (255, 220, 70), (230, 110, 255), (80, 240, 240)];

struct Particle { tx: f32, ty: f32, sx: f32, sy: f32, delay: f32, spin: f32, from: (u8, u8, u8), to: (u8, u8, u8) }

/// Remember the last letters typed; true when they spell the secret word (spaces and case ignored).
pub fn typed(buf: &mut String, c: char) -> bool {
    if c.is_whitespace() { return false; }
    buf.push(c.to_ascii_lowercase());
    while buf.chars().count() > 16 { buf.remove(0); }
    if buf.ends_with("slavcat") { buf.clear(); return true; }
    false
}

pub struct Egg { start: Instant, parts: Vec<Particle>, seed: u64, size: (u16, u16), scale: f32, ox: f32, oy: f32 }

const FLY: f32 = 1.4;        // seconds each pixel travels
const SPREAD: f32 = 1.0;     // pixels set off over this long
const LANDED: f32 = FLY + SPREAD;

fn rnd(seed: &mut u64) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (*seed >> 40) as f32 / (1u64 << 24) as f32
}

fn lerp(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }
fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    (lerp(a.0 as f32, b.0 as f32, t) as u8, lerp(a.1 as f32, b.1 as f32, t) as u8, lerp(a.2 as f32, b.2 as f32, t) as u8)
}
fn shade(c: (u8, u8, u8), k: f32) -> (u8, u8, u8) {
    let f = |v: u8| (v as f32 * k).clamp(0.0, 255.0) as u8;
    (f(c.0), f(c.1), f(c.2))
}

impl Egg {
    pub fn new() -> Egg {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7) | 1;
        Egg { start: Instant::now(), parts: Vec::new(), seed, size: (0, 0), scale: 1.0, ox: 0.0, oy: 0.0 }
    }

    pub fn age(&self) -> f32 { self.start.elapsed().as_secs_f32() }

    /// Place the picture for this window size (one pixel = half a character cell).
    fn layout(&mut self, w: u16, h: u16) {
        if self.size == (w, h) && !self.parts.is_empty() { return; }
        self.size = (w, h);
        let (aw, ah) = (ART[0].len() as f32, ART.len() as f32);
        let (cw, ch) = (w as f32, (h as f32 - 2.0) * 2.0);
        let s = ((cw - 12.0) / aw).min((ch - 10.0) / ah).floor().max(1.0);
        self.scale = s;
        self.ox = ((cw - aw * s) / 2.0).floor() + 2.0;
        self.oy = ((ch - ah * s) / 2.0).floor().max(5.0);
        let mut seed = self.seed;
        let mut parts = Vec::new();
        for (y, row) in ART.iter().enumerate() {
            for (x, c) in row.bytes().enumerate() {
                let from = SPARKS[(rnd(&mut seed) * 6.0) as usize % 6];
                let edge = rnd(&mut seed);
                let along = rnd(&mut seed);
                let (sx, sy) = if edge < 0.25 { (along * cw, -3.0) } else if edge < 0.5 { (along * cw, ch + 3.0) }
                               else if edge < 0.75 { (-3.0, along * ch) } else { (cw + 3.0, along * ch) };
                parts.push(Particle { tx: x as f32, ty: y as f32, sx, sy, delay: rnd(&mut seed) * SPREAD, spin: rnd(&mut seed) * 6.3, from, to: pal(c) });
            }
        }
        self.parts = parts;
    }

    pub fn draw(&mut self, buf: &mut Buffer, area: Rect, bg: (u8, u8, u8), dim: Color) {
        self.layout(area.width, area.height);
        let (w, hh) = (area.width as usize, (area.height as usize).saturating_sub(2) * 2);
        let mut fb: Vec<(u8, u8, u8)> = vec![bg; w * hh];
        let t = self.age();
        let s = self.scale;
        let aw = ART[0].len() as f32;
        let put = |x: f32, y: f32, c: (u8, u8, u8), fb: &mut Vec<(u8, u8, u8)>| {
            let (xi, yi) = (x.round() as i32, y.round() as i32);
            if xi >= 0 && yi >= 0 && (xi as usize) < w && (yi as usize) < hh { fb[yi as usize * w + xi as usize] = c; }
        };
        // the pole appears as the last pixels land
        if t > LANDED * 0.7 {
            let px = self.ox - 2.0;
            for y in (self.oy as i32 - 3).max(0)..hh as i32 {
                put(px, y as f32, (170, 170, 180), &mut fb);
                put(px - 1.0, y as f32, (110, 110, 120), &mut fb);
            }
            for dx in [-1.0, 0.0] { put(px + dx, self.oy - 4.0, (250, 210, 60), &mut fb); put(px + dx, self.oy - 5.0, (250, 210, 60), &mut fb); }
        }
        // the wave fades in after landing; the cloth is fixed at the pole and free at the far end
        let wave = ((t - LANDED) / 1.5).clamp(0.0, 1.0);
        for p in &self.parts {
            let k = ((t - p.delay) / FLY).clamp(0.0, 1.0);
            if k <= 0.0 { continue; }
            let e = 1.0 - (1.0 - k).powi(3);                          // ease-out
            let fx = p.tx / aw;
            let phase = t * 3.5 - p.tx * 0.30;
            let dy = phase.sin() * s * 1.8 * fx * wave;
            let light = 1.0 + phase.cos() * 0.25 * fx * wave;
            let swirl = (1.0 - e) * 12.0;                             // spiral in while flying
            let ang = p.spin + t * 4.0;
            let c = shade(mix(p.from, p.to, e.powf(0.6)), light);
            for oy in 0..s as i32 {
                for ox in 0..s as i32 {
                    let tx = self.ox + p.tx * s + ox as f32;
                    let ty = self.oy + p.ty * s + oy as f32 + dy;
                    put(lerp(p.sx, tx, e) + ang.cos() * swirl, lerp(p.sy, ty, e) + ang.sin() * swirl, c, &mut fb);
                }
            }
        }
        // two pixels per character: upper half block, top pixel = text colour, bottom = background
        for cy in 0..hh / 2 {
            for cx in 0..w {
                let (a, b) = (fb[cy * 2 * w + cx], fb[(cy * 2 + 1) * w + cx]);
                if let Some(cell) = buf.cell_mut((area.x + cx as u16, area.y + cy as u16)) {
                    cell.set_char('▀').set_style(Style::default().fg(Color::Rgb(a.0, a.1, a.2)).bg(Color::Rgb(b.0, b.1, b.2)));
                }
            }
        }
        for y in (hh / 2) as u16..area.height {
            for x in 0..area.width {
                if let Some(cell) = buf.cell_mut((area.x + x, area.y + y)) { cell.set_char(' ').set_style(Style::default().bg(Color::Rgb(bg.0, bg.1, bg.2))); }
            }
        }
        if t > LANDED + 0.8 {
            let msg = "slav cat approves of this file browser  ·  press any key";
            let x = area.x + area.width.saturating_sub(msg.chars().count() as u16) / 2;
            buf.set_string(x, area.y + area.height - 1, msg, Style::default().fg(dim).bg(Color::Rgb(bg.0, bg.1, bg.2)));
        }
    }
}
