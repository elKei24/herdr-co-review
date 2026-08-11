//! End-to-end integration test of the agent-facing flow against a local bare
//! repository standing in for GitHub. Exercises `start` (worktree + fake Herdr
//! layout), `add-finding`, `list --json`, `verdict`, `wait`, and `post
//! --dry-run` through the real compiled binary.
//!
//! Skips gracefully if `git` is unavailable. No network: the GitHub token is
//! forced empty so `start` takes the pure-git fallback path.

use std::path::Path;
use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_co-review")
}

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Run the co-review binary against a session, returning (stdout, stderr, ok).
fn co_review(
    home: &Path,
    session: Option<&Path>,
    cwd: &Path,
    args: &[&str],
) -> (String, String, bool) {
    let mut cmd = Command::new(bin());
    cmd.args(args)
        .current_dir(cwd)
        .env("CO_REVIEW_HOME", home)
        .env("CO_REVIEW_FAKE_HERDR", "1")
        // Force the no-token path so we never touch the network.
        .env("GH_TOKEN", "")
        .env("GITHUB_TOKEN", "");
    if let Some(s) = session {
        cmd.env("CO_REVIEW_SESSION", s);
    }
    let out = cmd.output().expect("spawn co-review");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

#[test]
fn full_agent_flow() {
    if !have_git() {
        eprintln!("git unavailable; skipping");
        return;
    }

    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let bare = root.path().join("origin.git");
    let work = root.path().join("work");

    git(
        root.path(),
        &["init", "-q", "--bare", bare.to_str().unwrap()],
    );
    git(
        root.path(),
        &[
            "clone",
            "-q",
            bare.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    git(&work, &["config", "user.email", "t@t"]);
    git(&work, &["config", "user.name", "t"]);
    std::fs::write(work.join("f.txt"), "a\nb\nc\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-qm", "base"]);
    git(&work, &["push", "-q", "origin", "HEAD:main"]);
    git(&work, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(work.join("f.txt"), "a\nB2\nc\nd\n").unwrap();
    git(&work, &["commit", "-qam", "change"]);
    git(&work, &["push", "-q", "origin", "HEAD:refs/pull/1/head"]);

    // start
    let (_o, err, ok) = co_review(&home, None, &work, &["start", "owner/repo#1"]);
    assert!(ok, "start failed: {err}");
    let session = home.join("sessions").join("owner-repo-1");
    assert!(
        session.join("state.json").is_file(),
        "state.json not created"
    );
    assert!(
        session.join("CO_REVIEW.md").is_file(),
        "protocol file not written"
    );
    // worktree checked out at head
    let wt = home.join("worktrees").join("owner-repo-1");
    assert_eq!(
        std::fs::read_to_string(wt.join("f.txt")).unwrap(),
        "a\nB2\nc\nd\n"
    );

    // add-finding -> prints id f1
    let (out, err, ok) = co_review(
        &home,
        Some(&session),
        &work,
        &[
            "add-finding",
            "--title",
            "Check line 2",
            "--severity",
            "high",
            "--location",
            "f.txt:2",
            "--body",
            "line 2 changed",
        ],
    );
    assert!(ok, "add-finding failed: {err}");
    assert_eq!(out.trim(), "f1");

    // list --json -> one pending finding
    let (out, _e, ok) = co_review(&home, Some(&session), &work, &["list", "--json"]);
    assert!(ok);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["findings"].as_array().unwrap().len(), 1);
    assert_eq!(v["findings"][0]["verdict"], "pending");
    assert_eq!(v["findings"][0]["severity"], "high");

    // verdict approve
    let (_o, err, ok) = co_review(&home, Some(&session), &work, &["verdict", "f1", "approved"]);
    assert!(ok, "verdict failed: {err}");

    // hand off + wait should return immediately (0 pending)
    let (_o, _e, ok) = co_review(
        &home,
        Some(&session),
        &work,
        &["set-status", "awaiting_review"],
    );
    assert!(ok);
    let (_o, err, ok) = co_review(&home, Some(&session), &work, &["wait", "--timeout", "3000"]);
    assert!(ok, "wait did not return: {err}");

    // post --dry-run lists the approved finding without needing a token
    let (out, err, ok) = co_review(&home, Some(&session), &work, &["post", "--dry-run"]);
    assert!(ok, "post --dry-run failed: {err}");
    assert!(out.contains("would post 1 finding"), "unexpected: {out}");
    assert!(out.contains("Check line 2"));
}

#[test]
fn doctor_runs_without_a_session() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let (out, _e, ok) = co_review(&home, None, root.path(), &["doctor"]);
    assert!(ok, "doctor should succeed");
    assert!(out.contains("co-review"));
}
