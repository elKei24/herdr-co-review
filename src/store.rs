//! The lock-guarded persistence layer for a session's [`State`].
//!
//! Every mutation of `state.json` — whether it comes from an agent running
//! `co-review add-finding`, from the human pressing a key in the TUI, or from a
//! script — funnels through [`Store::update`], which takes an exclusive advisory
//! lock, reads the current state, applies the closure, and writes the result
//! back atomically. That is what lets two writers (agent + human) share one file
//! without corrupting it or losing updates.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use fs4::fs_std::FileExt;

use crate::model::State;

/// Filename of the serialized session state inside the session directory.
pub const STATE_FILE: &str = "state.json";
/// Lockfile guarding access to [`STATE_FILE`].
const LOCK_FILE: &str = ".state.lock";

/// A handle to one session's on-disk state.
#[derive(Debug, Clone)]
pub struct Store {
    session_dir: PathBuf,
}

/// RAII guard holding the exclusive advisory lock. The lock is released when the
/// underlying file handle is dropped; we also unlock explicitly for clarity.
struct LockGuard {
    file: File,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

impl Store {
    pub fn new(session_dir: impl Into<PathBuf>) -> Self {
        Store {
            session_dir: session_dir.into(),
        }
    }

    pub fn session_dir(&self) -> &Path {
        &self.session_dir
    }

    pub fn state_path(&self) -> PathBuf {
        self.session_dir.join(STATE_FILE)
    }

    fn lock_path(&self) -> PathBuf {
        self.session_dir.join(LOCK_FILE)
    }

    pub fn exists(&self) -> bool {
        self.state_path().is_file()
    }

    /// Acquire the exclusive lock, creating the session directory and lockfile if
    /// needed. Blocks until the lock is available.
    fn lock(&self) -> Result<LockGuard> {
        fs::create_dir_all(&self.session_dir)
            .with_context(|| format!("creating session dir {}", self.session_dir.display()))?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.lock_path())
            .with_context(|| format!("opening lock file {}", self.lock_path().display()))?;
        FileExt::lock_exclusive(&file).context("acquiring session lock")?;
        Ok(LockGuard { file })
    }

    fn read_unlocked(&self) -> Result<State> {
        let path = self.state_path();
        let bytes = fs::read(&path)
            .with_context(|| format!("reading session state {}", path.display()))?;
        let state: State = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing session state {}", path.display()))?;
        Ok(state)
    }

    fn write_unlocked(&self, state: &mut State) -> Result<()> {
        // Bump the revision so readers detect the change regardless of mtime
        // granularity.
        state.rev = state.rev.wrapping_add(1);
        let json = serde_json::to_vec_pretty(state).context("serializing session state")?;
        crate::util::atomic_write(&self.state_path(), &json)
    }

    /// Read the current state under a shared… well, exclusive lock (advisory
    /// locks here are simple exclusive locks; reads are fast so this is fine).
    pub fn read(&self) -> Result<State> {
        let _guard = self.lock()?;
        self.read_unlocked()
    }

    /// Read without locking. Only for read-only consumers that can tolerate
    /// observing a momentarily-old snapshot (e.g. the TUI's polling refresh),
    /// where taking the lock on every frame would be wasteful.
    pub fn read_lossy(&self) -> Result<State> {
        self.read_unlocked()
    }

    /// Write an initial state. Overwrites any existing file (used by `start`).
    pub fn create(&self, state: &State) -> Result<()> {
        let _guard = self.lock()?;
        let mut state = state.clone();
        self.write_unlocked(&mut state)
    }

    /// Atomically read-modify-write the state. The closure receives a mutable
    /// reference to the current state and may return a value that is passed back
    /// to the caller. If the closure returns an error, the state is left
    /// unchanged.
    pub fn update<T>(&self, f: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        let _guard = self.lock()?;
        let mut state = self.read_unlocked()?;
        let out = f(&mut state)?;
        self.write_unlocked(&mut state)?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Finding, PrInfo, SessionMeta};

    fn seed(dir: &Path) -> Store {
        let store = Store::new(dir);
        let pr = PrInfo {
            owner: "o".into(),
            repo: "r".into(),
            number: 1,
            title: String::new(),
            author: String::new(),
            base_ref: String::new(),
            head_ref: String::new(),
            base_sha: String::new(),
            head_sha: String::new(),
            url: String::new(),
        };
        let session = SessionMeta {
            id: "o-r-1".into(),
            worktree: dir.display().to_string(),
            created_at_ms: 0,
            agent_pane_id: None,
            view_pane_id: None,
            workspace_id: None,
            agent_kind: "claude".into(),
            prompt: String::new(),
        };
        store.create(&State::new(pr, session)).unwrap();
        store
    }

    #[test]
    fn create_read_update_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let store = seed(dir.path());
        assert!(store.exists());

        let id = store
            .update(|s| {
                let id = s.mint_finding_id();
                s.findings.push(Finding::new(id.clone(), "t".into()));
                Ok(id)
            })
            .unwrap();
        assert_eq!(id, "f1");

        let state = store.read().unwrap();
        assert_eq!(state.findings.len(), 1);
        assert_eq!(state.findings[0].id, "f1");
    }

    #[test]
    fn concurrent_updates_do_not_lose_writes() {
        let dir = tempfile::tempdir().unwrap();
        let store = seed(dir.path());
        let path = dir.path().to_path_buf();

        let mut handles = Vec::new();
        for _ in 0..8 {
            let p = path.clone();
            handles.push(std::thread::spawn(move || {
                let s = Store::new(&p);
                for _ in 0..25 {
                    s.update(|st| {
                        let id = st.mint_finding_id();
                        st.findings.push(Finding::new(id, "t".into()));
                        Ok(())
                    })
                    .unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        let state = store.read().unwrap();
        // 8 threads * 25 each = 200 findings, none lost, ids unique.
        assert_eq!(state.findings.len(), 200);
        assert_eq!(state.next_finding_seq, 200);
        let mut ids: Vec<_> = state.findings.iter().map(|f| f.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 200);
    }

    #[test]
    fn rev_increments_on_every_write() {
        let dir = tempfile::tempdir().unwrap();
        let store = seed(dir.path()); // create() -> rev 1
        let r0 = store.read().unwrap().rev;
        assert_eq!(r0, 1);
        store.update(|_| Ok(())).unwrap();
        let r1 = store.read().unwrap().rev;
        assert_eq!(r1, 2);
        // a plain read does not bump the revision
        let r2 = store.read().unwrap().rev;
        assert_eq!(r2, 2);
    }

    #[test]
    fn closure_error_leaves_state_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let store = seed(dir.path());
        let _ = store.update::<()>(|s| {
            s.findings.push(Finding::new("f1".into(), "t".into()));
            anyhow::bail!("boom")
        });
        let state = store.read().unwrap();
        assert_eq!(state.findings.len(), 0);
    }
}
