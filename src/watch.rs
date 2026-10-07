//! Watch face drawn as a real picture (shown as Sixel in Windows Terminal): metal bezel, shaded dial,
//! hour markers, dauphine hands, date window and a sweeping second hand.

use chrono::{Datelike, NaiveDateTime, Timelike};
use image::DynamicImage;

pub struct Colors { pub dial: (u8, u8, u8), pub hands: (u8, u8, u8), pub second: (u8, u8, u8), pub accent: (u8, u8, u8), pub bg: (u8, u8, u8) }

/// A dial a little lighter than the background (or darker on light themes).
pub fn dial_color(bg: (u8, u8, u8)) -> (u8, u8, u8) {
    let light = (bg.0 as u32 + bg.1 as u32 + bg.2 as u32) > 380;
    if light { mix(bg, (40, 44, 52), 0.85) } else { mix(bg, (60, 66, 80), 0.5) }
}

fn hex(c: (u8, u8, u8)) -> String { format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2) }
fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
}

#[derive(PartialEq, Clone, Copy)]
pub enum Part { All, Dial, Hands }

/// The SVG for this moment (sub-second precision so the second hand sweeps).
pub fn svg(now: NaiveDateTime, c: &Colors, part: Part) -> String {
    let secs = now.second() as f64 + now.nanosecond().min(999_999_999) as f64 / 1e9;
    let mins = now.minute() as f64 + secs / 60.0;
    let hours = (now.hour() % 12) as f64 + mins / 60.0;
    let (ha, ma, sa) = (hours * 30.0, mins * 6.0, secs * 6.0);
    let light = (c.dial.0 as u32 + c.dial.1 as u32 + c.dial.2 as u32) > 380;
    let dial_hi = hex(mix(c.dial, if light { (255, 255, 255) } else { (90, 96, 110) }, 0.35));
    let dial_lo = hex(mix(c.dial, (0, 0, 0), if light { 0.08 } else { 0.45 }));
    let ink = hex(c.hands);
    let lume = hex(mix(c.hands, (255, 255, 255), 0.15));
    let sec = hex(c.second);
    let acc = hex(c.accent);
    let mut s = String::with_capacity(12_000);
    s += r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1000 1000">"##;
    let defs = format!(r##"<defs>
<radialGradient id="dial" cx="50%" cy="38%" r="65%"><stop offset="0" stop-color="{dial_hi}"/><stop offset="1" stop-color="{dial_lo}"/></radialGradient>
<linearGradient id="bezel" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="#f4f5f7"/><stop offset="0.45" stop-color="#9aa0a8"/><stop offset="0.55" stop-color="#7d838c"/><stop offset="1" stop-color="#e3e6ea"/></linearGradient>
<linearGradient id="ring" x1="1" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#d9dce0"/><stop offset="1" stop-color="#5c616a"/></linearGradient>
<linearGradient id="hand" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="{ink}"/><stop offset="0.5" stop-color="{ink}"/><stop offset="0.5" stop-color="{shade}"/><stop offset="1" stop-color="{shade}"/></linearGradient>
<filter id="sh" x="-20%" y="-20%" width="140%" height="140%"><feDropShadow dx="6" dy="10" stdDeviation="7" flood-color="#000" flood-opacity="0.45"/></filter>
</defs>"##, shade = hex(mix(c.hands, (0, 0, 0), 0.35)));
    s += &defs;
    if part == Part::Hands {
        s += &hands_svg(ha, ma, sa, &lume, &sec);
        s += "</svg>";
        return s;
    }
    // case, bezel, dial
    s += r##"<circle cx="500" cy="500" r="496" fill="url(#bezel)"/>"##;
    s += r##"<circle cx="500" cy="500" r="462" fill="url(#ring)"/>"##;
    s += &format!(r##"<circle cx="500" cy="500" r="448" fill="url(#dial)"/>"##);
    // minute track and hour markers
    s += r##"<g transform="translate(500 500)">"##;
    for i in 0..60 {
        let a = i * 6;
        if i % 5 == 0 {
            if i == 0 {
                s += &format!(r##"<g transform="rotate({a})"><rect x="-24" y="-430" width="16" height="92" rx="3" fill="{lume}" filter="url(#sh)"/><rect x="8" y="-430" width="16" height="92" rx="3" fill="{lume}" filter="url(#sh)"/></g>"##);
            } else {
                let len = if i % 15 == 0 { 92 } else { 74 };
                s += &format!(r##"<rect transform="rotate({a})" x="-11" y="-430" width="22" height="{len}" rx="3" fill="{lume}" filter="url(#sh)"/>"##);
            }
        } else {
            s += &format!(r##"<rect transform="rotate({a})" x="-2.5" y="-432" width="5" height="22" fill="{ink}" opacity="0.75"/>"##);
        }
    }
    s += "</g>";
    // writing and date window
    let font = r##"font-family="Segoe UI, Helvetica, Arial, sans-serif""##;
    s += &format!(r##"<text x="500" y="318" text-anchor="middle" {font} font-size="34" font-weight="600" letter-spacing="6" fill="{ink}">TASKS</text>"##);
    s += &format!(r##"<text x="500" y="352" text-anchor="middle" {font} font-size="20" letter-spacing="5" fill="{acc}">&amp; CLOCK</text>"##);
    s += &format!(r##"<text x="500" y="700" text-anchor="middle" {font} font-size="22" letter-spacing="7" fill="{ink}" opacity="0.8">AUTOMATIC</text>"##);
    s += &format!(r##"<rect x="652" y="470" width="96" height="60" rx="6" fill="#f7f7f2" stroke="url(#ring)" stroke-width="6"/>"##);
    s += &format!(r##"<text x="700" y="514" text-anchor="middle" {font} font-size="40" font-weight="700" fill="#16181d">{}</text>"##, now.day());
    s += &format!(r##"<text x="500" y="652" text-anchor="middle" {font} font-size="24" font-weight="600" letter-spacing="4" fill="{acc}">{}</text>"##, now.format("%a").to_string().to_uppercase());
    if part == Part::All { s += &hands_svg(ha, ma, sa, &lume, &sec); }
    s += "</svg>";
    s
}

fn hands_svg(ha: f64, ma: f64, sa: f64, lume: &str, sec: &str) -> String {
    let mut s = String::new();
    s += r##"<g transform="translate(500 500)">"##;
    s += &format!(r##"<g transform="rotate({ha:.3})" filter="url(#sh)"><polygon points="0,-262 26,-40 0,46 -26,-40" fill="url(#hand)"/><polygon points="0,-232 10,-70 -10,-70" fill="{lume}" opacity="0.9"/></g>"##);
    s += &format!(r##"<g transform="rotate({ma:.3})" filter="url(#sh)"><polygon points="0,-405 20,-40 0,52 -20,-40" fill="url(#hand)"/><polygon points="0,-372 8,-90 -8,-90" fill="{lume}" opacity="0.9"/></g>"##);
    s += &format!(r##"<g transform="rotate({sa:.3})" filter="url(#sh)"><rect x="-3.5" y="-420" width="7" height="520" rx="3" fill="{sec}"/><circle cx="0" cy="-318" r="15" fill="{sec}"/><circle cx="0" cy="-318" r="9" fill="{lume}"/><circle cx="0" cy="82" r="22" fill="{sec}"/></g>"##);
    s += &format!(r##"<circle r="20" fill="{sec}"/><circle r="8" fill="url(#ring)"/>"##);
    s += "</g>";
    s
}

fn tree(svg: &str) -> Option<resvg::usvg::Tree> {
    let mut opt = resvg::usvg::Options::default();
    opt.fontdb = crate::media::fontdb();
    resvg::usvg::Tree::from_str(svg, &opt).ok()
}

// the dial changes once a day: drawn once, then only the hands are drawn on a copy of it
static DIAL: std::sync::Mutex<Option<(String, resvg::tiny_skia::Pixmap)>> = std::sync::Mutex::new(None);

/// The watch as a square picture of `px` pixels.
pub fn render(now: NaiveDateTime, px: u32, c: &Colors) -> Option<DynamicImage> {
    let px = px.clamp(32, 1600);
    let s = px as f32 / 1000.0;
    let key = format!("{px} {} {:?} {:?} {:?} {:?} {:?}", now.date(), c.dial, c.hands, c.second, c.accent, c.bg);
    let mut cache = DIAL.lock().ok()?;
    if cache.as_ref().map(|(k, _)| *k != key).unwrap_or(true) {
        let mut pix = resvg::tiny_skia::Pixmap::new(px, px)?;
        pix.fill(resvg::tiny_skia::Color::from_rgba8(c.bg.0, c.bg.1, c.bg.2, 255));
        resvg::render(&tree(&svg(now, c, Part::Dial))?, resvg::tiny_skia::Transform::from_scale(s, s), &mut pix.as_mut());
        *cache = Some((key, pix));
    }
    let mut pix = cache.as_ref()?.1.clone();
    drop(cache);
    resvg::render(&tree(&svg(now, c, Part::Hands))?, resvg::tiny_skia::Transform::from_scale(s, s), &mut pix.as_mut());
    image::RgbaImage::from_raw(px, px, pix.take()).map(DynamicImage::ImageRgba8)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn draws() {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 10, 5).unwrap().and_hms_milli_opt(10, 8, 37, 500).unwrap();
        let c = Colors { dial: (30, 34, 44), hands: (235, 235, 230), second: (230, 70, 60), accent: (99, 179, 237), bg: (20, 22, 28) };
        let img = render(now, 400, &c).expect("rendered");
        assert_eq!((img.width(), img.height()), (400, 400));
        if let Ok(p) = std::env::var("FB_WATCH_PNG") { img.save(p).unwrap(); }
    }
}

#[cfg(test)]
mod bench {
    #[test]
    fn speed() {
        let c = super::Colors { dial: (30, 34, 44), hands: (235, 235, 230), second: (230, 70, 60), accent: (99, 179, 237), bg: (20, 22, 28) };
        let now = chrono::Local::now().naive_local();
        let _ = super::render(now, 100, &c);
        for px in [300u32, 500, 800] {
            let t = std::time::Instant::now();
            for _ in 0..5 { let _ = super::render(now, px, &c); }
            eprintln!("{px}px: {:.1} ms", t.elapsed().as_secs_f64() * 1000.0 / 5.0);
        }
    }
}
