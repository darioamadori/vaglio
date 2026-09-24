//! Which worktrees vaglio shows.
//!
//! Inside herdr it follows the workspace: the `compri-layout` plugin and its `follow.sh` hook
//! append every worktree Claude works in to `<state>/spaces/<workspace id>`, one path per line,
//! so every chat of the workspace adds to the same list. Paths given on the command line
//! replace that list; with neither, vaglio shows the repo it was started in.

use std::path::PathBuf;
use std::process::Command;

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

pub fn worktrees_dir() -> PathBuf {
    home().join("Developer/compri-worktrees")
}

fn state_dir() -> PathBuf {
    std::env::var_os("VAGLIO_STATE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local/state/herdr/plugins/dario.compri-layout"))
}

#[derive(Clone, Debug)]
pub enum Source {
    Paths(Vec<PathBuf>),
    Workspace { id: String, file: PathBuf, cwd: PathBuf },
    Cwd(PathBuf),
}

impl Source {
    pub fn from_env(args: Vec<String>) -> Source {
        let cwd = std::env::current_dir().unwrap_or_default();
        if !args.is_empty() {
            return Source::Paths(args.into_iter().map(|a| cwd.join(a)).collect());
        }
        match std::env::var("HERDR_WORKSPACE_ID") {
            Ok(id) if !id.is_empty() => {
                let file = state_dir().join("spaces").join(id.replace(':', "_"));
                Source::Workspace { id, file, cwd }
            }
            _ => Source::Cwd(cwd),
        }
    }

    /// The file to watch besides the worktrees themselves.
    pub fn list_file(&self) -> Option<&PathBuf> {
        match self {
            Source::Workspace { file, .. } => Some(file),
            _ => None,
        }
    }

    pub fn roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = Vec::new();
        let mut push = |p: PathBuf| {
            if !roots.contains(&p) {
                roots.push(p);
            }
        };
        match self {
            Source::Paths(paths) => paths.iter().filter_map(|p| crate::git::toplevel(p)).for_each(&mut push),
            Source::Cwd(cwd) => crate::git::toplevel(cwd).into_iter().for_each(&mut push),
            Source::Workspace { file, cwd, .. } => {
                // A pane opened straight on a worktree counts even before the list names it.
                if let Some(top) = crate::git::toplevel(cwd).filter(|t| t.starts_with(worktrees_dir())) {
                    push(top);
                }
                // A worktree removed since it was listed just drops out.
                std::fs::read_to_string(file)
                    .unwrap_or_default()
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(PathBuf::from)
                    .filter(|p| p.join(".git").exists())
                    .for_each(&mut push);
            }
        }
        if roots.is_empty() {
            if let Source::Workspace { cwd, .. } = self {
                roots.extend(crate::git::toplevel(cwd));
            }
        }
        roots
    }

    /// The herdr workspace label, for the header.
    pub fn label(&self) -> Option<String> {
        let Source::Workspace { id, .. } = self else { return None };
        let herdr = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into());
        let out = Command::new(herdr).args(["workspace", "list"]).output().ok()?;
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
        json["result"]["workspaces"]
            .as_array()?
            .iter()
            .find(|w| w["workspace_id"] == id.as_str())?["label"]
            .as_str()
            .map(str::to_string)
    }
}
