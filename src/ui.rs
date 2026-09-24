//! Drawing. Colours follow delta's dark defaults so the diff reads the way it does there.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, DiffView, Item};
use crate::docs::{self, Kind as DocKind};
use crate::diff::Kind;
use crate::git::Status;
use crate::pr::PrStatus;

const MINUS_BG: Color = Color::Rgb(0x3f, 0x00, 0x01);
const MINUS_EMPH: Color = Color::Rgb(0x90, 0x10, 0x11);
const PLUS_BG: Color = Color::Rgb(0x00, 0x28, 0x00);
const PLUS_EMPH: Color = Color::Rgb(0x00, 0x60, 0x00);
const SELECTED_BG: Color = Color::Rgb(0x2e, 0x34, 0x40);
const DIM: Color = Color::Rgb(0x6c, 0x70, 0x86);
const TEXT: Color = Color::Rgb(0xd8, 0xd8, 0xd8);
const GREEN: Color = Color::Rgb(0x7f, 0xd9, 0x62);
const RED: Color = Color::Rgb(0xf0, 0x71, 0x78);
const YELLOW: Color = Color::Rgb(0xe6, 0xc0, 0x7b);
const BLUE: Color = Color::Rgb(0x73, 0xb8, 0xff);
const MAGENTA: Color = Color::Rgb(0xc6, 0x99, 0xe3);

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    if area.height < 3 {
        return;
    }
    if app.flash.as_ref().is_some_and(|(_, at)| at.elapsed().as_secs() >= 4) {
        app.flash = None;
    }
    let flash = app.flash.as_ref().map(|(msg, _)| Line::from(Span::styled(format!(" {msg}"), Style::new().fg(YELLOW))));
    let body = Rect { y: area.y + 1, height: area.height - 2, ..area };
    let footer = Rect { y: area.bottom() - 1, height: 1, ..area };
    if app.docs_view.is_some() {
        f.render_widget(Paragraph::new(docs_title(app, area.width)), Rect { height: 1, ..area });
        draw_docs(f, app, body);
        let hints = keys(&[("j/k", "muovi"), ("⏎", "apri"), ("y", "copia link"), ("esc", "lista")]);
        f.render_widget(Paragraph::new(flash.unwrap_or(hints)), footer);
    } else if let Some(view) = app.view.as_mut() {
        view.layout(body.width, body.height);
        let view = app.view.as_ref().unwrap();
        f.render_widget(Paragraph::new(diff_title(view, &review_badge(app), area.width)), Rect { height: 1, ..area });
        draw_diff(f, view, body);
        let hints = keys(&[
            ("j/k", "scorri"),
            ("n/N", "modifica"),
            ("f", if view.full { "solo hunk" } else { "file intero" }),
            ("w", "a capo"),
            ("[/]", "file"),
            ("p", "PR"),
            ("y", "copia path"),
            ("esc", "lista"),
        ]);
        f.render_widget(Paragraph::new(flash.unwrap_or(hints)), footer);
    } else {
        f.render_widget(Paragraph::new(list_title(app, area.width)), Rect { height: 1, ..area });
        // The documents row sits at the foot of the pane, whatever the list's length.
        app.docs_row_area = None;
        let list = if app.docs.is_some() && body.height > 2 {
            let row = Rect { y: body.bottom() - 1, height: 1, ..body };
            f.render_widget(Paragraph::new(docs_row(app, area.width)), row);
            app.docs_row_area = Some(row);
            Rect { height: body.height - 2, ..body }
        } else {
            body
        };
        app.list_area = list;
        draw_list(f, app, list);
        let hints = keys(&[("j/k", "muovi"), ("⏎", "apri"), ("p", "PR"), ("y", "copia path"), ("r", "aggiorna"), ("q", "esci")]);
        f.render_widget(Paragraph::new(flash.unwrap_or(hints)), footer);
    }
}

fn keys(pairs: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (k, what)) in pairs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::new().fg(DIM)));
        }
        spans.push(Span::styled(k.to_string(), Style::new().fg(BLUE)));
        spans.push(Span::styled(format!(" {what}"), Style::new().fg(DIM)));
    }
    Line::from(spans)
}

fn counts(added: Option<u32>, deleted: Option<u32>) -> Vec<Span<'static>> {
    match (added, deleted) {
        (None, _) | (_, None) => vec![Span::styled("bin", Style::new().fg(DIM))],
        (Some(a), Some(d)) => {
            let mut s = Vec::new();
            if a > 0 {
                s.push(Span::styled(format!("+{a}"), Style::new().fg(GREEN)));
            }
            if d > 0 {
                if a > 0 {
                    s.push(Span::raw(" "));
                }
                s.push(Span::styled(format!("−{d}"), Style::new().fg(RED)));
            }
            s
        }
    }
}

fn width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// Left and right parts on one line, the left one cut to make room.
fn split_line(mut left: Vec<Span<'static>>, right: Vec<Span<'static>>, total: u16, bg: Option<Color>) -> Line<'static> {
    let total = total as usize;
    let rw = width(&right);
    let room = total.saturating_sub(rw + 1);
    let mut used = 0;
    let mut cut = Vec::new();
    for span in left.drain(..) {
        let w = span.content.width();
        if used + w <= room {
            used += w;
            cut.push(span);
        } else {
            let mut s = String::new();
            for ch in span.content.chars() {
                let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
                if used + cw + 1 > room {
                    break;
                }
                s.push(ch);
                used += cw;
            }
            s.push('…');
            used += 1;
            cut.push(Span::styled(s, span.style));
            break;
        }
    }
    cut.push(Span::raw(" ".repeat(total.saturating_sub(used + rw))));
    cut.extend(right);
    let line = Line::from(cut);
    match bg {
        Some(bg) => line.style(Style::new().bg(bg)),
        None => line,
    }
}

/// The badge a review workspace carries on every screen, so it is never taken for a task's.
fn review_badge(app: &App) -> Vec<Span<'static>> {
    if !app.review {
        return Vec::new();
    }
    vec![Span::raw(" "), Span::styled(" REVIEW ", Style::new().fg(Color::Rgb(0, 0, 0)).bg(YELLOW).add_modifier(Modifier::BOLD))]
}

fn list_title(app: &App, w: u16) -> Line<'static> {
    let name = app.label.clone().unwrap_or_else(|| "vaglio".into());
    let n = app.file_count();
    let right = vec![Span::styled(format!("{n} file "), Style::new().fg(DIM))];
    let mut left = review_badge(app);
    left.push(Span::styled(format!(" {name}"), Style::new().fg(TEXT).add_modifier(Modifier::BOLD)));
    split_line(left, right, w, None)
}

/// `libs/ai-agents/…/deferral.py`, cut from the left so the file name always shows.
fn path_spans(path: &str, room: usize) -> Vec<Span<'static>> {
    let (dir, name) = match path.rfind('/') {
        Some(i) => (&path[..=i], &path[i + 1..]),
        None => ("", path),
    };
    let mut dir = dir.to_string();
    if dir.width() + name.width() > room {
        let keep = room.saturating_sub(name.width() + 1);
        let chars: Vec<char> = dir.chars().collect();
        let mut tail = String::new();
        for &c in chars.iter().rev() {
            if tail.width() + 1 > keep {
                break;
            }
            tail.insert(0, c);
        }
        dir = format!("…{tail}");
    }
    vec![Span::styled(dir, Style::new().fg(DIM)), Span::styled(name.to_string(), Style::new().fg(TEXT))]
}

fn draw_list(f: &mut Frame, app: &mut App, area: Rect) {
    if !app.loaded {
        f.render_widget(Paragraph::new(Span::styled(" leggo i worktree…", Style::new().fg(DIM))), area);
        return;
    }
    if app.groups.is_empty() {
        let msg = vec![
            Line::from(Span::styled(" nessun worktree in questo workspace", Style::new().fg(DIM))),
            Line::from(Span::styled(" compare appena Claude scrive in ~/Developer/compri-worktrees", Style::new().fg(DIM))),
        ];
        f.render_widget(Paragraph::new(msg), area);
        return;
    }
    let height = area.height as usize;
    if let Some(sel) = app.selected_index() {
        // Keep the group header visible when its first file is selected.
        let mut top = sel;
        while top > 0 && matches!(app.items[top - 1], Item::Header(_) | Item::Pr(_)) {
            top -= 1;
        }
        if top < app.list_scroll {
            app.list_scroll = top;
        } else if sel >= app.list_scroll + height {
            app.list_scroll = sel + 1 - height;
        }
    }
    app.list_scroll = app.list_scroll.min(app.items.len().saturating_sub(height));
    let selected = app.selected_index();
    let lines: Vec<Line> = app
        .items
        .iter()
        .enumerate()
        .skip(app.list_scroll)
        .take(height)
        .map(|(i, &item)| list_line(app, item, selected == Some(i), area.width))
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

fn list_line(app: &App, item: Item, selected: bool, w: u16) -> Line<'static> {
    match item {
        Item::Header(g) => {
            let group = &app.groups[g];
            match &group.tree {
                Ok(t) => {
                    let (a, d) = t.totals();
                    let own = app.current.as_ref() == Some(&t.root);
                    let mut left = vec![
                        Span::styled(if own { " ● " } else { "   " }, Style::new().fg(GREEN)),
                        Span::styled(t.repo.clone(), Style::new().fg(BLUE).add_modifier(Modifier::BOLD)),
                        Span::styled(format!("  {}", t.branch), Style::new().fg(MAGENTA).add_modifier(Modifier::BOLD)),
                    ];
                    if !t.base.ends_with("main") {
                        left.push(Span::styled(format!("  vs {}", t.base), Style::new().fg(DIM)));
                    }
                    let mut right = if t.files.is_empty() { Vec::new() } else { counts(Some(a), Some(d)) };
                    right.push(Span::raw(" "));
                    split_line(left, right, w, None)
                }
                Err(_) => Line::from(Span::styled(
                    format!(" {}", group.root.display()),
                    Style::new().fg(BLUE).add_modifier(Modifier::BOLD),
                )),
            }
        }
        Item::Pr(g) => pr_line(app.groups[g].pr.as_ref(), w),
        Item::Empty(g) => match &app.groups[g].tree {
            Ok(_) => Line::from(Span::styled("    nessuna modifica rispetto alla base", Style::new().fg(DIM))),
            Err(e) => Line::from(Span::styled(format!("    {e}"), Style::new().fg(RED))),
        },
        Item::File(..) => {
            let Some((_, file)) = app.file(item) else { return Line::default() };
            let color = match file.status {
                Status::Added | Status::Untracked => GREEN,
                Status::Deleted => RED,
                Status::Renamed => BLUE,
                Status::Modified => YELLOW,
            };
            let mut right = counts(file.added, file.deleted);
            right.push(Span::raw(" "));
            let room = (w as usize).saturating_sub(width(&right) + 6);
            let mut left = vec![
                Span::styled(if selected { " ›" } else { "  " }, Style::new().fg(BLUE)),
                Span::styled(format!(" {} ", file.status.letter()), Style::new().fg(color)),
            ];
            left.extend(path_spans(&file.path, room));
            split_line(left, right, w, selected.then_some(SELECTED_BG))
        }
    }
}

fn pr_line(status: Option<&PrStatus>, w: u16) -> Line<'static> {
    let dim = |t: &str| Line::from(Span::styled(format!("   {t}"), Style::new().fg(DIM)));
    match status {
        None => dim("PR …"),
        Some(PrStatus::NoHost) => dim("PR: origin non è su Bitbucket né su GitHub"),
        Some(PrStatus::OnBase) => dim("branch di integrazione: niente PR"),
        Some(PrStatus::NoCredentials) => dim("PR: nessun token Bitbucket (vedi README)"),
        Some(PrStatus::Error(e)) => Line::from(Span::styled(format!("   {e}"), Style::new().fg(RED))),
        Some(PrStatus::Missing { .. }) => Line::from(vec![
            Span::styled("   nessuna PR", Style::new().fg(RED).add_modifier(Modifier::BOLD)),
            Span::styled(" · p per crearla", Style::new().fg(DIM)),
        ]),
        Some(PrStatus::Found(pr)) => {
            let (badge, color) = match (pr.draft, pr.state.as_str()) {
                (true, "OPEN") => ("DRAFT", YELLOW),
                (_, "OPEN") => ("OPEN", GREEN),
                (_, "MERGED") => ("MERGED", MAGENTA),
                (_, other) => (if other == "CLOSED" { "CLOSED" } else { "DECLINED" }, RED),
            };
            split_line(
                vec![
                    Span::styled(format!("   #{} ", pr.id), Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
                    Span::styled(badge, Style::new().fg(color).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("  {}", pr.title), Style::new().fg(TEXT)),
                ],
                Vec::new(),
                w,
                None,
            )
        }
    }
}

fn docs_row(app: &App, w: u16) -> Line<'static> {
    let n = app.docs.as_ref().map_or(0, Vec::len);
    let selected = app.docs_selected();
    let left = vec![
        Span::styled(if selected { " ›" } else { "  " }, Style::new().fg(BLUE)),
        Span::styled(" design doc ", Style::new().fg(TEXT).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {n} "), Style::new().fg(if n > 0 { YELLOW } else { DIM }).add_modifier(Modifier::BOLD)),
    ];
    let right = if selected { vec![Span::styled("⏎ lista ", Style::new().fg(DIM))] } else { Vec::new() };
    split_line(left, right, w, selected.then_some(SELECTED_BG))
}

fn docs_title(app: &App, w: u16) -> Line<'static> {
    let n = app.docs.as_ref().map_or(0, Vec::len);
    split_line(
        vec![Span::styled(" design doc", Style::new().fg(TEXT).add_modifier(Modifier::BOLD))],
        vec![Span::styled(format!("{n} "), Style::new().fg(DIM))],
        w,
        None,
    )
}

fn draw_docs(f: &mut Frame, app: &mut App, area: Rect) {
    app.docs_area = area;
    app.docs_top = 0;
    let docs = app.docs.as_deref().unwrap_or_default();
    if docs.is_empty() {
        let msg = vec![
            Line::from(Span::styled(" nessun documento in questo workspace", Style::new().fg(DIM))),
            Line::from(Span::styled(" compare quando Claude pubblica un artifact, crea una pagina Notion", Style::new().fg(DIM))),
            Line::from(Span::styled(" o un Claude Doc, o scrive un file .md", Style::new().fg(DIM))),
        ];
        f.render_widget(Paragraph::new(msg), area);
        return;
    }
    let sel = app.docs_view.unwrap_or(0);
    let height = area.height as usize;
    let top = sel.saturating_sub(height.saturating_sub(1));
    app.docs_top = top;
    let docs = app.docs.as_deref().unwrap_or_default();
    let lines: Vec<Line> = docs
        .iter()
        .enumerate()
        .skip(top)
        .take(height)
        .map(|(i, doc)| {
            let selected = i == sel;
            let color = match doc.kind {
                DocKind::Artifact => BLUE,
                DocKind::Notion => TEXT,
                DocKind::ClaudeDoc => MAGENTA,
                DocKind::Markdown => YELLOW,
            };
            let left = vec![
                Span::styled(if selected { " ›" } else { "  " }, Style::new().fg(BLUE)),
                Span::styled(format!(" {:<8} ", doc.kind.label()), Style::new().fg(color)),
                Span::styled(doc.title.clone(), Style::new().fg(TEXT)),
            ];
            let right = vec![Span::styled(format!("{} ", docs::age(doc.at)), Style::new().fg(DIM))];
            split_line(left, right, area.width, selected.then_some(SELECTED_BG))
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}

fn diff_title(view: &DiffView, badge: &[Span<'static>], w: u16) -> Line<'static> {
    let mut left = badge.to_vec();
    left.push(Span::styled(format!(" {} ", view.file.status.letter()), Style::new().fg(YELLOW)));
    left.extend(path_spans(&view.file.path, (w as usize).saturating_sub(30)));
    let mut right = counts(view.file.added, view.file.deleted);
    let n = view.changes.len();
    if n > 0 {
        right.push(Span::styled(format!("  {}/{n}", view.change_position().max(1)), Style::new().fg(DIM)));
    }
    right.push(Span::raw(" "));
    split_line(left, right, w, None)
}

fn draw_diff(f: &mut Frame, view: &DiffView, area: Rect) {
    if let Some(e) = &view.error {
        f.render_widget(Paragraph::new(Span::styled(format!(" {e}"), Style::new().fg(RED))), area);
        return;
    }
    if view.rows.is_empty() {
        let msg = format!(" nessuna differenza con {}", view.base);
        f.render_widget(Paragraph::new(Span::styled(msg, Style::new().fg(DIM))), area);
        return;
    }
    let nw = view.number_width;
    let total = area.width as usize;
    let lines: Vec<Line> = view
        .visual
        .iter()
        .skip(view.scroll)
        .take(area.height as usize)
        .map(|vr| {
            let row = &view.rows[vr.row];
            if matches!(row.kind, Kind::Hunk | Kind::Note) {
                let text: String = row.segs.iter().map(|s| s.text.as_str()).collect();
                let label = if row.kind == Kind::Hunk { format!("{:┄<w$} {text}", "", w = nw * 2 + 2) } else { format!(" {text}") };
                return Line::from(Span::styled(label, Style::new().fg(BLUE).add_modifier(Modifier::DIM)));
            }
            let (bg, emph, num_fg) = match row.kind {
                Kind::Added => (Some(PLUS_BG), PLUS_EMPH, GREEN),
                Kind::Deleted => (Some(MINUS_BG), MINUS_EMPH, RED),
                _ => (None, Color::Reset, DIM),
            };
            let num = |n: Option<u32>| match (vr.first, n) {
                (true, Some(n)) => format!("{n:>nw$}"),
                _ => " ".repeat(nw),
            };
            let mut spans = vec![
                Span::styled(format!("{} {}", num(row.old), num(row.new)), Style::new().fg(num_fg)),
                Span::styled(if vr.first { " │ " } else { " ┆ " }, Style::new().fg(DIM)),
            ];
            let mut used = view.gutter_width();
            for seg in &vr.segs {
                let mut style = Style::new().fg(seg.fg.unwrap_or(TEXT));
                if let Some(bg) = bg {
                    style = style.bg(if seg.emph { emph } else { bg });
                }
                used += seg.text.width();
                spans.push(Span::styled(seg.text.clone(), style));
            }
            if let Some(bg) = bg {
                spans.push(Span::styled(" ".repeat(total.saturating_sub(used)), Style::new().bg(bg)));
            }
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), area);
}
