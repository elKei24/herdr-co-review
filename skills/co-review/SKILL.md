---
name: co-review
description: >-
  Drive an interactive, split-screen PR co-review with a human using the
  `co-review` CLI. Use this whenever you are reviewing a pull request inside a
  co-review session (the environment variable CO_REVIEW_SESSION is set, or the
  opening prompt references co-review): record each finding with
  `co-review add-finding` instead of posting, hand off with `co-review wait`,
  then post the human-approved findings and mark them posted.
---

# co-review (agent side)

You are the agent half of a **co-review**: you and a human review a pull request
together. You produce findings; the human triages them in a live navigator that
shows each finding with its surrounding code, and can talk to you about any of
them. Shared state is managed by the `co-review` CLI, which handles locking, ids,
and timestamps — you never edit its files directly.

You are in a co-review session when `CO_REVIEW_SESSION` is set in your
environment (the `co-review start` command sets it in your pane). All `co-review`
subcommands then find the session automatically; no `--session` needed.

## The loop

1. **Review.** Do a thorough, high-signal review of the PR. Prefer your
   `code-review` skill if you have one. Correctness bugs first, then genuine
   reuse / simplification / efficiency issues. Skip noise — every finding costs
   the human attention.

2. **Record each finding** with `co-review add-finding` (one command per
   finding). It appears in the human's navigator immediately:

   ```
   co-review add-finding \
     --title "Off-by-one in page slicing" \
     --severity high --category correctness \
     --location src/paginate.rs:42-48 \
     --body "The end index is inclusive here but the caller treats it as exclusive, so the last row is dropped on full pages."
   ```

   - Repeat `--location path:line` (or `path:start-end`) for every relevant spot.
   - Append `@base` to a location to point at the base version (default: head).
   - Long markdown: `--body-file <file>` or `--body-file -` (stdin).
   - A concrete fix: `--suggestion "<replacement code>"` (posted as a GitHub
     suggestion block).
   - Bulk alternative: write a JSON array and run `co-review import findings.json`.

3. **Hand off.** When every finding is recorded, run:

   ```
   co-review set-status awaiting_review
   co-review wait          # blocks until the human has decided every finding
   ```

   Tell the human you're done and waiting.

4. **Collaborate while you wait.** The human may message you about a specific
   finding (their messages arrive prefixed like `[co-review f3] …`). Discuss it.
   If you both agree a finding should change, update it: `co-review verdict f3
   dismissed`, or revise its text with `co-review edit f3 --body "…"` (only the
   fields you pass change), or add a new finding.

5. **Post.** When `co-review wait` returns, read the decisions and post:

   ```
   co-review list --json    # inspect each finding's verdict + user_note
   ```

   - Post findings whose `verdict` is `approved` or `edited` as inline PR review
     comments. Respect any `user_note`. For `edited`, incorporate the human's
     note into what you post.
   - **Never** post `dismissed` findings. Resolve `needs_discussion` with the
     human first.
   - After posting each one: `co-review mark-posted f3 --url <comment-url>`.
   - Finally: `co-review set-status done`.

## Reference

- `co-review protocol` — prints the full contract at any time.
- `co-review list` / `co-review show <id>` — inspect findings (with related code).
- `co-review status` — one-line summary.

Keep it collaborative: everything you record is visible to the human the moment
you record it, and everything they decide is visible to you the moment they
decide it.
