//! Thin wrappers around spawning external processes (`git`, `herdr`, …).
//!
//! Centralized so error messages are consistent ("`git status` failed: …") and
//! so tests can reason about one code path.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use anyhow::{anyhow, Context, Result};

/// Run a command and return its trimmed stdout, erroring on a non-zero exit with
/// the captured stderr included.
pub fn capture<S: AsRef<OsStr>>(program: &str, args: &[S], cwd: Option<&Path>) -> Result<String> {
    let out = raw(program, args, cwd)?;
    if !out.status.success() {
        return Err(command_error(program, args, &out));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Run a command and return the full [`Output`] regardless of exit status, for
/// callers that legitimately expect failures (e.g. `git show` on a missing path).
pub fn try_capture<S: AsRef<OsStr>>(
    program: &str,
    args: &[S],
    cwd: Option<&Path>,
) -> Result<Output> {
    raw(program, args, cwd)
}

/// Run a command for its side effects, streaming nothing, erroring on failure.
pub fn run<S: AsRef<OsStr>>(program: &str, args: &[S], cwd: Option<&Path>) -> Result<()> {
    let out = raw(program, args, cwd)?;
    if !out.status.success() {
        return Err(command_error(program, args, &out));
    }
    Ok(())
}

/// Whether a program is resolvable on `PATH`.
pub fn have(program: &str) -> bool {
    // `command -v` via a direct spawn attempt is unreliable across shells; probe
    // by trying to run it with `--version` and seeing if it spawns at all.
    which(program).is_some()
}

/// Locate a program on `PATH`, returning its full path if found.
pub fn which(program: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
        // Windows executables.
        for ext in ["exe", "cmd", "bat"] {
            let c = candidate.with_extension(ext);
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

fn raw<S: AsRef<OsStr>>(program: &str, args: &[S], cwd: Option<&Path>) -> Result<Output> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.output()
        .with_context(|| format!("failed to spawn `{program}` (is it installed and on PATH?)"))
}

fn command_error<S: AsRef<OsStr>>(program: &str, args: &[S], out: &Output) -> anyhow::Error {
    let argline = args
        .iter()
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    let stderr = String::from_utf8_lossy(&out.stderr);
    let stderr = stderr.trim();
    anyhow!(
        "`{program} {argline}` failed ({}){}",
        out.status,
        if stderr.is_empty() {
            String::new()
        } else {
            format!(": {stderr}")
        }
    )
}
