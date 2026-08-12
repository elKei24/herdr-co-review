//! Tests for `scripts/link-on-path.sh`, the step that makes `co-review`
//! runnable from a shell after `herdr plugin install`.
//!
//! No network and no real binary: the "binary" is an executable stub, and every
//! run gets its own HOME, PATH and install dir.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

const PLUGIN: &str = "herdr/plugins/github/elkei24.co-review-abc";

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/link-on-path.sh")
}

/// An executable stub standing in for the plugin's `bin/co-review`.
fn plugin_binary(root: &Path, plugin: &str) -> PathBuf {
    let bindir = root.join(plugin).join("bin");
    fs::create_dir_all(&bindir).unwrap();
    let bin = bindir.join("co-review");
    fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// A home with a plugin binary in it, plus the install dir the tests link into.
fn setup_at(plugin: &str) -> (TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let src = plugin_binary(root.path(), plugin);
    let dir = root.path().join("bin");
    fs::create_dir_all(&dir).unwrap();
    (root, src, dir)
}

fn setup() -> (TempDir, PathBuf, PathBuf) {
    setup_at(PLUGIN)
}

/// Run the script for `src`. `dir` is the install dir override, and every
/// directory in `path` goes on PATH. Returns (stdout + stderr, ok).
fn link(
    src: &Path,
    dir: Option<&Path>,
    home: &Path,
    path: &[&Path],
    envs: &[(&str, &str)],
) -> (String, bool) {
    let path = path
        .iter()
        .map(|p| p.display().to_string())
        .chain(["/usr/bin".into(), "/bin".into()])
        .collect::<Vec<_>>()
        .join(":");
    let mut cmd = Command::new("bash");
    cmd.arg(script())
        .arg(src)
        .env("HOME", home)
        .env("PATH", path);
    match dir {
        Some(d) => cmd.env("CO_REVIEW_INSTALL_DIR", d),
        None => cmd.env_remove("CO_REVIEW_INSTALL_DIR"),
    };
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn bash");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (text, out.status.success())
}

/// The common case: install dir given and on PATH.
fn link_into(src: &Path, dir: &Path, home: &Path) -> (String, bool) {
    link(src, Some(dir), home, &[dir], &[])
}

#[test]
fn links_the_binary_into_the_install_dir() {
    let (root, src, dir) = setup();

    let (out, ok) = link_into(&src, &dir, root.path());
    assert!(ok, "script failed: {out}");
    assert_eq!(fs::read_link(dir.join("co-review")).unwrap(), src);
    assert!(!out.contains("not on your PATH"), "{out}");
}

#[test]
fn replaces_its_own_link_after_a_plugin_update() {
    let (root, old, dir) = setup();
    let new = plugin_binary(root.path(), "herdr/plugins/github/elkei24.co-review-new");

    link_into(&old, &dir, root.path());
    let (out, ok) = link_into(&new, &dir, root.path());
    assert!(ok, "script failed: {out}");
    assert_eq!(fs::read_link(dir.join("co-review")).unwrap(), new);
}

#[test]
fn repairs_a_link_left_dangling_by_an_uninstall() {
    let (root, src, dir) = setup();
    symlink(
        root.path().join("gone/bin/co-review"),
        dir.join("co-review"),
    )
    .unwrap();

    let (out, ok) = link_into(&src, &dir, root.path());
    assert!(ok, "script failed: {out}");
    assert_eq!(fs::read_link(dir.join("co-review")).unwrap(), src);
}

#[test]
fn links_to_the_final_plugin_dir_when_built_in_a_staging_checkout() {
    // `herdr plugin install` runs the build in plugins/.tmp-install-*/checkout
    // and moves that checkout to plugins/github/<id>-<sha256(id)[:12]> after —
    // a link to the staging path would dangle (verified on herdr 0.8.0).
    let staging = "herdr/plugins/.tmp-install-123-456/checkout";
    let (root, src, dir) = setup_at(staging);
    fs::write(
        root.path().join(staging).join("herdr-plugin.toml"),
        "id = \"elkei24.co-review\"\nname = \"co-review\"\n",
    )
    .unwrap();

    let (out, ok) = link_into(&src, &dir, root.path());
    assert!(ok, "script failed: {out}");
    // sha256("elkei24.co-review")[..12]
    let expected = root
        .path()
        .join("herdr/plugins/github/elkei24.co-review-cd7e469ad4bb/bin/co-review");
    assert_eq!(fs::read_link(dir.join("co-review")).unwrap(), expected);
}

#[test]
fn keeps_a_binary_the_plugin_did_not_install() {
    let (root, src, dir) = setup();
    let existing = dir.join("co-review");
    fs::write(&existing, "hand-installed").unwrap();

    let (out, ok) = link_into(&src, &dir, root.path());
    assert!(ok, "script must not fail the install: {out}");
    assert_eq!(fs::read_to_string(&existing).unwrap(), "hand-installed");
    assert!(out.contains("leaving it alone"), "{out}");
}

#[test]
fn keeps_a_symlink_pointing_somewhere_else() {
    let (root, src, dir) = setup();
    let other = plugin_binary(root.path(), "elsewhere");
    symlink(&other, dir.join("co-review")).unwrap();

    let (out, ok) = link_into(&src, &dir, root.path());
    assert!(ok, "script must not fail the install: {out}");
    assert_eq!(fs::read_link(dir.join("co-review")).unwrap(), other);
    assert!(out.contains("leaving it alone"), "{out}");
}

#[test]
fn does_not_shadow_a_co_review_elsewhere_on_path() {
    let (root, src, dir) = setup();
    let theirs = root.path().join("usr-local-bin");
    fs::create_dir_all(&theirs).unwrap();
    fs::write(theirs.join("co-review"), "hand-installed").unwrap();

    let (out, ok) = link(&src, Some(&dir), root.path(), &[&dir, &theirs], &[]);
    assert!(ok, "script must not fail the install: {out}");
    assert!(!dir.join("co-review").exists(), "{out}");
    assert!(out.contains("leaving PATH alone"), "{out}");
}

#[test]
fn opts_out_via_env() {
    let (root, src, dir) = setup();

    let (out, ok) = link(
        &src,
        Some(&dir),
        root.path(),
        &[&dir],
        &[("CO_REVIEW_NO_PATH_LINK", "1")],
    );
    assert!(ok, "script failed: {out}");
    assert!(!dir.join("co-review").exists());
    assert!(out.contains("CO_REVIEW_NO_PATH_LINK"), "{out}");
}

#[test]
fn falls_back_to_local_bin_and_warns_when_it_is_not_on_path() {
    let (root, src, _) = setup();

    let (out, ok) = link(&src, None, root.path(), &[], &[]);
    assert!(ok, "script failed: {out}");
    let link = root.path().join(".local/bin/co-review");
    assert_eq!(fs::read_link(&link).unwrap(), src);
    assert!(out.contains("not on your PATH"), "{out}");
}
