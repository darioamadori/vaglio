//! A unified diff turned into rows ready to draw: line numbers on both sides, syntax colours,
//! and the words that changed inside a modified line.

use std::path::Path;
use std::sync::OnceLock;

use ratatui::style::Color;
use similar::{ChangeTag, TextDiff};
use syntect::easy::HighlightLines;
use syntect::highlighting::Theme;
use syntect::parsing::{SyntaxReference, SyntaxSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Context,
    Added,
    Deleted,
    /// The `@@` line opening a hunk, kept for its function context.
    Hunk,
    /// Anything git says instead of a diff ("Binary files differ").
    Note,
}

#[derive(Clone, Debug)]
pub struct Seg {
    pub text: String,
    pub fg: Option<Color>,
    /// Part of the words that changed in this line.
    pub emph: bool,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub kind: Kind,
    pub old: Option<u32>,
    pub new: Option<u32>,
    pub segs: Vec<Seg>,
}

const TAB: &str = "    ";
/// Past this, a line is probably minified or generated: colouring it is not worth the time.
const MAX_HIGHLIGHT_LINE: usize = 2_000;
const MAX_HIGHLIGHT_ROWS: usize = 30_000;

fn syntaxes() -> &'static SyntaxSet {
    static SET: OnceLock<SyntaxSet> = OnceLock::new();
    SET.get_or_init(two_face::syntax::extra_newlines)
}

fn theme() -> &'static Theme {
    static THEME: OnceLock<Theme> = OnceLock::new();
    THEME.get_or_init(|| two_face::theme::extra().get(two_face::theme::EmbeddedThemeName::MonokaiExtended).clone())
}

/// Loads the syntax set and theme ahead of the first diff, so opening one does not stall.
pub fn warm_up() {
    syntaxes();
    theme();
}

fn syntax_for(root: &Path, path: &str) -> &'static SyntaxReference {
    let set = syntaxes();
    set.find_syntax_for_file(root.join(path))
        .ok()
        .flatten()
        .or_else(|| Path::new(path).extension().and_then(|e| set.find_syntax_by_extension(&e.to_string_lossy())))
        .unwrap_or_else(|| set.find_syntax_plain_text())
}

struct Raw {
    kind: Kind,
    old: Option<u32>,
    new: Option<u32>,
    text: String,
    emph: Vec<(usize, usize)>,
    colors: Vec<(usize, usize, Color)>,
}

pub fn build(raw: &str, root: &Path, path: &str) -> Vec<Row> {
    let mut lines = parse(raw);
    mark_changed_words(&mut lines);
    if lines.len() <= MAX_HIGHLIGHT_ROWS {
        highlight(&mut lines, syntax_for(root, path));
    }
    lines.into_iter().map(into_row).collect()
}

fn parse(raw: &str) -> Vec<Raw> {
    let mut out = Vec::new();
    let (mut old, mut new) = (0u32, 0u32);
    let mut in_hunk = false;
    let line = |kind, old, new, text: &str| Raw {
        kind,
        old,
        new,
        text: text.trim_end_matches('\r').replace('\t', TAB),
        emph: Vec::new(),
        colors: Vec::new(),
    };
    for l in raw.lines() {
        if let Some(rest) = l.strip_prefix("@@") {
            // "@@ -12,7 +12,9 @@ def schedule(self):"
            let mut nums = rest.split_whitespace();
            let start = |s: Option<&str>| {
                s.and_then(|s| s[1..].split(',').next()).and_then(|n| n.parse().ok()).unwrap_or(1)
            };
            old = start(nums.next());
            new = start(nums.next());
            let context = rest.splitn(2, "@@").nth(1).unwrap_or("").trim();
            out.push(line(Kind::Hunk, None, None, context));
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            if l.starts_with("Binary files") {
                out.push(line(Kind::Note, None, None, "file binario: nessun diff testuale"));
            }
            continue;
        }
        match l.as_bytes().first() {
            Some(b'+') => {
                out.push(line(Kind::Added, None, Some(new), &l[1..]));
                new += 1;
            }
            Some(b'-') => {
                out.push(line(Kind::Deleted, Some(old), None, &l[1..]));
                old += 1;
            }
            Some(b' ') | None => {
                out.push(line(Kind::Context, Some(old), Some(new), l.get(1..).unwrap_or("")));
                old += 1;
                new += 1;
            }
            // "\ No newline at end of file", or the next file's header.
            _ => {}
        }
    }
    out
}

/// Pairs each deleted line with the added line (after it, in order) that resembles it most,
/// and marks the words that differ. An added line with no deleted twin stays plain green.
fn mark_changed_words(lines: &mut [Raw]) {
    let mut i = 0;
    while i < lines.len() {
        if lines[i].kind != Kind::Deleted {
            i += 1;
            continue;
        }
        let del_start = i;
        while i < lines.len() && lines[i].kind == Kind::Deleted {
            i += 1;
        }
        let add_start = i;
        while i < lines.len() && lines[i].kind == Kind::Added {
            i += 1;
        }
        let mut next_add = add_start;
        for d in del_start..add_start {
            let twin = (next_add..i).find(|&a| TextDiff::from_chars(&lines[d].text, &lines[a].text).ratio() >= 0.5);
            if let Some(a) = twin {
                let (old_emph, new_emph) = changed_words(&lines[d].text, &lines[a].text);
                lines[d].emph = old_emph;
                lines[a].emph = new_emph;
                next_add = a + 1;
            }
        }
    }
}

fn changed_words(old: &str, new: &str) -> (Vec<(usize, usize)>, Vec<(usize, usize)>) {
    let diff = TextDiff::from_words(old, new);
    let (mut o, mut n) = (0, 0);
    let (mut old_emph, mut new_emph) = (Vec::new(), Vec::new());
    for change in diff.iter_all_changes() {
        let len = change.value().len();
        match change.tag() {
            ChangeTag::Equal => {
                o += len;
                n += len;
            }
            ChangeTag::Delete => {
                old_emph.push((o, o + len));
                o += len;
            }
            ChangeTag::Insert => {
                new_emph.push((n, n + len));
                n += len;
            }
        }
    }
    let changed: usize = old_emph.iter().chain(&new_emph).map(|(a, b)| b - a).sum();
    // Two mostly different lines read better as plain red and green than as a wall of emphasis.
    if changed * 10 > (old.len() + new.len()) * 6 {
        return (Vec::new(), Vec::new());
    }
    (old_emph, new_emph)
}

/// Each hunk is coloured as two streams, the old file and the new one, so that a deleted line
/// never skews the parser state of the lines around it.
fn highlight(lines: &mut [Raw], syntax: &SyntaxReference) {
    let set = syntaxes();
    let mut old_hl = HighlightLines::new(syntax, theme());
    let mut new_hl = HighlightLines::new(syntax, theme());
    for line in lines.iter_mut() {
        let (main, other): (&mut HighlightLines, Option<&mut HighlightLines>) = match line.kind {
            Kind::Hunk => {
                old_hl = HighlightLines::new(syntax, theme());
                new_hl = HighlightLines::new(syntax, theme());
                continue;
            }
            Kind::Note => continue,
            Kind::Added => (&mut new_hl, None),
            Kind::Deleted => (&mut old_hl, None),
            Kind::Context => (&mut new_hl, Some(&mut old_hl)),
        };
        if line.text.len() > MAX_HIGHLIGHT_LINE {
            continue;
        }
        let text = format!("{}\n", line.text);
        if let Some(other) = other {
            let _ = other.highlight_line(&text, set);
        }
        let Ok(ranges) = main.highlight_line(&text, set) else { continue };
        let mut colors = Vec::new();
        let mut at = 0;
        for (style, piece) in ranges {
            let end = (at + piece.len()).min(line.text.len());
            if end > at {
                let c = style.foreground;
                colors.push((at, end, Color::Rgb(c.r, c.g, c.b)));
            }
            at += piece.len();
        }
        line.colors = colors;
    }
}

/// Cuts the line wherever its colour or its emphasis changes.
fn into_row(line: Raw) -> Row {
    let text = &line.text;
    let mut cuts: Vec<usize> = vec![0, text.len()];
    for &(a, b) in &line.emph {
        cuts.extend([a, b]);
    }
    for &(a, b, _) in &line.colors {
        cuts.extend([a, b]);
    }
    cuts.retain(|&c| c <= text.len() && text.is_char_boundary(c));
    cuts.sort_unstable();
    cuts.dedup();
    let segs = cuts
        .windows(2)
        .map(|w| {
            let (a, b) = (w[0], w[1]);
            Seg {
                text: text[a..b].to_string(),
                fg: line.colors.iter().find(|&&(s, e, _)| s <= a && b <= e).map(|&(_, _, c)| c),
                emph: line.emph.iter().any(|&(s, e)| s <= a && b <= e),
            }
        })
        .collect();
    Row { kind: line.kind, old: line.old, new: line.new, segs }
}
