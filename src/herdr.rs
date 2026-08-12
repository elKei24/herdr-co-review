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

/// A pane id like `w3:p1`.
pub type PaneId = String;
/// A workspace id like `w3`.
pub type WorkspaceId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Right,
    Left,
    Up,
    Down,
}

impl Direction {
    fn as_str(self) -> &'static str {
        match self {
            Direction::Right => "right",
            Direction::Left => "left",
            Direction::Up => "up",
            Direction::Down => "down",
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
        // Prefer an explicit pane id in the output; otherwise fall back to a
        // bare workspace id and assume its first pane (`wN` -> `wN:p1`), since
        // `herdr workspace create` may report only the new workspace.
        let (id, first_pane) = match parse_pane_id(&out) {
            Some(pane) => (workspace_of(&pane), pane),
            None => {
                let wid = parse_workspace_id(&out).ok_or_else(|| {
                    anyhow!(
                        "could not parse a workspace or pane id from \
                         `herdr workspace create` output: {out:?}"
                    )
                })?;
                let pane = format!("{wid}:p1");
                (wid, pane)
            }
        };
        Ok(Workspace { id, first_pane })
    }

    /// Split `pane` in a direction, returning the new pane's id.
    pub fn pane_split(&self, pane: &str, dir: Direction, focus: bool) -> Result<PaneId> {
        let args = build_pane_split_args(pane, dir, focus);
        let out = self.run_capture(&args).context("herdr pane split failed")?;
        if self.dry_run {
            return Ok(bump_pane(pane));
        }
        parse_pane_id(&out).ok_or_else(|| {
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

    /// Submit a line of text to a pane: type it, then press Enter. This is how the
    /// navigator injects a message into the live agent session.
    pub fn pane_submit_line(&self, pane: &str, text: &str) -> Result<()> {
        self.pane_send_text(pane, text)?;
        self.pane_send_keys(pane, &["Enter"])
    }

    /// Best-effort: ask Herdr for the state of the agent running in `pane`
    /// (e.g. "working", "blocked", "done"). Returns `None` if Herdr is
    /// unavailable or the state can't be determined — callers should treat the
    /// absence as "unknown" and show nothing.
    pub fn agent_state(&self, pane: &str) -> Option<String> {
        if self.dry_run {
            return None;
        }
        let out = exec::capture(&self.bin, &["agent", "list"], None).ok()?;
        parse_agent_state(&out, pane)
    }
}

/// Scan `herdr agent list` output for the state keyword on the line mentioning
/// `pane`. Deliberately lenient about the exact output format.
fn parse_agent_state(list: &str, pane: &str) -> Option<String> {
    const STATES: [&str; 8] = [
        "working", "blocked", "waiting", "thinking", "running", "done", "idle", "error",
    ];
    for line in list.lines() {
        if line.contains(pane) {
            let lower = line.to_ascii_lowercase();
            if let Some(state) = STATES.iter().find(|s| lower.contains(**s)) {
                return Some((*state).to_string());
            }
        }
    }
    None
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

fn build_pane_split_args(pane: &str, dir: Direction, focus: bool) -> Vec<String> {
    let mut args = vec![
        "pane".into(),
        "split".into(),
        pane.to_string(),
        "--direction".into(),
        dir.as_str().to_string(),
    ];
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

fn is_pane_id(tok: &str) -> bool {
    let Some((w, p)) = tok.split_once(':') else {
        return false;
    };
    is_wid(w) && p.starts_with('p') && p.len() > 1 && p[1..].chars().all(|c| c.is_ascii_digit())
}

fn is_wid(tok: &str) -> bool {
    tok.starts_with('w') && tok.len() > 1 && tok[1..].chars().all(|c| c.is_ascii_digit())
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
        assert!(!is_wid("x3"));
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
            build_pane_split_args("w1:p1", Direction::Right, false),
            vec![
                "pane",
                "split",
                "w1:p1",
                "--direction",
                "right",
                "--no-focus"
            ]
        );
        assert_eq!(
            build_pane_run_args("w1:p2", "co-review view"),
            vec!["pane", "run", "w1:p2", "co-review view"]
        );
    }

    #[test]
    fn parses_agent_state_leniently() {
        let listing = "\
w1:p1  claude   working   PR review
w1:p2  co-review view  idle";
        assert_eq!(
            parse_agent_state(listing, "w1:p1").as_deref(),
            Some("working")
        );
        assert_eq!(parse_agent_state(listing, "w1:p2").as_deref(), Some("idle"));
        assert_eq!(parse_agent_state(listing, "w9:p9"), None);
        assert_eq!(parse_agent_state("no state here for w1:p1", "w1:p1"), None);
    }

    #[test]
    fn dry_run_returns_synthetic_ids() {
        let h = Herdr {
            bin: "herdr".into(),
            dry_run: true,
        };
        let ws = h.workspace_create("/tmp", "x").unwrap();
        assert_eq!(ws.first_pane, "w1:p1");
        let p2 = h
            .pane_split(&ws.first_pane, Direction::Right, false)
            .unwrap();
        assert_eq!(p2, "w1:p2");
        // these are no-ops in dry-run and must not error
        h.pane_run(&p2, &["co-review".into(), "view".into()])
            .unwrap();
        h.pane_submit_line(&ws.first_pane, "hello").unwrap();
    }
}
