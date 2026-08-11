//! `co-review start <pr>` — the orchestrator.
//!
//! It resolves the PR, checks it out into an isolated worktree, writes the
//! session state, and asks Herdr to lay out the split-screen: the agent in the
//! left pane and the navigator (`co-review view`) in the right pane.
//!
//! `--dry-run` makes this a fully offline preview: it prints the git and Herdr
//! commands it *would* run (and the exact prompt) without touching the network,
//! Herdr, or the filesystem. Real runs execute everything.

use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};

use crate::cli::StartArgs;
use crate::config::Config;
use crate::git::{self, Git};
use crate::herdr::{Direction, Herdr};
use crate::model::{PrInfo, SessionMeta, State};
use crate::store::Store;

/// The concrete plan for laying out the Herdr panes. Pure data so it can be
/// unit-tested and printed for `--dry-run`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayoutPlan {
    pub label: String,
    pub worktree: String,
    /// argv for the agent pane, env-prefixed; `None` with `--no-agent`.
    pub agent_argv: Option<Vec<String>>,
    /// argv for the navigator pane, env-prefixed.
    pub view_argv: Vec<String>,
}

/// Prefix an argv with `env CO_REVIEW_SESSION=<dir>` so the process — and every
/// `co-review` command it spawns — targets this session without `--session`.
fn env_prefixed(session_dir: &str, argv: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(argv.len() + 2);
    out.push("env".to_string());
    out.push(format!("{}={}", crate::paths::SESSION_ENV, session_dir));
    out.extend(argv.iter().cloned());
    out
}

/// Build the layout plan. `self_bin` is the absolute path to the co-review
/// binary to invoke inside the panes.
pub fn plan_layout(
    self_bin: &str,
    session_dir: &str,
    worktree: &str,
    label: &str,
    agent_command: Option<&[String]>,
) -> LayoutPlan {
    let view_argv = env_prefixed(
        session_dir,
        &[
            self_bin.to_string(),
            "view".to_string(),
            "--session".to_string(),
            session_dir.to_string(),
        ],
    );
    let agent_argv = agent_command.map(|cmd| env_prefixed(session_dir, cmd));
    LayoutPlan {
        label: label.to_string(),
        worktree: worktree.to_string(),
        agent_argv,
        view_argv,
    }
}

/// The PR reference to act on: the CLI argument, or — when launched from Herdr's
/// GitHub-PR link handler — the clicked URL.
fn pr_argument(args: &StartArgs) -> Result<String> {
    if let Some(pr) = &args.pr {
        return Ok(pr.clone());
    }
    if let Ok(url) = std::env::var("HERDR_PLUGIN_CLICKED_URL") {
        if !url.trim().is_empty() {
            return Ok(url);
        }
    }
    bail!("no pull request given. Pass one, e.g. `co-review start 123`.")
}

/// Resolve a PR reference to its session directory, discovering owner/repo from
/// the surrounding git repo when the reference is bare. Used by `end`.
pub fn session_dir_for_pr(pr_arg: &str) -> Result<std::path::PathBuf> {
    let cwd = std::env::current_dir().context("getting current directory")?;
    // A bare number needs the repo to resolve owner/repo; a full ref does not.
    let git = Git::discover(&cwd).ok();
    let (owner, repo, number) = match &git {
        Some(g) => resolve_pr_ref(pr_arg, g)?,
        None => {
            let pref = crate::pr::parse(pr_arg)?;
            match (pref.owner, pref.repo) {
                (Some(o), Some(r)) => (o, r, pref.number),
                _ => bail!(
                    "'{pr_arg}' needs an owner/repo (run inside the repo, or pass owner/repo#{})",
                    pref.number
                ),
            }
        }
    };
    crate::paths::session_dir(&crate::model::pr_slug(&owner, &repo, number))
}

/// Resolve owner/repo/number from the CLI reference and the surrounding repo.
fn resolve_pr_ref(pr_arg: &str, git: &Git) -> Result<(String, String, u64)> {
    let pref = crate::pr::parse(pr_arg)?;
    let (owner, repo) = match (pref.owner.clone(), pref.repo.clone()) {
        (Some(o), Some(r)) => (o, r),
        _ => {
            let url = git.remote_url("origin").context(
                "could not determine owner/repo: pass owner/repo#123 or run inside the repo",
            )?;
            crate::pr::parse_github_remote(&url).ok_or_else(|| {
                anyhow!("origin remote '{url}' is not a github.com repo; pass owner/repo#123")
            })?
        }
    };
    Ok((owner, repo, pref.number))
}

pub fn start(args: &StartArgs) -> Result<()> {
    let cfg = Config::load()?;
    let cwd = std::env::current_dir().context("getting current directory")?;
    let git = Git::discover(&cwd)?;

    let pr_arg = pr_argument(args)?;
    let (owner, repo, number) = resolve_pr_ref(&pr_arg, &git)?;

    // Choose the agent and render the prompt.
    let agent_name = args
        .agent
        .clone()
        .unwrap_or_else(|| cfg.default_agent.clone());
    let agent = cfg.agent(&agent_name).ok_or_else(|| {
        anyhow!(
            "unknown agent '{agent_name}'; known agents: {}",
            cfg.agents.keys().cloned().collect::<Vec<_>>().join(", ")
        )
    })?;

    let slug = crate::model::pr_slug(&owner, &repo, number);
    let session_dir = crate::paths::session_dir(&slug)?;
    let worktree = crate::paths::worktree_path(&slug)?;
    let protocol_path = session_dir.join("CO_REVIEW.md");

    let prompt_template = resolve_prompt(args, &cfg)?;
    let pr_display = format!("#{number}");
    let prompt = crate::protocol::render_prompt(
        &prompt_template,
        &pr_display,
        &protocol_path.to_string_lossy(),
    );
    let agent_argv = if args.no_agent {
        None
    } else {
        Some(agent.build_command(&prompt))
    };

    let self_bin = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "co-review".to_string());
    let label = format!("co-review {owner}/{repo}#{number}");
    let plan = plan_layout(
        &self_bin,
        &session_dir.to_string_lossy(),
        &worktree.to_string_lossy(),
        &label,
        agent_argv.as_deref(),
    );

    if args.dry_run {
        print_dry_run(
            &owner,
            &repo,
            number,
            &agent_name,
            &prompt,
            &plan,
            &session_dir,
            &git,
        );
        return Ok(());
    }

    // ---- Real run from here on ----

    if session_dir.join(crate::store::STATE_FILE).is_file() && !args.resume {
        bail!(
            "a co-review session for {owner}/{repo}#{number} already exists at {}.\n\
             Pass --resume to reuse it, or delete that directory to start fresh.",
            session_dir.display()
        );
    }

    let pr = assemble_pr_info(&git, &owner, &repo, number)?;
    prepare_worktree(&git, &worktree, &pr, args.resume)?;

    // Persist the session state and protocol file.
    std::fs::create_dir_all(&session_dir)
        .with_context(|| format!("creating session dir {}", session_dir.display()))?;
    crate::util::atomic_write(&protocol_path, crate::protocol::PROTOCOL_MD.as_bytes())?;

    let store = Store::new(&session_dir);
    let session_meta = SessionMeta {
        id: slug.clone(),
        worktree: worktree.to_string_lossy().into_owned(),
        source_repo: git.root().to_string_lossy().into_owned(),
        created_at_ms: crate::util::now_ms(),
        agent_pane_id: None,
        view_pane_id: None,
        workspace_id: None,
        agent_kind: agent.kind.clone().unwrap_or_else(|| agent_name.clone()),
        prompt: prompt.clone(),
    };
    if store.exists() && args.resume {
        // keep existing findings; refresh PR metadata + prompt
        store.update(|s| {
            s.pr = pr.clone();
            s.session.prompt = prompt.clone();
            Ok(())
        })?;
    } else {
        store.create(&State::new(pr.clone(), session_meta))?;
    }

    launch_layout(&plan, &store)?;

    eprintln!(
        "co-review session ready for {owner}/{repo}#{number}\n  worktree: {}\n  session:  {}",
        worktree.display(),
        session_dir.display()
    );
    Ok(())
}

fn resolve_prompt(args: &StartArgs, cfg: &Config) -> Result<String> {
    if let Some(p) = &args.prompt {
        return Ok(p.clone());
    }
    if let Some(file) = &args.prompt_file {
        return crate::util::read_path_or_stdin(file);
    }
    Ok(cfg.prompt.clone())
}

/// Fetch PR metadata from GitHub if a token is available, else fall back to what
/// we can learn from git alone.
fn assemble_pr_info(git: &Git, owner: &str, repo: &str, number: u64) -> Result<PrInfo> {
    if let Some(token) = crate::github::resolve_token() {
        match crate::github::Client::new(token).fetch_pr(owner, repo, number) {
            Ok(pr) => return Ok(pr),
            Err(e) => eprintln!("warning: GitHub API lookup failed ({e}); continuing from git"),
        }
    } else {
        eprintln!("note: no GitHub token; base/head metadata will be inferred from git");
    }
    // Fallback: fetch the PR head, resolve its sha, approximate the base as the
    // merge-base with the origin default branch.
    git.fetch("origin", &git::pr_head_refspec(number))?;
    let head_sha = git.rev_parse(&git::pr_head_ref(number))?;
    let base_sha = default_base_sha(git, &head_sha).unwrap_or_default();
    Ok(PrInfo {
        owner: owner.to_string(),
        repo: repo.to_string(),
        number,
        title: String::new(),
        author: String::new(),
        base_ref: String::new(),
        head_ref: git::pr_head_ref(number),
        base_sha,
        head_sha,
        url: crate::github::pr_html_url(owner, repo, number),
    })
}

/// Best-effort base sha: merge-base of the head and the origin's default branch.
fn default_base_sha(git: &Git, head_sha: &str) -> Option<String> {
    for branch in ["origin/HEAD", "origin/main", "origin/master"] {
        if let Ok(mb) = git.merge_base(head_sha, branch) {
            if !mb.is_empty() {
                return Some(mb);
            }
        }
    }
    None
}

fn prepare_worktree(git: &Git, worktree: &Path, pr: &PrInfo, resume: bool) -> Result<()> {
    // Make sure the head is present locally.
    git.fetch("origin", &git::pr_head_refspec(pr.number))
        .with_context(|| format!("fetching PR #{} head", pr.number))?;
    // Bring the base branch too, so diffs work.
    if !pr.base_ref.is_empty() {
        git.fetch("origin", &pr.base_ref).ok();
    }

    let checkout_rev = if pr.head_sha.is_empty() {
        git::pr_head_ref(pr.number)
    } else {
        pr.head_sha.clone()
    };

    if git.worktree_exists(worktree) {
        if resume {
            // Reuse the worktree but move it to the (possibly newer) head so the
            // files and line numbers match the metadata we just refreshed.
            return git.checkout_detach_in(worktree, &checkout_rev);
        }
        // Recreate it clean.
        git.remove_worktree(worktree).ok();
    }
    git.add_worktree(worktree, &checkout_rev)?;
    Ok(())
}

fn launch_layout(plan: &LayoutPlan, store: &Store) -> Result<()> {
    let herdr = Herdr::new(false);
    if !herdr.available() {
        bail!(
            "herdr not found on PATH. Install Herdr (https://herdr.dev) or set HERDR_BIN_PATH.\n\
             The worktree and session are ready; you can open panes manually, or re-run with \
             --dry-run to see the intended layout."
        );
    }

    let ws = herdr.workspace_create(&plan.worktree, &plan.label)?;
    let agent_pane = ws.first_pane.clone();
    // Navigator on the right; keep focus on the agent pane.
    let view_pane = herdr.pane_split(&agent_pane, Direction::Right, false)?;

    herdr.pane_run(&view_pane, &plan.view_argv)?;
    if let Some(agent_argv) = &plan.agent_argv {
        herdr.pane_run(&agent_pane, agent_argv)?;
    }
    herdr.pane_focus(&agent_pane).ok();

    store.update(|s| {
        s.session.workspace_id = Some(ws.id.clone());
        s.session.agent_pane_id = Some(agent_pane.clone());
        s.session.view_pane_id = Some(view_pane.clone());
        Ok(())
    })?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn print_dry_run(
    owner: &str,
    repo: &str,
    number: u64,
    agent_name: &str,
    prompt: &str,
    plan: &LayoutPlan,
    session_dir: &Path,
    git: &Git,
) {
    println!("# co-review start — dry run (no side effects)\n");
    println!("PR:        {owner}/{repo}#{number}");
    println!("agent:     {agent_name}");
    println!("repo:      {}", git.root().display());
    println!("session:   {}", session_dir.display());
    println!("worktree:  {}", plan.worktree);
    println!("\n## git (would run)");
    println!(
        "git fetch --no-tags origin {}",
        git::pr_head_refspec(number)
    );
    println!(
        "git worktree add --detach --force {} <pr-head>",
        plan.worktree
    );
    println!("\n## herdr (would run)");
    println!(
        "herdr workspace create --cwd {} --label {:?}",
        plan.worktree, plan.label
    );
    println!("herdr pane split <w:p1> --direction right --no-focus");
    println!("herdr pane run <w:p2> {:?}", plan.view_argv.join(" "));
    match &plan.agent_argv {
        Some(a) => println!("herdr pane run <w:p1> {:?}", a.join(" ")),
        None => println!("(--no-agent: left pane left as a shell)"),
    }
    println!("\n## prompt handed to the agent\n");
    println!("{prompt}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_prefix_targets_session() {
        let out = env_prefixed("/s/dir", &["claude".into(), "hi".into()]);
        assert_eq!(out[0], "env");
        assert_eq!(out[1], "CO_REVIEW_SESSION=/s/dir");
        assert_eq!(out[2], "claude");
        assert_eq!(out[3], "hi");
    }

    #[test]
    fn plan_has_env_prefixed_view_and_agent() {
        let plan = plan_layout(
            "/usr/bin/co-review",
            "/s/dir",
            "/w/tree",
            "co-review o/r#1",
            Some(&["claude".to_string(), "prompt text".to_string()]),
        );
        assert_eq!(plan.worktree, "/w/tree");
        assert!(plan.view_argv.contains(&"view".to_string()));
        assert_eq!(plan.view_argv[0], "env");
        assert!(plan
            .view_argv
            .iter()
            .any(|a| a == "CO_REVIEW_SESSION=/s/dir"));
        let agent = plan.agent_argv.unwrap();
        assert_eq!(agent[0], "env");
        assert!(agent.contains(&"claude".to_string()));
        assert!(agent.contains(&"prompt text".to_string()));
    }

    #[test]
    fn plan_without_agent() {
        let plan = plan_layout("/b", "/s", "/w", "l", None);
        assert!(plan.agent_argv.is_none());
    }
}
