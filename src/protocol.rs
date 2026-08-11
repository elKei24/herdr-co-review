//! The agent-facing contract.
//!
//! [`DEFAULT_PROMPT`] is the opening message handed to the agent when a session
//! starts. [`PROTOCOL_MD`] is the full reference the agent can re-read at any
//! time (`co-review protocol`), and is written into the session directory as
//! `CO_REVIEW.md` so it travels with the checkout.
//!
//! Both are plain text on purpose: any agent that can run shell commands can
//! follow them, which is what makes co-review agent-agnostic.

/// Placeholder in the prompt, replaced with the PR reference (e.g. `#123`).
pub const PR_PLACEHOLDER: &str = "{pr}";
/// Placeholder in the prompt, replaced with the absolute path to `CO_REVIEW.md`.
pub const PROTOCOL_PLACEHOLDER: &str = "{protocol}";

/// The default opening prompt. Substitute [`PR_PLACEHOLDER`] and
/// [`PROTOCOL_PLACEHOLDER`] before handing it to the agent.
pub const DEFAULT_PROMPT: &str = r#"You and I are co-reviewing pull request {pr} together, side by side.

You are in the LEFT pane. In the RIGHT pane I have a navigator where I can see
each of your findings with its surrounding code, mark it approved / dismissed /
needs-discussion, and talk to you about it. We drive this review together.

Your job:

1. Do a thorough, high-signal code review of this PR. If you have a `code-review`
   skill, use it. Focus on correctness bugs first, then real
   reuse/simplification/efficiency issues. Skip noise.

2. Record EACH finding by running the `co-review add-finding` command instead of
   posting anything to GitHub yet. One command per finding, for example:

     co-review add-finding \
       --title "Off-by-one in page slicing" \
       --severity high --category correctness \
       --location src/paginate.rs:42-48 \
       --body "The end index is inclusive here but exclusive at the call site, so the last row is dropped when the page is full."

   Repeat `--location path:line` (or `path:start-end`) for every relevant spot.
   The finding shows up live in my navigator the moment you run the command.

3. When you have added all findings, run `co-review set-status awaiting_review`
   and tell me you're done. Then run `co-review wait` — it blocks until I have
   made a decision on every finding. While it blocks, I will be triaging on the
   right and may message you here about specific findings; respond conversationally
   and, if we agree a finding should change, update it with
   `co-review verdict <id> ...` or `co-review add-finding` / edit as needed.

4. Once `co-review wait` returns, post the approved findings to GitHub as inline
   PR review comments (respect my per-finding notes; do NOT post dismissed ones),
   then run `co-review mark-posted <id> --url <comment-url>` for each. Finally run
   `co-review set-status done`.

The full contract, including how to read my decisions back, is in {protocol}
(also available via `co-review protocol`). Read it if anything is unclear.

Start the review now."#;

/// The full protocol reference.
pub const PROTOCOL_MD: &str = r#"# co-review protocol

This session is a **co-review**: an AI agent (you) and a human review a pull
request together. Shared state lives in a session directory and is mutated only
through `co-review` subcommands, which handle locking, ids, and timestamps for
you. `$CO_REVIEW_SESSION` points at that directory; you normally don't need it
because the commands find the session automatically.

## The loop

1. **Review.** Produce high-signal findings. Correctness bugs first.
2. **Record.** One `co-review add-finding` per finding (see below). Findings
   appear live in the human's navigator.
3. **Hand off.** `co-review set-status awaiting_review`, then `co-review wait`.
4. **Collaborate.** While `wait` blocks, the human triages and may talk to you.
   Adjust findings if you both agree.
5. **Post.** When `wait` returns, post approved findings to GitHub, then
   `co-review mark-posted <id> --url <url>` and `co-review set-status done`.

## Recording a finding

    co-review add-finding \
      --title "<short title>" \
      --severity <critical|high|medium|low|nit> \
      --category <free text, e.g. correctness|security|simplification|efficiency> \
      --location <path:line | path:start-end>   (repeatable) \
      --body "<markdown explanation, ideally with the fix>" \
      [--suggestion "<concrete replacement code>"]

- `--location` may be repeated for multiple spots in one finding.
- Add `@base` to a location (e.g. `src/x.rs:10@base`) to point at the base
  version instead of the PR's head; the default is the head.
- Long markdown: use `--body-file <path>` or `--body-file -` to read stdin.
- Bulk: `co-review import <file.json>` ingests a JSON array of findings using the
  same field names as the state schema (`title`, `severity`, `category`, `body`,
  `suggestion`, `locations: [{file, start_line, end_line, side}]`).

`add-finding` prints the new finding id (e.g. `f3`).

## Reading the human's decisions

- `co-review list --json` prints the full state, including each finding's
  `verdict` (`pending`, `approved`, `dismissed`, `needs_discussion`, `edited`)
  and any `user_note` the human attached.
- `co-review wait` blocks until every finding has a verdict other than
  `pending` (add `--timeout <ms>` to bound it). Use it for the hand-off.
- Only post findings whose verdict is `approved` or `edited`. Never post
  `dismissed` ones. For `needs_discussion`, resolve it with the human first.

## Updating and posting

- `co-review verdict <id> <verdict> [--note "..."]` — set a verdict/note (you
  normally only do this if you and the human agree to change one).
- `co-review edit <id> [--title ...] [--severity ...] [--body ...] [--body-file -]
  [--suggestion ...] [--location path:line ...]` — revise an existing finding
  after you and the human discuss it. Only the fields you pass change (use
  `--clear-suggestion` / `--clear-category` / `--clear-locations` to remove one).
  Editing a decided finding resets its verdict to `pending` so the revised text
  gets re-triaged; pass `--keep-verdict` to override.
- `co-review mark-posted <id> --url <comment-url>` — record that you posted it.
- `co-review set-status <reviewing|awaiting_review|posting|done>` — move the
  lifecycle along; the human's navigator shows this status.

Keep it collaborative: the human sees everything you record in real time.
"#;

/// Render the default prompt with the PR reference and protocol path filled in.
pub fn render_prompt(template: &str, pr_display: &str, protocol_path: &str) -> String {
    template
        .replace(PR_PLACEHOLDER, pr_display)
        .replace(PROTOCOL_PLACEHOLDER, protocol_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_substitutes_placeholders() {
        let out = render_prompt(DEFAULT_PROMPT, "#123", "/tmp/s/CO_REVIEW.md");
        assert!(out.contains("#123"));
        assert!(out.contains("/tmp/s/CO_REVIEW.md"));
        assert!(!out.contains(PR_PLACEHOLDER));
        assert!(!out.contains(PROTOCOL_PLACEHOLDER));
    }

    #[test]
    fn protocol_mentions_key_commands() {
        for cmd in ["add-finding", "set-status", "wait", "mark-posted", "import"] {
            assert!(PROTOCOL_MD.contains(cmd), "protocol should mention {cmd}");
        }
    }
}
