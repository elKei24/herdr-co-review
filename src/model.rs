//! The shared data model: the persisted session `State` and everything in it.
//!
//! The whole point of `co-review` is that an agent and a human collaborate on
//! the *same* set of findings. That shared truth lives in [`State`], which is
//! serialized to `state.json` in the session directory and mutated only through
//! the lock-guarded [`crate::store::Store`].

use serde::{Deserialize, Serialize};

use crate::util::now_ms;

/// Bumped whenever the on-disk schema changes in an incompatible way.
pub const SCHEMA_VERSION: u32 = 1;

/// The entire persisted state of a co-review session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct State {
    pub schema_version: u32,
    pub pr: PrInfo,
    pub session: SessionMeta,
    #[serde(default)]
    pub status: ReviewStatus,
    #[serde(default)]
    pub findings: Vec<Finding>,
    /// Append-only log of messages the human sent to the agent (for context /
    /// replay). Not every message needs to go here; the TUI records the ones it
    /// injects into the agent pane.
    #[serde(default)]
    pub chat: Vec<ChatEntry>,
    /// Monotonic counter used to mint finding ids (`f1`, `f2`, …). Kept on the
    /// state so ids are stable and never reused even after deletions.
    #[serde(default)]
    pub next_finding_seq: u64,
    /// Monotonic revision, bumped by the store on every write. Lets readers (the
    /// navigator) detect *any* change reliably, without depending on filesystem
    /// mtime granularity.
    #[serde(default)]
    pub rev: u64,
}

impl State {
    pub fn new(pr: PrInfo, session: SessionMeta) -> Self {
        State {
            schema_version: SCHEMA_VERSION,
            pr,
            session,
            status: ReviewStatus::Reviewing,
            findings: Vec::new(),
            chat: Vec::new(),
            next_finding_seq: 0,
            rev: 0,
        }
    }

    /// Mint the next finding id and advance the counter.
    pub fn mint_finding_id(&mut self) -> String {
        self.next_finding_seq += 1;
        format!("f{}", self.next_finding_seq)
    }

    pub fn finding(&self, id: &str) -> Option<&Finding> {
        self.findings.iter().find(|f| f.id == id)
    }

    pub fn finding_mut(&mut self, id: &str) -> Option<&mut Finding> {
        self.findings.iter_mut().find(|f| f.id == id)
    }

    /// Findings the human approved (or edited) and that have not yet been posted.
    pub fn postable(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| f.is_postable())
    }

    /// Number of findings still awaiting a human decision.
    pub fn pending_count(&self) -> usize {
        self.counts().pending
    }

    /// Tally findings by verdict/posted state in a single pass. Both the CLI and
    /// the TUI headers render from this, so the grouping lives in exactly one
    /// place.
    pub fn counts(&self) -> Counts {
        let mut c = Counts {
            total: self.findings.len(),
            ..Default::default()
        };
        for f in &self.findings {
            match f.verdict {
                Verdict::Pending => c.pending += 1,
                Verdict::Approved | Verdict::Edited => c.approved += 1,
                Verdict::Dismissed => c.dismissed += 1,
                Verdict::NeedsDiscussion => c.needs_discussion += 1,
            }
            if f.posted {
                c.posted += 1;
            }
        }
        c
    }

    /// Whether the review is ready for the agent to proceed to posting: every
    /// finding is decided AND the agent has actually started producing findings
    /// or explicitly handed off. This prevents `wait` from returning before any
    /// finding is recorded (the initial empty "reviewing" state).
    pub fn handoff_complete(&self) -> bool {
        self.pending_count() == 0
            && (self.status != ReviewStatus::Reviewing || !self.findings.is_empty())
    }
}

/// A one-pass tally of findings by state, shared by the CLI and TUI headers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub total: usize,
    pub pending: usize,
    /// Approved or human-edited (i.e. will be posted).
    pub approved: usize,
    pub dismissed: usize,
    pub needs_discussion: usize,
    pub posted: usize,
}

/// The filesystem-safe session id for a PR (`owner-repo-number`). The session
/// directory and worktree path both derive from this, so it has exactly one
/// definition.
pub fn pr_slug(owner: &str, repo: &str, number: u64) -> String {
    format!(
        "{}-{}-{}",
        crate::util::slugify(owner),
        crate::util::slugify(repo),
        number
    )
}

/// Metadata about the pull request under review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrInfo {
    pub owner: String,
    pub repo: String,
    pub number: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub author: String,
    /// The PR's base branch name (e.g. `main`).
    #[serde(default)]
    pub base_ref: String,
    /// The PR's head branch name.
    #[serde(default)]
    pub head_ref: String,
    #[serde(default)]
    pub base_sha: String,
    #[serde(default)]
    pub head_sha: String,
    #[serde(default)]
    pub url: String,
}

impl PrInfo {
    pub fn slug(&self) -> String {
        pr_slug(&self.owner, &self.repo, self.number)
    }
}

/// Where the session's files and panes live.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    /// Stable session id, derived from the PR slug.
    pub id: String,
    /// Absolute path to the checked-out worktree the agent runs in.
    pub worktree: String,
    /// Absolute path to the source repository the worktree was created from.
    /// Recorded so `co-review end` can prune the worktree cleanly. May be empty
    /// for sessions created before this field existed.
    #[serde(default)]
    pub source_repo: String,
    pub created_at_ms: u64,
    /// The Herdr pane id running the agent (e.g. `w3:p1`). Used to inject chat.
    #[serde(default)]
    pub agent_pane_id: Option<String>,
    /// The Herdr pane id running the navigator (`co-review view`).
    #[serde(default)]
    pub view_pane_id: Option<String>,
    /// The Herdr workspace id hosting the session.
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// Which agent kind is driving (e.g. `claude`, `codex`). Informational.
    #[serde(default)]
    pub agent_kind: String,
    /// The prompt handed to the agent when the session started.
    #[serde(default)]
    pub prompt: String,
}

/// Coarse lifecycle of the review, surfaced in the TUI header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    /// The agent is still producing findings.
    #[default]
    Reviewing,
    /// The agent finished; the human is triaging.
    AwaitingReview,
    /// Approved findings are being posted.
    Posting,
    /// Everything approved has been posted; session complete.
    Done,
}

impl ReviewStatus {
    pub fn label(self) -> &'static str {
        match self {
            ReviewStatus::Reviewing => "reviewing",
            ReviewStatus::AwaitingReview => "awaiting review",
            ReviewStatus::Posting => "posting",
            ReviewStatus::Done => "done",
        }
    }

    /// Parse leniently from user/agent input.
    pub fn parse(s: &str) -> Option<ReviewStatus> {
        match s.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "reviewing" | "review" => Some(ReviewStatus::Reviewing),
            "awaiting_review" | "awaiting" | "handoff" => Some(ReviewStatus::AwaitingReview),
            "posting" => Some(ReviewStatus::Posting),
            "done" | "complete" | "finished" => Some(ReviewStatus::Done),
            _ => None,
        }
    }
}

/// A single review finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub severity: Severity,
    #[serde(default)]
    pub category: Option<String>,
    /// Markdown explanation of the problem and, ideally, the fix.
    #[serde(default)]
    pub body: String,
    /// Optional concrete suggestion / patch text.
    #[serde(default)]
    pub suggestion: Option<String>,
    #[serde(default)]
    pub locations: Vec<Location>,
    #[serde(default)]
    pub verdict: Verdict,
    /// A note the human attached while triaging (shown to the agent).
    #[serde(default)]
    pub user_note: Option<String>,
    #[serde(default)]
    pub posted: bool,
    #[serde(default)]
    pub posted_url: Option<String>,
    #[serde(default)]
    pub created_at_ms: u64,
    #[serde(default)]
    pub updated_at_ms: u64,
}

impl Finding {
    pub fn new(id: String, title: String) -> Self {
        let now = now_ms();
        Finding {
            id,
            title,
            severity: Severity::default(),
            category: None,
            body: String::new(),
            suggestion: None,
            locations: Vec::new(),
            verdict: Verdict::Pending,
            user_note: None,
            posted: false,
            posted_url: None,
            created_at_ms: now,
            updated_at_ms: now,
        }
    }

    pub fn primary_location(&self) -> Option<&Location> {
        self.locations.first()
    }

    /// Approved (or human-edited) and not yet posted.
    pub fn is_postable(&self) -> bool {
        !self.posted && matches!(self.verdict, Verdict::Approved | Verdict::Edited)
    }

    pub fn touch(&mut self) {
        self.updated_at_ms = now_ms();
    }
}

/// A location a finding points at, in the PR's head or base tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    pub file: String,
    pub start_line: u32,
    /// Inclusive end line. `None` means a single line.
    #[serde(default)]
    pub end_line: Option<u32>,
    #[serde(default)]
    pub side: Side,
}

impl Location {
    /// Parse a location from `path:line`, `path:start-end`, with an optional
    /// `@head`/`@base` suffix (default head). Examples:
    /// `src/foo.rs:42`, `src/foo.rs:42-50`, `src/foo.rs:10@base`.
    pub fn parse(input: &str) -> Result<Location, String> {
        let s = input.trim();
        if s.is_empty() {
            return Err("empty location".to_string());
        }
        // Only a trailing `@head`/`@base` is a side selector; any other `@`
        // (e.g. an npm-scope path like `packages/@scope/x.rs:10`) is part of the
        // path.
        let (rest, side) = match s.rsplit_once('@') {
            Some((r, "head")) => (r, Side::Head),
            Some((r, "base")) => (r, Side::Base),
            _ => (s, Side::Head),
        };
        let (file, lines) = rest
            .rsplit_once(':')
            .ok_or_else(|| format!("location '{input}' must be path:line"))?;
        if file.is_empty() {
            return Err(format!("location '{input}' has an empty path"));
        }
        let (start, end) = match lines.split_once('-') {
            Some((a, b)) => {
                let start: u32 = a
                    .trim()
                    .parse()
                    .map_err(|_| format!("bad start line in '{input}'"))?;
                let end: u32 = b
                    .trim()
                    .parse()
                    .map_err(|_| format!("bad end line in '{input}'"))?;
                (start, Some(end))
            }
            None => {
                let start: u32 = lines
                    .trim()
                    .parse()
                    .map_err(|_| format!("bad line number in '{input}'"))?;
                (start, None)
            }
        };
        if start == 0 {
            return Err(format!("line numbers are 1-based; got 0 in '{input}'"));
        }
        Ok(Location {
            file: file.to_string(),
            start_line: start,
            end_line: end.filter(|e| *e != start),
            side,
        })
    }

    pub fn end(&self) -> u32 {
        self.end_line
            .unwrap_or(self.start_line)
            .max(self.start_line)
    }

    /// A compact `path:line` or `path:start-end` label.
    pub fn label(&self) -> String {
        match self.end_line {
            Some(end) if end != self.start_line => {
                format!("{}:{}-{}", self.file, self.start_line, end)
            }
            _ => format!("{}:{}", self.file, self.start_line),
        }
    }
}

/// Which side of the diff a location refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// The PR's proposed version (the common case).
    #[default]
    Head,
    /// The base/original version.
    Base,
}

impl Side {
    pub fn label(self) -> &'static str {
        match self {
            Side::Head => "head",
            Side::Base => "base",
        }
    }
}

/// Severity, ordered from most to least important.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    #[default]
    Medium,
    Low,
    /// A nitpick / optional cleanup.
    Nit,
}

impl Severity {
    pub const ALL: [Severity; 5] = [
        Severity::Critical,
        Severity::High,
        Severity::Medium,
        Severity::Low,
        Severity::Nit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Severity::Critical => "critical",
            Severity::High => "high",
            Severity::Medium => "medium",
            Severity::Low => "low",
            Severity::Nit => "nit",
        }
    }

    /// Parse leniently from user/agent input (accepts a few synonyms).
    pub fn parse(s: &str) -> Option<Severity> {
        match s.trim().to_ascii_lowercase().as_str() {
            "critical" | "crit" | "blocker" => Some(Severity::Critical),
            "high" | "major" | "error" => Some(Severity::High),
            "medium" | "med" | "moderate" | "warning" | "warn" => Some(Severity::Medium),
            "low" | "minor" => Some(Severity::Low),
            "nit" | "nitpick" | "info" | "trivial" | "style" => Some(Severity::Nit),
            _ => None,
        }
    }

    /// A short glyph used in the list view.
    pub fn glyph(self) -> &'static str {
        match self {
            Severity::Critical => "●",
            Severity::High => "●",
            Severity::Medium => "◆",
            Severity::Low => "○",
            Severity::Nit => "·",
        }
    }
}

/// The human's decision about a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Not yet triaged.
    #[default]
    Pending,
    /// Accepted; will be posted.
    Approved,
    /// Rejected; will not be posted.
    Dismissed,
    /// Flagged for live discussion with the agent.
    NeedsDiscussion,
    /// Accepted after the human edited the finding text; will be posted.
    Edited,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Pending => "pending",
            Verdict::Approved => "approved",
            Verdict::Dismissed => "dismissed",
            Verdict::NeedsDiscussion => "discuss",
            Verdict::Edited => "edited",
        }
    }

    pub fn parse(s: &str) -> Option<Verdict> {
        match s.trim().to_ascii_lowercase().as_str() {
            "pending" | "reset" => Some(Verdict::Pending),
            "approved" | "approve" | "accept" | "ok" | "yes" => Some(Verdict::Approved),
            "dismissed" | "dismiss" | "reject" | "no" | "wontfix" => Some(Verdict::Dismissed),
            "needs_discussion" | "discuss" | "discussion" | "?" => Some(Verdict::NeedsDiscussion),
            "edited" | "edit" => Some(Verdict::Edited),
            _ => None,
        }
    }
}

/// One message the human sent to the agent, optionally about a finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatEntry {
    pub at_ms: u64,
    #[serde(default)]
    pub finding_id: Option<String>,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_pr() -> PrInfo {
        PrInfo {
            owner: "elKei24".into(),
            repo: "herdr-co-review".into(),
            number: 123,
            title: "Add thing".into(),
            author: "someone".into(),
            base_ref: "main".into(),
            head_ref: "feature".into(),
            base_sha: "aaa".into(),
            head_sha: "bbb".into(),
            url: "https://github.com/elKei24/herdr-co-review/pull/123".into(),
        }
    }

    fn sample_session() -> SessionMeta {
        SessionMeta {
            id: "elkei24-herdr-co-review-123".into(),
            worktree: "/tmp/wt".into(),
            source_repo: "/tmp/repo".into(),
            created_at_ms: 1,
            agent_pane_id: None,
            view_pane_id: None,
            workspace_id: None,
            agent_kind: "claude".into(),
            prompt: String::new(),
        }
    }

    #[test]
    fn severity_orders_most_important_first() {
        let mut v = vec![Severity::Nit, Severity::Critical, Severity::Medium];
        v.sort();
        assert_eq!(v, vec![Severity::Critical, Severity::Medium, Severity::Nit]);
    }

    #[test]
    fn ids_are_stable_and_unique() {
        let mut s = State::new(sample_pr(), sample_session());
        assert_eq!(s.mint_finding_id(), "f1");
        assert_eq!(s.mint_finding_id(), "f2");
        assert_eq!(s.next_finding_seq, 2);
    }

    #[test]
    fn handoff_complete_semantics() {
        let mut s = State::new(sample_pr(), sample_session());
        // empty + reviewing => not complete (nothing recorded yet)
        assert!(!s.handoff_complete());
        // empty + explicitly handed off => complete (a clean review, nothing to triage)
        s.status = ReviewStatus::AwaitingReview;
        assert!(s.handoff_complete());
        // a pending finding => not complete
        s.status = ReviewStatus::Reviewing;
        s.findings.push(Finding::new("f1".into(), "t".into()));
        assert!(!s.handoff_complete());
        // decided => complete
        s.findings[0].verdict = Verdict::Approved;
        assert!(s.handoff_complete());
    }

    #[test]
    fn postable_filters_correctly() {
        let mut s = State::new(sample_pr(), sample_session());
        let mut f1 = Finding::new("f1".into(), "a".into());
        f1.verdict = Verdict::Approved;
        let mut f2 = Finding::new("f2".into(), "b".into());
        f2.verdict = Verdict::Dismissed;
        let mut f3 = Finding::new("f3".into(), "c".into());
        f3.verdict = Verdict::Approved;
        f3.posted = true;
        s.findings = vec![f1, f2, f3];
        let ids: Vec<_> = s.postable().map(|f| f.id.clone()).collect();
        assert_eq!(ids, vec!["f1"]);
    }

    #[test]
    fn location_parse_variants() {
        let l = Location::parse("src/foo.rs:42").unwrap();
        assert_eq!(l.file, "src/foo.rs");
        assert_eq!(l.start_line, 42);
        assert_eq!(l.end_line, None);
        assert_eq!(l.side, Side::Head);

        let r = Location::parse("a/b.rs:10-20").unwrap();
        assert_eq!((r.start_line, r.end_line), (10, Some(20)));

        let base = Location::parse("x.rs:5@base").unwrap();
        assert_eq!(base.side, Side::Base);
        assert_eq!(base.start_line, 5);

        // start == end collapses to single-line
        assert_eq!(Location::parse("x.rs:7-7").unwrap().end_line, None);

        // An npm-scope style path keeps its '@'.
        let scoped = Location::parse("packages/@scope/pkg/x.rs:10").unwrap();
        assert_eq!(scoped.file, "packages/@scope/pkg/x.rs");
        assert_eq!(scoped.start_line, 10);

        assert!(Location::parse("noline").is_err());
        assert!(Location::parse("x.rs:0").is_err());
        assert!(Location::parse("x.rs:5@weird").is_err()); // '5@weird' isn't a line
        assert!(Location::parse(":5").is_err());
    }

    #[test]
    fn location_label() {
        let single = Location {
            file: "a.rs".into(),
            start_line: 5,
            end_line: None,
            side: Side::Head,
        };
        assert_eq!(single.label(), "a.rs:5");
        let range = Location {
            file: "a.rs".into(),
            start_line: 5,
            end_line: Some(9),
            side: Side::Head,
        };
        assert_eq!(range.label(), "a.rs:5-9");
    }

    #[test]
    fn counts_tally_in_one_pass() {
        let mut s = State::new(sample_pr(), sample_session());
        let mk = |id: &str, v: Verdict, posted: bool| {
            let mut f = Finding::new(id.into(), "t".into());
            f.verdict = v;
            f.posted = posted;
            f
        };
        s.findings = vec![
            mk("f1", Verdict::Approved, true),
            mk("f2", Verdict::Edited, false),
            mk("f3", Verdict::Dismissed, false),
            mk("f4", Verdict::Pending, false),
            mk("f5", Verdict::NeedsDiscussion, false),
        ];
        let c = s.counts();
        assert_eq!(c.total, 5);
        assert_eq!(c.approved, 2); // approved + edited
        assert_eq!(c.dismissed, 1);
        assert_eq!(c.needs_discussion, 1);
        assert_eq!(c.pending, 1);
        assert_eq!(c.posted, 1);
        assert_eq!(s.pending_count(), 1);
    }

    #[test]
    fn slug_and_status_parse() {
        assert_eq!(
            pr_slug("elKei24", "Herdr-Co-Review", 7),
            "elkei24-herdr-co-review-7"
        );
        assert_eq!(sample_pr().slug(), "elkei24-herdr-co-review-123");
        assert_eq!(ReviewStatus::parse("done"), Some(ReviewStatus::Done));
        assert_eq!(
            ReviewStatus::parse("awaiting-review"),
            Some(ReviewStatus::AwaitingReview)
        );
        assert_eq!(ReviewStatus::parse("nope"), None);
        assert_eq!(Side::Head.label(), "head");
        assert_eq!(Side::Base.label(), "base");
    }

    #[test]
    fn verdict_and_severity_parse_synonyms() {
        assert_eq!(Verdict::parse("approve"), Some(Verdict::Approved));
        assert_eq!(Verdict::parse("WONTFIX"), Some(Verdict::Dismissed));
        assert_eq!(Severity::parse("Blocker"), Some(Severity::Critical));
        assert_eq!(Severity::parse("warn"), Some(Severity::Medium));
        assert_eq!(Severity::parse("bogus"), None);
    }

    #[test]
    fn state_json_roundtrips() {
        let mut s = State::new(sample_pr(), sample_session());
        let id = s.mint_finding_id();
        s.findings.push(Finding::new(id, "title".into()));
        let json = serde_json::to_string_pretty(&s).unwrap();
        let back: State = serde_json::from_str(&json).unwrap();
        assert_eq!(back.findings.len(), 1);
        assert_eq!(back.pr.number, 123);
        assert_eq!(back.schema_version, SCHEMA_VERSION);
    }
}
