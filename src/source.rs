//! Which worktrees vaglio shows.
//!
//! Inside herdr it follows the chat of its tab. The `compri-layout` plugin and its `follow.sh`
//! hook write the worktree that tab's Claude is working in to `<state>/tabs/<tab id>` (the file
//! yazi follows too), and append every worktree any chat of the workspace works in to
//! `<state>/spaces/<workspace id>`. The tab's worktree comes first and is where the selection
//! lands; the rest of the workspace follows, for a task spread over several repos. With neither,
//! vaglio shows the repo the pane was opened in, usually a main checkout on `main`.
//!
//! Paths given on the command line replace all of that. Outside herdr, with no paths, vaglio
//! shows the repo it was started in.

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
    Workspace { id: String, tab: Option<PathBuf>, space: PathBuf, cwd: PathBuf },
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
                let state = state_dir();
                let space = state.join("spaces").join(id.replace(':', "_"));
                let tab = std::env::var("HERDR_TAB_ID").ok().filter(|t| !t.is_empty());
                let tab = tab.map(|t| state.join("tabs").join(t.replace(':', "_")));
                Source::Workspace { id, tab, space, cwd }
            }
            _ => Source::Cwd(cwd),
        }
    }

    /// The state files to watch besides the worktrees themselves.
    pub fn state_files(&self) -> Vec<PathBuf> {
        match self {
            Source::Workspace { tab, space, .. } => tab.iter().chain([space]).cloned().collect(),
            _ => Vec::new(),
        }
    }

    pub fn roots(&self) -> Roots {
        let mut roots = Roots::default();
        match self {
            Source::Paths(paths) => paths.iter().filter_map(|p| crate::git::toplevel(p)).for_each(|p| roots.push(p)),
            Source::Cwd(cwd) => roots.list.extend(crate::git::toplevel(cwd)),
            Source::Workspace { tab, space, cwd, .. } => {
                let tab_wt = tab.as_ref().and_then(|f| read_list(f).into_iter().next());
                // A pane opened straight on a worktree is that chat's until the hook says otherwise.
                let own = tab_wt.or_else(|| crate::git::toplevel(cwd).filter(|t| t.starts_with(worktrees_dir())));
                if let Some(own) = own {
                    roots.current = Some(own.clone());
                    roots.push(own);
                }
                read_list(space).into_iter().for_each(|p| roots.push(p));
                if roots.list.is_empty() {
                    roots.list.extend(crate::git::toplevel(cwd));
                }
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Roots {
    pub list: Vec<PathBuf>,
    /// The worktree this tab's chat is working in, when it has one.
    pub current: Option<PathBuf>,
}

impl Roots {
    fn push(&mut self, p: PathBuf) {
        if !self.list.contains(&p) {
            self.list.push(p);
        }
    }
}

/// A state file's paths, one per line; worktrees removed since they were written drop out.
fn read_list(file: &PathBuf) -> Vec<PathBuf> {
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.join(".git").exists())
        .collect()
}
