# Decision Log

This document records the significant decisions made while building `co-review`,
and *why*. It is append-only: newer decisions go at the bottom. When a decision
is superseded, we keep the old entry and add a new one that references it.

Author: autonomous build session (Claude, `claude-opus-4-8`), starting 2026-08-11.

---

## 0. Problem statement (what the user asked for)

The user reviews PRs with Claude (running inside **Herdr**, a Rust terminal
multiplexer for AI coding agents) and manually on GitHub. Pain point: reading a
wall of findings in the Claude pane with no surrounding code context. Their
current workaround is to have Claude post every finding to GitHub and review
them there.

Desired workflow:

- Run something like `co-review PR123`.
- It checks out the PR in a **new workspace** and starts a **new Claude session**
  in Herdr with a **configurable prompt** (default: the builtin `code-review`
  skill).
- A **split-screen**: on the right, select an individual finding → see all the
  related code. Discuss it live with Claude, decide if it is relevant, adjust,
  and post.
- Claude keeps driving everything that does not need the human (e.g. posting the
  approved findings to GitHub once done, depending on the prompt).
- Bonus: works with agents other than Claude.

Plus: set the repository up with Renovate, semantic releases, CI, etc., like the
user's public `herdr-title-sync` repo.

Two clarifying answers from the user before they disconnected:

1. **Stack: Rust.** ("Something fancy like Rust or Go.")
2. **Chat coupling: Both.** The navigator records a per-finding verdict/notes
   *and* can inject a message straight into the live agent pane, so the human
   keeps chatting with the agent about the selected finding while shared state
   stays authoritative.

---

## 1. Language: Rust

- The user preferred "something fancy like Rust or Go" and confirmed **Rust**.
- Herdr itself is Rust; a Rust plugin fits its ecosystem naturally.
- A single static binary is easy to ship as a Herdr plugin and to install.
- `ratatui` gives us a genuinely polished TUI for the findings navigator, which
  is the centerpiece of the UX.

## 2. Shape: one binary, several subcommands, + a Herdr plugin manifest

`co-review` is a single binary with subcommands. This keeps the agent-facing
contract, the orchestrator, and the TUI in one place sharing one state model.

- `co-review start <pr>` — orchestrate: create the worktree, lay out the Herdr
  split, launch the agent (left) and the navigator (right).
- `co-review view` — the findings navigator TUI (runs in the right pane).
- `co-review add-finding …` — an agent appends a finding (atomic, locked).
- `co-review verdict <id> <state>` — set a per-finding decision (the TUI uses
  the same code path; also usable from a script).
- `co-review wait …` — an agent blocks until the human has acted.
- `co-review list [--json]` — inspect findings/verdicts (agent- and human-usable).
- `co-review post …` — post approved findings to GitHub directly (a fallback for
  agent-agnostic use; the primary path is the agent doing it).
- `co-review doctor` — environment diagnostics.
- `co-review protocol` / `co-review prompt` — print the embedded contract/prompt.

A root `herdr-plugin.toml` (mirroring `herdr-title-sync`'s manifest style) makes
it installable as a Herdr plugin, exposing a "Co-review this PR" action and a
GitHub-PR-URL link handler that Ctrl+click routes to `co-review start`.

## 3. The split-screen is real Herdr panes, not an in-process split

Herdr already owns terminal panes and exposes them over a CLI + local socket
(`herdr workspace create`, `herdr pane split`, `herdr pane run`,
`herdr pane send-text`, `herdr agent start/wait`). So:

- Left pane: the agent (Claude by default), started in the worktree with the
  review prompt.
- Right pane: `co-review view`, the navigator.

This means the "split-screen" is native Herdr — resizable, detachable, and
consistent with the rest of the user's environment — rather than a bespoke split
we would have to reimplement and that would fight Herdr for the terminal.

## 4. Shared state: one lock-guarded `state.json`, never multi-writer JSON

Both the agent and the navigator mutate shared state. Rather than split it into
"agent-owned" and "human-owned" files and hope writes don't interleave, **all**
mutations go through the same `co-review` `Store` type, which takes an advisory
file lock (`fs4`) around every read-modify-write of `state.json`. The agent
mutates it via `co-review add-finding` / `mark-posted`; the navigator mutates it
in-process via the same `Store`. Single source of truth, no races, and it works
identically whether the writer is Claude, another agent, or a human keypress.

The state schema is versioned (`schema_version`) so we can evolve it.

## 5. Agent contract is a CLI, not "please hand-write this JSON"

Agents are unreliable at emitting exact JSON to an exact path. Instead the
contract is a set of small, well-documented CLI verbs (`add-finding`, `list
--json`, `wait`, `mark-posted`). This is:

- **Robust**: the binary owns schema, locking, IDs, timestamps.
- **Agent-agnostic**: any agent that can run shell commands can drive it — which
  is exactly the "bonus points if it works with other agents" ask. The default
  agent/prompt are configurable in `~/.config/co-review/config.toml`.

The embedded protocol (`co-review protocol`) documents these verbs for whatever
agent is driving; the embedded prompt (`co-review prompt`) is the default
instruction handed to the agent, which by default runs the builtin `code-review`
skill and routes each finding through `add-finding` instead of posting directly.

## 6. HTTP + git: blocking `ureq`, and shell out to `git`

- **`ureq`** (blocking, rustls TLS) for the GitHub REST API: no async runtime to
  pull in for a CLI, and rustls avoids an OpenSSL C dependency in CI.
- **Shell out to `git`** for worktree/fetch/diff/blob reads rather than linking
  `libgit2`: simpler builds, and `git` is already a hard dependency of the whole
  workflow. The diff/context computation for a finding reads blobs and diffs via
  `git` and formats them itself.

## 7. Syntax highlighting: `syntect` with `fancy-regex` (no C deps)

The "related code" view highlights code with `syntect`, configured to use the
pure-Rust `fancy-regex` engine instead of `onig` (C). Keeps CI builds portable
and avoids native-toolchain surprises. Highlighting degrades gracefully to plain
text if a syntax/theme can't be resolved.

## 8. Testability without Herdr or gh in the sandbox

The build sandbox has `cargo`, `git`, `node`, `python3`, and the `claude` CLI,
but **not** `herdr` or `gh`. So:

- The herdr layer is a thin wrapper that builds argv and, when
  `CO_REVIEW_FAKE_HERDR` is set (or `--dry-run` is passed to `start`), prints the
  commands instead of executing them — making the orchestrator testable and
  inspectable.
- GitHub auth uses `GH_TOKEN`/`GITHUB_TOKEN` (or `gh auth token` if present).
  Network-touching code is isolated so unit tests never need the network.

## 9. Release tooling: semantic-release, mirroring `herdr-title-sync`

The user asked for a setup "like herdr-title-sync", which uses **semantic-release**
(Conventional Commits → automated versioning + GitHub releases). We use the same
tool, driving a Cargo version bump via `@semantic-release/exec` and attaching
cross-compiled binaries via `@semantic-release/github`. Renovate keeps Cargo,
GitHub-Actions, and npm (release tooling) dependencies current. This gives
cross-language parity with the user's existing repo conventions while remaining
idiomatic for a Rust project.
