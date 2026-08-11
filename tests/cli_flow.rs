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

    // edit revises only the passed fields
    let (_o, err, ok) = co_review(
        &home,
        Some(&session),
        &work,
        &[
            "edit",
            "f1",
            "--title",
            "Off-by-one on line 2",
            "--severity",
            "critical",
        ],
    );
    assert!(ok, "edit failed: {err}");
    let (out, _e, _ok) = co_review(&home, Some(&session), &work, &["list", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["findings"][0]["title"], "Off-by-one on line 2");
    assert_eq!(v["findings"][0]["severity"], "critical");
    assert_eq!(v["findings"][0]["body"], "line 2 changed"); // untouched

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
    assert!(out.contains("Off-by-one on line 2")); // the edited title

    // sessions lists the live session
    let (out, _e, ok) = co_review(&home, None, &work, &["sessions"]);
    assert!(ok);
    assert!(
        out.contains("owner/repo #1"),
        "sessions missing entry: {out}"
    );

    // end removes the worktree and the session directory (--force: panes still
    // recorded from the fake-herdr start)
    let (_o, err, ok) = co_review(&home, None, &work, &["end", "owner/repo#1", "--force"]);
    assert!(ok, "end failed: {err}");
    assert!(
        !session.join("state.json").exists(),
        "session dir not removed"
    );
    assert!(!wt.join(".git").exists(), "worktree not removed");
}

#[test]
fn resume_updates_worktree_to_new_head() {
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
    std::fs::write(work.join("f.txt"), "one\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-qm", "base"]);
    git(&work, &["push", "-q", "origin", "HEAD:main"]);
    git(&work, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(work.join("f.txt"), "one\ntwo\n").unwrap();
    git(&work, &["commit", "-qam", "v1"]);
    git(&work, &["push", "-q", "origin", "HEAD:refs/pull/1/head"]);

    let (_o, err, ok) = co_review(&home, None, &work, &["start", "owner/repo#1"]);
    assert!(ok, "start failed: {err}");
    let wt = home.join("worktrees").join("owner-repo-1");
    assert_eq!(
        std::fs::read_to_string(wt.join("f.txt")).unwrap(),
        "one\ntwo\n"
    );

    // PR is rebased/amended so its head no longer descends from what we fetched
    // (a non-fast-forward update — the case the `+` in the refspec handles).
    std::fs::write(work.join("f.txt"), "one\ntwo\nthree\n").unwrap();
    git(&work, &["commit", "-qam", "v1 amended", "--amend"]);
    git(
        &work,
        &["push", "-q", "-f", "origin", "HEAD:refs/pull/1/head"],
    );

    // resume should move the worktree to the new head.
    let (_o, err, ok) = co_review(&home, None, &work, &["start", "owner/repo#1", "--resume"]);
    assert!(ok, "resume failed: {err}");
    assert_eq!(
        std::fs::read_to_string(wt.join("f.txt")).unwrap(),
        "one\ntwo\nthree\n"
    );
}

#[test]
fn edit_resets_decided_verdict_and_clears_fields() {
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
    std::fs::write(work.join("f.txt"), "a\nb\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-qm", "base"]);
    git(&work, &["push", "-q", "origin", "HEAD:main"]);
    git(&work, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(work.join("f.txt"), "a\nB\n").unwrap();
    git(&work, &["commit", "-qam", "c"]);
    git(&work, &["push", "-q", "origin", "HEAD:refs/pull/1/head"]);

    let (_o, err, ok) = co_review(&home, None, &work, &["start", "owner/repo#1"]);
    assert!(ok, "start failed: {err}");
    let session = home.join("sessions").join("owner-repo-1");

    co_review(
        &home,
        Some(&session),
        &work,
        &[
            "add-finding",
            "--title",
            "T",
            "--severity",
            "low",
            "--location",
            "f.txt:2",
            "--suggestion",
            "let x = 1;",
            "--category",
            "style",
        ],
    );
    co_review(&home, Some(&session), &work, &["verdict", "f1", "approved"]);

    // editing a decided finding resets its verdict and can clear fields
    let (out, err, ok) = co_review(
        &home,
        Some(&session),
        &work,
        &[
            "edit",
            "f1",
            "--body",
            "new body",
            "--clear-suggestion",
            "--clear-category",
        ],
    );
    assert!(ok, "edit failed: {err}");
    assert!(
        out.contains("reset to pending"),
        "expected reset note: {out}"
    );

    let (out, _e, _ok) = co_review(&home, Some(&session), &work, &["list", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["findings"][0]["verdict"], "pending");
    assert_eq!(v["findings"][0]["body"], "new body");
    assert!(
        v["findings"][0]["suggestion"].is_null(),
        "suggestion not cleared"
    );
    assert!(
        v["findings"][0]["category"].is_null(),
        "category not cleared"
    );
}

#[test]
fn doctor_runs_without_a_session() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let (out, _e, ok) = co_review(&home, None, root.path(), &["doctor"]);
    assert!(ok, "doctor should succeed");
    assert!(out.contains("co-review"));
}
