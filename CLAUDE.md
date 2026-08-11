# CLAUDE.md

Guidance for AI agents working in this repository.

## What this is

`co-review` is a Rust CLI + Herdr plugin for **interactive, split-screen PR
review**: an agent reviews a PR in one Herdr pane and records findings; the human
triages them in a navigator TUI pane that shows each finding with its related
code, then the agent posts the approved ones. See `README.md` for the product and
`docs/DECISIONS.md` for *why* it's built the way it is — read the decision log
before making structural changes, and add a new dated entry rather than rewriting
old ones.

## Commands

```sh
cargo build                                  # build
cargo test                                   # unit + integration tests
cargo clippy --all-targets -- -D warnings    # lint (CI is warning-free — keep it so)
cargo fmt --all                              # format (run before committing)
cargo test dump_frame -- --ignored --nocapture   # eyeball the TUI render
```

Node is only for release/lint tooling: `npm ci`, then `npx commitlint`.

## Layout

- `model.rs` — the shared `State` and everything in it; the single source of
  truth for finding tallies (`counts`), the session slug (`pr_slug`), and enum
  parsing/labels. Put shared logic here, not at call sites.
- `store.rs` — the **only** way state is mutated: `Store::update` takes an
  exclusive advisory lock and bumps a monotonic `rev`. Never write `state.json`
  directly.
- `commands.rs` — agent/human subcommands. `orchestrate.rs` — `start`.
  `tui/` — the navigator.
- `git.rs` / `github.rs` / `herdr.rs` / `exec.rs` — external integrations, each a
  thin wrapper. `diffview.rs` — turns a finding into its related code.

## Conventions

- **Conventional Commits** (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`,
  `chore:`, `ci:`) — semantic-release derives versions from them; CI lints them.
- **Tests never touch the network.** Integration tests (`tests/cli_flow.rs`)
  drive the real binary against a local bare repo standing in for GitHub, with
  `GH_TOKEN=""` to force the offline path and `CO_REVIEW_FAKE_HERDR=1` so the
  Herdr layer prints instead of executing. Follow that pattern.
- Keep the agent-facing contract a **CLI** (`add-finding`, `edit`, `wait`, …),
  not hand-written JSON — that's what makes co-review agent-agnostic. If you add
  an agent capability, add it to the CLI, document it in `src/protocol.rs`
  (`PROTOCOL_MD`) and `skills/co-review/SKILL.md`, and cover it in a test.
- Add a unit test next to new logic; add an integration step for new subcommands.
