//! Handlers for the agent- and human-facing subcommands (everything except
//! `start`, which lives in [`crate::orchestrate`], and `view`, in [`crate::tui`]).

use anyhow::{anyhow, bail, Context, Result};

use crate::cli::*;
use crate::diffview;
use crate::git::Git;
use crate::model::{Finding, Location, ReviewStatus, Severity, State, Verdict};
use crate::store::Store;

/// Resolve the [`Store`] a command should act on.
fn open_store(session: &SessionArgs) -> Result<Store> {
    let dir = crate::paths::resolve_session_dir(session.session.as_deref())?;
    Ok(Store::new(dir))
}

/// Resolve the body text from `--body` / `--body-file` (`-` = stdin).
fn resolve_text(inline: Option<&str>, file: Option<&str>) -> Result<Option<String>> {
    match file {
        Some(path) => Ok(Some(crate::util::read_path_or_stdin(path)?)),
        None => Ok(inline.map(|s| s.to_string())),
    }
}

pub fn add_finding(args: &AddFindingArgs) -> Result<()> {
    let store = open_store(&args.session)?;

    let severity = Severity::parse(&args.severity)
        .ok_or_else(|| anyhow!("unknown severity '{}'", args.severity))?;

    let locations = args
        .locations
        .iter()
        .map(|raw| Location::parse(raw).map_err(|e| anyhow!("{e}")))
        .collect::<Result<Vec<_>>>()?;

    let body = resolve_text(args.body.as_deref(), args.body_file.as_deref())?.unwrap_or_default();

    let id = store.update(|state| {
        let id = state.mint_finding_id();
        let mut f = Finding::new(id.clone(), args.title.clone());
        f.severity = severity;
        f.category = args.category.clone();
        f.body = body.clone();
        f.suggestion = args.suggestion.clone();
        f.locations = locations.clone();
        state.findings.push(f);
        // Recording a finding means we're actively reviewing.
        if state.status == ReviewStatus::Done {
            state.status = ReviewStatus::Reviewing;
        }
        Ok(id)
    })?;

    println!("{id}");
    Ok(())
}

pub fn import(args: &ImportArgs) -> Result<()> {
    let store = open_store(&args.session)?;
    let raw = resolve_text(None, Some(&args.file))?.unwrap_or_default();
    let incoming: Vec<IncomingFinding> =
        serde_json::from_str(&raw).context("parsing findings JSON (expected an array)")?;

    let ids = store.update(|state| {
        let mut ids = Vec::new();
        for inc in &incoming {
            let id = state.mint_finding_id();
            state.findings.push(inc.to_finding(id.clone()));
            ids.push(id);
        }
        Ok(ids)
    })?;

    for id in &ids {
        println!("{id}");
    }
    eprintln!("imported {} finding(s)", ids.len());
    Ok(())
}

/// A finding as an agent might emit it in a JSON array for `import` — no id,
/// verdict, or posted state (co-review assigns those).
#[derive(serde::Deserialize)]
struct IncomingFinding {
    title: String,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    body: String,
    #[serde(default)]
    suggestion: Option<String>,
    #[serde(default)]
    locations: Vec<Location>,
}

impl IncomingFinding {
    fn to_finding(&self, id: String) -> Finding {
        let mut f = Finding::new(id, self.title.clone());
        f.severity = match self.severity.as_deref() {
            Some(s) => Severity::parse(s).unwrap_or_else(|| {
                eprintln!(
                    "warning: unknown severity '{s}' on \"{}\"; defaulting to {}",
                    self.title,
                    Severity::default().label()
                );
                Severity::default()
            }),
            None => Severity::default(),
        };
        f.category = self.category.clone();
        f.body = self.body.clone();
        f.suggestion = self.suggestion.clone();
        f.locations = self.locations.clone();
        f
    }
}

pub fn verdict(args: &VerdictArgs) -> Result<()> {
    let store = open_store(&args.session)?;
    let verdict = Verdict::parse(&args.verdict)
        .ok_or_else(|| anyhow!("unknown verdict '{}'", args.verdict))?;
    store.update(|state| {
        let note = args.note.clone();
        let f = state
            .finding_mut(&args.id)
            .ok_or_else(|| anyhow!("no finding with id '{}'", args.id))?;
        f.verdict = verdict;
        if let Some(note) = note {
            f.user_note = Some(note);
        }
        f.touch();
        Ok(())
    })?;
    println!("{} -> {}", args.id, verdict.label());
    Ok(())
}

pub fn edit(args: &EditArgs) -> Result<()> {
    let store = open_store(&args.session)?;

    // Validate inputs before taking the lock so a bad value doesn't touch state.
    let severity = match &args.severity {
        Some(s) => Some(Severity::parse(s).ok_or_else(|| anyhow!("unknown severity '{s}'"))?),
        None => None,
    };
    let new_body = resolve_text(args.body.as_deref(), args.body_file.as_deref())?;
    let new_locations = if args.locations.is_empty() {
        None
    } else {
        Some(
            args.locations
                .iter()
                .map(|raw| Location::parse(raw).map_err(|e| anyhow!("{e}")))
                .collect::<Result<Vec<_>>>()?,
        )
    };

    let reset = store.update(|state| {
        let f = state
            .finding_mut(&args.id)
            .ok_or_else(|| anyhow!("no finding with id '{}'", args.id))?;
        if let Some(t) = &args.title {
            f.title = t.clone();
        }
        if let Some(s) = severity {
            f.severity = s;
        }
        if args.clear_category {
            f.category = None;
        } else if let Some(c) = &args.category {
            f.category = Some(c.clone());
        }
        if let Some(b) = &new_body {
            f.body = b.clone();
        }
        if args.clear_suggestion {
            f.suggestion = None;
        } else if let Some(s) = &args.suggestion {
            f.suggestion = Some(s.clone());
        }
        if args.clear_locations {
            f.locations.clear();
        } else if let Some(locs) = &new_locations {
            f.locations = locs.clone();
        }
        // Revised text should be re-triaged: reset a decided verdict to pending
        // unless the caller opted to keep it.
        let reset = !args.keep_verdict && f.verdict != Verdict::Pending;
        if reset {
            f.verdict = Verdict::Pending;
        }
        f.touch();
        Ok(reset)
    })?;
    if reset {
        println!(
            "{} updated (verdict reset to pending — re-triage it)",
            args.id
        );
    } else {
        println!("{} updated", args.id);
    }
    Ok(())
}

pub fn mark_posted(args: &MarkPostedArgs) -> Result<()> {
    let store = open_store(&args.session)?;
    store.update(|state| {
        let url = args.url.clone();
        let f = state
            .finding_mut(&args.id)
            .ok_or_else(|| anyhow!("no finding with id '{}'", args.id))?;
        f.posted = true;
        f.posted_url = url;
        f.touch();
        Ok(())
    })?;
    println!("{} marked posted", args.id);
    Ok(())
}

pub fn set_status(args: &SetStatusArgs) -> Result<()> {
    let store = open_store(&args.session)?;
    let status = ReviewStatus::parse(&args.status)
        .ok_or_else(|| anyhow!("unknown status '{}'", args.status))?;
    store.update(|state| {
        state.status = status;
        Ok(())
    })?;
    println!("status: {}", status.label());
    Ok(())
}

pub fn list(args: &ListArgs) -> Result<()> {
    let store = open_store(&args.session)?;
    let state = store.read()?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&state)?);
        return Ok(());
    }

    let filter = match &args.verdict {
        Some(v) => Some(Verdict::parse(v).ok_or_else(|| anyhow!("unknown verdict '{v}'"))?),
        None => None,
    };

    print_header(&state);
    let mut shown = 0;
    for f in &state.findings {
        if let Some(want) = filter {
            if f.verdict != want {
                continue;
            }
        }
        shown += 1;
        let loc = f
            .primary_location()
            .map(|l| l.label())
            .unwrap_or_else(|| "—".to_string());
        println!(
            "  {:<4} {} {:<8} [{:<9}] {}  ({})",
            f.id,
            f.severity.glyph(),
            f.severity.label(),
            f.verdict.label(),
            f.title,
            loc
        );
    }
    if shown == 0 {
        println!("  (no findings yet)");
    }
    Ok(())
}

fn print_header(state: &State) {
    println!(
        "co-review {}/{} #{} — {} finding(s), {} pending [{}]",
        state.pr.owner,
        state.pr.repo,
        state.pr.number,
        state.findings.len(),
        state.pending_count(),
        state.status.label()
    );
}

pub fn status(args: &SessionArgs) -> Result<()> {
    let store = open_store(args)?;
    let state = store.read()?;
    print_header(&state);
    let c = state.counts();
    println!(
        "  approved/edited: {}   dismissed: {}   posted: {}",
        c.approved, c.dismissed, c.posted
    );
    println!("  worktree: {}", state.session.worktree);
    println!("  session:  {}", store.session_dir().display());
    Ok(())
}

pub fn show(args: &ShowArgs) -> Result<()> {
    let store = open_store(&args.session)?;
    let state = store.read()?;
    let finding = state
        .finding(&args.id)
        .ok_or_else(|| anyhow!("no finding with id '{}'", args.id))?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(finding)?);
        return Ok(());
    }

    println!(
        "{}  {} {} [{}]",
        finding.id,
        finding.severity.glyph(),
        finding.severity.label(),
        finding.verdict.label()
    );
    println!("{}", finding.title);
    if let Some(cat) = &finding.category {
        println!("category: {cat}");
    }
    if !finding.body.is_empty() {
        println!("\n{}", finding.body);
    }
    if let Some(s) = &finding.suggestion {
        println!("\nsuggestion:\n{s}");
    }
    if let Some(note) = &finding.user_note {
        println!("\nyour note: {note}");
    }

    // Related code (best-effort; skip silently if git isn't usable here).
    if let Ok(git) = Git::discover(std::path::Path::new(&state.session.worktree)) {
        println!();
        for loc in &finding.locations {
            match diffview::snippet_for(&git, &state, loc, diffview::DEFAULT_CONTEXT) {
                Ok(snippet) => print!("{}", diffview::render_plain(&snippet)),
                Err(e) => println!("── {} ── (could not render: {e})", loc.label()),
            }
        }
    } else if !finding.locations.is_empty() {
        println!("\nlocations:");
        for loc in &finding.locations {
            println!("  {}", loc.label());
        }
    }
    Ok(())
}

pub fn wait(args: &WaitArgs) -> Result<()> {
    let store = open_store(&args.session)?;
    let start = std::time::Instant::now();
    let interval = std::time::Duration::from_millis(args.interval.max(50));
    loop {
        let state = store.read()?;
        let pending = state.pending_count();
        // Don't treat the initial empty "reviewing" state as "all decided" — that
        // would let `wait` return before any finding is recorded.
        if state.handoff_complete() {
            eprintln!(
                "all {} finding(s) decided; proceed to post the approved ones",
                state.findings.len()
            );
            return Ok(());
        }
        if args.timeout > 0 && start.elapsed().as_millis() as u64 >= args.timeout {
            bail!("timed out with {pending} finding(s) still pending");
        }
        std::thread::sleep(interval);
    }
}

pub fn post(args: &PostArgs) -> Result<()> {
    let store = open_store(&args.session)?;
    let state = store.read()?;
    let postable: Vec<Finding> = state.postable().cloned().collect();
    if postable.is_empty() {
        eprintln!("nothing to post (no approved/edited, un-posted findings)");
        return Ok(());
    }

    if args.dry_run {
        println!("would post {} finding(s):", postable.len());
        for f in &postable {
            println!(
                "  {} [{}] {} ({})",
                f.id,
                f.severity.label(),
                f.title,
                f.primary_location()
                    .map(|l| l.label())
                    .unwrap_or_else(|| "no location".into())
            );
        }
        return Ok(());
    }

    let client = crate::github::Client::from_env()?;
    let mut posted = 0;
    for f in &postable {
        let Some(loc) = f.primary_location() else {
            eprintln!("skip {}: no location to attach a comment to", f.id);
            continue;
        };
        let comment = crate::github::ReviewComment {
            body: render_comment_body(f, true),
            path: loc.file.clone(),
            line: loc.end(),
            start_line: Some(loc.start_line),
            side: loc.side,
        };
        // Only fall back to a general PR comment when GitHub rejects the *line*
        // (422 — the line isn't part of the diff). Transient/auth failures
        // propagate so the post can be retried instead of being downgraded.
        let url = match client.post_review_comment(&state.pr, &comment) {
            Ok(url) => url,
            Err(e) if line_not_in_diff(&e) => {
                eprintln!("{}: line not in the diff; posting as a PR comment", f.id);
                // A ```suggestion block is only applyable inline, so render the
                // conversation comment with a plain code block instead.
                let body = format!(
                    "{}\n\n_re `{}`_",
                    render_comment_body(f, false),
                    loc.label()
                );
                client.post_issue_comment(&state.pr, &body)?
            }
            Err(e) => return Err(e),
        };
        store.update(|st| {
            if let Some(ff) = st.finding_mut(&f.id) {
                ff.posted = true;
                ff.posted_url = Some(url.clone());
                ff.touch();
            }
            Ok(())
        })?;
        println!("{} -> {url}", f.id);
        posted += 1;
    }
    store.update(|st| {
        if st.postable().next().is_none() {
            st.status = ReviewStatus::Done;
        }
        Ok(())
    })?;
    eprintln!("posted {posted} finding(s)");
    Ok(())
}

/// Whether a post error is GitHub's "line isn't part of the diff" rejection.
fn line_not_in_diff(e: &anyhow::Error) -> bool {
    e.to_string().contains("422")
}

/// Render the markdown body posted to GitHub for a finding. `applyable_suggestion`
/// controls whether a suggestion is emitted as a GitHub ```suggestion block
/// (only meaningful on an inline review comment) or a plain code block.
fn render_comment_body(f: &Finding, applyable_suggestion: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!("**{}** ({})\n\n", f.title, f.severity.label()));
    if !f.body.is_empty() {
        out.push_str(&f.body);
        out.push('\n');
    }
    if let Some(s) = &f.suggestion {
        let fence = if applyable_suggestion {
            "suggestion"
        } else {
            ""
        };
        out.push_str(&format!("\n**Suggested fix:**\n```{fence}\n"));
        out.push_str(s.trim_end_matches('\n'));
        out.push_str("\n```\n");
    }
    if f.verdict == Verdict::Edited {
        if let Some(note) = &f.user_note {
            out.push_str(&format!("\n> {note}\n"));
        }
    }
    out
}

/// Enumerate all sessions on disk as `(dir, state)`, oldest first.
fn all_sessions() -> Result<Vec<(std::path::PathBuf, State)>> {
    let mut out = Vec::new();
    let dir = crate::paths::sessions_dir()?;
    if dir.is_dir() {
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?
        {
            // Tolerate a single unreadable entry (permissions, a delete race)
            // rather than failing the whole listing.
            let Ok(entry) = entry else { continue };
            let store = Store::new(entry.path());
            if let Ok(state) = store.read_lossy() {
                out.push((entry.path(), state));
            }
        }
    }
    out.sort_by_key(|(_, s)| s.session.created_at_ms);
    Ok(out)
}

pub fn sessions(args: &SessionsArgs) -> Result<()> {
    let sessions = all_sessions()?;

    if args.json {
        let rows: Vec<serde_json::Value> = sessions
            .iter()
            .map(|(dir, s)| {
                let c = s.counts();
                serde_json::json!({
                    "id": s.session.id,
                    "owner": s.pr.owner,
                    "repo": s.pr.repo,
                    "number": s.pr.number,
                    "status": s.status.label(),
                    "counts": {
                        "total": c.total, "pending": c.pending, "approved": c.approved,
                        "dismissed": c.dismissed, "needs_discussion": c.needs_discussion,
                        "posted": c.posted,
                    },
                    "session_dir": dir.display().to_string(),
                    "worktree": s.session.worktree,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if sessions.is_empty() {
        println!("no co-review sessions");
        return Ok(());
    }
    for (dir, s) in &sessions {
        let c = s.counts();
        println!(
            "{}/{} #{}  [{}]  {} finding(s) — {} pending, {} approved, {} posted",
            s.pr.owner,
            s.pr.repo,
            s.pr.number,
            s.status.label(),
            c.total,
            c.pending,
            c.approved,
            c.posted
        );
        println!("    {}", dir.display());
    }
    Ok(())
}

pub fn end(args: &EndArgs) -> Result<()> {
    let session_dir = match (&args.session, &args.pr) {
        (Some(s), _) => std::path::PathBuf::from(s),
        (None, Some(pr)) => crate::orchestrate::session_dir_for_pr(pr)?,
        (None, None) => bail!("pass a PR (e.g. `co-review end 123`) or --session <dir>"),
    };
    let store = Store::new(&session_dir);
    if !store.exists() {
        bail!("no co-review session at {}", session_dir.display());
    }
    let state = store.read()?;

    // Guard against wiping a session whose Herdr panes are still open (which
    // would orphan the agent/navigator and could discard uncommitted work).
    let looks_active = state.status != ReviewStatus::Done
        && (state.session.agent_pane_id.is_some() || state.session.view_pane_id.is_some());
    if looks_active && !args.force {
        bail!(
            "session for {}/{} #{} looks active (status: {}, panes still recorded).\n\
             Close its Herdr panes first, or pass --force to end it anyway.",
            state.pr.owner,
            state.pr.repo,
            state.pr.number,
            state.status.label()
        );
    }

    if !args.keep_worktree {
        let wt = std::path::Path::new(&state.session.worktree);
        if !state.session.source_repo.is_empty() {
            // Prune it as a git worktree so the source repo's admin data is clean.
            Git::new(&state.session.source_repo)
                .remove_worktree(wt)
                .ok();
        } else if wt.exists() {
            // Pre-`source_repo` session: remove the checkout, then best-effort
            // prune the stale worktree registration from the surrounding repo.
            std::fs::remove_dir_all(wt).ok();
            if let Ok(cwd) = std::env::current_dir() {
                if let Ok(git) = Git::discover(&cwd) {
                    let _ = git.remove_worktree(wt);
                }
            }
        }
    }

    std::fs::remove_dir_all(&session_dir)
        .with_context(|| format!("removing session dir {}", session_dir.display()))?;
    println!(
        "ended co-review for {}/{} #{}",
        state.pr.owner, state.pr.repo, state.pr.number
    );
    Ok(())
}

pub fn completions(args: &CompletionsArgs) -> Result<()> {
    use clap::CommandFactory;
    let mut cmd = crate::cli::Cli::command();
    let name = cmd.get_name().to_string();
    clap_complete::generate(args.shell, &mut cmd, name, &mut std::io::stdout());
    Ok(())
}

pub fn man() -> Result<()> {
    use clap::CommandFactory;
    let cmd = crate::cli::Cli::command();
    let man = clap_mangen::Man::new(cmd);
    let mut out = Vec::new();
    man.render(&mut out).context("rendering man page")?;
    use std::io::Write;
    std::io::stdout()
        .write_all(&out)
        .context("writing man page")?;
    Ok(())
}

pub fn protocol() -> Result<()> {
    print!("{}", crate::protocol::PROTOCOL_MD);
    Ok(())
}

pub fn doctor() -> Result<()> {
    fn mark(ok: bool) -> &'static str {
        if ok {
            "ok  "
        } else {
            "MISS"
        }
    }

    println!("co-review {}", env!("CARGO_PKG_VERSION"));

    let git = crate::exec::have("git");
    println!("[{}] git", mark(git));

    let herdr = crate::herdr::Herdr::new(false);
    let herdr_ok = herdr.available();
    println!(
        "[{}] herdr{}",
        mark(herdr_ok),
        if herdr.is_dry_run() {
            " (dry-run forced via env)"
        } else {
            ""
        }
    );

    let cfg = crate::config::Config::load().unwrap_or_default();
    let agent_ok = crate::exec::have(&cfg.default_agent)
        || cfg
            .agent(&cfg.default_agent)
            .and_then(|a| a.command.first())
            .map(|c| crate::exec::have(c))
            .unwrap_or(false);
    println!("[{}] default agent: {}", mark(agent_ok), cfg.default_agent);

    let token = crate::github::resolve_token().is_some();
    println!(
        "[{}] github token ({})",
        mark(token),
        if token {
            "found"
        } else {
            "set $GH_TOKEN/$GITHUB_TOKEN or run gh auth login"
        }
    );

    match crate::paths::config_path() {
        Ok(p) => println!(
            "     config:   {} ({})",
            p.display(),
            if p.is_file() { "present" } else { "defaults" }
        ),
        Err(e) => println!("     config:   unavailable ({e})"),
    }
    match crate::paths::base_dir() {
        Ok(p) => println!("     state:    {}", p.display()),
        Err(e) => println!("     state:    unavailable ({e})"),
    }

    // Enumerate any live sessions.
    let sessions = all_sessions().unwrap_or_default();
    if !sessions.is_empty() {
        println!("     sessions:");
        for (_, state) in &sessions {
            println!(
                "       {} #{}  {} finding(s) [{}]",
                state.pr.repo,
                state.pr.number,
                state.findings.len(),
                state.status.label()
            );
        }
    }
    Ok(())
}

pub fn prompt() -> Result<()> {
    let cfg = crate::config::Config::load()?;
    println!("{}", cfg.prompt);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Finding, Severity, Verdict};

    #[test]
    fn comment_body_includes_title_and_suggestion() {
        let mut f = Finding::new("f1".into(), "Bug".into());
        f.severity = Severity::High;
        f.body = "explanation".into();
        f.suggestion = Some("let x = 2;".into());
        let body = render_comment_body(&f, true);
        assert!(body.contains("**Bug** (high)"));
        assert!(body.contains("explanation"));
        assert!(body.contains("```suggestion"));
    }

    #[test]
    fn comment_body_appends_edited_note() {
        let mut f = Finding::new("f1".into(), "T".into());
        f.verdict = Verdict::Edited;
        f.user_note = Some("tweak wording".into());
        let body = render_comment_body(&f, true);
        assert!(body.contains("> tweak wording"));
    }

    #[test]
    fn resolve_text_prefers_file() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("b.md");
        std::fs::write(&p, "from file").unwrap();
        let got = resolve_text(Some("inline"), Some(p.to_str().unwrap())).unwrap();
        assert_eq!(got.as_deref(), Some("from file"));
        let got2 = resolve_text(Some("inline"), None).unwrap();
        assert_eq!(got2.as_deref(), Some("inline"));
    }
}
