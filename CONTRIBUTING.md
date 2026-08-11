# Contributing

Thanks for helping improve `co-review`.

## Development

Requires a recent Rust toolchain (see `rust-version` in `Cargo.toml`) and `git`.

```sh
cargo build            # build
cargo test             # run the test suite
cargo fmt --all        # format
cargo clippy --all-targets -- -D warnings   # lint (CI is warning-free)
```

Node is only needed for the release/lint tooling:

```sh
npm ci                 # semantic-release + commitlint
```

### Trying it end to end

`co-review start` needs a real PR and Herdr. To exercise the machinery without
either, use `--dry-run` (an offline preview) or drive the CLI against a local
session directly — `CO_REVIEW_FAKE_HERDR=1` makes the Herdr layer print commands
instead of running them, and `CO_REVIEW_HOME` relocates all state. The tests in
`src/orchestrate.rs`, `src/git.rs`, and `src/tui/mod.rs` show the patterns.

## Commit messages

This repo uses [Conventional Commits](https://www.conventionalcommits.org/); the
version and changelog are derived from them by
[semantic-release](https://semantic-release.gitbook.io/). CI lints PR commits.

- `feat: …` — a new feature (minor release)
- `fix: …` — a bug fix (patch release)
- `docs:`, `refactor:`, `test:`, `chore:`, `ci:` — no release on their own
- A `!` after the type or a `BREAKING CHANGE:` footer triggers a major release

Examples:

```
feat: show base-side context for @base locations
fix: keep selection stable when the agent adds a finding
```

## Releases

Pushing to `main` runs `semantic-release`, which decides the next version from
the commits, updates `Cargo.toml`/`Cargo.lock`/`CHANGELOG.md`, tags the release,
and attaches a built Linux binary. There's nothing to do manually.

## Architecture

`docs/DECISIONS.md` records the significant design decisions and why they were
made. If you change one of them, add a new dated entry rather than rewriting the
old one.
