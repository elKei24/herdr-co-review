//! Turning a finding's location into the "related code" the human sees.
//!
//! For a head-side location we prefer to show the PR's own diff hunk around the
//! referenced lines (that is what a reviewer cares about); if the location isn't
//! part of the diff we fall back to plain file context. Base-side locations show
//! the base file's context.
//!
//! The unified-diff parser is pure and unit-tested; the snippet builder wires it
//! to `git`.

use anyhow::Result;
use std::path::Path;

use crate::git::{self, Git};
use crate::model::{Location, Side};

/// How a line relates to the diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Added,
    Removed,
}

/// One line of parsed unified diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    /// Line number in the old (base) file, if present on this line.
    pub old: Option<u32>,
    /// Line number in the new (head) file, if present on this line.
    pub new: Option<u32>,
    pub kind: LineKind,
    pub text: String,
}

/// A rendered line in a [`CodeSnippet`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeLine {
    /// The line number to display (head number for head side, base for base).
    pub number: Option<u32>,
    pub kind: LineKind,
    /// True when the line is inside the finding's referenced range.
    pub focus: bool,
    pub text: String,
}

/// The related-code view for one location.
#[derive(Debug, Clone)]
pub struct CodeSnippet {
    pub file: String,
    pub side: Side,
    pub lines: Vec<CodeLine>,
    /// True when the lines came from the PR diff rather than plain file context.
    pub from_diff: bool,
    /// Set when the file/location could not be read (message for the UI).
    pub note: Option<String>,
}

/// Default lines of context shown around a finding.
pub const DEFAULT_CONTEXT: u32 = 6;

/// Parse a unified diff (the output of `git diff`) into a flat list of lines with
/// old/new line numbers attached. File headers and hunk headers are consumed.
pub fn parse_unified_diff(diff: &str) -> Vec<DiffLine> {
    let mut out = Vec::new();
    let mut old_no = 0u32;
    let mut new_no = 0u32;
    let mut in_hunk = false;

    for line in diff.lines() {
        if let Some(header) = line.strip_prefix("@@") {
            match parse_hunk_header(header) {
                Some((o, n)) => {
                    old_no = o;
                    new_no = n;
                    in_hunk = true;
                }
                // Unparseable header: stop consuming body lines rather than
                // numbering them from the previous hunk's counters.
                None => in_hunk = false,
            }
            continue;
        }
        if !in_hunk {
            continue; // diff --git / index / --- / +++ preamble
        }
        // Ignore the "\ No newline at end of file" marker.
        if line.starts_with('\\') {
            continue;
        }
        let (marker, text) = split_marker(line);
        match marker {
            b' ' => {
                out.push(DiffLine {
                    old: Some(old_no),
                    new: Some(new_no),
                    kind: LineKind::Context,
                    text: text.to_string(),
                });
                old_no += 1;
                new_no += 1;
            }
            b'+' => {
                out.push(DiffLine {
                    old: None,
                    new: Some(new_no),
                    kind: LineKind::Added,
                    text: text.to_string(),
                });
                new_no += 1;
            }
            b'-' => {
                out.push(DiffLine {
                    old: Some(old_no),
                    new: None,
                    kind: LineKind::Removed,
                    text: text.to_string(),
                });
                old_no += 1;
            }
            _ => {}
        }
    }
    out
}

fn split_marker(line: &str) -> (u8, &str) {
    match line.as_bytes().first() {
        Some(&b) if matches!(b, b' ' | b'+' | b'-') => (b, &line[1..]),
        _ => (b' ', line),
    }
}

/// Parse the ` -a,b +c,d @@ …` portion of a hunk header, returning `(old_start,
/// new_start)`.
fn parse_hunk_header(header: &str) -> Option<(u32, u32)> {
    // header looks like " -12,7 +12,8 @@ optional context"
    let body = header.trim_start();
    let mut old_start = None;
    let mut new_start = None;
    for tok in body.split_whitespace() {
        if let Some(rest) = tok.strip_prefix('-') {
            old_start = rest.split(',').next().and_then(|n| n.parse::<u32>().ok());
        } else if let Some(rest) = tok.strip_prefix('+') {
            new_start = rest.split(',').next().and_then(|n| n.parse::<u32>().ok());
            break;
        }
    }
    Some((old_start?, new_start?))
}

/// Build the related-code snippet for a location.
pub fn snippet_for_location(
    git: &Git,
    worktree: &Path,
    base_sha: &str,
    head_sha: &str,
    loc: &Location,
    context: u32,
) -> Result<CodeSnippet> {
    match loc.side {
        Side::Head => head_snippet(git, worktree, base_sha, head_sha, loc, context),
        Side::Base => base_snippet(git, base_sha, loc, context),
    }
}

fn head_snippet(
    git: &Git,
    worktree: &Path,
    base_sha: &str,
    head_sha: &str,
    loc: &Location,
    context: u32,
) -> Result<CodeSnippet> {
    let start = loc.start_line;
    let end = loc.end();
    let win_lo = start.saturating_sub(context).max(1);
    let win_hi = end.saturating_add(context);

    // Prefer the diff hunk when the location is part of the change.
    if !base_sha.is_empty() && !head_sha.is_empty() {
        let diff = git.diff_file(base_sha, head_sha, &loc.file).unwrap_or_default();
        let parsed = parse_unified_diff(&diff);
        let windowed = window_diff(&parsed, win_lo, win_hi, start, end);
        if !windowed.is_empty() {
            return Ok(CodeSnippet {
                file: loc.file.clone(),
                side: Side::Head,
                lines: windowed,
                from_diff: true,
                note: None,
            });
        }
    }

    // Fall back to plain head-file context.
    match git::read_worktree_file(worktree, &loc.file)? {
        Some(content) => Ok(CodeSnippet {
            file: loc.file.clone(),
            side: Side::Head,
            lines: plain_window(&content, win_lo, win_hi, start, end),
            from_diff: false,
            note: None,
        }),
        None => Ok(CodeSnippet {
            file: loc.file.clone(),
            side: Side::Head,
            lines: Vec::new(),
            from_diff: false,
            note: Some(format!("{} not found in the head checkout", loc.file)),
        }),
    }
}

fn base_snippet(git: &Git, base_sha: &str, loc: &Location, context: u32) -> Result<CodeSnippet> {
    let start = loc.start_line;
    let end = loc.end();
    let win_lo = start.saturating_sub(context).max(1);
    let win_hi = end.saturating_add(context);
    match git.blob(base_sha, &loc.file)? {
        Some(content) => Ok(CodeSnippet {
            file: loc.file.clone(),
            side: Side::Base,
            lines: plain_window(&content, win_lo, win_hi, start, end),
            from_diff: false,
            note: None,
        }),
        None => Ok(CodeSnippet {
            file: loc.file.clone(),
            side: Side::Base,
            lines: Vec::new(),
            from_diff: false,
            note: Some(format!("{} not found in the base revision", loc.file)),
        }),
    }
}

/// Select diff lines whose new-line number falls in `[lo, hi]`, keeping removed
/// lines that sit next to the kept region so the change reads naturally.
fn window_diff(parsed: &[DiffLine], lo: u32, hi: u32, focus_lo: u32, focus_hi: u32) -> Vec<CodeLine> {
    let mut out = Vec::new();
    let mut last_new: u32 = 0;
    for dl in parsed {
        match dl.kind {
            LineKind::Context | LineKind::Added => {
                if let Some(n) = dl.new {
                    last_new = n;
                    if n >= lo && n <= hi {
                        out.push(CodeLine {
                            number: Some(n),
                            kind: dl.kind,
                            focus: n >= focus_lo && n <= focus_hi,
                            text: dl.text.clone(),
                        });
                    }
                }
            }
            LineKind::Removed => {
                // Show a removed line when it is adjacent to the visible window.
                // In a head-side view a removed line has no head line number, so
                // we leave `number` unset rather than show its (misleading) base
                // number next to head numbers.
                if last_new + 1 >= lo && last_new <= hi {
                    out.push(CodeLine {
                        number: None,
                        kind: LineKind::Removed,
                        focus: false,
                        text: dl.text.clone(),
                    });
                }
            }
        }
    }
    out
}

fn plain_window(content: &str, lo: u32, hi: u32, focus_lo: u32, focus_hi: u32) -> Vec<CodeLine> {
    content
        .lines()
        .enumerate()
        .map(|(i, l)| (i as u32 + 1, l))
        .filter(|(n, _)| *n >= lo && *n <= hi)
        .map(|(n, text)| CodeLine {
            number: Some(n),
            kind: LineKind::Context,
            focus: n >= focus_lo && n <= focus_hi,
            text: text.to_string(),
        })
        .collect()
}

/// Render a snippet as plain text (used by `co-review show`).
pub fn render_plain(snippet: &CodeSnippet) -> String {
    let mut out = String::new();
    let tag = match snippet.side {
        Side::Head => "head",
        Side::Base => "base",
    };
    let src = if snippet.from_diff { "diff" } else { "context" };
    out.push_str(&format!("── {} ({tag}, {src}) ──\n", snippet.file));
    if let Some(note) = &snippet.note {
        out.push_str(&format!("   ({note})\n"));
        return out;
    }
    for line in &snippet.lines {
        let sign = match line.kind {
            LineKind::Added => '+',
            LineKind::Removed => '-',
            LineKind::Context => ' ',
        };
        let num = line
            .number
            .map(|n| format!("{n:>5}"))
            .unwrap_or_else(|| "     ".to_string());
        let marker = if line.focus { '▶' } else { ' ' };
        out.push_str(&format!("{marker}{num} {sign} {}\n", line.text));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
diff --git a/f.rs b/f.rs
index 111..222 100644
--- a/f.rs
+++ b/f.rs
@@ -1,4 +1,5 @@
 fn main() {
-    let x = 1;
+    let x = 2;
+    let y = 3;
     println!(\"{x}\");
 }
";

    #[test]
    fn parses_hunk_header() {
        assert_eq!(parse_hunk_header(" -1,4 +1,5 @@ fn main"), Some((1, 1)));
        assert_eq!(parse_hunk_header(" -10 +12,3 @@"), Some((10, 12)));
    }

    #[test]
    fn parses_lines_with_numbers() {
        let lines = parse_unified_diff(SAMPLE);
        // context, removed, added, added, context, context
        assert_eq!(lines[0].kind, LineKind::Context);
        assert_eq!(lines[0].new, Some(1));
        assert_eq!(lines[1].kind, LineKind::Removed);
        assert_eq!(lines[1].old, Some(2));
        assert_eq!(lines[1].new, None);
        assert_eq!(lines[2].kind, LineKind::Added);
        assert_eq!(lines[2].new, Some(2));
        assert_eq!(lines[3].kind, LineKind::Added);
        assert_eq!(lines[3].new, Some(3));
        // the trailing context line numbers continue
        let last_ctx = lines.iter().rev().find(|l| l.kind == LineKind::Context).unwrap();
        assert_eq!(last_ctx.new, Some(5));
    }

    #[test]
    fn unparseable_hunk_header_stops_numbering() {
        // A malformed second hunk header must not keep numbering body lines from
        // the previous hunk's counters.
        let diff = "@@ -1,1 +1,1 @@\n a\n@@ garbage @@\n b\n";
        let lines = parse_unified_diff(diff);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "a");
        assert_eq!(lines[0].new, Some(1));
    }

    #[test]
    fn window_includes_focus_and_removed() {
        let parsed = parse_unified_diff(SAMPLE);
        // focus on new line 2..3 (the added lines), context 6
        let win = window_diff(&parsed, 1, 9, 2, 3);
        // The removed `let x = 1;` should appear adjacent to the window.
        assert!(win.iter().any(|l| l.kind == LineKind::Removed && l.text.contains("let x = 1")));
        // Added line 2 is focused.
        let added2 = win.iter().find(|l| l.number == Some(2)).unwrap();
        assert_eq!(added2.kind, LineKind::Added);
        assert!(added2.focus);
        // Context line 5 present, not focused.
        let ctx5 = win.iter().find(|l| l.number == Some(5)).unwrap();
        assert!(!ctx5.focus);
    }

    #[test]
    fn plain_window_slices_and_focuses() {
        let content = "l1\nl2\nl3\nl4\nl5\n";
        let win = plain_window(content, 2, 4, 3, 3);
        assert_eq!(win.len(), 3);
        assert_eq!(win[0].number, Some(2));
        assert_eq!(win[1].number, Some(3));
        assert!(win[1].focus);
        assert!(!win[0].focus);
    }

    #[test]
    fn render_plain_shows_markers() {
        let snippet = CodeSnippet {
            file: "f.rs".into(),
            side: Side::Head,
            from_diff: true,
            note: None,
            lines: vec![
                CodeLine { number: Some(1), kind: LineKind::Context, focus: false, text: "a".into() },
                CodeLine { number: Some(2), kind: LineKind::Added, focus: true, text: "b".into() },
            ],
        };
        let text = render_plain(&snippet);
        assert!(text.contains("f.rs (head, diff)"));
        assert!(text.contains("▶"));
        assert!(text.contains("+ b"));
    }
}
