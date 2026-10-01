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
    /// The branch has no pull request yet: `p` pushes it and opens a draft one (`create_draft`).
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
    keychain(KEYCHAIN_SERVICE).map(|item| (item.account, item.secret))
}

/// A generic password in the macOS Keychain: account, secret, and the comment (`-j`).
pub struct KeychainItem {
    pub account: String,
    pub secret: String,
    pub comment: Option<String>,
}

pub fn keychain(service: &str) -> Option<KeychainItem> {
    let secret = Command::new("security").args(["find-generic-password", "-s", service, "-w"]).output().ok()?;
    if !secret.status.success() {
        return None;
    }
    let item = Command::new("security").args(["find-generic-password", "-s", service]).output().ok()?;
    let text = String::from_utf8_lossy(&item.stdout);
    // `    "acct"<blob>="you@example.com"`
    let attr = |name: &str| {
        let prefix = format!("\"{name}\"<blob>=\"");
        text.lines().find_map(|l| l.trim().strip_prefix(prefix.as_str())?.strip_suffix('"').map(str::to_string))
    };
    Some(KeychainItem {
        account: attr("acct")?,
        secret: String::from_utf8_lossy(&secret.stdout).trim().to_string(),
        comment: attr("icmt").filter(|c| !c.is_empty()),
    })
}

fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// One GET with basic auth, the credentials on curl's stdin, never its command line, where
/// `ps` would show them. Returns the HTTP status and the JSON body (`Null` when it is not JSON).
pub fn curl_get(url: &str, user: &str, token: &str) -> Result<(String, Value), String> {
    curl(url, user, token, None)
}

/// `curl_get` with a JSON body, POSTed. The body carries no secret, so it may sit on the command line.
fn curl_post(url: &str, user: &str, token: &str, body: &Value) -> Result<(String, Value), String> {
    curl(url, user, token, Some(body))
}

fn curl(url: &str, user: &str, token: &str, body: Option<&Value>) -> Result<(String, Value), String> {
    let mut cmd = Command::new("curl");
    cmd.args(["-sS", "--max-time", "15", "-w", "\n%{http_code}", "--config", "-"]);
    if let Some(body) = body {
        cmd.args(["-X", "POST", "-H", "Content-Type: application/json", "--data-binary", &body.to_string()]);
    }
    let child = cmd
        .arg(url)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else { return Err("curl non trovato".into()) };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = writeln!(stdin, "user = \"{user}:{token}\"");
    }
    let Ok(out) = child.wait_with_output() else { return Err("curl interrotto".into()) };
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
    Ok((code.trim().to_string(), serde_json::from_str::<Value>(body).unwrap_or(Value::Null)))
}

/// One GET on the Bitbucket REST API. Errors come back as the status to show.
fn bitbucket_get(url: &str) -> Result<Value, PrStatus> {
    let Some((user, token)) = bitbucket_credentials() else { return Err(PrStatus::NoCredentials) };
    let (code, json) = curl_get(url, &user, &token).map_err(|e| PrStatus::Error(format!("Bitbucket: {e}")))?;
    match code.as_str() {
        "200" => Ok(json),
        other => Err(PrStatus::Error(bitbucket_error(other, &json))),
    }
}

/// What a failed Bitbucket call says, for the footer.
fn bitbucket_error(code: &str, json: &Value) -> String {
    match code {
        "401" => "Bitbucket 401: email o token sbagliati, o token scaduto".into(),
        "403" => {
            // Bitbucket names every scope the endpoint needs, not only the missing ones.
            let required: Vec<&str> =
                json["error"]["detail"]["required"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
            let msg = if required.is_empty() {
                json["error"]["message"].as_str().unwrap_or("permesso negato").to_string()
            } else {
                format!("questa chiamata vuole gli scope {} e il token non li ha tutti", required.join(", "))
            };
            format!("Bitbucket 403: {msg}")
        }
        other => format!("Bitbucket {other}: {}", json["error"]["message"].as_str().unwrap_or("errore")),
    }
}

/// Pushes the branch, so it has its own upstream on origin, and opens a draft pull request for it
/// into the integration branch the diff is taken against, titled after the branch's first commit.
/// Returns the pull request's web URL.
pub fn create_draft(root: &Path, branch: &str) -> Result<String, String> {
    let host = host(root).ok_or("PR: origin non è né Bitbucket né GitHub")?;
    let push = Command::new("git").arg("-C").arg(root).args(["push", "-q", "-u", "origin", branch]).output();
    let push = push.map_err(|_| "git non trovato".to_string())?;
    if !push.status.success() {
        let err = String::from_utf8_lossy(&push.stderr);
        return Err(format!("push fallito: {}", err.trim().lines().last().unwrap_or("errore")));
    }
    let base_ref = crate::git::base_branch(root);
    let base = base_ref.strip_prefix("origin/").unwrap_or(&base_ref).to_string();
    let log = Command::new("git").arg("-C").arg(root).args(["log", "--reverse", "--format=%s", &format!("{base_ref}..{branch}")]).output();
    let title = log
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string))
        .unwrap_or_else(|| branch.to_string());
    match host {
        Host::Bitbucket { workspace, repo } => {
            let Some((user, token)) = bitbucket_credentials() else { return Err("PR: manca il token Bitbucket (vedi README)".into()) };
            let url = format!("https://api.bitbucket.org/2.0/repositories/{workspace}/{repo}/pullrequests");
            let body = serde_json::json!({
                "title": title,
                "source": {"branch": {"name": branch}},
                "destination": {"branch": {"name": base}},
                "draft": true,
            });
            let (code, json) = curl_post(&url, &user, &token, &body).map_err(|e| format!("Bitbucket: {e}"))?;
            match code.as_str() {
                "200" | "201" => json["links"]["html"]["href"].as_str().map(str::to_string).ok_or_else(|| "PR creata, ma senza link".into()),
                other => Err(bitbucket_error(other, &json)),
            }
        }
        Host::GitHub { owner, repo } => {
            let out = Command::new("gh")
                .current_dir(root)
                .args(["pr", "create", "--draft", "--repo", &format!("{owner}/{repo}"), "--head", branch, "--base", &base])
                .args(["--title", &title, "--body", ""])
                .output()
                .map_err(|_| "gh non trovato".to_string())?;
            if !out.status.success() {
                return Err(format!("gh: {}", String::from_utf8_lossy(&out.stderr).trim()));
            }
            Ok(String::from_utf8_lossy(&out.stdout).trim().lines().last().unwrap_or("").to_string())
        }
    }
}

fn bitbucket(workspace: &str, repo: &str, branch: &str) -> PrStatus {
    let new_url = format!("https://bitbucket.org/{workspace}/{repo}/pull-requests/new?source={}", percent_encode(branch));
    // BBQL string literal: a quote or backslash in the branch must not end it early.
    let literal = branch.replace('\\', "\\\\").replace('"', "\\\"");
    let query = percent_encode(&format!("source.branch.name=\"{literal}\""));
    let url = format!(
        "https://api.bitbucket.org/2.0/repositories/{workspace}/{repo}/pullrequests?q={query}\
         &state=OPEN&state=MERGED&state=DECLINED&sort=-updated_on&pagelen=1\
         &fields=values.id,values.title,values.state,values.draft,values.links.html.href"
    );
    let json = match bitbucket_get(&url) {
        Ok(json) => json,
        Err(status) => return status,
    };
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

/// The source and destination branches of pull request `id`.
pub fn branches(root: &Path, host: &Host, id: u64) -> Result<(String, String), String> {
    let describe = |status: PrStatus| match status {
        PrStatus::NoCredentials => "PR: manca il token Bitbucket (vedi README)".to_string(),
        PrStatus::Error(e) => e,
        other => format!("{other:?}"),
    };
    let (src, dst) = match host {
        Host::Bitbucket { workspace, repo } => {
            let url = format!(
                "https://api.bitbucket.org/2.0/repositories/{workspace}/{repo}/pullrequests/{id}\
                 ?fields=source.branch.name,destination.branch.name"
            );
            let json = bitbucket_get(&url).map_err(describe)?;
            (json["source"]["branch"]["name"].clone(), json["destination"]["branch"]["name"].clone())
        }
        Host::GitHub { owner, repo } => {
            let out = Command::new("gh")
                .current_dir(root)
                .args(["pr", "view", &id.to_string(), "--repo", &format!("{owner}/{repo}"), "--json", "headRefName,baseRefName"])
                .output()
                .map_err(|_| "gh non trovato".to_string())?;
            if !out.status.success() {
                return Err(format!("gh: {}", String::from_utf8_lossy(&out.stderr).trim()));
            }
            let json: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
            (json["headRefName"].clone(), json["baseRefName"].clone())
        }
    };
    match (src.as_str(), dst.as_str()) {
        (Some(s), Some(d)) => Ok((s.to_string(), d.to_string())),
        _ => Err(format!("PR #{id}: branch non trovati nella risposta")),
    }
}

/// The same request a lookup makes, on a branch name no pull request has, so only the
/// credentials decide the outcome. Never prints the token.
pub fn check_token(dir: &Path) -> (bool, String) {
    let Some(root) = crate::git::toplevel(dir) else { return (false, format!("{} non è un repo git", dir.display())) };
    let Some(Host::Bitbucket { workspace, repo }) = host(&root) else {
        return (false, format!("{}: origin non è su Bitbucket", root.display()));
    };
    match bitbucket(&workspace, &repo, "vaglio-check-token") {
        PrStatus::Missing { .. } | PrStatus::Found(_) => (true, format!("token ok: legge le PR di {workspace}/{repo}")),
        PrStatus::NoCredentials => {
            (false, format!("nessun token: manca l'elemento {KEYCHAIN_SERVICE} nel portachiavi (o VAGLIO_BITBUCKET_USER/TOKEN)"))
        }
        PrStatus::Error(e) => (false, e),
        other => (false, format!("{other:?}")),
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

/// Opens a URL in the default browser. Only https: the URL comes from an API response, and
/// `open` would just as happily launch a local file or another app's URL scheme.
pub fn open(url: &str) -> bool {
    if !url.starts_with("https://") {
        return false;
    }
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
