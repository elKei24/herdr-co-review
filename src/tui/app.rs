//! State and behavior of the navigator TUI.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::diffview::{self, CodeSnippet, LineKind};
use crate::git::Git;
use crate::herdr::Herdr;
use crate::model::{ChatEntry, State, Verdict};
use crate::store::Store;
use crate::tui::syntax::Highlighter;

/// What the bottom input line is currently collecting, if anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// Editing the human note on the selected finding.
    Note,
    /// Composing a message to send to the agent about the selected finding.
    Chat,
}

/// A rendered related-code block for one finding location.
pub struct CodeBlock {
    pub header: String,
    pub lines: Vec<Line<'static>>,
}

pub struct App {
    store: Store,
    pub state: State,
    last_rev: u64,

    git: Option<Git>,
    worktree: PathBuf,
    herdr: Herdr,
    highlighter: Highlighter,

    pub selected: usize,
    selected_id: Option<String>,
    pub detail_scroll: u16,

    pub input: Option<Input>,
    pub input_buffer: String,
    status_msg: Option<(String, Instant)>,
    pub show_help: bool,
    pub should_quit: bool,

    cached_for: Option<String>,
    pub code_blocks: Vec<CodeBlock>,
}

impl App {
    pub fn new(store: Store) -> Result<Self> {
        let state = store.read()?;
        let worktree = PathBuf::from(&state.session.worktree);
        let git = Git::discover(&worktree).ok();
        let last_rev = state.rev;
        let mut app = App {
            store,
            state,
            last_rev,
            git,
            worktree,
            herdr: Herdr::new(false),
            highlighter: Highlighter::new(),
            selected: 0,
            selected_id: None,
            detail_scroll: 0,
            input: None,
            input_buffer: String::new(),
            status_msg: None,
            show_help: false,
            should_quit: false,
            cached_for: None,
            code_blocks: Vec::new(),
        };
        app.sync_selection();
        app.refresh_code();
        Ok(app)
    }

    /// The currently selected finding id, if any.
    pub fn selected_id(&self) -> Option<String> {
        self.state.findings.get(self.selected).map(|f| f.id.clone())
    }

    pub fn status_line(&self) -> Option<&str> {
        self.status_msg.as_ref().map(|(m, _)| m.as_str())
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status_msg = Some((msg.into(), Instant::now()));
    }

    /// Expire a transient status message after a few seconds.
    pub fn tick_status(&mut self) {
        if let Some((_, at)) = &self.status_msg {
            if at.elapsed() > Duration::from_secs(4) {
                self.status_msg = None;
            }
        }
    }

    /// Reload state from disk if its revision advanced, preserving the selected
    /// finding by id. Reads on every tick (cheap for a small JSON) and compares
    /// the monotonic `rev`, so no change is ever missed to mtime granularity.
    pub fn poll_reload(&mut self) {
        if let Ok(state) = self.store.read_lossy() {
            if state.rev != self.last_rev {
                self.last_rev = state.rev;
                self.state = state;
                self.sync_selection();
                self.cached_for = None; // rebuild code against possibly-new shas/locations
                self.refresh_code();
            }
        }
    }

    /// Keep `selected`/`selected_id` consistent after list changes.
    fn sync_selection(&mut self) {
        if self.state.findings.is_empty() {
            self.selected = 0;
            self.selected_id = None;
            return;
        }
        // Try to keep the same finding selected across reloads.
        if let Some(id) = &self.selected_id {
            if let Some(pos) = self.state.findings.iter().position(|f| &f.id == id) {
                self.selected = pos;
                return;
            }
        }
        self.selected = self.selected.min(self.state.findings.len() - 1);
        self.selected_id = self.selected_id();
    }

    pub fn select_next(&mut self) {
        if self.state.findings.is_empty() {
            return;
        }
        self.selected = (self.selected + 1).min(self.state.findings.len() - 1);
        self.after_move();
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
        self.after_move();
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
        self.after_move();
    }

    pub fn select_last(&mut self) {
        if !self.state.findings.is_empty() {
            self.selected = self.state.findings.len() - 1;
        }
        self.after_move();
    }

    fn after_move(&mut self) {
        self.detail_scroll = 0;
        self.selected_id = self.selected_id();
        self.refresh_code();
    }

    pub fn scroll_detail_down(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_add(3);
    }

    pub fn scroll_detail_up(&mut self) {
        self.detail_scroll = self.detail_scroll.saturating_sub(3);
    }

    /// Set the selected finding's verdict.
    pub fn set_verdict(&mut self, verdict: Verdict) {
        let Some(id) = self.selected_id() else {
            self.set_status("no finding selected");
            return;
        };
        let res = self.store.update(|s| {
            if let Some(f) = s.finding_mut(&id) {
                f.verdict = verdict;
                f.touch();
            }
            Ok(())
        });
        match res {
            Ok(()) => {
                self.set_status(format!("{id} → {}", verdict.label()));
                self.reload_now();
            }
            Err(e) => self.set_status(format!("error: {e}")),
        }
    }

    /// Begin collecting input (note or chat) for the selected finding.
    pub fn begin_input(&mut self, kind: Input) {
        if self.selected_id().is_none() {
            self.set_status("no finding selected");
            return;
        }
        // Pre-fill the note editor with the existing note.
        self.input_buffer = if kind == Input::Note {
            self.state
                .findings
                .get(self.selected)
                .and_then(|f| f.user_note.clone())
                .unwrap_or_default()
        } else {
            String::new()
        };
        self.input = Some(kind);
    }

    pub fn cancel_input(&mut self) {
        self.input = None;
        self.input_buffer.clear();
    }

    pub fn push_input_char(&mut self, c: char) {
        self.input_buffer.push(c);
    }

    pub fn pop_input_char(&mut self) {
        self.input_buffer.pop();
    }

    /// Commit the pending input.
    pub fn submit_input(&mut self) {
        let Some(kind) = self.input.clone() else {
            return;
        };
        let text = self.input_buffer.trim().to_string();
        let Some(id) = self.selected_id() else {
            self.cancel_input();
            return;
        };
        match kind {
            Input::Note => {
                let note = if text.is_empty() { None } else { Some(text) };
                let _ = self.store.update(|s| {
                    if let Some(f) = s.finding_mut(&id) {
                        f.user_note = note.clone();
                        f.touch();
                    }
                    Ok(())
                });
                self.set_status(format!("note updated on {id}"));
            }
            Input::Chat => {
                if !text.is_empty() {
                    self.send_chat(&id, &text);
                }
            }
        }
        self.cancel_input();
        self.reload_now();
    }

    /// Record a chat message and inject it into the agent pane if possible.
    fn send_chat(&mut self, finding_id: &str, text: &str) {
        let entry = ChatEntry {
            at_ms: crate::util::now_ms(),
            finding_id: Some(finding_id.to_string()),
            text: text.to_string(),
        };
        let _ = self.store.update(|s| {
            s.chat.push(entry.clone());
            Ok(())
        });
        let message = format!("[co-review {finding_id}] {text}");
        match &self.state.session.agent_pane_id {
            Some(pane) if self.herdr.available() => match self.herdr.pane_submit_line(pane, &message) {
                Ok(()) => self.set_status(format!("sent to agent about {finding_id}")),
                Err(e) => self.set_status(format!("couldn't reach agent pane: {e}")),
            },
            _ => self.set_status(format!("recorded (no agent pane wired); re: {finding_id}")),
        }
    }

    /// Nudge the agent to post the approved findings.
    pub fn nudge_post(&mut self) {
        let pending = self.state.pending_count();
        let msg = if pending == 0 {
            "All findings are decided — please post the approved ones to GitHub and mark them posted."
        } else {
            "Please post the findings I've already approved; I'll keep triaging the rest."
        };
        match &self.state.session.agent_pane_id {
            Some(pane) if self.herdr.available() => {
                match self.herdr.pane_submit_line(pane, msg) {
                    Ok(()) => self.set_status("asked the agent to post approved findings"),
                    Err(e) => self.set_status(format!("couldn't reach agent pane: {e}")),
                }
            }
            _ => self.set_status("no agent pane wired; run `co-review post` yourself"),
        }
    }

    /// Force an immediate reload from disk (bound to `r`).
    pub fn force_reload(&mut self) {
        self.reload_now();
        self.set_status("refreshed");
    }

    fn reload_now(&mut self) {
        if let Ok(state) = self.store.read_lossy() {
            self.last_rev = state.rev;
            self.state = state;
            self.sync_selection();
            self.cached_for = None;
            self.refresh_code();
        }
    }

    /// Rebuild the cached, highlighted related-code blocks for the selection.
    fn refresh_code(&mut self) {
        let Some(finding) = self.state.findings.get(self.selected) else {
            self.code_blocks.clear();
            self.cached_for = None;
            return;
        };
        if self.cached_for.as_deref() == Some(finding.id.as_str()) {
            return;
        }
        self.cached_for = Some(finding.id.clone());
        self.code_blocks.clear();

        let Some(git) = &self.git else {
            for loc in &finding.locations {
                self.code_blocks.push(CodeBlock {
                    header: loc.label(),
                    lines: vec![Line::from("(no git checkout available to show code)")],
                });
            }
            return;
        };

        let mut blocks = Vec::new();
        for loc in &finding.locations {
            let block = match diffview::snippet_for_location(
                git,
                &self.worktree,
                &self.state.pr.base_sha,
                &self.state.pr.head_sha,
                loc,
                diffview::DEFAULT_CONTEXT,
            ) {
                Ok(snippet) => CodeBlock {
                    header: snippet_header(&snippet),
                    lines: render_snippet(&self.highlighter, &snippet),
                },
                Err(e) => CodeBlock {
                    header: loc.label(),
                    lines: vec![Line::from(format!("(could not render: {e})"))],
                },
            };
            blocks.push(block);
        }
        self.code_blocks = blocks;
    }
}

fn snippet_header(snippet: &CodeSnippet) -> String {
    let side = match snippet.side {
        crate::model::Side::Head => "head",
        crate::model::Side::Base => "base",
    };
    let src = if snippet.from_diff { "diff" } else { "context" };
    format!("{} · {side} · {src}", snippet.file)
}

/// Colors for the code view.
const ADDED_BG: Color = Color::Rgb(22, 46, 22);
const REMOVED_BG: Color = Color::Rgb(58, 24, 24);
const FOCUS_BG: Color = Color::Rgb(52, 46, 20);
const GUTTER_FG: Color = Color::Rgb(120, 120, 130);

/// Render a snippet into styled ratatui lines: syntect foreground, diff-kind
/// background, and a focus marker in the gutter.
fn render_snippet(hl: &Highlighter, snippet: &CodeSnippet) -> Vec<Line<'static>> {
    if let Some(note) = &snippet.note {
        return vec![Line::from(Span::styled(
            format!("  ({note})"),
            Style::default().fg(Color::DarkGray),
        ))];
    }
    let syntax = hl.syntax_for(&snippet.file);
    let mut lighter = hl.line_highlighter(syntax);
    let mut out = Vec::with_capacity(snippet.lines.len());

    for line in &snippet.lines {
        let bg = match (line.kind, line.focus) {
            (LineKind::Added, _) => Some(ADDED_BG),
            (LineKind::Removed, _) => Some(REMOVED_BG),
            (LineKind::Context, true) => Some(FOCUS_BG),
            (LineKind::Context, false) => None,
        };
        let sign = match line.kind {
            LineKind::Added => '+',
            LineKind::Removed => '-',
            LineKind::Context => ' ',
        };
        let marker = if line.focus { '▶' } else { ' ' };
        let num = line
            .number
            .map(|n| format!("{n:>4}"))
            .unwrap_or_else(|| "    ".to_string());
        let gutter = format!("{marker}{num} {sign} ");

        let mut spans: Vec<Span<'static>> = Vec::new();
        let gutter_style = {
            let mut s = Style::default().fg(if line.focus {
                Color::Yellow
            } else {
                GUTTER_FG
            });
            if let Some(bg) = bg {
                s = s.bg(bg);
            }
            if line.focus {
                s = s.add_modifier(Modifier::BOLD);
            }
            s
        };
        spans.push(Span::styled(gutter, gutter_style));

        for (fg, text) in lighter.highlight(&line.text) {
            let mut style = Style::default();
            if let Some(c) = fg {
                style = style.fg(c);
            }
            if let Some(bg) = bg {
                style = style.bg(bg);
            }
            spans.push(Span::styled(text, style));
        }
        out.push(Line::from(spans));
    }
    out
}
