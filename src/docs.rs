//! The design documents the chats of a workspace have written: artifacts, Notion pages, Claude
//! Docs, Markdown files. A Claude Code hook appends one JSON object per line to
//! `<state>/docs/<workspace id>`: `{"kind", "title", "target", "at"}`, target being a URL or an
//! absolute path, `at` seconds since the epoch. The same document written again (same Notion page
//! or claude.ai artifact id, whatever the rest of the URL, else the same target) keeps its first
//! target and takes the newest title and time. A line with `"update": true` is an edit or a
//! rename: without a title it only moves a document already on the list to the top, and a
//! document not on the list joins it only when the line names it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Artifact,
    Notion,
    ClaudeDoc,
    Markdown,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Artifact => "artifact",
            Kind::Notion => "notion",
            Kind::ClaudeDoc => "artifact",
            Kind::Markdown => "md",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Doc {
    pub kind: Kind,
    pub title: String,
    pub target: String,
    pub at: u64,
}

/// Newest first, one entry per target.
pub fn read(file: &Path) -> Vec<Doc> {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    let mut docs: Vec<Doc> = Vec::new();
    for line in text.lines() {
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let kind = match v["kind"].as_str() {
            Some("artifact") => Kind::Artifact,
            Some("notion") => Kind::Notion,
            Some("doc") => Kind::ClaudeDoc,
            Some("md") => Kind::Markdown,
            _ => continue,
        };
        let Some(target) = v["target"].as_str().filter(|t| !t.is_empty()) else { continue };
        let title = v["title"].as_str().map(str::trim).filter(|t| !t.is_empty());
        let at = v["at"].as_u64().unwrap_or(0);
        let update = v["update"].as_bool().unwrap_or(false);
        let id = key(target);
        match docs.iter().position(|d| key(&d.target) == id) {
            Some(i) => {
                let old = docs.remove(i);
                let title = title.map_or(old.title, str::to_string);
                docs.push(Doc { kind: old.kind, title, target: old.target, at });
            }
            None if update && title.is_none() => {}
            None => docs.push(Doc { kind, title: title.unwrap_or(target).to_string(), target: target.to_string(), at }),
        }
    }
    // A Markdown file deleted since it was written is no longer a document.
    docs.retain(|d| d.kind != Kind::Markdown || Path::new(&d.target).is_file());
    docs.sort_by(|a, b| b.at.cmp(&a.at));
    docs
}

/// What tells one document from another: a Notion page id, a claude.ai artifact id, else the target.
fn key(target: &str) -> String {
    if let Some(id) = notion_page_id(target).filter(|_| target.contains("notion")) {
        return format!("notion:{}", id.to_ascii_lowercase());
    }
    if https_host(target) == Some("claude.ai") {
        let path = target.split(['?', '#']).next().unwrap_or(target);
        if let Some((_, id)) = path.trim_end_matches('/').rsplit_once("/artifact/") {
            return format!("claude:{id}");
        }
    }
    target.to_string()
}

/// `https://app.notion.com/p/<id>?pvs=…` → the page id, for the desktop app's own scheme.
fn notion_page_id(url: &str) -> Option<&str> {
    let path = url.split(['?', '#']).next()?;
    let last = path.trim_end_matches('/').rsplit('/').next()?;
    // Page URLs end in the id, sometimes after a slug: `Title-3e28…`.
    let id = last.rsplit('-').next()?;
    (id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())).then_some(id)
}

fn https_host(url: &str) -> Option<&str> {
    url.strip_prefix("https://")?.split(['/', '?', '#']).next()
}

fn run(cmd: &mut Command) -> bool {
    cmd.stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

/// Opens a document where it belongs: Notion pages in the Notion app, artifacts and Claude Docs
/// in the browser, Markdown in VS Code, each falling back to the system default. Only targets
/// the hook could have written are opened: https on claude.ai or notion, or an existing `.md`.
pub fn open(doc: &Doc) -> Result<(), String> {
    match doc.kind {
        Kind::Artifact | Kind::ClaudeDoc => {
            if https_host(&doc.target) != Some("claude.ai") {
                return Err(format!("non apro {}: non è un link claude.ai", doc.target));
            }
            run(Command::new("open").arg(&doc.target)).then_some(()).ok_or_else(|| format!("non riesco ad aprire {}", doc.target))
        }
        Kind::Notion => {
            let host = https_host(&doc.target).unwrap_or("");
            if !["app.notion.com", "www.notion.so", "notion.so"].contains(&host) {
                return Err(format!("non apro {}: non è un link Notion", doc.target));
            }
            let app = Path::new("/Applications/Notion.app").exists();
            if let (true, Some(id)) = (app, notion_page_id(&doc.target)) {
                if run(Command::new("open").arg(format!("notion://www.notion.so/{id}"))) {
                    return Ok(());
                }
            }
            run(Command::new("open").arg(&doc.target)).then_some(()).ok_or_else(|| format!("non riesco ad aprire {}", doc.target))
        }
        Kind::Markdown => {
            let path = PathBuf::from(&doc.target);
            if !path.is_absolute() || path.extension().is_none_or(|e| e != "md") || !path.is_file() {
                return Err(format!("{} non c'è più", doc.target));
            }
            if run(Command::new("open").args(["-b", "com.microsoft.VSCode"]).arg(&path)) || run(Command::new("open").arg(&path)) {
                Ok(())
            } else {
                Err(format!("non riesco ad aprire {}", doc.target))
            }
        }
    }
}

/// `3 min`, `2 h`, `4 g`: how long ago, for the list.
pub fn age(at: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let s = now.saturating_sub(at);
    match s {
        0..60 => "ora".into(),
        60..3600 => format!("{} min", s / 60),
        3600..86400 => format!("{} h", s / 3600),
        _ => format!("{} g", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_newest_first_one_per_target() {
        let dir = std::env::temp_dir().join(format!("vaglio-docs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ws");
        std::fs::write(
            &file,
            [
                r#"{"kind":"artifact","title":"Old","target":"https://claude.ai/code/artifact/a","at":1}"#,
                r#"{"kind":"notion","title":"Page","target":"https://app.notion.com/p/b","at":2}"#,
                "not json",
                r#"{"kind":"artifact","title":"New","target":"https://claude.ai/code/artifact/a","at":3}"#,
                r#"{"kind":"md","title":"Gone","target":"/nowhere/x.md","at":4}"#,
            ]
            .join("\n"),
        )
        .unwrap();
        let docs = read(&file);
        assert_eq!(docs.iter().map(|d| d.title.as_str()).collect::<Vec<_>>(), ["New", "Page"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn edits_and_renames_update_the_same_document() {
        let dir = std::env::temp_dir().join(format!("vaglio-docs-upd-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ws");
        let id = "0123456789abcdef0123456789abcdef";
        std::fs::write(
            &file,
            [
                format!(r#"{{"kind":"notion","title":"Draft","target":"https://app.notion.com/p/{id}?pvs=204","at":1}}"#),
                r#"{"kind":"doc","title":"Doc","target":"https://claude.ai/code/artifact/abcd1234","at":2}"#.to_string(),
                // A rename reaches the page by another URL.
                format!(r#"{{"kind":"notion","title":"Final","target":"https://app.notion.com/p/{id}","at":3,"update":true}}"#),
                // A content edit, no title: the doc moves to the top, its name unchanged.
                r#"{"kind":"doc","title":"","target":"https://claude.ai/code/artifact/abcd1234","at":4,"update":true}"#.to_string(),
                // An edit of a document never listed, without a name: not a document of the workspace.
                r#"{"kind":"notion","title":"","target":"https://app.notion.com/p/ffffffffffffffffffffffffffffffff","at":5,"update":true}"#.to_string(),
            ]
            .join("\n"),
        )
        .unwrap();
        let docs = read(&file);
        let got: Vec<_> = docs.iter().map(|d| (d.title.as_str(), d.at)).collect();
        assert_eq!(got, [("Doc", 4), ("Final", 3)]);
        assert_eq!(docs[1].target, format!("https://app.notion.com/p/{id}?pvs=204"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn finds_notion_page_ids() {
        let id = "0123456789abcdef0123456789abcdef";
        assert_eq!(notion_page_id(&format!("https://app.notion.com/p/{id}?pvs=204")), Some(id));
        assert_eq!(notion_page_id(&format!("https://www.notion.so/ws/Some-Title-{id}")), Some(id));
        assert_eq!(notion_page_id("https://app.notion.com/p/short"), None);
    }

    #[test]
    fn refuses_foreign_targets() {
        let doc = |kind, target: &str| Doc { kind, title: String::new(), target: target.into(), at: 0 };
        assert!(open(&doc(Kind::Artifact, "file:///etc/passwd")).is_err());
        assert!(open(&doc(Kind::Artifact, "https://claude.ai.evil.com/x")).is_err());
        assert!(open(&doc(Kind::Notion, "https://example.com/p/x")).is_err());
        assert!(open(&doc(Kind::Markdown, "/etc/hosts")).is_err());
    }
}
