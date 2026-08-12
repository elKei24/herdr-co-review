//! State and behavior of the navigator TUI.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::diffview::{self, CodeSnippet, LineKind};
use crate::git::Git;
use crate::herdr::{AgentState, Herdr};
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

/// Which pane the wheel scrolls. Clicking a pane makes it the focused one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Findings,
    Detail,
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
    pub focus: Pane,

    /// Geometry of the two scrollable panes and the list's scroll offset, all
    /// written back by the renderer so mouse clicks can be mapped to a finding.
    pub list_area: Rect,
    pub detail_area: Rect,
    pub list_offset: usize,
    detail_max_scroll: u16,

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

    /// Best-effort agent state from Herdr. A background thread writes here so
    /// the (potentially slow) `herdr agent list` subprocess never blocks the
    /// render loop; `agent_state_shown` is the UI-thread copy.
    agent_state: Arc<Mutex<Option<AgentState>>>,
    agent_state_shown: Option<AgentState>,
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
            focus: Pane::Findings,
            list_area: Rect::default(),
            detail_area: Rect::default(),
            list_offset: 0,
            detail_max_scroll: 0,
            input: None,
            input_buffer: String::new(),
            status_msg: None,
            show_help: false,
            should_quit: false,
            dirty: true,
            code_cache: HashMap::new(),
            agent_state: Arc::new(Mutex::new(None)),
            agent_state_shown: None,
        };
        app.reconcile_selection(None);
        app.ensure_code();
        app.spawn_agent_poller();
        Ok(app)
    }

    /// Poll the agent's Herdr state on a background thread (best-effort) so the
    /// render loop never blocks on the subprocess. The thread ends with the
    /// process. No-op without an agent pane or a real Herdr.
    fn spawn_agent_poller(&self) {
        if self.herdr.is_dry_run() {
            return;
        }
        let Some(pane) = self.state.session.agent_pane_id.clone() else {
            return;
        };
        let shared = Arc::clone(&self.agent_state);
        std::thread::spawn(move || {
            let herdr = Herdr::new(false);
            if !herdr.available() {
                return;
            }
            loop {
                let next = herdr.agent_state(&pane);
                if let Ok(mut guard) = shared.lock() {
                    *guard = next;
                }
                std::thread::sleep(Duration::from_millis(1500));
            }
        });
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

    /// The best-effort agent state to show in the header, if known.
    pub fn agent_state(&self) -> Option<AgentState> {
        self.agent_state_shown
    }

    /// Pick up the latest agent state from the background poller (non-blocking).
    pub fn tick_agent(&mut self) {
        let latest = self.agent_state.lock().ok().and_then(|g| *g);
        if latest != self.agent_state_shown {
            self.agent_state_shown = latest;
            self.dirty = true;
        }
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
        self.focus_pane(Pane::Findings);
        self.detail_scroll = 0;
        self.ensure_code();
        self.dirty = true;
    }

    pub fn scroll_detail_down(&mut self) {
        self.focus_pane(Pane::Detail);
        self.detail_scroll = self
            .detail_scroll
            .saturating_add(3)
            .min(self.detail_max_scroll);
        self.dirty = true;
    }

    pub fn scroll_detail_up(&mut self) {
        self.focus_pane(Pane::Detail);
        self.detail_scroll = self.detail_scroll.saturating_sub(3);
        self.dirty = true;
    }

    /// Record what the renderer laid out, so mouse positions can be resolved
    /// against the panes that were actually painted.
    pub fn record_layout(
        &mut self,
        list_area: Rect,
        detail_area: Rect,
        list_offset: usize,
        detail_max_scroll: u16,
    ) {
        self.list_area = list_area;
        self.detail_area = detail_area;
        self.list_offset = list_offset;
        self.detail_max_scroll = detail_max_scroll;
        self.detail_scroll = self.detail_scroll.min(detail_max_scroll);
    }

    /// The pane containing a terminal cell, if any.
    pub fn pane_at(&self, col: u16, row: u16) -> Option<Pane> {
        if self.list_area.contains((col, row).into()) {
            Some(Pane::Findings)
        } else if self.detail_area.contains((col, row).into()) {
            Some(Pane::Detail)
        } else {
            None
        }
    }

    pub fn focus_pane(&mut self, pane: Pane) {
        if self.focus != pane {
            self.focus = pane;
            self.dirty = true;
        }
    }

    /// Select the finding drawn on a terminal row of the list pane. Rows outside
    /// the list's inner area (borders) or past the last finding are ignored.
    pub fn select_at_row(&mut self, row: u16) {
        let top = self.list_area.y.saturating_add(1);
        let bottom = self.list_area.bottom().saturating_sub(1);
        if row < top || row >= bottom {
            return;
        }
        let index = self.list_offset + (row - top) as usize;
        if index < self.state.findings.len() && index != self.selected {
            self.selected = index;
            self.after_move();
        }
    }

    pub fn scroll_focus_down(&mut self) {
        match self.focus {
            Pane::Findings => self.select_next(),
            Pane::Detail => self.scroll_detail_down(),
        }
    }

    pub fn scroll_focus_up(&mut self) {
        match self.focus {
            Pane::Findings => self.select_prev(),
            Pane::Detail => self.scroll_detail_up(),
        }
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
    /// there is no agent pane wired (or no Herdr), `Err` on a delivery failure
    /// (surfaced in the status line so the user knows to retry).
    fn deliver_to_agent(&self, text: &str) -> Result<bool> {
        match &self.state.session.agent_pane_id {
            Some(pane) if self.herdr.available() => {
                self.herdr.submit_to_agent(pane, text)?;
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
