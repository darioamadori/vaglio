//! The Jira ticket a workspace is named after (`AB-12 widget`): its status, and its page.
//!
//! Needs an Atlassian email and API token with the Jira scope `read:jira-work`, plus the site
//! (`your-company.atlassian.net`): from `VAGLIO_JIRA_USER`, `VAGLIO_JIRA_TOKEN` and
//! `VAGLIO_JIRA_SITE`, or from the macOS Keychain item `vaglio-jira` (account = the email,
//! password = the token, comment = the site). A classic token is answered on the site itself, a
//! token with scopes only through `api.atlassian.com`; vaglio tries the site, then the gateway.

use serde_json::Value;

use crate::pr::{curl_get, keychain};

const KEYCHAIN_SERVICE: &str = "vaglio-jira";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ticket {
    pub key: String,
    /// The ticket's page; `None` until a site is configured.
    pub url: Option<String>,
    /// `None` until the first lookup answers.
    pub status: Option<TicketStatus>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TicketStatus {
    NoCredentials,
    /// `category` is Jira's status category: `new`, `indeterminate` or `done`.
    Found { name: String, category: String },
    Error(String),
}

struct Credentials {
    user: String,
    token: String,
    site: String,
}

fn credentials() -> Option<Credentials> {
    let site = std::env::var("VAGLIO_JIRA_SITE").ok().filter(|s| !s.is_empty());
    if let (Ok(user), Ok(token), Some(site)) = (std::env::var("VAGLIO_JIRA_USER"), std::env::var("VAGLIO_JIRA_TOKEN"), site.clone()) {
        return Some(Credentials { user, token, site: bare_site(&site) });
    }
    let item = keychain(KEYCHAIN_SERVICE)?;
    let site = site.or(item.comment)?;
    Some(Credentials { user: item.account, token: item.secret, site: bare_site(&site) })
}

/// `https://x.atlassian.net/` → `x.atlassian.net`.
fn bare_site(site: &str) -> String {
    site.trim().trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_string()
}

/// The first ticket key in a workspace label, if it has one.
pub fn key_in(label: &str) -> Option<String> {
    crate::review::ticket_keys(label).into_iter().next()
}

/// The ticket's page, when a site is known; needs no token.
pub fn browse_url(key: &str) -> Option<String> {
    let site = std::env::var("VAGLIO_JIRA_SITE").ok().filter(|s| !s.is_empty()).or_else(|| keychain(KEYCHAIN_SERVICE)?.comment)?;
    Some(format!("https://{}/browse/{key}", bare_site(&site)))
}

pub fn lookup(key: &str) -> TicketStatus {
    let Some(creds) = credentials() else { return TicketStatus::NoCredentials };
    let path = format!("rest/api/3/issue/{key}?fields=status");
    let site = format!("https://{}/{path}", creds.site);
    let mut answer = curl_get(&site, &creds.user, &creds.token);
    // A scoped token counts as anonymous on the site, and Jira answers anonymous with a 404.
    if matches!(&answer, Ok((code, _)) if ["401", "403", "404"].contains(&code.as_str())) {
        if let Some(cloud) = cloud_id(&creds.site) {
            let gateway = format!("https://api.atlassian.com/ex/jira/{cloud}/{path}");
            answer = curl_get(&gateway, &creds.user, &creds.token);
        }
    }
    match answer {
        Err(e) => TicketStatus::Error(format!("Jira: {e}")),
        Ok((code, json)) => match code.as_str() {
            "200" => TicketStatus::Found {
                name: json["fields"]["status"]["name"].as_str().unwrap_or("?").to_string(),
                category: json["fields"]["status"]["statusCategory"]["key"].as_str().unwrap_or("").to_string(),
            },
            "401" => TicketStatus::Error("Jira 401: email o token sbagliati, o token scaduto".into()),
            "403" => TicketStatus::Error("Jira 403: al token manca lo scope read:jira-work".into()),
            "404" => TicketStatus::Error(format!("Jira 404: {key} non esiste o il token non lo vede")),
            other => {
                let msg = json["errorMessages"].as_array().and_then(|m| m.first()).and_then(Value::as_str).unwrap_or("errore");
                TicketStatus::Error(format!("Jira {other}: {msg}"))
            }
        },
    }
}

/// The site's cloud id, which the gateway wants in the path. The endpoint needs no login.
fn cloud_id(site: &str) -> Option<String> {
    let out = std::process::Command::new("curl").args(["-sS", "--max-time", "10", &format!("https://{site}/_edge/tenant_info")]).output().ok()?;
    let json: Value = serde_json::from_slice(&out.stdout).ok()?;
    json["cloudId"].as_str().map(str::to_string)
}

/// `vaglio --check-jira KEY`: the same lookup the pane makes, spelled out. Never prints the token.
pub fn check(key: &str) -> (bool, String) {
    match lookup(key) {
        TicketStatus::Found { name, .. } => (true, format!("token ok: {key} è {name}")),
        TicketStatus::NoCredentials => (
            false,
            format!("nessun token: manca l'elemento {KEYCHAIN_SERVICE} nel portachiavi, o il sito nel suo commento (o VAGLIO_JIRA_USER/TOKEN/SITE)"),
        ),
        TicketStatus::Error(e) => (false, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_key_in_a_workspace_label() {
        assert_eq!(key_in("AB-12 widget"), Some("AB-12".into()));
        assert_eq!(key_in("rivedi XY-345, poi AB-1"), Some("XY-345".into()));
        assert_eq!(key_in("supplier reason"), None);
    }

    #[test]
    fn strips_the_site_down_to_its_host() {
        assert_eq!(bare_site("https://x.atlassian.net/"), "x.atlassian.net");
        assert_eq!(bare_site("x.atlassian.net"), "x.atlassian.net");
    }
}
