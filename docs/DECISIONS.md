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

## 10. Pane sizing is left to Herdr

An earlier draft had an `agent_pane_ratio` config knob, but the `herdr pane
split` CLI takes no ratio, and we can't verify a resize verb from here, so the
field did nothing. Rather than ship a config option that silently has no effect,
we removed it: the split is created 50/50 and the user resizes with Herdr's own
mouse/keys. Only worktree checkouts are supported (the unused `clone` mode was
removed for the same reason). If a reliable Herdr resize API is confirmed later,
the ratio can come back wired to it.

## 11. Quality-pass outcomes (code-review + simplify)

The build was reviewed by an adversarial `/code-review` pass and a 4-angle
`/simplify` pass. Notable fixes that shaped the code:

- **Diff base**: the related-code view diffs the *merge-base* (three-dot
  `base...head`) so it matches GitHub's diff even when the base branch advanced
  after the PR branched — a two-dot range would fold in unrelated base changes.
- **Live-reload correctness**: state carries a monotonic `rev` bumped on every
  write; the navigator reloads on `rev` change rather than file mtime, so two
  rapid agent writes are never coalesced into a missed update.
- **One source of truth**: finding tallies (`State::counts`), the session slug
  (`model::pr_slug`), status parsing (`ReviewStatus::parse`), the file-or-stdin
  reader (`util::read_path_or_stdin`), and the `Side`/`LineKind` label/sign
  helpers are each defined once and reused by the CLI and TUI.
- **TUI efficiency**: related-code blocks are memoized per finding id (git diff
  runs at most once per finding), and the event loop repaints only when a
  `dirty` flag is set instead of several times a second while idle.
- **Live agent status** in the navigator header (working/blocked/done) is
  best-effort: it polls `herdr agent list` every ~1.5s and leniently scans the
  line for the agent pane. If Herdr isn't present or the format differs, it shows
  nothing rather than erroring — so it can only add signal, never break the UI.

## 12. Prebuilt binaries, and a plugin that doesn't need Rust

Users shouldn't have to compile the tool. The release runs in two stages:
semantic-release computes the version, bumps `Cargo.toml`/lock/CHANGELOG, tags,
and creates the GitHub release (stage 1); a cross-platform matrix then builds a
binary for each target from that tag and uploads it to the release (stage 2,
gated on stage 1 having published — the version flows between stages via the
exec plugin's `successCmd` writing to `$GITHUB_OUTPUT`). Assets are named without
the version (`co-review-<target>.tar.gz`) so the stable
`releases/latest/download/<asset>` URL works.

The Herdr plugin's install step therefore runs `scripts/install-binary.sh`, which
**downloads** the right prebuilt asset for the platform and only falls back to
`cargo build` if none is available — so installing the plugin needs no Rust
toolchain. Targets: linux and macOS (x86_64 + aarch64) and Windows x86_64;
linux-aarch64 cross-compiles on the ubuntu runner with the
`gcc-aarch64-linux-gnu` linker (the crate is pure-Rust: rustls, `fancy-regex`
instead of `onig`, no other C deps).

## 13. First contact with real Herdr (0.8.0): JSON responses, `agent prompt`, opaque ids

The tool was built blind against a simulated Herdr (§8). Running against a real
Herdr 0.8.0 session (2026-08-12) invalidated several guesses, all fixed:

- **Herdr control commands return JSON**, not prose. `workspace create` reports
  `.result.workspace.workspace_id` / `.result.root_pane.pane_id`, `pane split`
  reports `.result.pane.pane_id`, and `agent list` reports
  `.result.agents[].agent_status`. The wrapper now parses these; the old
  token-scan survives only as a fallback.
- **Ids are opaque** — not necessarily `w<digits>` (a live session produced
  `wP:p1`), so nothing may assume numeric ids.
- **Chat injection uses `herdr agent prompt`**, which submits text + Enter
  atomically and honors bracketed paste. The raw `pane send-text` + `send-keys
  Enter` path is kept only as a fallback when Herdr has not recognized an agent
  in the pane (custom agent commands); any other prompt failure is surfaced in
  the navigator instead of silently pretending delivery (a prompt to a
  just-started agent can stall — observed live).
- **Agent lifecycle states are `idle|working|blocked|done|unknown`**; `unknown`
  is shown as nothing.
- **The clicked-URL env var does not exist.** Plugin invocations receive
  `$HERDR_PLUGIN_CONTEXT_JSON` (with `clicked_url`, `focused_pane_cwd`,
  `workspace_cwd`). Plugin actions also run with the *plugin root* as cwd — a
  git checkout of co-review itself — so `start` now resolves the source repo
  from the context's pane cwd, and fetches from the PR's GitHub URL when the
  surrounding repo's origin is a different GitHub repo. Herdr also runs plugin
  commands with a minimal PATH, so the action goes through
  `scripts/run-action.sh`, which restores common bin dirs.
- `pane split` only supports `right|down`, and takes `--cwd` (now passed);
  a `--ratio` also exists in 0.8.0, so §10's removed knob could return.

## 14. Dependabot instead of Renovate; supply-chain hardening

§9 chose Renovate, but the app was never installed on the (private) repo, so no
update PRs ever arrived; herdr-title-sync meanwhile uses native Dependabot.
Switched to `.github/dependabot.yml` (github-actions, cargo, npm — weekly,
grouped, Conventional-Commit prefixes so commitlint passes and cargo bumps
release as `fix`). Renovate's config was deleted to avoid two bots if the app
ever gets installed.

Hardening, mirroring herdr-title-sync PRs #3/#4: all GitHub Actions pinned to
full commit SHAs (Dependabot keeps the pins current), `persist-credentials:
false` on every checkout that doesn't push, job-level least-privilege
permissions in the release workflow, and the release commit pushed over SSH via
a `RELEASE_DEPLOY_KEY` deploy key (which can bypass a branch ruleset once one
exists — rulesets need the repo to be public or on GitHub Pro). With the secret
unset, checkout falls back to token auth, so the pipeline still works before
the key is configured.

## 15. Sessions put the launching binary on the panes' PATH

The prompt and protocol tell the agent to run bare `co-review …`, but a
plugin-only install keeps the binary private under the plugin root — the agent's
first `add-finding` would die with "command not found" (caught by the user right
after install; the live e2e had missed it by always using absolute paths). The
fix is in the layout, not the docs: the `env` prefix on both pane commands now
also prepends the launching binary's directory to PATH, so whatever copy ran
`start` — plugin, installer, or a dev build — is the one the agent and the
navigator find. A separate CLI install remains optional, for running `co-review`
in shells outside a session.

## 16. The navigator captures the mouse

Without mouse capture the terminal translated wheel events into arrow keys, so
the wheel always moved the findings selection and the detail/code pane — the
larger of the two — could only be scrolled with `J`/`K`. Clicking did nothing.

`co-review view` now enables mouse capture (and disables it on exit and in the
panic hook, next to the alternate-screen teardown). A left click focuses the
pane under it, and in the findings list also selects the row under the cursor; a
wheel event focuses the pane it happens over and scrolls it. The focused pane
gets a lit border so the wheel's target is never a guess.

Two consequences worth knowing. The list keeps a *selection*, not a free scroll
offset — ratatui forces the offset to keep the selection visible, so a wheel
over the list moves the selection (exactly what `j`/`k` do) rather than
fighting the widget. And capture takes the mouse away from the terminal's own
text selection; `Shift`+drag is the standard escape hatch, so the help overlay
and the README say so.

Hit-testing needs the geometry that was actually painted, so `ui::draw` takes
`&mut App` and records the two pane rects, the list's settled scroll offset, and
the detail's maximum scroll. That also gave the detail scroll a real upper bound
(`J` used to increment forever, and the clamp only happened at render time).

## 17. Installing the plugin puts `co-review` on the user's PATH

Decision 15 fixed the agent's PATH inside a session, but not the human's: after
`herdr plugin install`, the README's very next section tells you to run
`co-review start 123`, and the binary lived only under the plugin root. Every
plugin-first user hit "command not found" between the install and the usage
section.

Herdr has no manifest field for exposing a plugin binary (verified on 0.8.0: the
manifest is `build`/`startup`/`panes`/`actions`/`events`/`link_handlers`, and no
event fires on install or uninstall — `herdr-reviewr` sidesteps this by making
its build step `cargo install`, which we can't, since the point of decision 12
is not needing Rust). So the build step does it: `scripts/link-on-path.sh`
symlinks the downloaded binary into a PATH directory.

A symlink, not a copy, so a plugin update is instantly live and there is only
ever one binary. It replaces only a link it owns — one pointing at a `co-review`
under a herdr plugin root, or one left dangling, which is worth nothing to
anyone. The user's own install outranks ours, in both directions: we neither
overwrite it nor *shadow* it from another PATH directory, so the step first
scans PATH and bails if some other `co-review` is already there. That is also
why the directory policy is shared with `install.sh` (both prefer a writable
standard directory that is already on PATH) — two policies would have put the
plugin's link in `~/.local/bin` in front of a `curl | sh` install in
`/usr/local/bin`. Nothing here can fail the plugin install: no writable
directory, no permission, an existing file — all warn and exit 0, because the
action and the link handler work without a PATH entry.

The cost of the symlink is that `herdr plugin uninstall` leaves it dangling.
Without an uninstall hook the options were a wrapper script that prints a nicer
error, or docs; we took docs, plus two repairs for the case that actually
recurs — the next install removes a stale link, and `doctor` reports a broken
one (and a PATH entry that is not the running binary) instead of silently
saying `co-review` is missing.

The considered alternative was moving all of this into the binary as a
`co-review install` subcommand with an ownership receipt, which would also buy
a real `co-review uninstall`. It is the better home for install *policy*, but
it grows the agent-facing CLI contract (which every subcommand must document
and test) for a step that runs twice in a plugin's life. Reconsider if the
receipt is ever needed for something else.

## 18. The plugin builds in a staging checkout — link to the final path

Decision 17 shipped with a bug: `herdr plugin install` runs the build step in
`<plugins>/.tmp-install-<pid>-<ms>/checkout/` and only *afterwards* moves that
checkout to its final home, so the symlink — resolved from the binary's path at
build time — dangled after every real plugin install (caught by the user right
after updating; the tests had only exercised final-looking paths).

The build step gets no `HERDR_PLUGIN_*` environment and no manifest field names
the destination (verified on 0.8.0 by dumping the build environment), so the
final path cannot be read — but it can be computed: the installed plugin lives
at `<plugins>/github/<id>-<hash>/` where `<hash>` is the first 12 hex digits of
`sha256(<id>)`, verified against both plugins installed on the reporting
machine. `link-on-path.sh` now detects the staging pattern in the binary's
path, reads the id from the checkout's own manifest, and links to the computed
final location; anywhere else (dev checkout, standalone run) it links to the
real path as before.

That hash scheme is a herdr internal and could change. The failure mode is the
one decision 17 already repairs — a dangling link that `doctor` reports and the
next install replaces — and the link self-corrects with every plugin update, so
the guess is cheap to be wrong about. The alternative, a wrapper script that
globs `<plugins>/github/<id>-*/` at run time, would survive a hash change but
turns the PATH entry into a file the ownership checks cannot distinguish from a
user-installed binary; decision 17's reasons for a symlink stand.

## 19. The hand-off is push, not poll: the navigator notifies the agent (2026-08-20)

The original contract (§2, §5) had the agent run `co-review wait` after handing
off — a blocking subprocess that polls `state.json` until every finding is
decided. In practice (reported by the user) that made Herdr show the agent as
*busy* for the whole triage, which can take a while: the human cannot tell
"working on the review" from "idling in `wait`", and agent-level automations
that key off the busy state stall with it.

The navigator already owns a delivery path into the agent pane (`herdr agent
prompt`, §13), so the hand-back is now a push:

- The prompt, protocol, and skill tell the agent to `set-status
  awaiting_review`, say it is done, and **end its turn** — no blocking command.
- When a state change completes the triage (`State::triage_done`: no pending
  findings left, and only while the status is `awaiting_review`, so deciding
  findings early never interrupts an agent that is still reviewing), the
  navigator detects the edge on its state-ingestion path — so it fires no
  matter whether the last verdict came from a TUI keypress or an external
  `co-review verdict` — and injects
  `protocol::TRIAGE_DONE_MSG` into the agent pane — the same path the `c` chat
  and `P` nudge keys use, with the same graceful degradation when no pane is
  wired.
- Races where the human finishes triage *before* the agent hands off are closed
  in the CLI: `set-status awaiting_review` prints whether findings are still
  pending or everything is already decided, and a `verdict` that completes a
  handed-off triage says so — so whichever side acts last, the agent learns the
  outcome without polling.

`co-review wait` stays, as a documented fallback for setups where the navigator
cannot reach the agent (no Herdr, scripted/CI use); the docs now steer agents
away from it inside a normal session.
