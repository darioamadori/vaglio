//! Re-reads the worktrees in the background: at once when a file in them changes, and every
//! few seconds regardless, since commits land in git dirs that are not always watched. Pull
//! requests are asked for far less often, and never hold up the file list.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use notify::{EventKind, RecursiveMode, Watcher};

use crate::app::{Group, Snapshot};
use crate::jira::{self, Ticket};
use crate::pr::{self, PrStatus};
use crate::review::{self, Target};
use crate::source::Source;

/// Why the worker should look again.
pub enum Poke {
    /// A file changed.
    Files,
    /// The user asked: re-read the pull requests too.
    All,
}

const EVERY: Duration = Duration::from_secs(3);
/// Claude writes a file in several events: wait for the burst to end before reading.
const SETTLE: Duration = Duration::from_millis(200);
const PR_EVERY: Duration = Duration::from_secs(60);
/// A review whose branch was not found yet: the chat may be about to fetch it or check it out.
const REVIEW_RETRY: Duration = Duration::from_secs(20);

/// Directories whose churn never shows in a diff. `.git` too: commits are caught by the timer.
const NOISE: &[&str] = &[
    ".git", "node_modules", ".venv", "target", "__pycache__", ".ruff_cache", ".pytest_cache", ".mypy_cache", ".turbo", "dist",
];

fn is_noise(path: &Path) -> bool {
    path.components().any(|c| NOISE.contains(&c.as_os_str().to_string_lossy().as_ref()))
}

/// A group as its target stands now; the pull request is looked up separately.
pub fn load_group(target: &Target) -> Group {
    let (root, tree) = match target {
        Target::Live(root) => (root.clone(), crate::git::load(root).map_err(|e| e.to_string())),
        Target::Ref { root, head, base } => (root.clone(), crate::git::load_ref(root, head, base).map_err(|e| e.to_string())),
        Target::Missing { what, why } => (PathBuf::from(what), Err(why.clone())),
    };
    Group { root, tree, pr: None }
}

/// What to show: the pull request under review in a review workspace, the worktrees otherwise.
/// `resolved` caches the review's resolution, which may fetch; `refresh` forces a new one.
pub fn targets(source: &Source, resolved: &mut Option<(String, Instant, Vec<Target>)>, refresh: bool) -> (Vec<Target>, Option<PathBuf>, bool) {
    let Some(args) = source.review_args() else {
        let found = source.roots();
        return (found.list.into_iter().map(Target::Live).collect(), found.current, false);
    };
    let stale = match resolved.as_ref() {
        None => true,
        Some((prev, at, found)) => {
            *prev != args
                || refresh
                || (found.iter().any(Target::is_missing) && at.elapsed() > REVIEW_RETRY)
                // Re-fetch now and then: the pull request may get new commits during the review.
                || (found.iter().any(|t| matches!(t, Target::Ref { .. })) && at.elapsed() > PR_EVERY)
        }
    };
    if stale {
        let found = review::resolve(&args);
        *resolved = Some((args, Instant::now(), found));
    }
    (resolved.iter().flat_map(|(_, _, found)| found.clone()).collect(), None, true)
}

pub fn spawn(source: Source, out: Sender<Snapshot>, poke: Sender<Poke>, pokes: Receiver<Poke>) {
    std::thread::spawn(move || run(source, out, poke, pokes));
}

fn run(source: Source, out: Sender<Snapshot>, poke: Sender<Poke>, pokes: Receiver<Poke>) {
    let mut watcher = {
        let poke = poke.clone();
        notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, EventKind::Access(_)) || event.paths.iter().all(|p| is_noise(p)) {
                return;
            }
            let _ = poke.send(Poke::Files);
        })
        .ok()
    };
    let mut watched: Vec<PathBuf> = Vec::new();
    if let Some(w) = watcher.as_mut() {
        // The directories, not the files: the hook replaces them, and a new one must count too.
        for file in source.state_files() {
            if let Some(dir) = file.parent() {
                let _ = std::fs::create_dir_all(dir);
                let _ = w.watch(dir, RecursiveMode::NonRecursive);
            }
        }
    }

    let mut ticket: Option<(Instant, Ticket)> = None;
    let mut prs: HashMap<(PathBuf, String), (Instant, PrStatus)> = HashMap::new();
    let mut resolved = None;
    let mut refresh = false;
    loop {
        let (targets, current, in_review) = targets(&source, &mut resolved, std::mem::take(&mut refresh));
        let roots: Vec<PathBuf> =
            targets.iter().filter_map(|t| if let Target::Live(p) = t { Some(p.clone()) } else { None }).collect();
        if let Some(w) = watcher.as_mut() {
            for gone in watched.iter().filter(|p| !roots.contains(p)) {
                let _ = w.unwatch(gone);
            }
            for root in roots.iter().filter(|r| !watched.contains(r)) {
                let _ = w.watch(root, RecursiveMode::Recursive);
            }
        }
        watched = roots.clone();

        // Every round: a rename in herdr must show, and asking herdr takes a few milliseconds.
        let label = source.label();
        let key = label.as_deref().and_then(jira::key_in);
        if ticket.as_ref().map(|(_, t)| &t.key) != key.as_ref() {
            ticket = key.map(|key| (Instant::now(), Ticket { url: None, status: None, key }));
        }
        let mut groups: Vec<Group> = targets
            .iter()
            .map(|target| {
                let mut group = load_group(target);
                let key = group.tree.as_ref().ok().map(|t| (group.root.clone(), t.branch.clone()));
                group.pr = key.and_then(|k| prs.get(&k)).map(|(_, s)| s.clone());
                group
            })
            .collect();
        let docs = source.docs();
        let snapshot = |groups: Vec<Group>, docs, ticket: &Option<(Instant, Ticket)>| Snapshot {
            label: label.clone(),
            ticket: ticket.as_ref().map(|(_, t)| t.clone()),
            current: current.clone(),
            groups,
            docs,
            review: in_review,
        };
        if out.send(snapshot(groups.clone(), docs.clone(), &ticket)).is_err() {
            return;
        }

        let mut looked_up = false;
        if let Some((at, t)) = ticket.as_mut() {
            if t.status.is_none() || at.elapsed() > PR_EVERY {
                t.status = Some(jira::lookup(&t.key));
                t.url = jira::browse_url(&t.key);
                *at = Instant::now();
                looked_up = true;
            }
        }
        for group in groups.iter_mut() {
            let Ok(tree) = &group.tree else { continue };
            let key = (group.root.clone(), tree.branch.clone());
            if prs.get(&key).is_some_and(|(at, _)| at.elapsed() < PR_EVERY) {
                continue;
            }
            let status = pr::lookup(&group.root, &tree.branch);
            prs.insert(key, (Instant::now(), status.clone()));
            group.pr = Some(status);
            looked_up = true;
        }
        if looked_up && out.send(snapshot(groups, docs, &ticket)).is_err() {
            return;
        }

        let mut on_poke = |poke: Poke| {
            if matches!(poke, Poke::All) {
                prs.clear();
                if let Some(t) = ticket.as_mut() {
                    t.1.status = None;
                }
                refresh = true;
            }
        };
        match pokes.recv_timeout(EVERY) {
            Ok(poke) => {
                on_poke(poke);
                std::thread::sleep(SETTLE);
                while let Ok(poke) = pokes.try_recv() {
                    on_poke(poke);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}
