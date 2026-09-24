//! Re-reads the worktrees in the background: at once when a file in them changes, and every
//! few seconds regardless, since commits land in git dirs that are not always watched.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

use crate::app::{Group, Snapshot};
use crate::source::Source;

const EVERY: Duration = Duration::from_secs(3);
/// Claude writes a file in several events: wait for the burst to end before reading.
const SETTLE: Duration = Duration::from_millis(200);
const LABEL_EVERY: Duration = Duration::from_secs(30);

/// Directories whose churn never shows in a diff. `.git` too: commits are caught by the timer.
const NOISE: &[&str] = &[
    ".git", "node_modules", ".venv", "target", "__pycache__", ".ruff_cache", ".pytest_cache", ".mypy_cache", ".turbo", "dist",
];

fn is_noise(path: &Path) -> bool {
    path.components().any(|c| NOISE.contains(&c.as_os_str().to_string_lossy().as_ref()))
}

pub fn spawn(source: Source, out: Sender<Snapshot>, poke: Sender<()>, pokes: Receiver<()>) {
    std::thread::spawn(move || run(source, out, poke, pokes));
}

fn run(source: Source, out: Sender<Snapshot>, poke: Sender<()>, pokes: Receiver<()>) {
    let mut watcher = {
        let poke = poke.clone();
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, EventKind::Access(_)) || event.paths.iter().all(|p| is_noise(p)) {
                return;
            }
            let _ = poke.send(());
        })
        .ok()
    };
    let mut watched: Vec<PathBuf> = Vec::new();
    if let (Some(w), Some(file)) = (watcher.as_mut(), source.list_file()) {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
            let _ = w.watch(dir, RecursiveMode::NonRecursive);
        }
    }

    let mut label = None;
    let mut label_at: Option<Instant> = None;
    loop {
        let roots = source.roots();
        if let Some(w) = watcher.as_mut() {
            for gone in watched.iter().filter(|p| !roots.contains(p)) {
                let _ = w.unwatch(gone);
            }
            for root in roots.iter().filter(|r| !watched.contains(r)) {
                let _ = w.watch(root, RecursiveMode::Recursive);
            }
        }
        watched = roots.clone();

        if label_at.is_none_or(|t| t.elapsed() > LABEL_EVERY) {
            label = source.label();
            label_at = Some(Instant::now());
        }
        let groups = roots
            .into_iter()
            .map(|root| Group { tree: crate::git::load(&root).map_err(|e| e.to_string()), root })
            .collect();
        if out.send(Snapshot { label: label.clone(), groups }).is_err() {
            return;
        }

        match pokes.recv_timeout(EVERY) {
            Ok(()) => {
                std::thread::sleep(SETTLE);
                while pokes.try_recv().is_ok() {}
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}
