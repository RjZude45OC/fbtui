//! Rich media: PDF pages (PDFium), video / audio (FFmpeg), SVG (resvg), photo EXIF,
//! and extra picture formats decoded by FFmpeg (HEIC, AVIF, JPEG XL, PSD, TGA, EXR ...).

use crate::deps;
use crate::fsutil::ext_of;
use image::DynamicImage;
use pdfium_render::prelude::*;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub const IMAGE_EXTS: &[&str] = &[".png", ".jpg", ".jpeg", ".jfif", ".gif", ".bmp", ".ico", ".tif", ".tiff", ".webp"];
pub const FFMPEG_PIC_EXTS: &[&str] = &[".heic", ".heif", ".avif", ".jxl", ".tga", ".dds", ".exr", ".psd", ".pcx", ".qoi", ".hdr", ".sgi", ".ppm", ".pgm", ".pbm"];
pub const VIDEO_EXTS: &[&str] = &[".mp4", ".mkv", ".avi", ".mov", ".wmv", ".webm", ".m4v", ".mpg", ".mpeg", ".3gp", ".flv", ".ts", ".mts", ".m2ts", ".vob", ".ogv", ".gifv"];
pub const AUDIO_EXTS: &[&str] = &[".mp3", ".wav", ".flac", ".ogg", ".oga", ".opus", ".m4a", ".aac", ".wma", ".aiff", ".aif", ".ape", ".mka", ".alac", ".amr", ".mid"];
pub const SHELL_ONLY_EXTS: &[&str] = &[".dng", ".cr2", ".cr3", ".nef", ".arw", ".orf", ".rw2", ".raf"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind { Picture, Svg, Pdf, Video, Audio, None }

pub fn kind_of(p: &Path) -> Kind {
    let e = ext_of(p);
    let e = e.as_str();
    if e == ".svg" || e == ".svgz" { Kind::Svg }
    else if e == ".pdf" { Kind::Pdf }
    else if IMAGE_EXTS.contains(&e) || FFMPEG_PIC_EXTS.contains(&e) || SHELL_ONLY_EXTS.contains(&e) { Kind::Picture }
    else if VIDEO_EXTS.contains(&e) { Kind::Video }
    else if AUDIO_EXTS.contains(&e) { Kind::Audio }
    else { Kind::None }
}

// ------------------------------------------------------------------ PDF

pub struct PdfInfo { pub pages: u32, pub lines: Vec<String> }

pub fn pdf_info(path: &Path) -> Result<PdfInfo, String> {
    let lock = deps::pdfium().ok_or("PDFium not found (run get-deps.bat)")?;
    let pdfium = lock.lock().map_err(|_| "PDFium busy")?;
    let doc = pdfium.load_pdf_from_file(path, None).map_err(pdf_err)?;
    let pages = doc.pages().len().max(0) as u32;
    let mut lines = Vec::new();
    lines.push(format!("{:<12}{pages}", "Pages:"));
    if let Ok(p) = doc.pages().get(0) {
        let (w, h) = (p.width().value, p.height().value);
        lines.push(format!("{:<12}{:.0} x {:.0} mm{}", "Page size:", w / 72.0 * 25.4, h / 72.0 * 25.4, paper_name(w, h)));
    }
    let m = doc.metadata();
    for (label, tag) in [("Title:", PdfDocumentMetadataTagType::Title), ("Author:", PdfDocumentMetadataTagType::Author),
                         ("Subject:", PdfDocumentMetadataTagType::Subject), ("Created:", PdfDocumentMetadataTagType::CreationDate),
                         ("Creator:", PdfDocumentMetadataTagType::Creator), ("Producer:", PdfDocumentMetadataTagType::Producer)] {
        if let Some(v) = m.get(tag) {
            let v = v.value().trim().to_string();
            if v.is_empty() { continue; }
            let v = if label == "Created:" { pdf_date(&v) } else { v };
            lines.push(format!("{label:<12}{v}"));
        }
    }
    Ok(PdfInfo { pages, lines })
}

/// Render one page (0-based) to fit max_w x max_h pixels.
pub fn pdf_render(path: &Path, page: u32, max_w: u32, max_h: u32) -> Result<DynamicImage, String> {
    let lock = deps::pdfium().ok_or("PDFium not found (run get-deps.bat)")?;
    let pdfium = lock.lock().map_err(|_| "PDFium busy")?;
    let doc = pdfium.load_pdf_from_file(path, None).map_err(pdf_err)?;
    let p = doc.pages().get(page as i32).map_err(pdf_err)?;
    let (pw, ph) = (p.width().value.max(1.0), p.height().value.max(1.0));
    let scale = (max_w as f32 / pw).min(max_h as f32 / ph);
    let w = ((pw * scale) as i32).clamp(16, 12000);
    let cfg = PdfRenderConfig::new().set_target_width(w).render_form_data(true);
    let bmp = p.render_with_config(&cfg).map_err(pdf_err)?;
    bmp.as_image().map_err(pdf_err)
}

fn pdf_err(e: PdfiumError) -> String {
    let s = format!("{e:?}");
    if s.contains("Password") { "Password-protected PDF".into() } else { format!("Cannot read PDF: {s}") }
}

fn paper_name(w: f32, h: f32) -> &'static str {
    let (a, b) = if w < h { (w, h) } else { (h, w) };
    let near = |x: f32, y: f32| (a - x).abs() < 6.0 && (b - y).abs() < 6.0;
    if near(595.0, 842.0) { "  (A4)" } else if near(612.0, 792.0) { "  (Letter)" } else if near(842.0, 1191.0) { "  (A3)" }
    else if near(420.0, 595.0) { "  (A5)" } else if near(612.0, 1008.0) { "  (Legal)" } else { "" }
}

fn pdf_date(v: &str) -> String {
    // D:20240131120000+01'00'
    let d = v.trim_start_matches("D:");
    if d.len() >= 12 && d[..12].chars().all(|c| c.is_ascii_digit()) {
        format!("{}-{}-{} {}:{}", &d[0..4], &d[4..6], &d[6..8], &d[8..10], &d[10..12])
    } else { v.to_string() }
}

// ------------------------------------------------------------------ FFmpeg: video / audio

#[derive(Clone, Default, Debug)]
pub struct MediaInfo {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub has_video: bool,      // a real video stream (not just cover art)
    pub has_cover: bool,      // audio file with embedded cover picture
    pub has_audio: bool,
    pub lines: Vec<String>,
}

pub fn probe(path: &Path) -> Option<MediaInfo> {
    let exe = deps::ffprobe()?;
    let mut c = deps::command(exe);
    c.args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"]).arg(path);
    let out = deps::run_capture(c, Duration::from_secs(15))?;
    let j: serde_json::Value = serde_json::from_slice(&out).ok()?;
    let mut m = MediaInfo::default();
    let fmt = &j["format"];
    let num = |v: &serde_json::Value| v.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| v.as_f64());
    m.duration = num(&fmt["duration"]).unwrap_or(0.0);
    let mut lines = Vec::new();
    if m.duration > 0.0 { lines.push(format!("{:<12}{}", "Duration:", fmt_dur(m.duration))); }
    let empty = vec![];
    let streams = j["streams"].as_array().unwrap_or(&empty);
    let mut subs = Vec::new();
    for s in streams {
        let codec = s["codec_name"].as_str().unwrap_or("?");
        let lang = s["tags"]["language"].as_str().filter(|l| *l != "und").map(|l| format!("  [{l}]")).unwrap_or_default();
        match s["codec_type"].as_str() {
            Some("video") => {
                let w = s["width"].as_u64().unwrap_or(0) as u32;
                let h = s["height"].as_u64().unwrap_or(0) as u32;
                if s["disposition"]["attached_pic"].as_i64() == Some(1) {
                    m.has_cover = true;
                    lines.push(format!("{:<12}{codec}  {w} x {h}", "Cover art:"));
                    continue;
                }
                if m.has_video { continue; }
                m.has_video = true;
                m.width = w; m.height = h;
                let fr = s["avg_frame_rate"].as_str().or(s["r_frame_rate"].as_str()).unwrap_or("0/1");
                let (a, b) = fr.split_once('/').unwrap_or((fr, "1"));
                m.fps = a.parse::<f64>().unwrap_or(0.0) / b.parse::<f64>().unwrap_or(1.0).max(1e-9);
                let mut v = format!("{codec}  {w} x {h}");
                if m.fps > 0.0 && m.fps < 1000.0 { v += &format!("  {} fps", format!("{:.3}", m.fps).trim_end_matches('0').trim_end_matches('.')); }
                if let Some(pf) = s["pix_fmt"].as_str() { if pf.contains("10") || pf.contains("12") { v += "  10-bit"; } }
                if s["color_transfer"].as_str().map(|t| t == "smpte2084" || t == "arib-std-b67").unwrap_or(false) { v += "  HDR"; }
                lines.push(format!("{:<12}{v}", "Video:"));
            }
            Some("audio") => {
                m.has_audio = true;
                let sr = s["sample_rate"].as_str().and_then(|x| x.parse::<f64>().ok()).map(|x| format!("  {} kHz", trim_num(x / 1000.0))).unwrap_or_default();
                let ch = match s["channels"].as_u64() { Some(1) => "  mono".into(), Some(2) => "  stereo".into(), Some(n) => format!("  {n} ch"), None => String::new() };
                let br = num(&s["bit_rate"]).map(|b| format!("  {} kb/s", (b / 1000.0).round())).unwrap_or_default();
                lines.push(format!("{:<12}{codec}{sr}{ch}{br}{lang}", "Audio:"));
            }
            Some("subtitle") => subs.push(format!("{codec}{lang}")),
            _ => {}
        }
    }
    if !subs.is_empty() { lines.push(format!("{:<12}{}", "Subtitles:", subs.join(", "))); }
    if let Some(b) = num(&fmt["bit_rate"]) { lines.push(format!("{:<12}{:.1} Mb/s", "Bitrate:", b / 1_000_000.0)); }
    if let Some(n) = fmt["format_long_name"].as_str() { lines.push(format!("{:<12}{n}", "Container:")); }
    let tags = &fmt["tags"];
    let mut tag_lines = Vec::new();
    for (label, keys) in [("Title:", &["title", "TITLE"][..]), ("Artist:", &["artist", "ARTIST", "album_artist"]), ("Album:", &["album", "ALBUM"]),
                          ("Track:", &["track", "TRACK"]), ("Year:", &["date", "DATE", "year"]), ("Genre:", &["genre", "GENRE"]),
                          ("Comment:", &["comment", "COMMENT", "description"])] {
        if let Some(v) = keys.iter().find_map(|k| tags[*k].as_str()) {
            let v = v.trim();
            if !v.is_empty() { tag_lines.push(format!("{label:<12}{}", v.lines().next().unwrap_or(""))); }
        }
    }
    if !tag_lines.is_empty() { lines.push(String::new()); lines.extend(tag_lines); }
    m.lines = lines;
    Some(m)
}

fn trim_num(x: f64) -> String { let s = format!("{x:.1}"); s.trim_end_matches(".0").to_string() }

pub fn fmt_dur(s: f64) -> String {
    let t = s.max(0.0) as u64;
    let (h, m, sec) = (t / 3600, (t / 60) % 60, t % 60);
    if h > 0 { format!("{h}:{m:02}:{sec:02}") } else { format!("{m}:{sec:02}") }
}

/// Filter that fits the frame inside w x h (and fixes non-square pixels).
fn fit_filter(w: u32, h: u32) -> String {
    format!("scale='trunc(iw*max(1,sar)/2)*2':'trunc(ih*max(1,1/sar)/2)*2',setsar=1,scale='min({w},iw)':'min({h},ih)':force_original_aspect_ratio=decrease")
}

fn png_out(c: &mut std::process::Command) { c.args(["-frames:v", "1", "-f", "image2pipe", "-c:v", "png", "-"]); }

/// One video frame at `at` seconds, at most max_w x max_h.
pub fn video_frame(path: &Path, at: f64, max_w: u32, max_h: u32) -> Option<DynamicImage> {
    let exe = deps::ffmpeg()?;
    let mut c = deps::command(exe);
    c.args(["-v", "error", "-nostdin"]);
    if at > 0.0 { c.args(["-ss", &format!("{at:.3}")]); }
    c.arg("-i").arg(path).args(["-map", "0:V:0", "-an", "-sn", "-vf", &fit_filter(max_w, max_h), "-pix_fmt", "rgb24"]);
    png_out(&mut c);
    let out = deps::run_capture(c, Duration::from_secs(20))?;
    image::load_from_memory(&out).ok()
}

/// Embedded cover picture of an audio file.
pub fn cover_art(path: &Path, max_w: u32, max_h: u32) -> Option<DynamicImage> {
    let exe = deps::ffmpeg()?;
    let mut c = deps::command(exe);
    c.args(["-v", "error", "-nostdin"]).arg("-i").arg(path).args(["-an", "-vf", &fit_filter(max_w, max_h), "-pix_fmt", "rgb24"]);
    png_out(&mut c);
    image::load_from_memory(&deps::run_capture(c, Duration::from_secs(10))?).ok()
}

/// Waveform picture of an audio file.
pub fn waveform(path: &Path, w: u32, h: u32, rgb: (u8, u8, u8)) -> Option<DynamicImage> {
    let exe = deps::ffmpeg()?;
    let mut c = deps::command(exe);
    let col = format!("0x{:02X}{:02X}{:02X}", rgb.0, rgb.1, rgb.2);
    c.args(["-v", "error", "-nostdin"]).arg("-i").arg(path)
        .args(["-filter_complex", &format!("aformat=channel_layouts=mono,showwavespic=s={}x{}:colors={col}:scale=sqrt:draw=full", w.max(64), h.max(16))]);
    png_out(&mut c);
    image::load_from_memory(&deps::run_capture(c, Duration::from_secs(25))?).ok()
}

/// Any picture FFmpeg can decode (HEIC, AVIF, JPEG XL, PSD, TGA, EXR ...).
pub fn ffmpeg_picture(path: &Path) -> Option<DynamicImage> {
    let exe = deps::ffmpeg()?;
    let mut c = deps::command(exe);
    c.args(["-v", "error", "-nostdin"]).arg("-i").arg(path).args(["-pix_fmt", "rgba"]);
    png_out(&mut c);
    image::load_from_memory(&deps::run_capture(c, Duration::from_secs(15))?).ok()
}

/// Play with sound in a separate small window (ffplay), or in the default app.
pub fn play_external(path: &Path) -> Result<&'static str, String> {
    if let Some(exe) = deps::ffplay() {
        let mut c = std::process::Command::new(exe);
        c.args(["-loglevel", "quiet", "-autoexit", "-x", "960", "-y", "540", "-window_title"]).arg(path.file_name().unwrap_or_default()).arg(path);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x08000000);
        }
        c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        c.spawn().map(|_| "ffplay").map_err(|e| e.to_string())
    } else {
        crate::winapi::open_default(path).map(|_| "default app").map_err(|e| e.to_string())
    }
}

// ------------------------------------------------------------------ pictures

pub fn fontdb() -> Arc<resvg::usvg::fontdb::Database> {
    static DB: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    }).clone()
}

/// Size of an SVG in its own units.
pub fn svg_size(path: &Path) -> Option<(f32, f32)> {
    let tree = svg_tree(path)?;
    Some((tree.size().width(), tree.size().height()))
}

fn svg_tree(path: &Path) -> Option<resvg::usvg::Tree> {
    let data = std::fs::read(path).ok()?;
    let mut opt = resvg::usvg::Options { resources_dir: path.parent().map(|p| p.to_path_buf()), ..Default::default() };
    opt.fontdb = fontdb();
    resvg::usvg::Tree::from_data(&data, &opt).ok()
}

/// Render an SVG sharply at the size it will be shown.
pub fn svg_render(path: &Path, max_w: u32, max_h: u32) -> Option<DynamicImage> {
    let tree = svg_tree(path)?;
    let (sw, sh) = (tree.size().width().max(1.0), tree.size().height().max(1.0));
    let s = (max_w as f32 / sw).min(max_h as f32 / sh).min(64.0);
    let (w, h) = (((sw * s).round() as u32).max(1), ((sh * s).round() as u32).max(1));
    let mut pix = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(s, s), &mut pix.as_mut());
    let mut data = pix.take();
    for px in data.chunks_exact_mut(4) {        // premultiplied -> straight alpha
        let a = px[3] as u32;
        if a > 0 && a < 255 { for c in 0..3 { px[c] = ((px[c] as u32 * 255 + a / 2) / a).min(255) as u8; } }
    }
    image::RgbaImage::from_raw(w, h, data).map(DynamicImage::ImageRgba8)
}

pub struct Exif { pub lines: Vec<String>, pub orientation: u32 }

pub fn exif(path: &Path) -> Option<Exif> {
    let f = std::fs::File::open(path).ok()?;
    let mut r = std::io::BufReader::new(f);
    let ex = exif::Reader::new().read_from_container(&mut r).ok()?;
    let get = |t: exif::Tag| ex.get_field(t, exif::In::PRIMARY).map(|f| f.display_value().with_unit(&ex).to_string().trim_matches('"').trim().to_string()).filter(|s| !s.is_empty());
    let mut lines = Vec::new();
    let make = get(exif::Tag::Make).unwrap_or_default();
    let model = get(exif::Tag::Model).unwrap_or_default();
    let cam = if model.to_lowercase().starts_with(&make.to_lowercase()) { model } else { format!("{make} {model}") };
    if !cam.trim().is_empty() { lines.push(format!("{:<12}{}", "Camera:", cam.trim())); }
    if let Some(l) = get(exif::Tag::LensModel) { lines.push(format!("{:<12}{l}", "Lens:")); }
    if let Some(d) = get(exif::Tag::DateTimeOriginal).or_else(|| get(exif::Tag::DateTime)) { lines.push(format!("{:<12}{d}", "Taken:")); }
    let mut shot = Vec::new();
    if let Some(v) = get(exif::Tag::FNumber) { shot.push(v); }
    if let Some(v) = get(exif::Tag::ExposureTime) { shot.push(v); }
    if let Some(v) = get(exif::Tag::PhotographicSensitivity) { shot.push(format!("ISO {v}")); }
    if let Some(v) = get(exif::Tag::FocalLength) { shot.push(v); }
    if !shot.is_empty() { lines.push(format!("{:<12}{}", "Exposure:", shot.join("  ·  "))); }
    if let (Some(la), Some(lo)) = (gps(&ex, exif::Tag::GPSLatitude, exif::Tag::GPSLatitudeRef), gps(&ex, exif::Tag::GPSLongitude, exif::Tag::GPSLongitudeRef)) {
        lines.push(format!("{:<12}{la:.5}, {lo:.5}", "GPS:"));
    }
    let orientation = ex.get_field(exif::Tag::Orientation, exif::In::PRIMARY).and_then(|f| f.value.get_uint(0)).unwrap_or(1);
    Some(Exif { lines, orientation })
}

fn gps(ex: &exif::Exif, tag: exif::Tag, rtag: exif::Tag) -> Option<f64> {
    let f = ex.get_field(tag, exif::In::PRIMARY)?;
    let exif::Value::Rational(v) = &f.value else { return None };
    if v.len() < 3 { return None; }
    let d = v[0].to_f64() + v[1].to_f64() / 60.0 + v[2].to_f64() / 3600.0;
    let r = ex.get_field(rtag, exif::In::PRIMARY).map(|f| f.display_value().to_string()).unwrap_or_default();
    Some(if r.contains('S') || r.contains('W') { -d } else { d })
}

pub fn orient(img: DynamicImage, o: u32) -> DynamicImage {
    match o {
        2 => img.fliph(), 3 => img.rotate180(), 4 => img.flipv(),
        5 => img.rotate90().fliph(), 6 => img.rotate90(), 7 => img.rotate270().fliph(), 8 => img.rotate270(),
        _ => img,
    }
}

/// Load any picture at full resolution: Rust decoders, then FFmpeg, then the Windows thumbnail.
/// Returns the picture, its original size, and extra info lines (EXIF).
pub fn load_picture(path: &Path, want_px: u32) -> Option<(DynamicImage, (u32, u32), Vec<String>)> {
    let e = ext_of(path);
    if e == ".svg" || e == ".svgz" {
        let (w, h) = svg_size(path).unwrap_or((0.0, 0.0));
        let img = svg_render(path, want_px, want_px)?;
        return Some((img, (w as u32, h as u32), vec![format!("{:<12}vector (drawn sharp at any size)", "Type:")]));
    }
    let mut info = Vec::new();
    if IMAGE_EXTS.contains(&e.as_str()) {
        if let Ok(img) = image::open(path) {
            let dims = (img.width(), img.height());
            let mut img = img;
            if let Some(x) = exif(path) { img = orient(img, x.orientation); info = x.lines; }
            return Some((img, dims, info));
        }
    }
    if !SHELL_ONLY_EXTS.contains(&e.as_str()) {
        if let Some(img) = ffmpeg_picture(path) {
            if let Some(x) = exif(path) { info = x.lines; }
            let dims = (img.width(), img.height());
            return Some((img, dims, info));
        }
    }
    let img = crate::winapi::shell_image(path, want_px.clamp(256, 2048) as i32, false)?;
    let dims = (img.width(), img.height());
    Some((img, dims, info))
}

/// Frames of an animated GIF / WebP / APNG (None if it is a still picture).
pub fn animation_frames(path: &Path, max_frames: usize) -> Option<Vec<(DynamicImage, Duration)>> {
    use image::AnimationDecoder;
    let e = ext_of(path);
    let f = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    let frames: Vec<image::Frame> = match e.as_str() {
        ".gif" => image::codecs::gif::GifDecoder::new(f).ok()?.into_frames().take(max_frames).filter_map(Result::ok).collect(),
        ".webp" => {
            let d = image::codecs::webp::WebPDecoder::new(f).ok()?;
            if !d.has_animation() { return None; }
            d.into_frames().take(max_frames).filter_map(Result::ok).collect()
        }
        ".png" => {
            let d = image::codecs::png::PngDecoder::new(f).ok()?;
            if !d.is_apng().ok()? { return None; }
            d.apng().ok()?.into_frames().take(max_frames).filter_map(Result::ok).collect()
        }
        _ => return None,
    };
    if frames.len() < 2 { return None; }
    Some(frames.into_iter().map(|fr| {
        let (n, d) = fr.delay().numer_denom_ms();
        let ms = if d == 0 { 100 } else { (n / d).max(20) };
        (DynamicImage::ImageRgba8(fr.into_buffer()), Duration::from_millis(ms as u64))
    }).collect())
}

/// Resize to fit w x h exactly once: Lanczos3 when shrinking (sharp), CatmullRom when enlarging,
/// Nearest for tiny pixel-art / icons. `max_up` limits enlarging.
pub fn fit_sharp(img: DynamicImage, w: u32, h: u32, max_up: f32) -> DynamicImage {
    use image::imageops::FilterType;
    let (iw, ih) = (img.width().max(1), img.height().max(1));
    let s = (w as f32 / iw as f32).min(h as f32 / ih as f32).min(max_up);
    let (tw, th) = (((iw as f32 * s).floor() as u32).max(1), ((ih as f32 * s).floor() as u32).max(1));
    if tw == iw && th == ih { return img; }
    let f = if s > 1.0 && iw.max(ih) <= 64 { FilterType::Nearest } else if s > 1.0 { FilterType::CatmullRom } else { FilterType::Lanczos3 };
    img.resize_exact(tw, th, f)
}
