//! Text documents for the editor: load (encoding, line endings), save, and display-column helpers.
//! Columns: tabs stop every 4 columns, wide (CJK) characters take 2, control characters 1.

use unicode_width::UnicodeWidthChar;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Enc { Utf8, Utf8Bom, Utf16Le, Utf16Be, Ansi }

impl Enc {
    pub fn name(self) -> &'static str {
        match self { Enc::Utf8 => "UTF-8", Enc::Utf8Bom => "UTF-8 BOM", Enc::Utf16Le => "UTF-16 LE", Enc::Utf16Be => "UTF-16 BE", Enc::Ansi => "ANSI" }
    }
}

pub struct Doc {
    pub lines: Vec<String>,
    pub eol: &'static str,
    pub end_eol: bool,
    pub enc: Enc,
    pub binary: bool,
}

pub fn load(b: &[u8]) -> Doc {
    let binary = crate::text::looks_binary(b);
    let (text, enc) = if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        (String::from_utf8_lossy(&b[3..]).into_owned(), Enc::Utf8Bom)
    } else if b.starts_with(&[0xFF, 0xFE]) {
        (encoding_rs::UTF_16LE.decode(&b[2..]).0.into_owned(), Enc::Utf16Le)
    } else if b.starts_with(&[0xFE, 0xFF]) {
        (encoding_rs::UTF_16BE.decode(&b[2..]).0.into_owned(), Enc::Utf16Be)
    } else {
        match std::str::from_utf8(b) {
            Ok(s) => (s.to_string(), Enc::Utf8),
            Err(e) if e.error_len().is_none() && e.valid_up_to() + 4 >= b.len() => (String::from_utf8_lossy(&b[..e.valid_up_to()]).into_owned(), Enc::Utf8),
            Err(_) => (encoding_rs::WINDOWS_1252.decode(b).0.into_owned(), Enc::Ansi),
        }
    };
    let eol = if text.contains("\r\n") { "\r\n" } else if text.contains('\n') { "\n" } else if text.contains('\r') { "\r" } else { "\r\n" };
    let end_eol = text.ends_with('\n') || text.ends_with('\r');
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\r' => { if it.peek() == Some(&'\n') { it.next(); } lines.push(std::mem::take(&mut cur)); }
            '\n' => lines.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    if !cur.is_empty() || !end_eol || lines.is_empty() { lines.push(cur); }
    Doc { lines, eol, end_eol, enc, binary }
}

pub fn encode(text: &str, enc: Enc) -> Vec<u8> {
    match enc {
        Enc::Utf8 => text.as_bytes().to_vec(),
        Enc::Utf8Bom => { let mut v = vec![0xEF, 0xBB, 0xBF]; v.extend_from_slice(text.as_bytes()); v }
        Enc::Utf16Le => { let mut v = vec![0xFF, 0xFE]; for u in text.encode_utf16() { v.extend_from_slice(&u.to_le_bytes()); } v }
        Enc::Utf16Be => { let mut v = vec![0xFE, 0xFF]; for u in text.encode_utf16() { v.extend_from_slice(&u.to_be_bytes()); } v }
        Enc::Ansi => encoding_rs::WINDOWS_1252.encode(text).0.into_owned(),
    }
}

// ------------------------------------------------------------------ columns

pub fn char_w(c: char, col: usize) -> usize {
    match c {
        '\t' => 4 - col % 4,
        c if (c as u32) < 32 || (0x7F..0xA0).contains(&(c as u32)) => 1,
        c => UnicodeWidthChar::width(c).unwrap_or(0),
    }
}

pub fn nchars(s: &str) -> usize { s.chars().count() }

/// Byte offset of char index `ci` (clamped).
pub fn byte_at(s: &str, ci: usize) -> usize { s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len()) }

/// Display column of char index `ci`.
pub fn disp_col(s: &str, ci: usize) -> usize {
    let mut col = 0;
    for (i, c) in s.chars().enumerate() {
        if i >= ci { break; }
        col += char_w(c, col);
    }
    col
}

/// Char index at display column `target` (the character covering it, or the end).
pub fn index_at_col(s: &str, target: usize) -> usize {
    let mut col = 0;
    for (i, c) in s.chars().enumerate() {
        let w = char_w(c, col);
        if col + w > target { return if target - col > w / 2 && w > 1 { i + 1 } else { i }; }
        col += w;
    }
    nchars(s)
}

pub fn line_width(s: &str) -> usize { disp_col(s, usize::MAX) }

/// Display columns where each visual row of a wrapped line starts (breaks after spaces when possible).
pub fn wrap_starts(s: &str, width: usize) -> Vec<usize> {
    let width = width.max(4);
    let mut starts = vec![0usize];
    let mut col = 0usize;          // absolute display column
    let mut row_start = 0usize;
    let mut last_break: Option<usize> = None;   // column just after the last space in this row
    for c in s.chars() {
        let w = char_w(c, col);
        if col + w - row_start > width && col > row_start {
            let at = match last_break { Some(b) if b > row_start => b, _ => col };
            starts.push(at);
            row_start = at;
            last_break = None;
        }
        col += w;
        if c == ' ' || c == '\t' { last_break = Some(col); }
    }
    starts
}

/// Which visual row (index into `starts`) contains display column `dc`.
pub fn seg_of(starts: &[usize], dc: usize) -> usize {
    let mut k = 0;
    for (j, &s) in starts.iter().enumerate().skip(1) { if s <= dc { k = j } else { break } }
    k
}

fn cls(c: char) -> u8 { if c.is_whitespace() { 0 } else if c.is_alphanumeric() || c == '_' { 1 } else { 2 } }

pub fn word_right(s: &str, i: usize) -> usize {
    let v: Vec<char> = s.chars().collect();
    let n = v.len();
    let mut i = i.min(n);
    if i >= n { return n; }
    let k = cls(v[i]);
    if k != 0 { while i < n && cls(v[i]) == k { i += 1; } }
    while i < n && cls(v[i]) == 0 { i += 1; }
    i
}

pub fn word_left(s: &str, i: usize) -> usize {
    let v: Vec<char> = s.chars().collect();
    let mut i = i.min(v.len());
    if i == 0 { return 0; }
    i -= 1;
    while i > 0 && cls(v[i]) == 0 { i -= 1; }
    let k = cls(v[i]);
    while i > 0 && cls(v[i - 1]) == k { i -= 1; }
    i
}

/// Case-insensitive find of `q` in `s` starting at char index `from`; returns char index.
pub fn find_ci(s: &str, q: &str, from: usize) -> Option<usize> {
    let hay: Vec<char> = s.chars().flat_map(|c| c.to_lowercase().next()).collect();
    let needle: Vec<char> = q.chars().flat_map(|c| c.to_lowercase().next()).collect();
    if needle.is_empty() || needle.len() > hay.len() { return None; }
    (from..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == needle[..])
}

/// Last case-insensitive match starting at or before char index `upto`.
pub fn rfind_ci(s: &str, q: &str, upto: usize) -> Option<usize> {
    let hay: Vec<char> = s.chars().flat_map(|c| c.to_lowercase().next()).collect();
    let needle: Vec<char> = q.chars().flat_map(|c| c.to_lowercase().next()).collect();
    if needle.is_empty() || needle.len() > hay.len() { return None; }
    let max = (hay.len() - needle.len()).min(upto);
    (0..=max).rev().find(|&i| hay[i..i + needle.len()] == needle[..])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let d = load(b"a\r\nb\r\n");
        assert_eq!(d.lines, vec!["a", "b"]);
        assert!(d.end_eol);
        assert_eq!(d.eol, "\r\n");
        let d = load(b"x\ny");
        assert_eq!(d.lines, vec!["x", "y"]);
        assert!(!d.end_eol);
        let d = load(b"");
        assert_eq!(d.lines, vec![""]);
    }
    #[test]
    fn wrap() {
        assert_eq!(wrap_starts("hello world foo", 8), vec![0, 6, 12]);
        assert_eq!(wrap_starts("abcdefghij", 4), vec![0, 4, 8]);
        assert_eq!(disp_col("\tab", 1), 4);
        assert_eq!(index_at_col("\tab", 4), 1);
        assert_eq!(find_ci("Hello World", "world", 0), Some(6));
        assert_eq!(rfind_ci("abcabc", "ABC", 5), Some(3));
        assert_eq!(word_right("foo bar", 0), 4);
        assert_eq!(word_left("foo bar", 7), 4);
    }
}
