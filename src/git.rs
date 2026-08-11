//! Git operations, done by shelling out to `git` (see decision log §6).
//!
//! We deliberately avoid linking `libgit2`: `git` is already a hard dependency of
//! the whole workflow, shelling out keeps builds simple, and the operations we
//! need (fetch a PR, add a worktree, read blobs and diffs) are trivial on the
//! command line.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};

use crate::exec;

/// A handle to the source repository co-review reads objects from.
#[derive(Debug, Clone)]
pub struct Git {
    repo: PathBuf,
}

impl Git {
    pub fn new(repo: impl Into<PathBuf>) -> Self {
        Git { repo: repo.into() }
    }

    /// Discover the repository containing `cwd`.
    pub fn discover(cwd: &Path) -> Result<Git> {
        let top = exec::capture("git", &["rev-parse", "--show-toplevel"], Some(cwd))
            .context("not inside a git repository (run co-review from your repo checkout)")?;
        Ok(Git::new(PathBuf::from(top)))
    }

    pub fn root(&self) -> &Path {
        &self.repo
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        exec::capture("git", args, Some(&self.repo))
    }

    /// The URL of a remote (default `origin`).
    pub fn remote_url(&self, remote: &str) -> Result<String> {
        self.git(&["remote", "get-url", remote])
            .with_context(|| format!("no git remote named '{remote}'"))
    }

    /// Fetch a refspec from a remote, e.g. `pull/123/head:refs/co-review/pr-123`.
    pub fn fetch(&self, remote: &str, refspec: &str) -> Result<()> {
        exec::run(
            "git",
            &["fetch", "--no-tags", remote, refspec],
            Some(&self.repo),
        )
        .with_context(|| format!("fetching {refspec} from {remote}"))
    }

    /// Resolve a ref to a full commit sha.
    pub fn rev_parse(&self, rev: &str) -> Result<String> {
        self.git(&["rev-parse", rev])
            .with_context(|| format!("resolving revision {rev}"))
    }

    /// Add a detached worktree at `path`, checked out to `rev`.
    pub fn add_worktree(&self, path: &Path, rev: &str) -> Result<()> {
        let path_s = path.to_string_lossy().into_owned();
        exec::run(
            "git",
            &["worktree", "add", "--detach", "--force", &path_s, rev],
            Some(&self.repo),
        )
        .with_context(|| format!("adding worktree at {} for {rev}", path.display()))
    }

    /// Remove a worktree (best effort; also prunes the admin entry).
    pub fn remove_worktree(&self, path: &Path) -> Result<()> {
        let path_s = path.to_string_lossy().into_owned();
        exec::run(
            "git",
            &["worktree", "remove", "--force", &path_s],
            Some(&self.repo),
        )
        .ok();
        exec::run("git", &["worktree", "prune"], Some(&self.repo))?;
        Ok(())
    }

    pub fn worktree_exists(&self, path: &Path) -> bool {
        path.join(".git").exists()
    }

    /// The contents of a file at a given revision, or `None` if it doesn't exist
    /// there (e.g. a newly-added file has no base version).
    pub fn blob(&self, rev: &str, file: &str) -> Result<Option<String>> {
        let spec = format!("{rev}:{file}");
        let out = exec::try_capture("git", &["show", &spec], Some(&self.repo))?;
        if out.status.success() {
            Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
        } else {
            Ok(None)
        }
    }

    /// The unified diff for a single file between two revisions.
    pub fn diff_file(&self, base: &str, head: &str, file: &str) -> Result<String> {
        self.git(&[
            "diff",
            "--no-color",
            &format!("{base}..{head}"),
            "--",
            file,
        ])
    }

    /// The list of files changed between two revisions.
    pub fn changed_files(&self, base: &str, head: &str) -> Result<Vec<String>> {
        let out = self.git(&["diff", "--name-only", &format!("{base}..{head}")])?;
        Ok(out.lines().map(|l| l.to_string()).filter(|l| !l.is_empty()).collect())
    }

    /// The merge-base of two revisions (the effective diff base for a PR).
    pub fn merge_base(&self, a: &str, b: &str) -> Result<String> {
        self.git(&["merge-base", a, b])
            .with_context(|| format!("computing merge-base of {a} and {b}"))
    }
}

/// The local ref name co-review uses for a PR head.
pub fn pr_head_ref(number: u64) -> String {
    format!("refs/co-review/pr-{number}")
}

/// The refspec that fetches a PR head into [`pr_head_ref`].
pub fn pr_head_refspec(number: u64) -> String {
    format!("pull/{number}/head:{}", pr_head_ref(number))
}

/// Read a file from a worktree directly off the filesystem (faster than `git
/// show` for the head side, which is already checked out).
pub fn read_worktree_file(worktree: &Path, file: &str) -> Result<Option<String>> {
    let path = worktree.join(file);
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow!("reading {}: {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refspec_helpers() {
        assert_eq!(pr_head_ref(123), "refs/co-review/pr-123");
        assert_eq!(
            pr_head_refspec(123),
            "pull/123/head:refs/co-review/pr-123"
        );
    }

    /// End-to-end against a real temp git repo: create commits, add a worktree,
    /// read blobs and diffs. Skips gracefully if `git` is unavailable.
    #[test]
    fn worktree_blob_and_diff_roundtrip() {
        if !exec::have("git") {
            eprintln!("git not available; skipping");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| exec::run("git", args, Some(&repo)).unwrap();
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(repo.join("a.txt"), "line1\nline2\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "base"]);
        let g = Git::new(&repo);
        let base = g.rev_parse("HEAD").unwrap();

        std::fs::write(repo.join("a.txt"), "line1\nCHANGED\nline3\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "head"]);
        let head = g.rev_parse("HEAD").unwrap();

        // blob at base
        let base_blob = g.blob(&base, "a.txt").unwrap().unwrap();
        assert!(base_blob.contains("line2"));
        // missing file -> None
        assert!(g.blob(&base, "nope.txt").unwrap().is_none());
        // diff mentions the change
        let diff = g.diff_file(&base, &head, "a.txt").unwrap();
        assert!(diff.contains("CHANGED"));
        // changed files
        let changed = g.changed_files(&base, &head).unwrap();
        assert_eq!(changed, vec!["a.txt".to_string()]);

        // worktree add + filesystem read
        let wt = dir.path().join("wt");
        g.add_worktree(&wt, &head).unwrap();
        assert!(g.worktree_exists(&wt));
        let f = read_worktree_file(&wt, "a.txt").unwrap().unwrap();
        assert!(f.contains("CHANGED"));
        g.remove_worktree(&wt).unwrap();
    }
}
