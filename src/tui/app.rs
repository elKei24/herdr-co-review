//! State and behavior of the navigator TUI.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::diffview::{self, CodeSnippet, LineKind};
use crate::git::Git;
use crate::herdr::Herdr;
use crate::model::{ChatEntry, Location, State, Verdict};
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
    herdr: Herdr,
    highlighter: Highlighter,

    pub selected: usize,
    pub detail_scroll: u16,

    pub input: Option<Input>,
    pub input_buffer: String,
    status_msg: Option<(String, Instant)>,
    pub show_help: bool,
    pub should_quit: bool,
    /// Set whenever something visible changed; the event loop only repaints when
    /// this is set, so an idle session doesn't redraw several times a second.
    pub dirty: bool,

    /// Highlighted related-code blocks, memoized per finding id so navigating the
    /// list re-runs the (subprocess-backed) git diff at most once per finding.
    code_cache: HashMap<String, Vec<CodeBlock>>,
}

impl App {
    pub fn new(store: Store) -> Result<Self> {
        let state = store.read()?;
        let git = Git::discover(Path::new(&state.session.worktree)).ok();
        let last_rev = state.rev;
        let mut app = App {
            store,
            state,
            last_rev,
            git,
            herdr: Herdr::new(false),
            highlighter: Highlighter::new(),
            selected: 0,
            detail_scroll: 0,
            input: None,
            input_buffer: String::new(),
            status_msg: None,
            show_help: false,
            should_quit: false,
            dirty: true,
            code_cache: HashMap::new(),
        };
        app.reconcile_selection(None);
        app.ensure_code();
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
        self.dirty = true;
    }

    /// Expire a transient status message after a few seconds.
    pub fn tick_status(&mut self) {
        if let Some((_, at)) = &self.status_msg {
            if at.elapsed() > Duration::from_secs(4) {
                self.status_msg = None;
                self.dirty = true;
            }
        }
    }

    /// Reload from disk if the revision advanced. Reads a small JSON each tick
    /// and compares the monotonic `rev`, so no change is missed to mtime
    /// granularity.
    pub fn poll_reload(&mut self) {
        if let Ok(state) = self.store.read_lossy() {
            if state.rev != self.last_rev {
                self.last_rev = state.rev;
                self.apply_state(state);
            }
        }
    }

    /// Adopt a freshly-read state, keeping the same finding selected by id and
    /// invalidating the code cache (shas/locations may have changed).
    fn apply_state(&mut self, new_state: State) {
        let prev = self.selected_id();
        self.state = new_state;
        self.reconcile_selection(prev.as_deref());
        self.code_cache.clear();
        self.ensure_code();
        self.dirty = true;
    }

    /// Clamp/restore the selection index after the findings list changed.
    fn reconcile_selection(&mut self, prev_id: Option<&str>) {
        if self.state.findings.is_empty() {
            self.selected = 0;
            return;
        }
        if let Some(id) = prev_id {
            if let Some(pos) = self.state.findings.iter().position(|f| f.id == id) {
                self.selected = pos;
                return;
            }
        }
        self.selected = self.selected.min(self.state.findings.len() - 1);
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
        self.ensure_code();
        self.dirty = true;
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
        match self.deliver_to_agent(&message) {
            Ok(true) => self.set_status(format!("sent to agent about {finding_id}")),
            Ok(false) => {
                self.set_status(format!("recorded (no agent pane wired); re: {finding_id}"))
            }
            Err(e) => self.set_status(format!("couldn't reach agent pane: {e}")),
        }
    }

    /// Nudge the agent to post the approved findings.
    pub fn nudge_post(&mut self) {
        let msg = if self.state.pending_count() == 0 {
            "All findings are decided — please post the approved ones to GitHub and mark them posted."
        } else {
            "Please post the findings I've already approved; I'll keep triaging the rest."
        };
        match self.deliver_to_agent(msg) {
            Ok(true) => self.set_status("asked the agent to post approved findings"),
            Ok(false) => self.set_status("no agent pane wired; run `co-review post` yourself"),
            Err(e) => self.set_status(format!("couldn't reach agent pane: {e}")),
        }
    }

    /// Submit a line to the agent's pane. `Ok(true)` delivered, `Ok(false)` when
    /// there is no agent pane wired (or no Herdr), `Err` on a delivery failure.
    fn deliver_to_agent(&self, text: &str) -> Result<bool> {
        match &self.state.session.agent_pane_id {
            Some(pane) if self.herdr.available() => {
                self.herdr.pane_submit_line(pane, text)?;
                Ok(true)
            }
            _ => Ok(false),
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
            self.apply_state(state);
        }
    }

    /// The related-code blocks for the current selection (memoized).
    pub fn code_blocks(&self) -> &[CodeBlock] {
        self.state
            .findings
            .get(self.selected)
            .and_then(|f| self.code_cache.get(&f.id))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Ensure the code blocks for the current selection are built and cached.
    fn ensure_code(&mut self) {
        let Some((id, locations)) = self
            .state
            .findings
            .get(self.selected)
            .map(|f| (f.id.clone(), f.locations.clone()))
        else {
            return;
        };
        if self.code_cache.contains_key(&id) {
            return;
        }
        let blocks = self.build_blocks(&locations);
        self.code_cache.insert(id, blocks);
    }

    fn build_blocks(&self, locations: &[Location]) -> Vec<CodeBlock> {
        let Some(git) = &self.git else {
            return locations
                .iter()
                .map(|loc| CodeBlock {
                    header: loc.label(),
                    lines: vec![Line::from("(no git checkout available to show code)")],
                })
                .collect();
        };
        locations
            .iter()
            .map(|loc| {
                match diffview::snippet_for(git, &self.state, loc, diffview::DEFAULT_CONTEXT) {
                    Ok(snippet) => CodeBlock {
                        header: snippet_header(&snippet),
                        lines: render_snippet(&self.highlighter, &snippet),
                    },
                    Err(e) => CodeBlock {
                        header: loc.label(),
                        lines: vec![Line::from(format!("(could not render: {e})"))],
                    },
                }
            })
            .collect()
    }
}

fn snippet_header(snippet: &CodeSnippet) -> String {
    let src = if snippet.from_diff { "diff" } else { "context" };
    format!("{} · {} · {src}", snippet.file, snippet.side.label())
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
        let sign = line.kind.sign();
        let marker = if line.focus { '▶' } else { ' ' };
        let num = line
            .number
            .map(|n| format!("{n:>4}"))
            .unwrap_or_else(|| "    ".to_string());
        let gutter = format!("{marker}{num} {sign} ");

        let mut spans: Vec<Span<'static>> = Vec::new();
        let gutter_style = {
            let mut s = Style::default().fg(if line.focus { Color::Yellow } else { GUTTER_FG });
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
