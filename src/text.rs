//! Text decoding and display helpers.

/// Decode file bytes: BOM (UTF-8 / UTF-16 LE / BE), valid UTF-8, otherwise Windows-1252.
/// Returns (text, encoding name).
pub fn decode(b: &[u8]) -> (String, &'static str) {
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return (String::from_utf8_lossy(&b[3..]).into_owned(), "UTF-8 BOM");
    }
    if b.starts_with(&[0xFF, 0xFE]) {
        let (s, _, _) = encoding_rs::UTF_16LE.decode(&b[2..]);
        return (s.into_owned(), "UTF-16 LE");
    }
    if b.starts_with(&[0xFE, 0xFF]) {
        let (s, _, _) = encoding_rs::UTF_16BE.decode(&b[2..]);
        return (s.into_owned(), "UTF-16 BE");
    }
    match std::str::from_utf8(b) {
        Ok(s) => (s.to_string(), "UTF-8"),
        Err(e) if e.error_len().is_none() && e.valid_up_to() + 4 >= b.len() => {
            // cut in the middle of a character at the end of a partial read
            (String::from_utf8_lossy(&b[..e.valid_up_to()]).into_owned(), "UTF-8")
        }
        Err(_) => {
            let (s, _, _) = encoding_rs::WINDOWS_1252.decode(b);
            (s.into_owned(), "ANSI")
        }
    }
}

/// A NUL byte in the first 8 KB (and no UTF-16 BOM) means binary.
pub fn looks_binary(b: &[u8]) -> bool {
    if b.starts_with(&[0xFF, 0xFE]) || b.starts_with(&[0xFE, 0xFF]) { return false; }
    b.iter().take(8192).any(|&c| c == 0)
}

/// Tabs to 4-column stops, control characters to spaces (so files can never inject escape codes).
pub fn clean_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut col = 0usize;
    for ch in s.chars() {
        match ch {
            '\t' => { let n = 4 - col % 4; for _ in 0..n { out.push(' '); } col += n; }
            c if (c as u32) < 32 || (0x7F..0xA0).contains(&(c as u32)) => { out.push(' '); col += 1; }
            c => { out.push(c); col += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0); }
        }
    }
    out
}

pub fn format_size(b: u64) -> String {
    const K: f64 = 1024.0;
    let f = b as f64;
    if f >= K * K * K { format!("{:.1} GB", f / (K * K * K)) }
    else if f >= K * K { format!("{:.1} MB", f / (K * K)) }
    else if f >= K { format!("{:.0} KB", f / K) }
    else { format!("{b} B") }
}
