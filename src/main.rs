//! vaglio: the files a task changed, per worktree, against the branch they will merge into.

mod app;
mod clipboard;
mod diff;
mod docs;
mod git;
mod jira;
mod pr;
mod review;
mod source;
mod ui;
mod worker;

use std::sync::mpsc;
use std::time::Duration;

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::crossterm::execute;

use app::App;
use source::Source;

const HELP: &str = "vaglio [PATH...]

Mostra i file che ogni worktree ha cambiato rispetto al branch di integrazione
(origin/develop se esiste, altrimenti origin/main), con il diff di ciascuno.

Senza argomenti, dentro herdr segue il workspace (lista scritta dal plugin
layout); fuori da herdr mostra il repo della directory corrente.

vaglio --check-token [PATH]   prova il token Bitbucket sul repo di PATH (default: qui):
                              lettura e creazione delle PR
vaglio --create-pr PATH [--title T] [--description D]
                              push del branch di PATH e PR draft verso il branch di
                              integrazione; se la PR c'è già, ne stampa solo il link
vaglio --check-jira KEY       prova il token Jira leggendo lo stato del ticket KEY";

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{HELP}");
        return Ok(());
    }
    if args.first().is_some_and(|a| a == "--check-token") {
        let dir = args.get(1).map_or_else(|| std::env::current_dir().unwrap_or_default(), std::path::PathBuf::from);
        let (ok, msg) = pr::check_token(&dir);
        println!("{msg}");
        std::process::exit(if ok { 0 } else { 1 });
    }
    if args.first().is_some_and(|a| a == "--create-pr") {
        std::process::exit(create_pr(&args[1..]));
    }
    if args.first().is_some_and(|a| a == "--check-jira") {
        let Some(key) = args.get(1) else {
            println!("uso: vaglio --check-jira AB-123");
            std::process::exit(2);
        };
        let (ok, msg) = jira::check(key);
        println!("{msg}");
        std::process::exit(if ok { 0 } else { 1 });
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
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let mut app = App::new();
    let result = (|| -> anyhow::Result<()> {
        loop {
            while let Ok(snap) = snap_rx.try_recv() {
                app.apply(snap);
            }
            if app.collect_created() {
                let _ = poke_tx.send(worker::Poke::All);
            }
            terminal.draw(|f| ui::draw(f, &mut app))?;
            if !event::poll(Duration::from_millis(100))? {
                continue;
            }
            match event::read()? {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Release && !handle(&mut app, key, terminal.size()?.height, &poke_tx) {
                        return Ok(());
                    }
                }
                Event::Mouse(mouse) => handle_mouse(&mut app, mouse),
                _ => {}
            }
        }
    })();
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

/// Returns false to quit.
fn handle(app: &mut App, key: KeyEvent, height: u16, poke: &mpsc::Sender<worker::Poke>) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && key.code == KeyCode::Char('c') {
        return false;
    }
    let page = height.saturating_sub(2).max(1) as isize;
    match key.code {
        KeyCode::Char('p') => {
            app.open_pr();
            return true;
        }
        KeyCode::Char('y') => {
            app.copy_path();
            return true;
        }
        KeyCode::Char('t') => {
            app.open_ticket();
            return true;
        }
        _ => {}
    }
    if app.docs_view.is_some() {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('h') | KeyCode::Left | KeyCode::Backspace => app.docs_view = None,
            KeyCode::Char('j') | KeyCode::Down => app.move_doc(1),
            KeyCode::Char('k') | KeyCode::Up => app.move_doc(-1),
            KeyCode::Char('g') | KeyCode::Home => app.move_doc(isize::MIN / 2),
            KeyCode::Char('G') | KeyCode::End => app.move_doc(isize::MAX / 2),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right | KeyCode::Char('o') => app.open_doc(),
            _ => {}
        }
        return true;
    }
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
            let _ = poke.send(worker::Poke::All);
        }
        _ => {}
    }
    true
}

/// The wheel scrolls the diff, or moves the selection in a list; a click selects, a second opens.
fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    let delta = match mouse.kind {
        MouseEventKind::ScrollDown => 1,
        MouseEventKind::ScrollUp => -1,
        MouseEventKind::Down(MouseButton::Left) => return app.click(mouse.column, mouse.row),
        _ => return,
    };
    if app.docs_view.is_some() {
        app.move_doc(delta);
    } else if let Some(view) = app.view.as_mut() {
        view.scroll_by(3 * delta);
    } else {
        app.move_by(delta);
    }
}

/// `vaglio --snapshot 100x30 KEYS [PATH...]`: draws one frame after the keys into a test backend
/// and prints it with ANSI colours. For checking the drawing without a terminal.
fn snapshot(args: &[String]) -> anyhow::Result<()> {
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    let (size, keys) = (args.first().map_or("100x30", String::as_str), args.get(1).cloned().unwrap_or_default());
    let (w, h) = size.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))).unwrap_or((100, 30));
    let source = Source::from_env(args.iter().skip(2).cloned().collect());
    let (targets, current, review) = worker::targets(&source, &mut None, false);
    let groups = targets
        .iter()
        .map(|target| {
            let mut group = worker::load_group(target);
            group.pr = group.tree.as_ref().ok().map(|t| pr::lookup(&group.root, &t.branch));
            group
        })
        .collect();
    let label = source.label();
    let ticket = label.as_deref().and_then(jira::key_in).map(|key| jira::Ticket {
        status: Some(jira::lookup(&key)),
        url: jira::browse_url(&key),
        key,
    });
    let mut app = App::new();
    app.apply(app::Snapshot { label, ticket, current, groups, docs: source.docs(), review });
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

/// `--create-pr PATH [--title T] [--description D]`: the draft pull request a chat opens with
/// vaglio's own token. Prints the link and exits 0, or the reason and exits 1.
fn create_pr(args: &[String]) -> i32 {
    let (mut path, mut title, mut description) = (None, None, String::new());
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--title" => title = it.next().cloned(),
            "--description" => description = it.next().cloned().unwrap_or_default(),
            _ if path.is_none() => path = Some(std::path::PathBuf::from(a)),
            other => {
                println!("argomento inatteso: {other}");
                return 2;
            }
        }
    }
    let Some(path) = path else {
        println!("uso: vaglio --create-pr PATH [--title T] [--description D]");
        return 2;
    };
    let Some(root) = git::toplevel(&path) else {
        println!("{} non è un repo git", path.display());
        return 1;
    };
    let out = std::process::Command::new("git").arg("-C").arg(&root).args(["branch", "--show-current"]).output();
    let branch = out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    if branch.is_empty() {
        println!("{}: nessun branch (HEAD staccato)", root.display());
        return 1;
    }
    match pr::lookup(&root, &branch) {
        pr::PrStatus::Found(found) => {
            println!("{}", found.url);
            return 0;
        }
        pr::PrStatus::Missing { .. } => {}
        pr::PrStatus::OnBase => {
            println!("{branch} è un branch di integrazione: niente PR da aprire");
            return 1;
        }
        pr::PrStatus::NoHost => {
            println!("origin non è né Bitbucket né GitHub");
            return 1;
        }
        pr::PrStatus::NoCredentials => {
            println!("manca il token Bitbucket di vaglio (vedi README)");
            return 1;
        }
        pr::PrStatus::Error(e) => {
            println!("{e}");
            return 1;
        }
    }
    match pr::create_draft(&root, &branch, title.as_deref(), &description) {
        Ok(url) => {
            println!("{url}");
            0
        }
        Err(e) => {
            println!("{e}");
            1
        }
    }
}
