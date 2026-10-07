//! config.ini: theme presets, colour overrides, bookmarks and options.
//! Same file and format as the PowerShell version, so both can share it.

use ratatui::style::Color;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

/// Built-in presets, in Ctrl+Tab order. Values are "r;g;b" (empty background = terminal default).
/// keys: accent text dim ok error select_bg highlight badge_text background python batch cmd powershell vbscript
const PRESETS: &[(&str, &[(&str, &str)])] = &[
    ("dark", &[("accent", "99;179;237"), ("text", "230;235;240"), ("dim", "120;130;145"), ("ok", "72;199;116"), ("error", "255;99;99"),
               ("select_bg", "38;56;84"), ("highlight", "255;212;59"), ("badge_text", "20;20;20"), ("background", ""),
               ("python", "255;212;59"), ("batch", "77;208;225"), ("cmd", "38;166;154"), ("powershell", "100;149;237"), ("vbscript", "186;104;200")]),
    ("light", &[("accent", "0;102;204"), ("text", "30;35;40"), ("dim", "110;118;128"), ("ok", "20;140;60"), ("error", "200;40;40"),
                ("select_bg", "205;225;250"), ("highlight", "176;96;0"), ("badge_text", "20;20;20"), ("background", "250;250;250"),
                ("python", "230;180;0"), ("batch", "0;150;170"), ("cmd", "20;140;120"), ("powershell", "60;110;210"), ("vbscript", "150;80;180")]),
    ("dracula", &[("accent", "189;147;249"), ("text", "248;248;242"), ("dim", "98;114;164"), ("ok", "80;250;123"), ("error", "255;85;85"),
                  ("select_bg", "68;71;90"), ("highlight", "241;250;140"), ("badge_text", "40;42;54"), ("background", "40;42;54"),
                  ("python", "241;250;140"), ("batch", "139;233;253"), ("cmd", "80;250;123"), ("powershell", "189;147;249"), ("vbscript", "255;121;198")]),
    ("nord", &[("accent", "136;192;208"), ("text", "216;222;233"), ("dim", "129;140;160"), ("ok", "163;190;140"), ("error", "191;97;106"),
               ("select_bg", "67;76;94"), ("highlight", "235;203;139"), ("badge_text", "46;52;64"), ("background", "46;52;64"),
               ("python", "235;203;139"), ("batch", "136;192;208"), ("cmd", "163;190;140"), ("powershell", "129;161;193"), ("vbscript", "180;142;173")]),
    ("gruvbox", &[("accent", "131;165;152"), ("text", "235;219;178"), ("dim", "146;131;116"), ("ok", "184;187;38"), ("error", "251;73;52"),
                  ("select_bg", "80;73;69"), ("highlight", "254;128;25"), ("badge_text", "40;40;40"), ("background", "40;40;40"),
                  ("python", "250;189;47"), ("batch", "142;192;124"), ("cmd", "184;187;38"), ("powershell", "131;165;152"), ("vbscript", "211;134;155")]),
    ("solarized", &[("accent", "38;139;210"), ("text", "147;161;161"), ("dim", "88;110;117"), ("ok", "133;153;0"), ("error", "220;50;47"),
                    ("select_bg", "23;74;88"), ("highlight", "181;137;0"), ("badge_text", "0;43;54"), ("background", "0;43;54"),
                    ("python", "181;137;0"), ("batch", "42;161;152"), ("cmd", "133;153;0"), ("powershell", "38;139;210"), ("vbscript", "211;54;130")]),
    ("solarized-light", &[("accent", "38;139;210"), ("text", "88;110;117"), ("dim", "147;161;161"), ("ok", "133;153;0"), ("error", "220;50;47"),
                          ("select_bg", "238;232;213"), ("highlight", "203;75;22"), ("badge_text", "253;246;227"), ("background", "253;246;227"),
                          ("python", "181;137;0"), ("batch", "42;161;152"), ("cmd", "133;153;0"), ("powershell", "38;139;210"), ("vbscript", "211;54;130")]),
    ("monokai", &[("accent", "102;217;239"), ("text", "248;248;242"), ("dim", "117;113;94"), ("ok", "166;226;46"), ("error", "249;38;114"),
                  ("select_bg", "73;72;62"), ("highlight", "230;219;116"), ("badge_text", "39;40;34"), ("background", "39;40;34"),
                  ("python", "230;219;116"), ("batch", "102;217;239"), ("cmd", "166;226;46"), ("powershell", "174;129;255"), ("vbscript", "249;38;114")]),
    ("tokyonight", &[("accent", "122;162;247"), ("text", "192;202;245"), ("dim", "86;95;137"), ("ok", "158;206;106"), ("error", "247;118;142"),
                     ("select_bg", "41;46;66"), ("highlight", "224;175;104"), ("badge_text", "26;27;38"), ("background", "26;27;38"),
                     ("python", "224;175;104"), ("batch", "125;207;255"), ("cmd", "158;206;106"), ("powershell", "122;162;247"), ("vbscript", "187;154;247")]),
    ("catppuccin", &[("accent", "137;180;250"), ("text", "205;214;244"), ("dim", "108;112;134"), ("ok", "166;227;161"), ("error", "243;139;168"),
                     ("select_bg", "49;50;68"), ("highlight", "249;226;175"), ("badge_text", "30;30;46"), ("background", "30;30;46"),
                     ("python", "249;226;175"), ("batch", "137;220;235"), ("cmd", "166;227;161"), ("powershell", "137;180;250"), ("vbscript", "203;166;247")]),
    ("onedark", &[("accent", "97;175;239"), ("text", "171;178;191"), ("dim", "92;99;112"), ("ok", "152;195;121"), ("error", "224;108;117"),
                  ("select_bg", "62;68;81"), ("highlight", "229;192;123"), ("badge_text", "40;44;52"), ("background", "40;44;52"),
                  ("python", "229;192;123"), ("batch", "86;182;194"), ("cmd", "152;195;121"), ("powershell", "97;175;239"), ("vbscript", "198;120;221")]),
    ("rosepine", &[("accent", "196;167;231"), ("text", "224;222;244"), ("dim", "110;106;134"), ("ok", "156;207;216"), ("error", "235;111;146"),
                   ("select_bg", "64;61;82"), ("highlight", "246;193;119"), ("badge_text", "25;23;36"), ("background", "25;23;36"),
                   ("python", "246;193;119"), ("batch", "156;207;216"), ("cmd", "235;188;186"), ("powershell", "196;167;231"), ("vbscript", "235;111;146")]),
    ("matrix", &[("accent", "180;255;180"), ("text", "0;255;65"), ("dim", "0;130;40"), ("ok", "0;255;65"), ("error", "255;60;60"),
                 ("select_bg", "0;60;20"), ("highlight", "255;255;255"), ("badge_text", "0;0;0"), ("background", "0;0;0"),
                 ("python", "0;255;65"), ("batch", "0;200;80"), ("cmd", "0;160;60"), ("powershell", "120;255;160"), ("vbscript", "200;255;200")]),
    ("mono", &[("accent", "255;255;255"), ("text", "200;200;200"), ("dim", "128;128;128"), ("ok", "200;200;200"), ("error", "255;255;255"),
               ("select_bg", "60;60;60"), ("highlight", "255;255;255"), ("badge_text", "0;0;0"), ("background", ""),
               ("python", "220;220;220"), ("batch", "190;190;190"), ("cmd", "160;160;160"), ("powershell", "230;230;230"), ("vbscript", "140;140;140")]),
];

/// Monkeytype themes go after the built-in ones. A name that a built-in preset already uses gets "-mt" ("dracula-mt").
pub fn mt_name(name: &str) -> String {
    if PRESETS.iter().any(|p| p.0 == name) { format!("{name}-mt") } else { name.to_string() }
}

/// Names of every preset: built-in first, then the monkeytype themes.
pub fn all_preset_names() -> Vec<String> {
    PRESETS.iter().map(|p| p.0.to_string()).chain(crate::mt_themes::THEMES.iter().map(|t| mt_name(t.0))).collect()
}

/// Is this one of the monkeytype themes?
pub fn is_mt(name: &str) -> bool { crate::mt_themes::THEMES.iter().any(|t| mt_name(t.0) == name) }

/// A preset's colours as "r;g;b" strings: a built-in one, or a monkeytype theme mapped onto the app's colour keys.
fn preset_values(name: &str) -> Option<Vec<(String, String)>> {
    if let Some(p) = PRESETS.iter().find(|p| p.0 == name) {
        return Some(p.1.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
    }
    let (_, c) = crate::mt_themes::THEMES.iter().find(|t| mt_name(t.0) == name)?;
    let rgb = |v: u32| ((v >> 16) as u8, (v >> 8) as u8, v as u8);
    let s = |c: (u8, u8, u8)| format!("{};{};{}", c.0, c.1, c.2);
    let mix = |a: (u8, u8, u8), b: (u8, u8, u8), t: f32| {
        let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
        (m(a.0, b.0), m(a.1, b.1), m(a.2, b.2))
    };
    let (bg, main, caret, sub, sub_alt, text, error) = (rgb(c[0]), rgb(c[1]), rgb(c[2]), rgb(c[3]), rgb(c[4]), rgb(c[5]), rgb(c[6]));
    // the selection bar needs to stand out from the background
    let lum = |c: (u8, u8, u8)| (c.0 as i32 * 299 + c.1 as i32 * 587 + c.2 as i32 * 114) / 1000;
    let sel = if (lum(sub_alt) - lum(bg)).abs() < 18 { mix(bg, sub, 0.3) } else { sub_alt };
    let caret2 = if caret == main { text } else { caret };
    Some(vec![
        ("accent".into(), s(main)), ("text".into(), s(text)), ("dim".into(), s(sub)), ("ok".into(), s(main)), ("error".into(), s(error)),
        ("select_bg".into(), s(sel)), ("highlight".into(), s(caret2)), ("badge_text".into(), s(bg)), ("background".into(), s(bg)),
        ("python".into(), s(main)), ("batch".into(), s(caret2)), ("cmd".into(), s(text)), ("powershell".into(), s(mix(main, text, 0.5))), ("vbscript".into(), s(error)),
    ])
}

/// Resolved colours for drawing.
#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,
    pub accent: Color,
    pub text: Color,
    pub dim: Color,
    pub ok: Color,
    pub error: Color,
    pub select_bg: Color,
    pub highlight: Color,
    pub background: Option<Color>,
    /// syntax colours: keyword string comment number variable type attribute
    pub syn: [(u8, u8, u8); 7],
    pub text_rgb: (u8, u8, u8),
    pub bg_rgb: (u8, u8, u8),
    /// every resolved colour by name (python, batch, badge_text ... for the launcher)
    pub rgb: HashMap<String, (u8, u8, u8)>,
}

/// Parsed ini: section -> key -> value, plus section order.
#[derive(Default, Clone)]
pub struct Ini {
    pub sections: HashMap<String, HashMap<String, String>>,
    pub order: Vec<String>,
}

impl Ini {
    pub fn get(&self, sec: &str, key: &str) -> Option<&str> {
        self.sections.get(sec).and_then(|s| s.get(key)).map(|s| s.as_str()).filter(|s| !s.is_empty())
    }
}

pub fn parse_ini(text: &str) -> Ini {
    let mut ini = Ini::default();
    let mut sec = String::new();
    ini.sections.insert(sec.clone(), HashMap::new());
    for raw in text.lines() {
        let l = raw.trim().trim_start_matches('\u{feff}');
        if l.is_empty() || l.starts_with(';') { continue; }
        if l.starts_with('[') && l.ends_with(']') {
            sec = l[1..l.len() - 1].trim().to_lowercase();
            if !ini.sections.contains_key(&sec) {
                ini.sections.insert(sec.clone(), HashMap::new());
                ini.order.push(sec.clone());
            }
            continue;
        }
        if let Some(eq) = l.find('=') {
            let key = l[..eq].trim().to_lowercase();
            let mut val = l[eq + 1..].to_string();
            // strip " ; comment" (a ';' preceded by whitespace)
            let bytes: Vec<char> = val.chars().collect();
            for i in 1..bytes.len() {
                if bytes[i] == ';' && bytes[i - 1].is_whitespace() { val = bytes[..i].iter().collect(); break; }
            }
            let val = val.trim().trim_matches('"').to_string();
            ini.sections.get_mut(&sec).unwrap().insert(key, val);
        }
    }
    ini
}

/// "r,g,b", "r;g;b" or "#rrggbb" -> (r,g,b)
pub fn parse_rgb(v: &str) -> Option<(u8, u8, u8)> {
    let v = v.trim();
    let h = v.strip_prefix('#').unwrap_or(v);
    if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) {
        let n = u32::from_str_radix(h, 16).ok()?;
        return Some(((n >> 16) as u8, (n >> 8) as u8, n as u8));
    }
    let parts: Vec<&str> = v.split(|c| c == ',' || c == ';' || c == ' ').filter(|s| !s.is_empty()).collect();
    if parts.len() == 3 {
        let r = parts[0].parse::<u16>().ok()?; let g = parts[1].parse::<u16>().ok()?; let b = parts[2].parse::<u16>().ok()?;
        if r <= 255 && g <= 255 && b <= 255 { return Some((r as u8, g as u8, b as u8)); }
    }
    None
}

pub struct Config {
    pub file: Option<PathBuf>,
    pub default_file: PathBuf,
    pub ini: Ini,
    pub theme: Theme,
    pub warning: Option<String>,
    stamp: Option<SystemTime>,
}

fn candidates() -> Vec<PathBuf> {
    // next to the exe, then the folders above it (script\migration\file-browser.exe -> script\config.ini)
    let mut v = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        for _ in 0..3 {
            if let Some(d) = dir.clone() {
                v.push(d.join("config.ini"));
                v.push(d.join("theme.ini"));
                dir = d.parent().map(|p| p.to_path_buf());
            }
        }
    }
    v
}

impl Config {
    pub fn load() -> Config {
        let c = candidates();
        let default_file = c.iter().find(|p| p.file_name().map(|n| n == "config.ini").unwrap_or(false))
            .cloned().unwrap_or_else(|| PathBuf::from("config.ini"));
        // prefer config.ini one level up (shared with the PowerShell tools) if it exists
        let file = c.into_iter().find(|p| p.is_file());
        let mut cfg = Config { file, default_file, ini: Ini::default(), theme: resolve_theme(&Ini::default()).0, warning: None, stamp: None };
        cfg.reload();
        cfg
    }

    pub fn reload(&mut self) {
        self.warning = None;
        self.ini = Ini::default();
        if let Some(f) = &self.file {
            self.stamp = std::fs::metadata(f).and_then(|m| m.modified()).ok();
            match std::fs::read(f) {
                Ok(b) => self.ini = parse_ini(&crate::text::decode(&b).0),
                Err(e) => self.warning = Some(format!("config.ini could not be read: {e}")),
            }
        }
        let (t, w) = resolve_theme(&self.ini);
        self.theme = t;
        if w.is_some() { self.warning = w; }
    }

    /// true when config.ini changed on disk since the last load
    pub fn changed(&self) -> bool {
        match &self.file {
            Some(f) => std::fs::metadata(f).and_then(|m| m.modified()).ok() != self.stamp,
            None => false,
        }
    }

    /// The colours of any preset, without saving it (for previews).
    pub fn theme_for(&self, name: &str) -> Theme {
        let mut ini = self.ini.clone();
        ini.sections.entry("theme".to_string()).or_default().insert("preset".to_string(), name.to_string());
        resolve_theme(&ini).0
    }

    pub fn preset_names(&self) -> Vec<String> {
        let mut v: Vec<String> = all_preset_names();
        for s in &self.ini.order {
            if let Some(n) = s.strip_prefix("preset.") {
                if !n.is_empty() && !v.iter().any(|x| x == n) { v.push(n.to_string()); }
            }
        }
        v
    }

    pub fn cycle_theme(&mut self, step: i32) {
        let names = self.preset_names();
        let i = names.iter().position(|n| *n == self.theme.name).unwrap_or(0) as i32;
        let n = names.len() as i32;
        let next = names[(((i + step) % n) + n) as usize % n as usize].clone();
        if let Err(e) = self.set_value("theme", "preset", Some(&next)) {
            self.warning = Some(format!("Could not save config.ini: {e}"));
        }
        self.reload();
    }

    /// Write (or with None remove) one "key = value" line, keeping comments and layout.
    pub fn set_value(&mut self, section: &str, key: &str, value: Option<&str>) -> std::io::Result<()> {
        let path = self.file.clone().unwrap_or_else(|| self.default_file.clone());
        let text = std::fs::read(&path).map(|b| crate::text::decode(&b).0).unwrap_or_default();
        let mut lines: Vec<String> = text.lines().map(|s| s.to_string()).collect();
        let (mut sec, mut sec_idx, mut last_idx, mut done) = (String::new(), None::<usize>, None::<usize>, false);
        let mut i = 0;
        while i < lines.len() {
            let l = lines[i].trim().to_string();
            if l.starts_with('[') && l.ends_with(']') {
                sec = l[1..l.len() - 1].trim().to_lowercase();
                if sec == section { sec_idx = Some(i); last_idx = Some(i); }
                i += 1; continue;
            }
            if sec == section {
                if !l.is_empty() && !l.starts_with(';') { last_idx = Some(i); }
                if let Some(eq) = l.find('=') {
                    if l[..eq].trim().eq_ignore_ascii_case(key) {
                        match value { Some(v) => lines[i] = format!("{key} = {v}"), None => { lines.remove(i); } }
                        done = true; break;
                    }
                }
            }
            i += 1;
        }
        if !done {
            if let Some(v) = value {
                if sec_idx.is_some() { lines.insert(last_idx.unwrap() + 1, format!("{key} = {v}")); }
                else {
                    if lines.last().map(|l| !l.trim().is_empty()).unwrap_or(false) { lines.push(String::new()); }
                    lines.push(format!("[{section}]")); lines.push(format!("{key} = {v}"));
                }
            }
        }
        std::fs::write(&path, lines.join("\r\n") + "\r\n")?;
        self.file = Some(path);
        Ok(())
    }

    /// [bookmarks] n = path  (folders, files, apps; 1-9 also work with Ctrl+1..9)
    pub fn bookmarks(&self) -> Vec<(u32, String)> {
        let mut v = Vec::new();
        if let Some(s) = self.ini.sections.get("bookmarks") {
            for (k, p) in s {
                if let Ok(n) = k.parse::<u32>() { if n >= 1 && !p.is_empty() { v.push((n, p.clone())); } }
            }
        }
        v.sort();
        v
    }

    /// [editor] split = percent of the window for the editor (50-90)
    pub fn editor_split(&self) -> u16 {
        self.ini.get("editor", "split").and_then(|v| v.trim().split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse::<u16>().ok()))
            .unwrap_or(70).clamp(50, 90)
    }

    /// [editor] wrap = on | off
    pub fn editor_wrap(&self) -> bool {
        !matches!(self.ini.get("editor", "wrap").map(|v| v.trim().to_lowercase()).as_deref(), Some("off" | "no" | "false" | "0"))
    }

    pub fn image_mode(&self) -> String {
        self.ini.get("preview", "images").unwrap_or("auto").to_lowercase()
    }

    pub fn start_dir(&self) -> Option<PathBuf> {
        self.ini.get("browser", "start_dir").map(PathBuf::from)
    }

    pub fn label(&self) -> String {
        if self.file.is_some() { format!("theme: {}", self.theme.name) } else { format!("theme: {} (no config.ini)", self.theme.name) }
    }
}

fn rgb_color(v: &str) -> Option<Color> { parse_rgb(v).map(|(r, g, b)| Color::Rgb(r, g, b)) }

fn resolve_theme(ini: &Ini) -> (Theme, Option<String>) {
    let mut warn = None;
    let mut preset = ini.get("theme", "preset").unwrap_or("dark").to_lowercase();
    let custom_key = format!("preset.{preset}");
    let is_custom = ini.sections.contains_key(&custom_key);
    let builtin = |n: &str| preset_values(n);
    if !is_custom && builtin(&preset).is_none() {
        warn = Some(format!("config.ini: unknown preset '{preset}' - using dark"));
        preset = "dark".into();
    }
    let mut base = preset.clone();
    if is_custom {
        base = ini.get(&custom_key, "base").unwrap_or("dark").to_lowercase();
        if builtin(&base).is_none() { warn = Some(format!("config.ini [{custom_key}]: unknown base '{base}'")); base = "dark".into(); }
    }
    let mut m: HashMap<String, String> = HashMap::new();
    for (k, v) in builtin("dark").unwrap() { m.insert(k, v); }
    for (k, v) in builtin(&base).unwrap() { m.insert(k, v); }
    let mut merge = |sec: &str| {
        if let Some(s) = ini.sections.get(sec) {
            for (k, v) in s {
                if k == "base" || v.is_empty() { continue; }
                if k == "background" && matches!(v.to_lowercase().as_str(), "none" | "default" | "terminal") { m.insert(k.clone(), String::new()); continue; }
                match parse_rgb(v) {
                    Some((r, g, b)) => { m.insert(k.clone(), format!("{r};{g};{b}")); }
                    None => warn = Some(format!("config.ini [{sec}]: bad color '{k} = {v}'")),
                }
            }
        }
    };
    merge("colors");
    merge("badges");
    if is_custom { merge(&custom_key); }
    let c = |k: &str| rgb_color(m.get(k).map(|s| s.as_str()).unwrap_or("")).unwrap_or(Color::Reset);
    let t = |k: &str| parse_rgb(m.get(k).map(|s| s.as_str()).unwrap_or("")).unwrap_or((200, 200, 200));
    let pick = |own: &str, fallback: &str| {
        m.get(own).and_then(|v| parse_rgb(v)).unwrap_or_else(|| t(fallback))
    };
    let background = m.get("background").and_then(|v| rgb_color(v));
    let theme = Theme {
        name: preset,
        accent: c("accent"), text: c("text"), dim: c("dim"), ok: c("ok"), error: c("error"),
        select_bg: c("select_bg"), highlight: c("highlight"), background,
        syn: [
            pick("syntax_keyword", "accent"), pick("syntax_string", "ok"), pick("syntax_comment", "dim"),
            pick("syntax_number", "highlight"), pick("syntax_variable", "vbscript"), pick("syntax_type", "batch"),
            pick("syntax_attribute", "powershell"),
        ],
        text_rgb: t("text"),
        bg_rgb: m.get("background").and_then(|v| parse_rgb(v)).unwrap_or((12, 12, 12)),
        rgb: m.iter().filter_map(|(k, v)| parse_rgb(v).map(|c| (k.clone(), c))).collect(),
    };
    (theme, warn)
}

