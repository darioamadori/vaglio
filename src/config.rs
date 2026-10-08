//! Where vaglio looks, read once from `~/.config/vaglio/config.json` (or
//! `$XDG_CONFIG_HOME/vaglio/config.json`). Every key is optional, and a leading `~/` is the
//! home directory:
//!
//! ```json
//! {
//!   "worktrees": "~/Developer/worktrees",
//!   "clones": "~/Developer",
//!   "state_dir": "~/.local/state/vaglio",
//!   "review_default": "api"
//! }
//! ```
//!
//! `worktrees` holds the task worktrees as `<repo>/<name>`; `clones` the main checkouts, as
//! `<repo>` or `<category>/<repo>`; `state_dir` the files the herdr hook writes (see `source`);
//! `review_default` is the clone a bare `/pr-review` reviews. `VAGLIO_STATE_DIR` overrides
//! `state_dir`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde_json::Value;

use crate::source::home;

#[derive(Debug, PartialEq, Eq)]
pub struct Config {
    pub worktrees: PathBuf,
    pub clones: PathBuf,
    pub state_dir: PathBuf,
    pub review_default: Option<String>,
}

pub fn get() -> &'static Config {
    static CONFIG: OnceLock<Config> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let dir = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home().join(".config"));
        let json = std::fs::read(dir.join("vaglio/config.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or(Value::Null);
        from_json(&json, std::env::var_os("VAGLIO_STATE_DIR").map(PathBuf::from))
    })
}

fn from_json(json: &Value, state_override: Option<PathBuf>) -> Config {
    let path = |key: &str, default: &str| expand(json[key].as_str().unwrap_or(default));
    Config {
        worktrees: path("worktrees", "~/Developer/worktrees"),
        clones: path("clones", "~/Developer"),
        state_dir: state_override.unwrap_or_else(|| path("state_dir", "~/.local/state/vaglio")),
        review_default: json["review_default"].as_str().filter(|s| !s.is_empty()).map(str::to_string),
    }
}

fn expand(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if path == "~" => home(),
        None => PathBuf::from(path),
    }
}

/// The path as the user wrote it: the home directory back to `~`.
pub fn tilde(path: &Path) -> String {
    match path.strip_prefix(home()) {
        Ok(rest) if !home().as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_keys_and_falls_back_to_defaults() {
        let json = serde_json::json!({ "worktrees": "~/wt", "clones": "/src", "review_default": "api" });
        let config = from_json(&json, None);
        assert_eq!(config.worktrees, home().join("wt"));
        assert_eq!(config.clones, PathBuf::from("/src"));
        assert_eq!(config.state_dir, home().join(".local/state/vaglio"));
        assert_eq!(config.review_default.as_deref(), Some("api"));
        let empty = from_json(&Value::Null, Some(PathBuf::from("/state")));
        assert_eq!(empty.worktrees, home().join("Developer/worktrees"));
        assert_eq!(empty.state_dir, PathBuf::from("/state"));
        assert_eq!(empty.review_default, None);
    }
}
