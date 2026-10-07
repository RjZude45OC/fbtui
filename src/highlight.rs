//! Syntax highlighting with syntect (Sublime Text grammars), coloured from the active theme.

use crate::config::Theme;
use crate::text::clean_line;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::path::Path;
use std::str::FromStr;
use std::sync::OnceLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color as SColor, ScopeSelectors, StyleModifier, Theme as SynTheme, ThemeItem, ThemeSettings};
use syntect::parsing::{SyntaxDefinition, SyntaxSet};

const HIGHLIGHT_LINES: usize = 5000;   // colour this many lines, the rest stays plain
const LONG_LINE: usize = 2000;         // minified one-liners stay plain (too slow to parse)

const POWERSHELL: &str = r#"%YAML 1.2
---
name: PowerShell
file_extensions: [ps1, psm1, psd1]
scope: source.powershell
contexts:
  main:
    - match: '<#'
      push: block_comment
    - match: '#.*$'
      scope: comment.line.powershell
    - match: '@"\s*$'
      push: here_d
    - match: "@'\\s*$"
      push: here_s
    - match: '"'
      push: dstring
    - match: "'"
      push: sstring
    - match: '\$\{[^}]*\}|\$(?:[A-Za-z_][\w]*:)?[\w?]+'
      scope: variable.other.powershell
    - match: '(?i)\b(begin|break|catch|class|continue|data|do|dynamicparam|else|elseif|end|enum|exit|filter|finally|for|foreach|from|function|if|in|param|process|return|switch|throw|trap|try|until|using|while)\b'
      scope: keyword.control.powershell
    - match: '(?i)-(eq|ne|gt|ge|lt|le|like|notlike|match|notmatch|contains|notcontains|in|notin|replace|split|join|and|or|not|xor|is|isnot|as|f|band|bor)\b'
      scope: keyword.operator.powershell
    - match: '(?<![\w-])-[A-Za-z]\w*'
      scope: variable.parameter.powershell
    - match: '\b[A-Za-z]+-[A-Za-z]\w*\b'
      scope: support.function.powershell
    - match: '\[[\w.\[\]]+\]'
      scope: storage.type.powershell
    - match: '\b(0x[0-9a-fA-F]+|\d+(\.\d+)?)(kb|mb|gb|KB|MB|GB)?\b'
      scope: constant.numeric.powershell
  block_comment:
    - meta_scope: comment.block.powershell
    - match: '#>'
      pop: true
  dstring:
    - meta_scope: string.quoted.double.powershell
    - match: '`.'
    - match: '\$\{[^}]*\}|\$(?:[A-Za-z_][\w]*:)?[\w?]+'
      scope: variable.other.powershell
    - match: '"'
      pop: true
  sstring:
    - meta_scope: string.quoted.single.powershell
    - match: "''"
    - match: "'"
      pop: true
  here_d:
    - meta_scope: string.quoted.double.heredoc.powershell
    - match: '^"@'
      pop: true
  here_s:
    - meta_scope: string.quoted.single.heredoc.powershell
    - match: "^'@"
      pop: true
"#;

const INI: &str = r#"%YAML 1.2
---
name: INI
file_extensions: [ini, cfg, conf, inf, reg, properties, toml, env, editorconfig, gitconfig, desktop]
scope: source.ini
contexts:
  main:
    - match: '^\s*[;#].*$'
      scope: comment.line.ini
    - match: '^\s*\[[^\]]*\]'
      scope: entity.name.section.ini
    - match: '^\s*([^=:\s][^=:]*?)\s*(?=[=:])'
      captures:
        1: entity.other.attribute-name.ini
    - match: '"[^"]*"|''[^'']*'''
      scope: string.quoted.ini
    - match: '(?<=\s)[;#].*$'
      scope: comment.line.ini
    - match: '\b(true|false|yes|no|on|off)\b'
      scope: constant.language.ini
    - match: '\b\d+(\.\d+)?\b'
      scope: constant.numeric.ini
"#;

fn syntaxes() -> &'static SyntaxSet {
    static SS: OnceLock<SyntaxSet> = OnceLock::new();
    SS.get_or_init(|| {
        let mut b = SyntaxSet::load_defaults_nonewlines().into_builder();
        for src in [POWERSHELL, INI] {
            if let Ok(d) = SyntaxDefinition::load_from_str(src, false, None) { b.add(d); }
        }
        b.build()
    })
}

fn sc((r, g, b): (u8, u8, u8)) -> SColor { SColor { r, g, b, a: 255 } }

fn syn_theme(t: &Theme) -> SynTheme {
    let item = |sel: &str, c: (u8, u8, u8)| ThemeItem {
        scope: ScopeSelectors::from_str(sel).unwrap_or_default(),
        style: StyleModifier { foreground: Some(sc(c)), background: None, font_style: None },
    };
    let [kw, st, cm, num, var, ty, attr] = t.syn;
    SynTheme {
        name: Some(t.name.clone()),
        author: None,
        settings: ThemeSettings { foreground: Some(sc(t.text_rgb)), background: Some(sc(t.bg_rgb)), ..Default::default() },
        scopes: vec![
            item("keyword, storage, keyword.operator.word, markup.heading, meta.tag.sgml.doctype", kw),
            item("string, punctuation.definition.string, markup.raw", st),
            item("constant.numeric, constant.language, constant.character, constant.other", num),
            item("variable.other.powershell, variable.parameter, variable.language, variable.other.readwrite.shell, punctuation.definition.variable, variable.other.normal.shell, variable.other.bracket.shell, variable.other.dosbatch", var),
            item("entity.name, support.function, support.class, entity.name.tag, entity.name.section, storage.type.powershell, entity.name.type", ty),
            item("entity.other.attribute-name, support.type.property-name, meta.mapping.key string, string.unquoted.key", attr),
            item("comment, punctuation.definition.comment", cm),
        ],
    }
}

fn syntax_for(path: &Path, first: Option<&str>) -> Option<&'static syntect::parsing::SyntaxReference> {
    let ss = syntaxes();
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    ss.find_syntax_by_extension(&ext)
        .or_else(|| match ext.as_str() {
            "cmd" => ss.find_syntax_by_name("Batch File"),
            "jsonc" => ss.find_syntax_by_extension("json"),
            "ts" | "tsx" | "mjs" | "cjs" => ss.find_syntax_by_extension("js"),
            "csproj" | "vbproj" | "props" | "targets" | "config" | "xaml" | "resx" | "nuspec" | "manifest" | "svg" => ss.find_syntax_by_extension("xml"),
            "kt" | "kts" | "dart" | "swift" => ss.find_syntax_by_extension("java"),
            "yml" => ss.find_syntax_by_extension("yaml"),
            _ => None,
        })
        .or_else(|| first.and_then(|l| ss.find_syntax_by_first_line(l)))
}

pub fn highlight(path: &Path, raw: &[&str], t: &Theme) -> Vec<Line<'static>> {
    let plain = Style::default().fg(t.text);
    let ss = syntaxes();
    let syntax = syntax_for(path, raw.first().copied());
    let Some(syntax) = syntax else {
        return raw.iter().map(|l| Line::styled(clean_line(l), plain)).collect();
    };
    let theme = syn_theme(t);
    let mut h = HighlightLines::new(syntax, &theme);
    let mut out = Vec::with_capacity(raw.len());
    for (i, l) in raw.iter().enumerate() {
        if i >= HIGHLIGHT_LINES || l.len() > LONG_LINE {
            out.push(Line::styled(clean_line(l), plain));
            continue;
        }
        let c = clean_line(l);                       // tabs expanded first so columns stay right
        match h.highlight_line(&c, ss) {
            Ok(parts) => {
                let spans: Vec<Span<'static>> = parts.into_iter().map(|(st, s)| {
                    let f = st.foreground;
                    Span::styled(s.to_string(), Style::default().fg(Color::Rgb(f.r, f.g, f.b)))
                }).collect();
                out.push(Line::from(spans));
            }
            Err(_) => out.push(Line::styled(clean_line(l), plain)),
        }
    }
    out
}

/// Incremental highlighter for the editor: remembers the parser state at the start of every line,
/// so after an edit only the changed line and the ones below it are parsed again, and only when shown.
pub struct LineHighlighter {
    syntax: Option<&'static syntect::parsing::SyntaxReference>,
    theme: SynTheme,
    plain: (u8, u8, u8),
    states: Vec<(syntect::parsing::ParseState, syntect::highlighting::HighlightState)>,  // states[i] = before line i
    cache: Vec<Option<Vec<((u8, u8, u8), String)>>>,
}

impl LineHighlighter {
    pub fn new(path: &Path, first: Option<&str>, t: &Theme, n_lines: usize) -> LineHighlighter {
        let syntax = if n_lines > 200_000 { None } else { syntax_for(path, first) };
        LineHighlighter { syntax, theme: syn_theme(t), plain: t.text_rgb, states: Vec::new(), cache: Vec::new() }
    }

    pub fn set_theme(&mut self, t: &Theme) { self.theme = syn_theme(t); self.plain = t.text_rgb; self.invalidate(0); }

    /// Lines from `row` down changed.
    pub fn invalidate(&mut self, row: usize) {
        self.states.truncate(row + 1);
        self.cache.truncate(row);
    }

    fn parse_one(&self, st: &(syntect::parsing::ParseState, syntect::highlighting::HighlightState), text: &str)
        -> (Vec<((u8, u8, u8), String)>, (syntect::parsing::ParseState, syntect::highlighting::HighlightState)) {
        let hl = syntect::highlighting::Highlighter::new(&self.theme);
        let (mut ps, mut hs) = st.clone();
        let c = clean_line(text);
        if c.len() > LONG_LINE { return (vec![(self.plain, c)], (ps, hs)); }
        let pieces = match ps.parse_line(&c, syntaxes()) {
            Ok(ops) => syntect::highlighting::HighlightIterator::new(&mut hs, &ops, &c, &hl)
                .map(|(st, s)| ((st.foreground.r, st.foreground.g, st.foreground.b), s.to_string())).collect(),
            Err(_) => vec![(self.plain, c)],
        };
        (pieces, (ps, hs))
    }

    fn store(&mut self, i: usize, pieces: Vec<((u8, u8, u8), String)>) {
        if self.cache.len() <= i { self.cache.resize(i + 1, None); }
        self.cache[i] = Some(pieces);
    }

    /// Coloured pieces of line `i` (tabs already expanded to spaces).
    pub fn line(&mut self, lines: &[String], i: usize) -> Vec<((u8, u8, u8), String)> {
        if let Some(Some(c)) = self.cache.get(i) { return c.clone(); }
        let Some(syntax) = self.syntax else { return vec![(self.plain, clean_line(&lines[i]))] };
        if self.states.is_empty() {
            let hl = syntect::highlighting::Highlighter::new(&self.theme);
            self.states.push((syntect::parsing::ParseState::new(syntax),
                syntect::highlighting::HighlightState::new(&hl, syntect::parsing::ScopeStack::new())));
        }
        // parse forward from the last known state up to line i
        while self.states.len() <= i {
            let k = self.states.len() - 1;
            let (pieces, next) = self.parse_one(&self.states[k], &lines[k]);
            self.store(k, pieces);
            self.states.push(next);
        }
        if let Some(Some(c)) = self.cache.get(i) { return c.clone(); }
        let (pieces, next) = self.parse_one(&self.states[i], &lines[i]);
        if self.states.len() == i + 1 { self.states.push(next); }
        self.store(i, pieces.clone());
        pieces
    }
}
