//! A wrapper around the `herdr` CLI (see decision log §3 and §8).
//!
//! Herdr owns the terminal panes; co-review asks it to lay out the split-screen
//! and launch the agent. Because the build/CI sandbox has no `herdr`, the wrapper
//! has a **dry-run mode** (`CO_REVIEW_FAKE_HERDR=1`, or `--dry-run` on `start`)
//! that prints the commands it *would* run and returns synthetic ids, so the
//! orchestrator is fully exercisable without Herdr present.

use anyhow::{anyhow, Context, Result};

use crate::exec;

/// Environment variable that forces dry-run mode.
pub const FAKE_ENV: &str = "CO_REVIEW_FAKE_HERDR";

/// Environment variable herdr sets on plugin-invoked commands (actions, link
/// handlers) with the invocation context as JSON — `clicked_url`,
/// `focused_pane_cwd`, `workspace_cwd`, ….
pub const PLUGIN_CONTEXT_ENV: &str = "HERDR_PLUGIN_CONTEXT_JSON";

/// The plugin invocation context, parsed from [`PLUGIN_CONTEXT_ENV`].
#[derive(Debug, Default, serde::Deserialize)]
pub struct PluginContext {
    clicked_url: Option<String>,
    focused_pane_cwd: Option<String>,
    workspace_cwd: Option<String>,
}

impl PluginContext {
    /// Parse the context from the environment, when running as a plugin command.
    pub fn from_env() -> Option<PluginContext> {
        let raw = std::env::var(PLUGIN_CONTEXT_ENV).ok()?;
        serde_json::from_str(&raw).ok()
    }

    fn field(v: &Option<String>) -> Option<&str> {
        v.as_deref().map(str::trim).filter(|s| !s.is_empty())
    }

    /// The URL the user clicked to trigger a link handler.
    pub fn clicked_url(&self) -> Option<&str> {
        Self::field(&self.clicked_url)
    }

    /// The working directories the invocation happened in, most specific first.
    pub fn cwd_candidates(&self) -> impl Iterator<Item = &str> {
        [&self.focused_pane_cwd, &self.workspace_cwd]
            .into_iter()
            .filter_map(Self::field)
    }
}

/// A pane id like `w3:p1`.
pub type PaneId = String;
/// A workspace id like `w3`.
pub type WorkspaceId = String;

/// Herdr's agent lifecycle states. Herdr also reports "unknown" (an agent is
/// present but unclassified), which parses to `None` so the UI shows nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    Working,
    Blocked,
    Done,
}

impl AgentState {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "idle" => Some(AgentState::Idle),
            "working" => Some(AgentState::Working),
            "blocked" => Some(AgentState::Blocked),
            "done" => Some(AgentState::Done),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AgentState::Idle => "idle",
            AgentState::Working => "working",
            AgentState::Blocked => "blocked",
            AgentState::Done => "done",
        }
    }
}

/// A freshly created workspace and the pane it starts with.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub first_pane: PaneId,
}

/// Handle to the herdr CLI.
pub struct Herdr {
    bin: String,
    dry_run: bool,
}

impl Herdr {
    /// Build a handle, honoring `HERDR_BIN_PATH` for the binary and the dry-run
    /// environment/argument.
    pub fn new(force_dry_run: bool) -> Self {
        let bin = std::env::var("HERDR_BIN_PATH")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "herdr".to_string());
        let dry_run = force_dry_run || env_flag(FAKE_ENV);
        Herdr { bin, dry_run }
    }

    pub fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// Whether the herdr binary is actually available (always true in dry-run so
    /// callers don't bail).
    pub fn available(&self) -> bool {
        self.dry_run || exec::have(&self.bin)
    }

    fn run_capture(&self, args: &[String]) -> Result<String> {
        if self.dry_run {
            eprintln!("+ {} {}", self.bin, args.join(" "));
            return Ok(String::new());
        }
        exec::capture(&self.bin, args, None)
    }

    /// Create a workspace rooted at `cwd`.
    pub fn workspace_create(&self, cwd: &str, label: &str) -> Result<Workspace> {
        let args = build_workspace_create_args(cwd, label);
        let out = self
            .run_capture(&args)
            .context("herdr workspace create failed")?;
        if self.dry_run {
            return Ok(Workspace {
                id: "w1".into(),
                first_pane: "w1:p1".into(),
            });
        }
        parse_workspace_create(&out).ok_or_else(|| {
            anyhow!(
                "could not parse a workspace or pane id from \
                 `herdr workspace create` output: {out:?}"
            )
        })
    }

    /// Split `pane` to the right, returning the new pane's id. `cwd` pins the
    /// new pane's working directory (it would otherwise inherit herdr's choice).
    /// (Herdr can also split down, but the layout only ever splits right.)
    pub fn pane_split(&self, pane: &str, focus: bool, cwd: Option<&str>) -> Result<PaneId> {
        let args = build_pane_split_args(pane, focus, cwd);
        let out = self.run_capture(&args).context("herdr pane split failed")?;
        if self.dry_run {
            return Ok(bump_pane(pane));
        }
        json_str_at(&out, "/result/pane/pane_id")
            .or_else(|| parse_pane_id(&out))
            .ok_or_else(|| {
                anyhow!("could not parse a pane id from `herdr pane split` output: {out:?}")
            })
    }

    /// Run a command (given as argv) inside a pane.
    pub fn pane_run(&self, pane: &str, command: &[String]) -> Result<()> {
        let cmd = shell_join(command);
        let args = build_pane_run_args(pane, &cmd);
        self.run_capture(&args).context("herdr pane run failed")?;
        Ok(())
    }

    /// Type text into a pane (without pressing Enter).
    pub fn pane_send_text(&self, pane: &str, text: &str) -> Result<()> {
        let args = vec![
            "pane".into(),
            "send-text".into(),
            pane.to_string(),
            text.to_string(),
        ];
        self.run_capture(&args)
            .context("herdr pane send-text failed")?;
        Ok(())
    }

    /// Press one or more named keys in a pane (e.g. `Enter`).
    pub fn pane_send_keys(&self, pane: &str, keys: &[&str]) -> Result<()> {
        let mut args = vec!["pane".into(), "send-keys".into(), pane.to_string()];
        args.extend(keys.iter().map(|k| k.to_string()));
        self.run_capture(&args)
            .context("herdr pane send-keys failed")?;
        Ok(())
    }

    /// Focus a pane.
    pub fn pane_focus(&self, pane: &str) -> Result<()> {
        let args = vec!["pane".into(), "focus".into(), pane.to_string()];
        self.run_capture(&args).context("herdr pane focus failed")?;
        Ok(())
    }

    /// Submit a line of text to a pane: type it, then press Enter. Raw-terminal
    /// fallback for panes herdr does not recognize as an agent.
    fn pane_submit_line(&self, pane: &str, text: &str) -> Result<()> {
        self.pane_send_text(pane, text)?;
        self.pane_send_keys(pane, &["Enter"])
    }

    /// Submit a line of text to the agent in `pane`. Prefers `herdr agent
    /// prompt`, which encodes the text and Enter atomically and honors the
    /// pane's bracketed-paste mode, so it works with agent TUIs. Falls back to
    /// the raw send-text + Enter path only when herdr reports it has not
    /// recognized an agent in the pane (e.g. a custom agent command); any other
    /// failure (say, a stalled just-started agent) is returned so the caller
    /// can tell the user to retry, instead of pretending a raw send that agent
    /// TUIs may swallow was delivered.
    pub fn submit_to_agent(&self, pane: &str, text: &str) -> Result<()> {
        let args = vec![
            "agent".into(),
            "prompt".into(),
            pane.to_string(),
            text.to_string(),
        ];
        if self.dry_run {
            self.run_capture(&args)?;
            return Ok(());
        }
        let out = exec::try_capture(&self.bin, &args, None)?;
        if out.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&out.stderr);
        if error_code(&stderr).as_deref() == Some("agent_not_found") {
            return self.pane_submit_line(pane, text);
        }
        Err(anyhow!("herdr agent prompt failed: {}", stderr.trim()))
    }

    /// Best-effort: ask Herdr for the state of the agent running in `pane`.
    /// Returns `None` if Herdr is unavailable or the state can't be determined —
    /// callers should treat the absence as "unknown" and show nothing.
    pub fn agent_state(&self, pane: &str) -> Option<AgentState> {
        if self.dry_run {
            return None;
        }
        let out = exec::capture(&self.bin, &["agent", "list"], None).ok()?;
        parse_agent_state(&out, pane)
    }
}

/// Read the agent status for `pane` from `herdr agent list` JSON:
/// `{"result":{"agents":[{"pane_id":…,"agent_status":…}]}}`.
fn parse_agent_state(list: &str, pane: &str) -> Option<AgentState> {
    let v: serde_json::Value = serde_json::from_str(list.trim()).ok()?;
    let agents = v.pointer("/result/agents")?.as_array()?;
    let status = agents
        .iter()
        .find(|a| a.get("pane_id").and_then(|p| p.as_str()) == Some(pane))?
        .get("agent_status")?
        .as_str()?;
    AgentState::parse(status)
}

/// A string at `ptr` (a JSON pointer like `/result/pane/pane_id`) inside a
/// herdr JSON response.
fn json_str_at(out: &str, ptr: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(out.trim()).ok()?;
    v.pointer(ptr)?.as_str().map(str::to_string)
}

/// The `error.code` of a herdr JSON error envelope (printed to stderr).
fn error_code(stderr: &str) -> Option<String> {
    json_str_at(stderr, "/error/code")
}

/// Parse `herdr workspace create` output: prefer the JSON response's
/// `result.root_pane.pane_id`, fall back to scanning for id-shaped tokens.
/// The workspace id is the pane id's `w…` half.
fn parse_workspace_create(out: &str) -> Option<Workspace> {
    let pane = json_str_at(out, "/result/root_pane/pane_id")
        .or_else(|| parse_pane_id(out))
        .or_else(|| parse_workspace_id(out).map(|wid| format!("{wid}:p1")))?;
    Some(Workspace {
        id: workspace_of(&pane),
        first_pane: pane,
    })
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !(v.is_empty() || v == "0" || v == "false" || v == "no")
        })
        .unwrap_or(false)
}

fn build_workspace_create_args(cwd: &str, label: &str) -> Vec<String> {
    vec![
        "workspace".into(),
        "create".into(),
        "--cwd".into(),
        cwd.to_string(),
        "--label".into(),
        label.to_string(),
    ]
}

fn build_pane_split_args(pane: &str, focus: bool, cwd: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "pane".into(),
        "split".into(),
        pane.to_string(),
        "--direction".into(),
        "right".into(),
    ];
    if let Some(cwd) = cwd {
        args.push("--cwd".into());
        args.push(cwd.to_string());
    }
    if !focus {
        args.push("--no-focus".into());
    }
    args
}

fn build_pane_run_args(pane: &str, command: &str) -> Vec<String> {
    vec![
        "pane".into(),
        "run".into(),
        pane.to_string(),
        command.to_string(),
    ]
}

/// Join an argv into a single shell command line, quoting as needed. `herdr pane
/// run` takes the command as one string, so we must render argv safely.
pub fn shell_join(argv: &[String]) -> String {
    argv.iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Single-quote a shell argument, escaping embedded single quotes.
pub fn shell_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '=' | '@' | ',')
        })
    {
        return arg.to_string();
    }
    let escaped = arg.replace('\'', r"'\''");
    format!("'{escaped}'")
}

/// Extract the first pane id (`wN:pM`) appearing in some text.
fn parse_pane_id(text: &str) -> Option<PaneId> {
    for tok in text.split(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',') {
        if is_pane_id(tok) {
            return Some(tok.to_string());
        }
    }
    None
}

/// Extract the first bare workspace id (`wN`) appearing in some text.
fn parse_workspace_id(text: &str) -> Option<WorkspaceId> {
    for tok in text.split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ',' | ':')) {
        if is_wid(tok) {
            return Some(tok.to_string());
        }
    }
    None
}

// Herdr ids are opaque (`w`/`p` + an alphanumeric tail, not necessarily
// numeric), so the token fallback accepts any id-shaped token.
fn is_pane_id(tok: &str) -> bool {
    let Some((w, p)) = tok.split_once(':') else {
        return false;
    };
    is_wid(w)
        && p.starts_with('p')
        && p.len() > 1
        && p[1..].chars().all(|c| c.is_ascii_alphanumeric())
}

fn is_wid(tok: &str) -> bool {
    let tail = match tok.strip_prefix('w') {
        Some(t) if !t.is_empty() => t,
        _ => return false,
    };
    // Require a digit or uppercase so prose like "workspace" can't match.
    tail.chars().all(|c| c.is_ascii_alphanumeric())
        && tail
            .chars()
            .any(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
}

fn workspace_of(pane: &str) -> WorkspaceId {
    pane.split_once(':')
        .map(|(w, _)| w.to_string())
        .unwrap_or_else(|| pane.to_string())
}

/// Dry-run helper: given `wN:pM`, return `wN:p(M+1)`.
fn bump_pane(pane: &str) -> PaneId {
    if let Some((w, p)) = pane.split_once(':') {
        if let Some(n) = p.strip_prefix('p').and_then(|d| d.parse::<u32>().ok()) {
            return format!("{w}:p{}", n + 1);
        }
    }
    format!("{pane}b")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_leaves_simple_args_bare() {
        assert_eq!(shell_quote("claude"), "claude");
        assert_eq!(shell_quote("src/foo.rs:42"), "src/foo.rs:42");
        assert_eq!(shell_quote("--flag=value"), "--flag=value");
    }

    #[test]
    fn shell_quote_wraps_spaces_and_quotes() {
        assert_eq!(shell_quote("hello world"), "'hello world'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn shell_join_roundtrip() {
        let argv = vec!["claude".to_string(), "review PR #1".to_string()];
        assert_eq!(shell_join(&argv), "claude 'review PR #1'");
    }

    #[test]
    fn parses_pane_ids() {
        assert_eq!(parse_pane_id("created w3:p1"), Some("w3:p1".to_string()));
        assert_eq!(
            parse_pane_id(r#"{"pane":"w12:p4"}"#),
            Some("w12:p4".to_string())
        );
        assert_eq!(parse_pane_id("no id here"), None);
        assert!(is_wid("w3"));
        assert!(is_wid("wP"));
        assert!(!is_wid("x3"));
        assert!(!is_wid("workspace"));
        assert!(is_pane_id("wP:p1"));
    }

    #[test]
    fn parses_workspace_create_json() {
        let out = r#"{"id":"cli:workspace:create","result":{"root_pane":{"pane_id":"wP:p1","workspace_id":"wP"},"tab":{"tab_id":"wP:t1"},"type":"workspace_created","workspace":{"label":"x","workspace_id":"wP"}}}"#;
        let ws = parse_workspace_create(out).unwrap();
        assert_eq!(ws.id, "wP");
        assert_eq!(ws.first_pane, "wP:p1");
    }

    #[test]
    fn parses_pane_split_json() {
        let out = r#"{"id":"cli:pane:split","result":{"pane":{"pane_id":"wP:p2","workspace_id":"wP"},"type":"pane_info"}}"#;
        assert_eq!(
            json_str_at(out, "/result/pane/pane_id").as_deref(),
            Some("wP:p2")
        );
    }

    #[test]
    fn parses_error_code_from_stderr() {
        let stderr = r#"{"error":{"code":"agent_not_found","message":"agent target w1:p1 not found"},"id":"cli:agent:prompt"}"#;
        assert_eq!(error_code(stderr).as_deref(), Some("agent_not_found"));
        assert_eq!(error_code("plain text failure"), None);
    }

    #[test]
    fn workspace_id_fallback() {
        assert_eq!(
            parse_workspace_id("created workspace w5"),
            Some("w5".to_string())
        );
        assert_eq!(
            parse_workspace_id(r#"{"workspace":"w7"}"#),
            Some("w7".to_string())
        );
        assert_eq!(parse_workspace_id("nothing here"), None);
        // a full pane id should still yield its workspace part
        assert_eq!(parse_workspace_id("w3:p1"), Some("w3".to_string()));
    }

    #[test]
    fn workspace_and_bump() {
        assert_eq!(workspace_of("w3:p1"), "w3");
        assert_eq!(bump_pane("w3:p1"), "w3:p2");
    }

    #[test]
    fn arg_builders() {
        assert_eq!(
            build_workspace_create_args("/tmp/wt", "co-review-1"),
            vec![
                "workspace",
                "create",
                "--cwd",
                "/tmp/wt",
                "--label",
                "co-review-1"
            ]
        );
        assert_eq!(
            build_pane_split_args("w1:p1", false, Some("/tmp/wt")),
            vec![
                "pane",
                "split",
                "w1:p1",
                "--direction",
                "right",
                "--cwd",
                "/tmp/wt",
                "--no-focus"
            ]
        );
        assert_eq!(
            build_pane_run_args("w1:p2", "co-review view"),
            vec!["pane", "run", "w1:p2", "co-review view"]
        );
    }

    #[test]
    fn parses_agent_state_from_json() {
        let listing = r#"{"id":"cli:agent:list","result":{"agents":[
            {"agent":"claude","agent_status":"working","pane_id":"w1:p1"},
            {"agent":"codex","agent_status":"idle","pane_id":"w1:p2"},
            {"agent":"claude","agent_status":"unknown","pane_id":"w1:p3"}
        ],"type":"agent_list"}}"#;
        assert_eq!(
            parse_agent_state(listing, "w1:p1"),
            Some(AgentState::Working)
        );
        assert_eq!(parse_agent_state(listing, "w1:p2"), Some(AgentState::Idle));
        // "unknown" carries no signal — show nothing.
        assert_eq!(parse_agent_state(listing, "w1:p3"), None);
        assert_eq!(parse_agent_state(listing, "w9:p9"), None);
        assert_eq!(parse_agent_state("not json", "w1:p1"), None);
    }

    #[test]
    fn dry_run_returns_synthetic_ids() {
        let h = Herdr {
            bin: "herdr".into(),
            dry_run: true,
        };
        let ws = h.workspace_create("/tmp", "x").unwrap();
        assert_eq!(ws.first_pane, "w1:p1");
        let p2 = h.pane_split(&ws.first_pane, false, Some("/tmp")).unwrap();
        assert_eq!(p2, "w1:p2");
        // these are no-ops in dry-run and must not error
        h.pane_run(&p2, &["co-review".into(), "view".into()])
            .unwrap();
        h.submit_to_agent(&ws.first_pane, "hello").unwrap();
    }
}
