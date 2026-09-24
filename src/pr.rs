//! The pull request of a worktree's branch, looked up where its `origin` lives.
//!
//! Bitbucket needs an Atlassian email and API token of its own: from `VAGLIO_BITBUCKET_USER` +
//! `VAGLIO_BITBUCKET_TOKEN`, or from the macOS Keychain item `vaglio-bitbucket` (account = the
//! email, password = the token). GitHub goes through `gh`, with whatever login it has.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

const KEYCHAIN_SERVICE: &str = "vaglio-bitbucket";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Host {
    Bitbucket { workspace: String, repo: String },
    GitHub { owner: String, repo: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pr {
    pub id: u64,
    pub title: String,
    /// OPEN, MERGED, DECLINED (Bitbucket) or CLOSED (GitHub), upper case.
    pub state: String,
    pub draft: bool,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrStatus {
    /// No `origin`, or one on a host vaglio does not know.
    NoHost,
    /// The worktree sits on an integration branch itself: nothing to open a pull request from.
    OnBase,
    NoCredentials,
    /// The branch has no pull request yet; the URL opens the form to create one.
    Missing { new_url: String },
    Found(Pr),
    Error(String),
}

impl PrStatus {
    /// What `p` opens.
    pub fn web_url(&self) -> Option<&str> {
        match self {
            PrStatus::Found(pr) => Some(&pr.url),
            PrStatus::Missing { new_url } => Some(new_url),
            _ => None,
        }
    }
}

/// `git@bitbucket.org:ws/repo.git`, `https://user@bitbucket.org/ws/repo.git`, and the same for GitHub.
pub fn host(root: &Path) -> Option<Host> {
    let out = Command::new("git").arg("-C").arg(root).args(["remote", "get-url", "origin"]).output().ok()?;
    parse_remote(String::from_utf8_lossy(&out.stdout).trim())
}

fn parse_remote(url: &str) -> Option<Host> {
    let rest = url.split_once("bitbucket.org").map(|(_, r)| ("bb", r)).or_else(|| url.split_once("github.com").map(|(_, r)| ("gh", r)))?;
    let path = rest.1.trim_start_matches([':', '/']).trim_end_matches('/').trim_end_matches(".git");
    let (owner, repo) = path.split_once('/')?;
    let (owner, repo) = (owner.to_string(), repo.to_string());
    Some(if rest.0 == "bb" { Host::Bitbucket { workspace: owner, repo } } else { Host::GitHub { owner, repo } })
}

pub fn lookup(root: &Path, branch: &str) -> PrStatus {
    if ["main", "master", "develop"].contains(&branch) {
        return PrStatus::OnBase;
    }
    match host(root) {
        None => PrStatus::NoHost,
        Some(Host::Bitbucket { workspace, repo }) => bitbucket(&workspace, &repo, branch),
        Some(Host::GitHub { owner, repo }) => github(root, &owner, &repo, branch),
    }
}

fn bitbucket_credentials() -> Option<(String, String)> {
    if let (Ok(user), Ok(token)) = (std::env::var("VAGLIO_BITBUCKET_USER"), std::env::var("VAGLIO_BITBUCKET_TOKEN")) {
        return Some((user, token));
    }
    let token = Command::new("security").args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"]).output().ok()?;
    if !token.status.success() {
        return None;
    }
    let item = Command::new("security").args(["find-generic-password", "-s", KEYCHAIN_SERVICE]).output().ok()?;
    let text = String::from_utf8_lossy(&item.stdout);
    // `    "acct"<blob>="dario@example.com"`
    let user = text.lines().find_map(|l| l.trim().strip_prefix("\"acct\"<blob>=\"")?.strip_suffix('"').map(str::to_string))?;
    Some((user, String::from_utf8_lossy(&token.stdout).trim().to_string()))
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn bitbucket(workspace: &str, repo: &str, branch: &str) -> PrStatus {
    let new_url = format!("https://bitbucket.org/{workspace}/{repo}/pull-requests/new?source={}", percent_encode(branch));
    let Some((user, token)) = bitbucket_credentials() else { return PrStatus::NoCredentials };
    let query = percent_encode(&format!("source.branch.name=\"{branch}\""));
    let url = format!(
        "https://api.bitbucket.org/2.0/repositories/{workspace}/{repo}/pullrequests?q={query}\
         &state=OPEN&state=MERGED&state=DECLINED&sort=-updated_on&pagelen=1\
         &fields=values.id,values.title,values.state,values.draft,values.links.html.href"
    );
    // Credentials go through stdin, never the command line, where `ps` would show them.
    let child = Command::new("curl")
        .args(["-sS", "--max-time", "10", "--fail-with-body", "--config", "-", &url])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else { return PrStatus::Error("curl non trovato".into()) };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "user = \"{user}:{token}\"");
    }
    let Ok(out) = child.wait_with_output() else { return PrStatus::Error("curl interrotto".into()) };
    if !out.status.success() {
        let body = String::from_utf8_lossy(&out.stdout);
        let msg = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
            .unwrap_or_else(|| {
                let err = String::from_utf8_lossy(&out.stderr);
                if err.contains(" 401") || err.contains(" 403") { "token rifiutato (401/403)".into() } else { err.trim().to_string() }
            });
        return PrStatus::Error(format!("Bitbucket: {msg}"));
    }
    let Ok(json) = serde_json::from_slice::<Value>(&out.stdout) else { return PrStatus::Error("Bitbucket: risposta illeggibile".into()) };
    match json["values"].as_array().and_then(|v| v.first()) {
        None => PrStatus::Missing { new_url },
        Some(pr) => PrStatus::Found(Pr {
            id: pr["id"].as_u64().unwrap_or(0),
            title: pr["title"].as_str().unwrap_or("").to_string(),
            state: pr["state"].as_str().unwrap_or("").to_uppercase(),
            draft: pr["draft"].as_bool().unwrap_or(false),
            url: pr["links"]["html"]["href"].as_str().unwrap_or("").to_string(),
        }),
    }
}

fn github(root: &Path, owner: &str, repo: &str, branch: &str) -> PrStatus {
    let new_url = format!("https://github.com/{owner}/{repo}/pull/new/{branch}");
    let out = Command::new("gh")
        .current_dir(root)
        .args(["pr", "list", "--repo", &format!("{owner}/{repo}"), "--head", branch, "--state", "all", "--limit", "1"])
        .args(["--json", "number,title,state,isDraft,url"])
        .output();
    let Ok(out) = out else { return PrStatus::NoCredentials };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("auth login") {
            return PrStatus::NoCredentials;
        }
        return PrStatus::Error(format!("gh: {}", err.trim()));
    }
    let json: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    match json.as_array().and_then(|v| v.first()) {
        None => PrStatus::Missing { new_url },
        Some(pr) => PrStatus::Found(Pr {
            id: pr["number"].as_u64().unwrap_or(0),
            title: pr["title"].as_str().unwrap_or("").to_string(),
            state: pr["state"].as_str().unwrap_or("").to_uppercase(),
            draft: pr["isDraft"].as_bool().unwrap_or(false),
            url: pr["url"].as_str().unwrap_or("").to_string(),
        }),
    }
}

/// Opens a URL in the default browser.
pub fn open(url: &str) -> bool {
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    Command::new(opener).arg(url).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_remotes() {
        let bb = Host::Bitbucket { workspace: "ws".into(), repo: "api".into() };
        assert_eq!(parse_remote("git@bitbucket.org:ws/api.git"), Some(bb.clone()));
        assert_eq!(parse_remote("https://someone@bitbucket.org/ws/api.git"), Some(bb));
        let gh = Host::GitHub { owner: "me".into(), repo: "vaglio".into() };
        assert_eq!(parse_remote("https://github.com/me/vaglio.git"), Some(gh.clone()));
        assert_eq!(parse_remote("git@github.com:me/vaglio"), Some(gh));
        assert_eq!(parse_remote("https://gitlab.com/me/x.git"), None);
    }
}
