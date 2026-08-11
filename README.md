# co-review

**Interactive, split-screen PR co-review between you and your AI agent, inside
[Herdr](https://herdr.dev).**

Reviewing a PR with an agent usually means reading a wall of findings in the
agent's pane, with none of the surrounding code, and then either trusting them or
tediously cross-checking each one on GitHub. `co-review` turns that into a
side-by-side collaboration:

- One command checks the PR out into an isolated worktree and starts your agent
  (Claude by default) on it, with a configurable prompt.
- Herdr lays out a **split-screen**: the agent reviews on the left; a **navigator
  TUI** runs on the right.
- In the navigator you select a finding and immediately see **its related code**
  — the PR's own diff hunk around the referenced lines, syntax-highlighted, with
  the exact lines marked.
- You triage each finding (approve / dismiss / discuss / edit / note) and can
  **message the agent about the selected finding** straight into its pane.
- The agent keeps driving everything that doesn't need you — most importantly,
  **posting the approved findings to GitHub** when you're done.

It's the two of you reviewing together, instead of the agent throwing findings
over the wall.

```
┌ co-review ─────────────────────────────────────────────────────────────┐
│elKei24/herdr-co-review #7 — Fix pagination end index                    │
│ reviewing   1 findings  1 pending  0 approved  0 dismissed  0 posted    │
└─────────────────────────────────────────────────────────────────────────┘
┌ findings (j/k) ─────────────────────────────────────────────────────────┐
│▌● [pending ] Off-by-one in end index  paginate.rs:2                      │
└─────────────────────────────────────────────────────────────────────────┘
┌ detail & related code (J/K scroll) ─────────────────────────────────────┐
│● HIGH  [pending ]  f1                                                    │
│Off-by-one in end index                                                  │
│category: correctness                                                    │
│                                                                         │
│`end` was `size`, dropping the last element for full pages.              │
│                                                                         │
│── paginate.rs · head · diff ──                                          │
│    1   fn page(items: &[u32], size: usize) -> &[u32] {                  │
│      -     let end = size;                                              │
│▶   2 +     let end = (size + 1).min(items.len());                      │
│    3       &items[..end]                                                │
│    4   }                                                                 │
└─────────────────────────────────────────────────────────────────────────┘
 j/k move  J/K scroll  a approve  d dismiss  x discuss  n note  c chat  …
```

## How it works

`co-review` is a single Rust binary with a few subcommands. `start` is the
orchestrator; the agent and the navigator share one lock-guarded state file and
talk to each other through the `co-review` CLI:

```
        co-review start 123
                │
   ┌────────────┴─────────────┐
   │ git worktree of the PR   │
   │ + shared session state   │
   └────────────┬─────────────┘
                │  herdr workspace create / pane split
   ┌────────────┴───────────────────────────────┐
   │ Herdr workspace                             │
   │ ┌───────────────┐   ┌────────────────────┐  │
   │ │ agent (left)  │   │ navigator (right)  │  │
   │ │ reviews, runs │◀─▶│ co-review view     │  │
   │ │ `co-review    │   │ select finding →   │  │
   │ │  add-finding` │   │ see code, triage,  │  │
   │ │ then `wait`   │   │ chat, decide       │  │
   │ └───────────────┘   └────────────────────┘  │
   └─────────────────────────────────────────────┘
```

The contract between the two halves is a small set of CLI verbs (see
[`co-review protocol`](#the-agent-contract)), which is what makes it
**agent-agnostic**: anything that can run shell commands can drive it.

## Install

Requires a recent Rust toolchain and `git`. Herdr is needed for the split-screen
(everything else works without it).

```sh
# from a checkout
cargo install --path .

# or straight from git
cargo install --git https://github.com/elKei24/herdr-co-review
```

As a Herdr plugin (adds a "Co-review this PR" action and a GitHub-PR link
handler), point Herdr's plugin installer at this repo; the manifest is
[`herdr-plugin.toml`](./herdr-plugin.toml).

## Usage

From inside your repository checkout:

```sh
co-review start 123                     # PR #123 in the current repo's origin
co-review start '#123'
co-review start owner/repo#123
co-review start https://github.com/owner/repo/pull/123

co-review start 123 --agent codex       # use a different agent
co-review start 123 --prompt "Only look for security issues."
co-review start 123 --dry-run           # offline preview of everything it will do
```

`start` opens the Herdr split-screen. In the navigator:

| Key | Action |
| --- | --- |
| `j`/`k`, `↓`/`↑` | move between findings |
| `g`/`G` | first / last |
| `J`/`K`, `PgDn`/`PgUp` | scroll the detail & code |
| `a` / `d` / `x` / `u` | approve / dismiss / needs-discussion / reset |
| `e` | approve as edited |
| `n` | add / edit your note (shown to the agent) |
| `c` | message the agent about this finding (into its pane) |
| `P` | ask the agent to post the approved findings |
| `r` | force refresh · `?` help · `q` quit |

Findings appear live as the agent records them; your verdicts and notes are
visible to the agent immediately.

## Managing sessions

Each `start` creates an isolated worktree and a session directory that persist
until you remove them:

```sh
co-review sessions           # list all sessions with their status and counts
co-review view 123           # reopen the navigator for PR #123
co-review end 123            # remove PR #123's worktree and session
co-review end 123 --keep-worktree   # drop the session state but keep the checkout
co-review start 123 --resume        # reopen / refresh an existing session
```

## Configuration

Optional, at `~/.config/co-review/config.toml`. Everything has a default.

```toml
# Which agent to use when --agent is not given.
default_agent = "claude"

# The prompt handed to the agent. `{pr}` and `{protocol}` are substituted.
# Omit to use the built-in prompt (which runs your code-review skill and routes
# findings through co-review). See `co-review prompt`.
# prompt = "..."

# Define or override agents. `{prompt}` in a command is replaced with the review
# prompt; otherwise the prompt is appended as the final argument.
[agents.claude]
kind = "claude"
command = ["claude"]

[agents.my-tool]
command = ["my-tool", "--task", "{prompt}"]
```

Built-in agents: `claude`, `codex`, `gemini`, `cursor`, `amp`, `opencode`.

## The agent contract

The agent records findings and reads your decisions through these commands
(`co-review protocol` prints the full reference; the [Claude skill](./skills/co-review/SKILL.md)
teaches Claude to use them):

```sh
co-review add-finding --title "…" --severity high --category correctness \
  --location src/foo.rs:42-48 --body "…" [--suggestion "…"]
co-review import findings.json        # bulk add from a JSON array
co-review set-status awaiting_review
co-review wait                        # block until you've decided every finding
co-review list --json                 # read verdicts + your notes
co-review edit f3 --body "…"          # revise a finding after you discuss it
co-review mark-posted f3 --url <url>  # after posting to GitHub
```

You can also post directly (a fallback for agent-agnostic use), given a GitHub
token in `$GH_TOKEN`/`$GITHUB_TOKEN` or `gh auth login`:

```sh
co-review post            # posts approved/edited findings as inline PR comments
co-review post --dry-run  # show what would be posted
```

Run `co-review doctor` to check your environment (git, herdr, agent, token).

## Design

See [`docs/DECISIONS.md`](./docs/DECISIONS.md) for the full rationale — why Rust,
why real Herdr panes, why a single lock-guarded state file, why a CLI contract
instead of hand-written JSON, and more.

## License

MIT — see [LICENSE](./LICENSE).
