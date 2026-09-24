//! Everything vaglio asks git: which files a worktree changed against its integration branch,
//! and the unified diff of one of them.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
}

impl Status {
    pub fn letter(self) -> char {
        match self {
            Status::Added | Status::Untracked => 'A',
            Status::Modified => 'M',
            Status::Deleted => 'D',
            Status::Renamed => 'R',
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub path: String,
    /// Where a renamed file came from.
    pub old_path: Option<String>,
    pub status: Status,
    /// `None` for binary files.
    pub added: Option<u32>,
    pub deleted: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct Tree {
    pub root: PathBuf,
    pub repo: String,
    pub branch: String,
    /// The integration branch the diff is taken against (`origin/main`, `origin/develop`).
    pub base: String,
    pub merge_base: String,
    /// The ref whose changes are shown (`origin/<branch>` for a pull request under review);
    /// `None` means the working tree, untracked files included.
    pub head: Option<String>,
    pub files: Vec<FileChange>,
}

impl Tree {
    pub fn totals(&self) -> (u32, u32) {
        self.files.iter().fold((0, 0), |(a, d), f| {
            (a + f.added.unwrap_or(0), d + f.deleted.unwrap_or(0))
        })
    }
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["-c", "core.quotepath=off", "-c", "color.ui=never"])
        .args(args)
        .output()
        .context("git non trovato")?;
    // `git diff --no-index` exits 1 when the files differ: that is its success.
    if !out.status.success() && !(out.status.code() == Some(1) && args.contains(&"--no-index")) {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

fn git_str(root: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8_lossy(&git(root, args)?).trim().to_string())
}

pub fn toplevel(dir: &Path) -> Option<PathBuf> {
    git_str(dir, &["rev-parse", "--show-toplevel"]).ok().map(PathBuf::from)
}

/// `compri-worktrees/<repo>/<name>` is named after its repo; anything else after the folder
/// holding its main checkout.
fn repo_name(root: &Path) -> String {
    let worktrees = crate::source::worktrees_dir();
    if let Ok(rest) = root.strip_prefix(&worktrees) {
        if let Some(repo) = rest.components().next() {
            return repo.as_os_str().to_string_lossy().into_owned();
        }
    }
    let common = git_str(root, &["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join(".git"));
    let main = if common.ends_with(".git") { common.parent().map(Path::to_path_buf) } else { None };
    main.unwrap_or_else(|| root.to_path_buf())
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `develop` where the repo has one, `main` otherwise.
pub fn base_branch(root: &Path) -> String {
    for candidate in ["origin/develop", "origin/HEAD", "origin/main", "main", "master"] {
        if git(root, &["rev-parse", "--verify", "--quiet", candidate]).is_ok() {
            if candidate == "origin/HEAD" {
                if let Ok(name) = git_str(root, &["rev-parse", "--abbrev-ref", "origin/HEAD"]) {
                    return name;
                }
            }
            return candidate.to_string();
        }
    }
    "HEAD".to_string()
}

pub fn load(root: &Path) -> Result<Tree> {
    let branch = git_str(root, &["branch", "--show-current"])?;
    let base = base_branch(root);
    let merge_base = git_str(root, &["merge-base", "HEAD", &base]).unwrap_or_else(|_| "HEAD".into());

    let mut files = changed(root, &merge_base, None)?;
    for path in split_nul(&git(root, &["ls-files", "--others", "--exclude-standard", "-z"])?) {
        let lines = std::fs::read(root.join(&path)).ok().and_then(|b| {
            (!b.contains(&0)).then(|| b.iter().filter(|&&c| c == b'\n').count() as u32 + u32::from(!b.is_empty() && !b.ends_with(b"\n")))
        });
        files.push(FileChange { path, old_path: None, status: Status::Untracked, added: lines, deleted: lines.map(|_| 0) });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(Tree {
        root: root.to_path_buf(),
        repo: repo_name(root),
        branch: if branch.is_empty() { "(detached)".into() } else { branch },
        base,
        merge_base,
        head: None,
        files,
    })
}

/// A branch as it stands on `head` (say `origin/feature/x`), against its merge base with `base`:
/// what its pull request shows, with no checkout of it anywhere.
pub fn load_ref(root: &Path, head: &str, base: &str) -> Result<Tree> {
    let merge_base = git_str(root, &["merge-base", head, base])?;
    let files = changed(root, &merge_base, Some(head))?;
    Ok(Tree {
        root: root.to_path_buf(),
        repo: repo_name(root),
        branch: head.strip_prefix("origin/").unwrap_or(head).to_string(),
        base: base.to_string(),
        merge_base,
        head: Some(head.to_string()),
        files,
    })
}

fn split_nul(out: &[u8]) -> Vec<String> {
    out.split(|&b| b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect()
}

/// Committed, staged and unstaged changes since the merge base, in one pass; with `head`, only
/// what that ref committed.
fn changed(root: &Path, merge_base: &str, head: Option<&str>) -> Result<Vec<FileChange>> {
    let mut args = vec!["diff", "--no-ext-diff", "--name-status", "-z", "-M", merge_base];
    args.extend(head);
    let status = git(root, &args)?;
    let mut fields = split_nul(&status).into_iter();
    let mut files = Vec::new();
    while let Some(code) = fields.next() {
        let (status, old_path) = match code.chars().next() {
            Some('A') => (Status::Added, None),
            Some('D') => (Status::Deleted, None),
            Some('R') | Some('C') => (Status::Renamed, fields.next()),
            _ => (Status::Modified, None),
        };
        let Some(path) = fields.next() else { break };
        files.push(FileChange { path, old_path, status, added: None, deleted: None });
    }

    // numstat -z: "A\tD\tpath\0", or "A\tD\t\0old\0new\0" for a rename.
    args[2] = "--numstat";
    let numstat = git(root, &args)?;
    let mut records = numstat.split(|&b| b == 0).map(|s| String::from_utf8_lossy(s).into_owned());
    while let Some(rec) = records.next() {
        let mut parts = rec.splitn(3, '\t');
        let (Some(a), Some(d), Some(p)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let path = if p.is_empty() {
            records.next();
            records.next().unwrap_or_default()
        } else {
            p.to_string()
        };
        if let Some(f) = files.iter_mut().find(|f| f.path == path) {
            f.added = a.parse().ok();
            f.deleted = d.parse().ok();
        }
    }
    Ok(files)
}

/// The unified diff of one file against the merge base. `context` of `None` means the whole file.
pub fn diff(tree: &Tree, file: &FileChange, context: Option<u32>) -> Result<String> {
    let unified = format!("-U{}", context.unwrap_or(1_000_000));
    let out = if file.status == Status::Untracked {
        git(&tree.root, &["diff", "--no-index", "--no-ext-diff", "--no-textconv", &unified, "--", "/dev/null", &file.path])?
    } else {
        let mut args = vec!["diff", "--no-ext-diff", "--no-textconv", "-M", &unified, &tree.merge_base];
        args.extend(tree.head.as_deref());
        args.push("--");
        if let Some(old) = &file.old_path {
            args.push(old);
        }
        args.push(&file.path);
        git(&tree.root, &args)?
    };
    Ok(String::from_utf8_lossy(&out).into_owned())
}
