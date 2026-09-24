//! vaglio: the files a task changed, per worktree, against the branch they will merge into.

mod app;
mod diff;
mod git;
mod source;
mod ui;
mod worker;

use std::sync::mpsc;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use app::App;
use source::Source;

const HELP: &str = "vaglio [PATH...]

Mostra i file che ogni worktree ha cambiato rispetto al branch di integrazione
(origin/develop se esiste, altrimenti origin/main), con il diff di ciascuno.

Senza argomenti, dentro herdr segue il workspace (lista scritta dal plugin
compri-layout); fuori da herdr mostra il repo della directory corrente.";

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{HELP}");
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "--snapshot") {
        return snapshot(&args[1..]);
    }
    let source = Source::from_env(args);

    let (snap_tx, snap_rx) = mpsc::channel();
    let (poke_tx, poke_rx) = mpsc::channel();
    worker::spawn(source, snap_tx, poke_tx.clone(), poke_rx);
    std::thread::spawn(diff::warm_up);

    let mut terminal = ratatui::init();
    let mut app = App::new();
    let result = (|| -> anyhow::Result<()> {
        loop {
            while let Ok(snap) = snap_rx.try_recv() {
                app.apply(snap);
            }
            terminal.draw(|f| ui::draw(f, &mut app))?;
            if !event::poll(Duration::from_millis(100))? {
                continue;
            }
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Release && !handle(&mut app, key, terminal.size()?.height, &poke_tx) {
                    return Ok(());
                }
            }
        }
    })();
    ratatui::restore();
    result
}

/// Returns false to quit.
fn handle(app: &mut App, key: KeyEvent, height: u16, poke: &mpsc::Sender<()>) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && key.code == KeyCode::Char('c') {
        return false;
    }
    let page = height.saturating_sub(2).max(1) as isize;
    if let Some(view) = app.view.as_mut() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => app.view = None,
            KeyCode::Char('j') | KeyCode::Down => view.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => view.scroll_by(-1),
            KeyCode::Char('d') if ctrl => view.scroll_by(page / 2),
            KeyCode::Char('u') if ctrl => view.scroll_by(-page / 2),
            KeyCode::Char(' ') | KeyCode::PageDown => view.scroll_by(page),
            KeyCode::Char('b') | KeyCode::PageUp => view.scroll_by(-page),
            KeyCode::Char('g') | KeyCode::Home => view.scroll = 0,
            KeyCode::Char('G') | KeyCode::End => view.scroll = usize::MAX,
            KeyCode::Char('n') => view.jump_change(true),
            KeyCode::Char('N') => view.jump_change(false),
            KeyCode::Char('w') => view.toggle_wrap(),
            KeyCode::Char('f') => app.toggle_full(),
            KeyCode::Char(']') | KeyCode::Char('J') => app.step_file(1),
            KeyCode::Char('[') | KeyCode::Char('K') => app.step_file(-1),
            _ => {}
        }
        return true;
    }
    match key.code {
        KeyCode::Char('q') => return false,
        KeyCode::Char('j') | KeyCode::Down => app.move_by(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_by(-1),
        KeyCode::Char('d') if ctrl => app.move_by(page / 2),
        KeyCode::Char('u') if ctrl => app.move_by(-page / 2),
        KeyCode::Char('g') | KeyCode::Home => app.select_edge(false),
        KeyCode::Char('G') | KeyCode::End => app.select_edge(true),
        KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => app.open(),
        KeyCode::Char('r') => {
            let _ = poke.send(());
        }
        _ => {}
    }
    true
}

/// `vaglio --snapshot 100x30 KEYS [PATH...]`: draws one frame after the keys into a test backend
/// and prints it with ANSI colours. For checking the drawing without a terminal.
fn snapshot(args: &[String]) -> anyhow::Result<()> {
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    let (size, keys) = (args.first().map_or("100x30", String::as_str), args.get(1).cloned().unwrap_or_default());
    let (w, h) = size.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))).unwrap_or((100, 30));
    let source = Source::from_env(args.iter().skip(2).cloned().collect());
    let groups = source
        .roots()
        .into_iter()
        .map(|root| app::Group { tree: git::load(&root).map_err(|e| e.to_string()), root })
        .collect();
    let mut app = App::new();
    app.apply(app::Snapshot { label: source.label(), groups });
    let mut terminal = ratatui::Terminal::new(TestBackend::new(w, h))?;
    let (tx, _rx) = mpsc::channel();
    terminal.draw(|f| ui::draw(f, &mut app))?;
    for ch in keys.chars() {
        let code = match ch {
            '\n' => KeyCode::Enter,
            '~' => KeyCode::Esc,
            c => KeyCode::Char(c),
        };
        handle(&mut app, KeyEvent::new(code, KeyModifiers::NONE), h, &tx);
        terminal.draw(|f| ui::draw(f, &mut app))?;
    }
    let buf = terminal.backend().buffer();
    let ansi = |c: Color, bg: bool| match c {
        Color::Rgb(r, g, b) => format!("\x1b[{};2;{r};{g};{b}m", if bg { 48 } else { 38 }),
        _ => format!("\x1b[{}m", if bg { 49 } else { 39 }),
    };
    for y in 0..h {
        let mut line = String::new();
        for x in 0..w {
            let cell = &buf[(x, y)];
            line += &ansi(cell.fg, false);
            line += &ansi(cell.bg, true);
            line += cell.symbol();
        }
        println!("{line}\x1b[0m");
    }
    Ok(())
}
