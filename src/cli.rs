//! Command-line surface, defined with `clap`. Dispatch lives in [`crate::run`].

use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "co-review",
    version,
    about = "Interactive, split-screen PR co-review between you and your AI agent, inside Herdr.",
    long_about = "co-review turns a PR review into a side-by-side collaboration: your agent \
reviews in one Herdr pane and records findings; you triage them in a navigator pane that shows \
each finding with its surrounding code, then talk it over and let the agent post the approved \
ones. Run `co-review start <pr>` to begin."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Start a co-review session: check out the PR, lay out the Herdr split,
    /// launch the agent (left) and the navigator (right).
    Start(StartArgs),

    /// Run the findings navigator TUI (normally launched into the right pane by
    /// `start`; you can also run it standalone against a session).
    View(SessionArgs),

    /// [agent] Record a review finding.
    AddFinding(AddFindingArgs),

    /// [agent] Bulk-import findings from a JSON array (`-` reads stdin).
    Import(ImportArgs),

    /// List findings (human-readable, or `--json` for the full state).
    List(ListArgs),

    /// Show a single finding together with its related code.
    Show(ShowArgs),

    /// Set a finding's verdict (approved/dismissed/discuss/edited/pending).
    Verdict(VerdictArgs),

    /// [agent] Block until every finding has a verdict.
    Wait(WaitArgs),

    /// Post approved findings to GitHub as inline review comments (fallback for
    /// the agent; requires a GitHub token).
    Post(PostArgs),

    /// [agent] Record that a finding has been posted.
    MarkPosted(MarkPostedArgs),

    /// Set the review lifecycle status.
    SetStatus(SetStatusArgs),

    /// Print a short status summary for a session.
    Status(SessionArgs),

    /// Print the agent protocol reference.
    Protocol,

    /// Print the default review prompt (with placeholders unfilled).
    Prompt,

    /// Diagnose the environment (git, herdr, token, config).
    Doctor,
}

/// Selects which session a command operates on.
#[derive(Args, Debug, Default, Clone)]
pub struct SessionArgs {
    /// Path to the session directory. Defaults to $CO_REVIEW_SESSION, or the sole
    /// existing session.
    #[arg(long, value_name = "DIR", global = true)]
    pub session: Option<String>,
}

#[derive(Args, Debug)]
pub struct StartArgs {
    /// The pull request: `123`, `#123`, `owner/repo#123`, or a full GitHub URL.
    pub pr: String,

    /// Agent to drive the review (must exist in config). Defaults to config's
    /// `default_agent`.
    #[arg(long)]
    pub agent: Option<String>,

    /// Override the review prompt handed to the agent.
    #[arg(long)]
    pub prompt: Option<String>,

    /// Read the review prompt from a file (`-` for stdin).
    #[arg(long, value_name = "FILE", conflicts_with = "prompt")]
    pub prompt_file: Option<String>,

    /// Print the Herdr commands instead of running them (implies no real panes).
    #[arg(long)]
    pub dry_run: bool,

    /// Don't launch the agent pane; only prepare the worktree and navigator.
    #[arg(long)]
    pub no_agent: bool,

    /// Reuse an existing session/worktree for this PR instead of erroring.
    #[arg(long)]
    pub resume: bool,
}

#[derive(Args, Debug)]
pub struct AddFindingArgs {
    #[command(flatten)]
    pub session: SessionArgs,

    /// Short title.
    #[arg(long)]
    pub title: String,

    /// Severity: critical|high|medium|low|nit.
    #[arg(long, default_value = "medium")]
    pub severity: String,

    /// Free-text category (e.g. correctness, security, simplification).
    #[arg(long)]
    pub category: Option<String>,

    /// A location `path:line` or `path:start-end` (with optional `@base`).
    /// Repeatable.
    #[arg(long = "location", value_name = "PATH:LINE")]
    pub locations: Vec<String>,

    /// Markdown body.
    #[arg(long)]
    pub body: Option<String>,

    /// Read the body from a file (`-` for stdin). Overrides --body.
    #[arg(long, value_name = "FILE")]
    pub body_file: Option<String>,

    /// A concrete suggested replacement.
    #[arg(long)]
    pub suggestion: Option<String>,
}

#[derive(Args, Debug)]
pub struct ImportArgs {
    #[command(flatten)]
    pub session: SessionArgs,
    /// JSON file containing an array of findings (`-` reads stdin).
    pub file: String,
}

#[derive(Args, Debug)]
pub struct ListArgs {
    #[command(flatten)]
    pub session: SessionArgs,
    /// Print the full session state as JSON.
    #[arg(long)]
    pub json: bool,
    /// Only show findings with this verdict.
    #[arg(long)]
    pub verdict: Option<String>,
}

#[derive(Args, Debug)]
pub struct ShowArgs {
    #[command(flatten)]
    pub session: SessionArgs,
    /// Finding id (e.g. `f3`).
    pub id: String,
    /// Print as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args, Debug)]
pub struct VerdictArgs {
    #[command(flatten)]
    pub session: SessionArgs,
    /// Finding id (e.g. `f3`).
    pub id: String,
    /// Verdict: approved|dismissed|discuss|edited|pending.
    pub verdict: String,
    /// Attach or replace the human note shown to the agent.
    #[arg(long)]
    pub note: Option<String>,
}

#[derive(Args, Debug)]
pub struct WaitArgs {
    #[command(flatten)]
    pub session: SessionArgs,
    /// Give up after this many milliseconds (0 = wait forever).
    #[arg(long, default_value_t = 0)]
    pub timeout: u64,
    /// Poll interval in milliseconds.
    #[arg(long, default_value_t = 750)]
    pub interval: u64,
}

#[derive(Args, Debug)]
pub struct PostArgs {
    #[command(flatten)]
    pub session: SessionArgs,
    /// Show what would be posted without calling GitHub.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args, Debug)]
pub struct MarkPostedArgs {
    #[command(flatten)]
    pub session: SessionArgs,
    /// Finding id.
    pub id: String,
    /// The URL of the posted comment.
    #[arg(long)]
    pub url: Option<String>,
}

#[derive(Args, Debug)]
pub struct SetStatusArgs {
    #[command(flatten)]
    pub session: SessionArgs,
    /// One of: reviewing, awaiting_review, posting, done.
    pub status: String,
}
