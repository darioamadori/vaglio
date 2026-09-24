//! A review workspace: one where `/pr-review` was run. From then on vaglio shows the pull request
//! under review instead of the workspace's worktrees, whatever the chats touch.
//!
//! The hook saves what followed `/pr-review` (a pull request link, a branch name, or a sentence
//! naming one) to `<state>/review/<workspace id>`; each new `/pr-review` replaces it. Here it
//! becomes something to load: the worktree already on that branch when there is one (it is what
//! the chat reads), otherwise the repo's main checkout, diffed on `origin/<branch>` after a fetch.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::pr::{self, Host};
use crate::source::{home, worktrees_dir};

/// What a group of the list is loaded from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A checkout's working tree.
    Live(PathBuf),
    /// A ref of a checkout, against a base: a branch no one has checked out.
    Ref { root: PathBuf, head: String, base: String },
    /// A review whose pull request could not be found, and why.
    Missing { what: String, why: String },
}

impl Target {
    pub fn is_missing(&self) -> bool {
        matches!(self, Target::Missing { .. })
    }
}

fn compri() -> PathBuf {
    home().join("Developer/compri")
}

/// Resolves the `/pr-review` arguments. May fetch, so it runs in the worker, never per frame.
pub fn resolve(args: &str) -> Target {
    let args = args.trim();
    let what = args.lines().next().unwrap_or("").to_string();
    if args.is_empty() {
        // Bare `/pr-review` reviews monorepo-ai's current branch against main.
        return Target::Live(compri().join("monorepo-ai"));
    }
    if let Some((host, id)) = pr_link(args) {
        let repo = match &host {
            Host::Bitbucket { repo, .. } | Host::GitHub { repo, .. } => repo.clone(),
        };
        let Some(clone) = clones().into_iter().find(|c| c.file_name().is_some_and(|n| n == repo.as_str())) else {
            return Target::Missing { what, why: format!("nessun clone di {repo} in ~/Developer/compri") };
        };
        return match pr::branches(&clone, &host, id) {
            Ok((src, dst)) => locate(&src, Some(&clone), Some(&dst))
                .unwrap_or_else(|| Target::Missing { what, why: format!("{src}: git fetch non lo trova") }),
            Err(why) => Target::Missing { what, why },
        };
    }
    let candidates = branch_names(args);
    for branch in &candidates {
        if let Some(target) = locate(branch, None, None) {
            return target;
        }
    }
    let keys = ticket_keys(args);
    for key in &keys {
        if let Some(target) = locate_key(key) {
            return target;
        }
    }
    let why = match (candidates.first(), keys.first()) {
        (Some(b), _) => format!("{b}: nessun worktree né clone ha questo branch (manca un fetch?)"),
        (None, Some(k)) => format!("{k}: nessun branch con questa chiave nei worktree né nei clone (manca un fetch?)"),
        (None, None) => "nessun link di PR, nome di branch o chiave di ticket".to_string(),
    };
    Target::Missing { what, why }
}

/// `https://bitbucket.org/<ws>/<repo>/pull-requests/<id>…` or `https://github.com/<o>/<r>/pull/<n>`.
fn pr_link(text: &str) -> Option<(Host, u64)> {
    text.split_whitespace().find_map(|word| {
        let (kind, rest) = ["bitbucket.org/", "github.com/"].iter().find_map(|h| Some((*h, word.split_once(h)?.1)))?;
        let mut parts = rest.split('/');
        let (owner, repo, marker, id) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        let id: u64 = id.trim_end_matches(|c: char| !c.is_ascii_digit()).parse().ok()?;
        let (owner, repo) = (owner.to_string(), repo.to_string());
        match (kind, marker) {
            ("bitbucket.org/", "pull-requests") => Some((Host::Bitbucket { workspace: owner, repo }, id)),
            ("github.com/", "pull") => Some((Host::GitHub { owner, repo }, id)),
            _ => None,
        }
    })
}

/// Words shaped like a branch (`feature/CA-583-oc-change-reason`), in the order they appear.
fn branch_names(text: &str) -> Vec<String> {
    let allowed = |c: char| c.is_ascii_alphanumeric() || "._/-".contains(c);
    let mut out: Vec<String> = Vec::new();
    for word in text.split(|c: char| !allowed(c)) {
        let word = word.trim_matches(|c| c == '.' || c == '/');
        if word.contains('/') && !word.contains("//") && !out.iter().any(|w| w == word) {
            out.push(word.to_string());
        }
    }
    out
}

/// Ticket keys (`CA-615`) in the order they appear.
fn ticket_keys(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
        let Some((project, number)) = word.split_once('-') else { continue };
        let project_ok = project.len() >= 2
            && project.starts_with(|c: char| c.is_ascii_uppercase())
            && project.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
        if project_ok && !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()) && !out.iter().any(|w| w == word) {
            out.push(word.to_string());
        }
    }
    out
}

/// A branch carries the key as a whole segment: `feature/CA-615-…`, not `feature/CA-6150`.
fn names_key(branch: &str, key: &str) -> bool {
    branch.split('/').any(|seg| seg == key || seg.strip_prefix(key).is_some_and(|rest| rest.starts_with('-')))
}

/// The branch a ticket key names: a worktree on it first, else the most recently committed
/// `origin/` branch carrying the key in any main checkout.
fn locate_key(key: &str) -> Option<Target> {
    if let Some(wt) = worktrees().into_iter().find(|wt| current_branch(wt).is_some_and(|b| names_key(&b, key))) {
        return Some(Target::Live(wt));
    }
    let mut best: Option<(u64, PathBuf, String)> = None;
    for clone in clones() {
        let out = Command::new("git")
            .arg("-C")
            .arg(&clone)
            .args(["for-each-ref", "--format=%(committerdate:unix) %(refname:lstrip=3)", "refs/remotes/origin"])
            .output();
        let Ok(out) = out else { continue };
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let Some((date, branch)) = line.split_once(' ') else { continue };
            let date: u64 = date.parse().unwrap_or(0);
            if names_key(branch, key) && best.as_ref().is_none_or(|(d, _, _)| date > *d) {
                best = Some((date, clone.clone(), branch.to_string()));
            }
        }
    }
    let (_, clone, branch) = best?;
    locate(&branch, Some(&clone), None)
}

fn git_ok(root: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn current_branch(root: &Path) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(root).args(["branch", "--show-current"]).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The worktree on `branch`, else the checkout that knows `origin/<branch>`, fetched fresh.
/// With `clone`, only that repo is looked at, and it is fetched even if the ref is not there yet.
fn locate(branch: &str, clone: Option<&Path>, dest: Option<&str>) -> Option<Target> {
    let repo_of_wt = |wt: &Path| wt.parent().and_then(Path::file_name).map(|n| n.to_owned());
    let wanted_repo = clone.and_then(Path::file_name).map(|n| n.to_owned());
    for wt in worktrees() {
        if wanted_repo.is_some() && repo_of_wt(&wt) != wanted_repo {
            continue;
        }
        if current_branch(&wt).as_deref() == Some(branch) {
            return Some(Target::Live(wt));
        }
    }
    let remote_ref = format!("refs/remotes/origin/{branch}");
    let root = match clone {
        Some(c) => c.to_path_buf(),
        None => clones().into_iter().find(|c| git_ok(c, &["rev-parse", "--verify", "--quiet", &remote_ref]))?,
    };
    // Without a refspec, `git fetch origin <branch>` still moves `origin/<branch>`.
    let mut fetch = vec!["fetch", "--quiet", "--no-tags", "origin", branch];
    fetch.extend(dest);
    git_ok(&root, &fetch);
    if !git_ok(&root, &["rev-parse", "--verify", "--quiet", &remote_ref]) {
        return None;
    }
    let base = match dest {
        Some(d) => format!("origin/{d}"),
        None => crate::git::base_branch(&root),
    };
    Some(Target::Ref { root, head: format!("origin/{branch}"), base })
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    out
}

fn worktrees() -> Vec<PathBuf> {
    subdirs(&worktrees_dir()).iter().flat_map(|repo| subdirs(repo)).filter(|p| p.join(".git").exists()).collect()
}

/// The main checkouts under the umbrella: `compri/<repo>` and `compri/<category>/<repo>`.
fn clones() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in subdirs(&compri()) {
        if dir.join(".git").exists() {
            out.push(dir);
        } else {
            out.extend(subdirs(&dir).into_iter().filter(|p| p.join(".git").exists()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_pr_links() {
        let bb = pr_link("https://bitbucket.org/compri-vcs/monorepo-ai/pull-requests/325/overview");
        assert_eq!(bb, Some((Host::Bitbucket { workspace: "compri-vcs".into(), repo: "monorepo-ai".into() }, 325)));
        let gh = pr_link("guarda https://github.com/me/vaglio/pull/7 grazie");
        assert_eq!(gh, Some((Host::GitHub { owner: "me".into(), repo: "vaglio".into() }, 7)));
        assert_eq!(pr_link("https://bitbucket.org/compri-vcs/monorepo-ai/src/main/"), None);
    }

    #[test]
    fn finds_branch_names_in_a_sentence() {
        assert_eq!(branch_names("feature/CA-583-oc-change-reason"), vec!["feature/CA-583-oc-change-reason"]);
        let text = "Create a work tree, switch to this branch: \nfeature/CA-509-adaptive-polling-schedule.\n\nand then review.";
        assert_eq!(branch_names(text), vec!["feature/CA-509-adaptive-polling-schedule"]);
        assert!(branch_names("fai la review, grazie").is_empty());
    }

    #[test]
    fn finds_ticket_keys_and_the_branches_they_name() {
        assert_eq!(ticket_keys("CA-615"), vec!["CA-615"]);
        assert_eq!(ticket_keys("rivedi CA-615, poi CB-2784."), vec!["CA-615", "CB-2784"]);
        assert!(ticket_keys("utf-8 x-client-id A-1").is_empty());
        assert!(names_key("feature/CA-615-insight-agent-time-saved", "CA-615"));
        assert!(names_key("CA-615", "CA-615"));
        assert!(!names_key("feature/CA-6150-other", "CA-615"));
        assert!(!names_key("feature/XCA-615-other", "CA-615"));
    }
}
