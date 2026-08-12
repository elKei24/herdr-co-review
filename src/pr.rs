//! Parsing pull-request references and GitHub remote URLs.
//!
//! Accepts everything a human would plausibly type:
//! `123`, `#123`, `PR123`, `pr/123`, `pull/123`, `owner/repo#123`,
//! `owner/repo/123`, and full URLs like
//! `https://github.com/owner/repo/pull/123`.

use anyhow::{anyhow, Result};

/// A parsed reference to a pull request. `owner`/`repo` are optional because a
/// bare `123` relies on the surrounding git remote to fill them in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrRef {
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub number: u64,
}

impl PrRef {
    fn number_only(n: u64) -> Self {
        PrRef {
            owner: None,
            repo: None,
            number: n,
        }
    }
}

/// Parse a PR reference from arbitrary user input.
pub fn parse(input: &str) -> Result<PrRef> {
    let s = input.trim();
    if s.is_empty() {
        return Err(anyhow!("empty PR reference"));
    }

    // Full GitHub URL.
    if let Some(rest) = s
        .strip_prefix("https://github.com/")
        .or_else(|| s.strip_prefix("http://github.com/"))
        .or_else(|| s.strip_prefix("github.com/"))
    {
        return parse_url_path(rest);
    }

    // owner/repo#123  or  owner/repo/pull/123  or  owner/repo/123
    if s.contains('/') {
        if let Ok(r) = parse_url_path(s) {
            return Ok(r);
        }
    }

    // Bare forms: strip a leading '#', 'PR', 'pr/', 'pull/'.
    let n = parse_number_token(s)
        .ok_or_else(|| anyhow!("could not parse a PR number from '{}'", input))?;
    Ok(PrRef::number_only(n))
}

/// Parse the path portion of a GitHub URL (already stripped of the host), e.g.
/// `owner/repo/pull/123` or `owner/repo#123`.
fn parse_url_path(path: &str) -> Result<PrRef> {
    let path = path.trim_matches('/');

    // owner/repo#123
    if let Some((repo_part, num_part)) = path.split_once('#') {
        let mut it = repo_part.split('/');
        let owner = it.next().filter(|s| !s.is_empty());
        let repo = it.next().filter(|s| !s.is_empty());
        if let (Some(owner), Some(repo)) = (owner, repo) {
            if let Some(n) = parse_number_token(num_part) {
                return Ok(PrRef {
                    owner: Some(owner.to_string()),
                    repo: Some(repo.to_string()),
                    number: n,
                });
            }
        }
    }

    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    // owner/repo/(pull|pulls|pr)/123[/...]
    if segments.len() >= 4 && matches!(segments[2], "pull" | "pulls" | "pr") {
        if let Some(n) = parse_number_token(segments[3]) {
            return Ok(PrRef {
                owner: Some(segments[0].to_string()),
                repo: Some(segments[1].to_string()),
                number: n,
            });
        }
    }
    // owner/repo/123
    if segments.len() == 3 {
        if let Some(n) = parse_number_token(segments[2]) {
            return Ok(PrRef {
                owner: Some(segments[0].to_string()),
                repo: Some(segments[1].to_string()),
                number: n,
            });
        }
    }

    Err(anyhow!("could not parse a PR reference from '{}'", path))
}

/// Parse a number out of tokens like `123`, `#123`, `PR123`, `pr/123`, `pull/123`.
fn parse_number_token(tok: &str) -> Option<u64> {
    let t = tok.trim();
    let t = t.strip_prefix('#').unwrap_or(t);
    let lower = t.to_ascii_lowercase();
    let digits = lower
        .strip_prefix("pull/")
        .or_else(|| lower.strip_prefix("pulls/"))
        .or_else(|| lower.strip_prefix("pull-"))
        .or_else(|| lower.strip_prefix("pr/"))
        .or_else(|| lower.strip_prefix("pr-"))
        .or_else(|| lower.strip_prefix("pr"))
        .unwrap_or(&lower);
    let digits = digits.trim().trim_start_matches('#');
    if digits.is_empty() || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()
}

/// Parse `owner` and `repo` from a GitHub remote URL. Handles both SSH
/// (`git@github.com:owner/repo.git`) and HTTPS
/// (`https://github.com/owner/repo.git`) forms.
pub fn parse_github_remote(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let tail = if let Some(rest) = url.strip_prefix("git@github.com:") {
        rest
    } else if let Some(rest) = url.strip_prefix("ssh://git@github.com/") {
        rest
    } else if let Some(rest) = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))
    {
        rest
    } else {
        // Any other URL that mentions github.com/…
        let idx = url.find("github.com/")?;
        &url[idx + "github.com/".len()..]
    };

    let tail = tail.trim_end_matches('/');
    let tail = tail.strip_suffix(".git").unwrap_or(tail);
    let mut it = tail.split('/');
    let owner = it.next().filter(|s| !s.is_empty())?;
    let repo = it.next().filter(|s| !s.is_empty())?;
    Some((owner.to_string(), repo.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_numbers() {
        assert_eq!(parse("123").unwrap(), PrRef::number_only(123));
        assert_eq!(parse("#123").unwrap(), PrRef::number_only(123));
        assert_eq!(parse("PR123").unwrap(), PrRef::number_only(123));
        assert_eq!(parse("pr/123").unwrap(), PrRef::number_only(123));
        assert_eq!(parse("pull/123").unwrap(), PrRef::number_only(123));
        assert_eq!(parse("  42 ").unwrap(), PrRef::number_only(42));
    }

    #[test]
    fn owner_repo_forms() {
        let want = PrRef {
            owner: Some("elKei24".into()),
            repo: Some("herdr-co-review".into()),
            number: 7,
        };
        assert_eq!(parse("elKei24/herdr-co-review#7").unwrap(), want);
        assert_eq!(parse("elKei24/herdr-co-review/pull/7").unwrap(), want);
        assert_eq!(parse("elKei24/herdr-co-review/7").unwrap(), want);
    }

    #[test]
    fn urls() {
        let want = PrRef {
            owner: Some("elKei24".into()),
            repo: Some("herdr-co-review".into()),
            number: 7,
        };
        assert_eq!(
            parse("https://github.com/elKei24/herdr-co-review/pull/7").unwrap(),
            want
        );
        assert_eq!(
            parse("https://github.com/elKei24/herdr-co-review/pull/7/files").unwrap(),
            want
        );
        assert_eq!(
            parse("github.com/elKei24/herdr-co-review/pull/7").unwrap(),
            want
        );
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("").is_err());
        assert!(parse("not-a-pr").is_err());
        assert!(parse("owner/repo/tree/main").is_err());
    }

    #[test]
    fn remote_parsing() {
        assert_eq!(
            parse_github_remote("git@github.com:elKei24/herdr-co-review.git"),
            Some(("elKei24".into(), "herdr-co-review".into()))
        );
        assert_eq!(
            parse_github_remote("https://github.com/elKei24/herdr-co-review.git"),
            Some(("elKei24".into(), "herdr-co-review".into()))
        );
        assert_eq!(
            parse_github_remote("https://github.com/elKei24/herdr-co-review"),
            Some(("elKei24".into(), "herdr-co-review".into()))
        );
        assert_eq!(parse_github_remote("https://gitlab.com/x/y.git"), None);
    }
}
