//! A minimal GitHub REST client (blocking `ureq`, see decision log §6).
//!
//! Only two things touch the network: fetching PR metadata, and posting inline
//! review comments (the fallback poster — normally the agent posts). URL building
//! and response parsing are pure functions so they can be unit-tested without a
//! network.

use anyhow::{anyhow, Context, Result};
use serde_json::json;

use crate::model::{PrInfo, Side};

const API_ROOT: &str = "https://api.github.com";
const ACCEPT: &str = "application/vnd.github+json";
const API_VERSION: &str = "2022-11-28";
const USER_AGENT: &str = concat!("co-review/", env!("CARGO_PKG_VERSION"));

/// Resolve a GitHub token from the environment, then from `gh auth token`.
pub fn resolve_token() -> Option<String> {
    for var in ["GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(v) = std::env::var(var) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    if crate::exec::have("gh") {
        if let Ok(tok) = crate::exec::capture("gh", &["auth", "token"], None) {
            let tok = tok.trim().to_string();
            if !tok.is_empty() {
                return Some(tok);
            }
        }
    }
    None
}

pub struct Client {
    token: String,
}

impl Client {
    pub fn new(token: String) -> Self {
        Client { token }
    }

    /// Construct from the ambient token, erroring with guidance if none is found.
    pub fn from_env() -> Result<Self> {
        let token = resolve_token().ok_or_else(|| {
            anyhow!(
                "no GitHub token found. Set $GH_TOKEN or $GITHUB_TOKEN, or run `gh auth login`."
            )
        })?;
        Ok(Client::new(token))
    }

    fn get(&self, url: &str) -> Result<serde_json::Value> {
        let resp = ureq::get(url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Accept", ACCEPT)
            .set("X-GitHub-Api-Version", API_VERSION)
            .set("User-Agent", USER_AGENT)
            .call()
            .map_err(map_ureq_error)?;
        resp.into_json::<serde_json::Value>()
            .context("parsing GitHub JSON response")
    }

    fn post(&self, url: &str, body: serde_json::Value) -> Result<serde_json::Value> {
        let resp = ureq::post(url)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Accept", ACCEPT)
            .set("X-GitHub-Api-Version", API_VERSION)
            .set("User-Agent", USER_AGENT)
            .send_json(body)
            .map_err(map_ureq_error)?;
        resp.into_json::<serde_json::Value>()
            .context("parsing GitHub JSON response")
    }

    /// Fetch PR metadata and merge it into a [`PrInfo`].
    pub fn fetch_pr(&self, owner: &str, repo: &str, number: u64) -> Result<PrInfo> {
        let url = pr_url(owner, repo, number);
        let value = self
            .get(&url)
            .with_context(|| format!("fetching PR {owner}/{repo}#{number}"))?;
        parse_pr(owner, repo, number, &value)
    }

    /// Post an inline review comment. Returns the created comment's html URL.
    pub fn post_review_comment(&self, pr: &PrInfo, comment: &ReviewComment) -> Result<String> {
        let url = format!("{}/comments", pr_url(&pr.owner, &pr.repo, pr.number));
        let body = comment.to_json(&pr.head_sha);
        let value = self
            .post(&url, body)
            .with_context(|| format!("posting review comment on {}", comment.path))?;
        value
            .get("html_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("GitHub response had no html_url"))
    }

    /// Post a general (non-inline) comment on the PR conversation. Used as a
    /// fallback when an inline comment is rejected (e.g. the line isn't part of
    /// the diff). Returns the created comment's html URL.
    pub fn post_issue_comment(&self, pr: &PrInfo, body: &str) -> Result<String> {
        let url = format!(
            "{API_ROOT}/repos/{}/{}/issues/{}/comments",
            pr.owner, pr.repo, pr.number
        );
        let value = self
            .post(&url, json!({ "body": body }))
            .with_context(|| format!("posting a PR comment on {}", pr.number))?;
        value
            .get("html_url")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| anyhow!("GitHub response had no html_url"))
    }
}

/// The pieces of an inline review comment.
pub struct ReviewComment {
    pub body: String,
    pub path: String,
    pub line: u32,
    pub start_line: Option<u32>,
    pub side: Side,
}

impl ReviewComment {
    fn side_str(&self) -> &'static str {
        match self.side {
            Side::Head => "RIGHT",
            Side::Base => "LEFT",
        }
    }

    fn to_json(&self, commit_id: &str) -> serde_json::Value {
        let mut obj = json!({
            "body": self.body,
            "commit_id": commit_id,
            "path": self.path,
            "line": self.line,
            "side": self.side_str(),
        });
        if let Some(start) = self.start_line {
            if start < self.line {
                obj["start_line"] = json!(start);
                obj["start_side"] = json!(self.side_str());
            }
        }
        obj
    }
}

fn pr_url(owner: &str, repo: &str, number: u64) -> String {
    format!("{API_ROOT}/repos/{owner}/{repo}/pulls/{number}")
}

/// The public (html) URL of a pull request. One definition, used by the API
/// fallback and by the orchestrator's no-token path.
pub fn pr_html_url(owner: &str, repo: &str, number: u64) -> String {
    format!("https://github.com/{owner}/{repo}/pull/{number}")
}

/// Parse a PR API response into a [`PrInfo`].
fn parse_pr(owner: &str, repo: &str, number: u64, v: &serde_json::Value) -> Result<PrInfo> {
    let str_at = |path: &[&str]| -> String {
        let mut cur = v;
        for key in path {
            match cur.get(key) {
                Some(next) => cur = next,
                None => return String::new(),
            }
        }
        cur.as_str().unwrap_or("").to_string()
    };
    Ok(PrInfo {
        owner: owner.to_string(),
        repo: repo.to_string(),
        number,
        title: str_at(&["title"]),
        author: str_at(&["user", "login"]),
        base_ref: str_at(&["base", "ref"]),
        head_ref: str_at(&["head", "ref"]),
        base_sha: str_at(&["base", "sha"]),
        head_sha: str_at(&["head", "sha"]),
        url: {
            let u = str_at(&["html_url"]);
            if u.is_empty() {
                pr_html_url(owner, repo, number)
            } else {
                u
            }
        },
    })
}

fn map_ureq_error(e: ureq::Error) -> anyhow::Error {
    match e {
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            let detail = extract_github_message(&body).unwrap_or(body);
            anyhow!("GitHub API returned {code}: {}", detail.trim())
        }
        ureq::Error::Transport(t) => anyhow!("GitHub API transport error: {t}"),
    }
}

fn extract_github_message(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    v.get("message")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_pr_url() {
        assert_eq!(
            pr_url("o", "r", 5),
            "https://api.github.com/repos/o/r/pulls/5"
        );
    }

    #[test]
    fn parses_pr_response() {
        let v = json!({
            "title": "Add feature",
            "html_url": "https://github.com/o/r/pull/5",
            "user": {"login": "octocat"},
            "base": {"ref": "main", "sha": "basesha"},
            "head": {"ref": "feature", "sha": "headsha"},
        });
        let pr = parse_pr("o", "r", 5, &v).unwrap();
        assert_eq!(pr.title, "Add feature");
        assert_eq!(pr.author, "octocat");
        assert_eq!(pr.base_ref, "main");
        assert_eq!(pr.head_sha, "headsha");
        assert_eq!(pr.url, "https://github.com/o/r/pull/5");
    }

    #[test]
    fn parse_pr_fills_url_when_missing() {
        let v = json!({"title": "x"});
        let pr = parse_pr("o", "r", 9, &v).unwrap();
        assert_eq!(pr.url, "https://github.com/o/r/pull/9");
    }

    #[test]
    fn review_comment_single_line_json() {
        let c = ReviewComment {
            body: "hi".into(),
            path: "a.rs".into(),
            line: 10,
            start_line: None,
            side: Side::Head,
        };
        let j = c.to_json("sha1");
        assert_eq!(j["side"], "RIGHT");
        assert_eq!(j["line"], 10);
        assert_eq!(j["commit_id"], "sha1");
        assert!(j.get("start_line").is_none());
    }

    #[test]
    fn review_comment_multiline_json() {
        let c = ReviewComment {
            body: "hi".into(),
            path: "a.rs".into(),
            line: 12,
            start_line: Some(10),
            side: Side::Base,
        };
        let j = c.to_json("sha1");
        assert_eq!(j["side"], "LEFT");
        assert_eq!(j["start_line"], 10);
        assert_eq!(j["start_side"], "LEFT");
    }

    #[test]
    fn multiline_ignored_when_start_not_before_line() {
        let c = ReviewComment {
            body: "hi".into(),
            path: "a.rs".into(),
            line: 10,
            start_line: Some(10),
            side: Side::Head,
        };
        let j = c.to_json("sha1");
        assert!(j.get("start_line").is_none());
    }

    #[test]
    fn extracts_github_error_message() {
        let body = r#"{"message":"Not Found","documentation_url":"..."}"#;
        assert_eq!(extract_github_message(body).as_deref(), Some("Not Found"));
    }
}
