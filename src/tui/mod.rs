//! The findings navigator TUI (the right pane of the split-screen).
//!
//! `view` resolves the session, puts the terminal into raw/alternate-screen
//! mode, runs the event loop (input + live reload of the shared state), and
//! always restores the terminal on the way out.

mod app;
mod syntax;
mod ui;

use std::io::{self, Stdout};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::cli::ViewArgs;
use crate::model::Verdict;
use crate::store::Store;
use app::{App, Input, Pane};

type Tui = Terminal<CrosstermBackend<Stdout>>;

pub fn view(args: &ViewArgs) -> Result<()> {
    let dir = match &args.pr {
        Some(pr) => crate::orchestrate::session_dir_for_pr(pr)?,
        None => crate::paths::resolve_session_dir(args.session.session.as_deref())?,
    };
    let store = Store::new(dir);
    let mut app = App::new(store).context("loading co-review session")?;

    install_panic_hook();
    let mut terminal = setup_terminal().context("initializing terminal")?;
    let result = run_loop(&mut terminal, &mut app);
    restore_terminal(&mut terminal).ok();
    result
}

/// Restore the terminal out of raw/alternate-screen mode if the TUI panics, so a
/// crash never leaves the user with a mangled terminal. Chains to the previous
/// hook so the panic is still reported.
fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            io::stdout(),
            DisableMouseCapture,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        original(info);
    }));
}

fn setup_terminal() -> Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    Ok(())
}

fn run_loop(terminal: &mut Tui, app: &mut App) -> Result<()> {
    loop {
        // Only repaint when something changed, so an idle session doesn't redraw
        // several times a second.
        if app.dirty {
            terminal.draw(|f| ui::draw(f, app))?;
            app.dirty = false;
        }

        if event::poll(Duration::from_millis(200))? {
            handle_event(app, event::read()?);
        }

        app.tick_status();
        app.tick_agent();
        app.poll_reload();

        if app.should_quit {
            return Ok(());
        }
    }
}

fn handle_event(app: &mut App, ev: Event) {
    match ev {
        Event::Key(key) if key.kind != KeyEventKind::Release => {
            handle_key(app, key);
            app.dirty = true;
        }
        // `Terminal::draw` is what reflows the buffers to the new size, so a
        // resize has to request a repaint or the pane keeps the old geometry.
        Event::Resize(..) => app.dirty = true,
        Event::Mouse(mouse) => handle_mouse(app, mouse),
        _ => {}
    }
}

/// Clicking a pane focuses it (and, in the list, selects the finding under the
/// cursor); the wheel scrolls whichever pane is focused, after a wheel event
/// over a pane focuses it too.
fn handle_mouse(app: &mut App, mouse: event::MouseEvent) {
    if app.show_help || app.input.is_some() {
        return;
    }
    let pane = app.pane_at(mouse.column, mouse.row);
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let Some(pane) = pane else { return };
            app.focus_pane(pane);
            if pane == Pane::Findings {
                app.select_at_row(mouse.row);
            }
        }
        MouseEventKind::ScrollDown => {
            if let Some(pane) = pane {
                app.focus_pane(pane);
            }
            app.scroll_focus_down();
        }
        MouseEventKind::ScrollUp => {
            if let Some(pane) = pane {
                app.focus_pane(pane);
            }
            app.scroll_focus_up();
        }
        _ => {}
    }
}

fn handle_key(app: &mut App, key: event::KeyEvent) {
    // Ctrl-C always exits (cancels input first).
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
        if app.input.is_some() {
            app.cancel_input();
        } else {
            app.should_quit = true;
        }
        return;
    }

    // Input mode captures most keys.
    if app.input.is_some() {
        match key.code {
            KeyCode::Enter => app.submit_input(),
            KeyCode::Esc => app.cancel_input(),
            KeyCode::Backspace => app.pop_input_char(),
            KeyCode::Char(c) => app.push_input_char(c),
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Esc => {
            if app.show_help {
                app.show_help = false;
            } else {
                app.should_quit = true;
            }
        }
        KeyCode::Char('?') => app.show_help = !app.show_help,

        KeyCode::Char('j') | KeyCode::Down => app.select_next(),
        KeyCode::Char('k') | KeyCode::Up => app.select_prev(),
        KeyCode::Char('g') | KeyCode::Home => app.select_first(),
        KeyCode::Char('G') | KeyCode::End => app.select_last(),

        KeyCode::Char('J') | KeyCode::PageDown => app.scroll_detail_down(),
        KeyCode::Char('K') | KeyCode::PageUp => app.scroll_detail_up(),

        KeyCode::Char('a') => app.set_verdict(Verdict::Approved),
        KeyCode::Char('d') => app.set_verdict(Verdict::Dismissed),
        KeyCode::Char('x') => app.set_verdict(Verdict::NeedsDiscussion),
        KeyCode::Char('u') => app.set_verdict(Verdict::Pending),
        KeyCode::Char('e') => app.set_verdict(Verdict::Edited),

        KeyCode::Char('n') => app.begin_input(Input::Note),
        KeyCode::Char('c') => app.begin_input(Input::Chat),
        KeyCode::Char('P') => app.nudge_post(),

        KeyCode::Char('r') => app.force_reload(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Finding, Location, PrInfo, SessionMeta, Severity, Side, State};
    use ratatui::backend::TestBackend;

    fn buffer_text(term: &Terminal<TestBackend>) -> String {
        let buf = term.backend().buffer();
        let area = buf.area;
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn seed(dir: &std::path::Path) -> Store {
        let store = Store::new(dir);
        let pr = PrInfo {
            owner: "elKei24".into(),
            repo: "herdr-co-review".into(),
            number: 7,
            title: "Add pagination".into(),
            author: "someone".into(),
            base_ref: "main".into(),
            head_ref: "feature".into(),
            base_sha: String::new(),
            head_sha: String::new(),
            url: String::new(),
        };
        let session = SessionMeta {
            id: "s".into(),
            worktree: dir.display().to_string(),
            source_repo: String::new(),
            created_at_ms: 0,
            agent_pane_id: None,
            view_pane_id: None,
            workspace_id: None,
            agent_kind: "claude".into(),
            prompt: String::new(),
        };
        let mut state = State::new(pr, session);
        let mut f = Finding::new("f1".into(), "Off-by-one in slicing".into());
        f.severity = Severity::High;
        f.body = "The end index is inclusive.".into();
        f.locations = vec![Location {
            file: "src/paginate.rs".into(),
            start_line: 42,
            end_line: None,
            side: Side::Head,
        }];
        state.findings.push(f);
        store.create(&state).unwrap();
        store
    }

    /// A seeded session plus `extra` further findings, drawn once so the app
    /// knows its pane geometry (which is what mouse events are resolved against).
    fn drawn_app(dir: &std::path::Path, extra: usize) -> App {
        let store = seed(dir);
        if extra > 0 {
            store
                .update(|s| {
                    for i in 0..extra {
                        s.findings
                            .push(Finding::new(format!("f{}", i + 2), format!("Finding {i}")));
                    }
                    Ok(())
                })
                .unwrap();
        }
        let mut app = App::new(store).unwrap();
        let mut term = Terminal::new(TestBackend::new(100, 40)).unwrap();
        term.draw(|f| ui::draw(f, &mut app)).unwrap();
        app
    }

    fn click(column: u16, row: u16) -> Event {
        mouse(MouseEventKind::Down(MouseButton::Left), column, row)
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> Event {
        Event::Mouse(event::MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn clicking_a_findings_row_selects_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = drawn_app(dir.path(), 3);
        let (x, row) = (app.list_area.x + 4, app.list_area.y + 3); // third row
        handle_event(&mut app, click(x, row));
        assert_eq!(app.selected, 2);
        assert_eq!(app.focus, Pane::Findings);
    }

    #[test]
    fn clicking_past_the_last_finding_keeps_the_selection() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = drawn_app(dir.path(), 1);
        let (x, row) = (app.list_area.x + 4, app.list_area.bottom() - 2);
        handle_event(&mut app, click(x, row));
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn clicking_the_detail_pane_moves_the_scroll_there() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = drawn_app(dir.path(), 3);
        assert_eq!(app.focus, Pane::Findings);

        let (x, y) = (app.detail_area.x + 4, app.detail_area.y + 2);
        handle_event(&mut app, click(x, y));
        assert_eq!(app.focus, Pane::Detail);

        // The wheel now scrolls the detail, not the findings list.
        let before = app.selected;
        handle_event(&mut app, mouse(MouseEventKind::ScrollDown, x, y));
        assert_eq!(app.selected, before, "the list must not move");
        assert!(app.detail_scroll > 0, "the detail must scroll");
    }

    #[test]
    fn the_wheel_over_the_findings_list_moves_the_selection() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = drawn_app(dir.path(), 3);
        app.focus = Pane::Detail;
        let x = app.list_area.x + 4;
        let y = app.list_area.y + 2;

        handle_event(&mut app, mouse(MouseEventKind::ScrollDown, x, y));
        assert_eq!(app.focus, Pane::Findings, "scrolling a pane focuses it");
        assert_eq!(app.selected, 1);

        handle_event(&mut app, mouse(MouseEventKind::ScrollUp, x, y));
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn the_help_overlay_swallows_mouse_input() {
        let dir = tempfile::tempdir().unwrap();
        let mut app = drawn_app(dir.path(), 3);
        app.show_help = true;
        let (x, y) = (app.list_area.x + 4, app.list_area.y + 3);
        handle_event(&mut app, click(x, y));
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn renders_header_list_and_detail() {
        let dir = tempfile::tempdir().unwrap();
        let store = seed(dir.path());
        let mut app = App::new(store).unwrap();
        let backend = TestBackend::new(100, 40);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&term);
        assert!(text.contains("#7"), "header PR number missing:\n{text}");
        assert!(text.contains("Add pagination"), "PR title missing");
        assert!(
            text.contains("Off-by-one in slicing"),
            "finding title missing"
        );
        assert!(text.contains("HIGH"), "severity missing");
        assert!(text.contains("pending"), "verdict badge missing");
    }

    /// Type text through the real input path, one key at a time.
    fn type_str(app: &mut App, text: &str) {
        for c in text.chars() {
            app.push_input_char(c);
        }
    }

    #[test]
    fn long_input_wraps_instead_of_running_off_screen() {
        let dir = tempfile::tempdir().unwrap();
        let store = seed(dir.path());
        let mut app = App::new(store).unwrap();
        app.begin_input(Input::Note);
        type_str(
            &mut app,
            "alpha bravo charlie delta echo foxtrot golf hotel india juliett kilo lima",
        );
        let mut term = Terminal::new(TestBackend::new(40, 24)).unwrap();
        term.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&term);
        assert!(
            text.contains("lima"),
            "tail of a long input must stay visible:\n{text}"
        );
    }

    #[test]
    fn input_taller_than_the_cap_scrolls_to_the_tail() {
        let dir = tempfile::tempdir().unwrap();
        let store = seed(dir.path());
        let mut app = App::new(store).unwrap();
        app.begin_input(Input::Note);
        for i in 0..120 {
            type_str(&mut app, &format!("word{i} "));
        }
        let mut term = Terminal::new(TestBackend::new(40, 20)).unwrap();
        term.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&term);
        assert!(
            text.contains("word119"),
            "the cursor end of the input must stay visible:\n{text}"
        );
        assert!(
            text.contains("findings"),
            "the rest of the UI must survive a huge input:\n{text}"
        );
    }

    #[test]
    fn resize_event_marks_the_tui_for_repaint() {
        let dir = tempfile::tempdir().unwrap();
        let store = seed(dir.path());
        let mut app = App::new(store).unwrap();
        app.dirty = false;
        handle_event(&mut app, Event::Resize(120, 40));
        assert!(app.dirty, "a resize must trigger a repaint");
    }

    #[test]
    fn renders_in_a_narrow_short_terminal() {
        let dir = tempfile::tempdir().unwrap();
        let store = seed(dir.path());
        let mut app = App::new(store).unwrap();
        let backend = TestBackend::new(40, 12);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| ui::draw(f, &mut app)).unwrap();
    }

    #[test]
    fn deciding_the_last_finding_notifies_the_agent() {
        let dir = tempfile::tempdir().unwrap();
        let store = seed(dir.path());
        store
            .update(|s| {
                s.status = crate::model::ReviewStatus::AwaitingReview;
                Ok(())
            })
            .unwrap();
        let mut app = App::new(store).unwrap();
        app.set_verdict(Verdict::Approved);
        // No agent pane is wired in tests, so delivery is skipped — but the
        // completion must be detected and surfaced.
        let status = app.status_line().unwrap_or_default().to_string();
        assert!(
            status.contains("all findings decided"),
            "expected a triage-done notification, got: {status}"
        );

        // Re-deciding an already-complete triage must not re-notify.
        app.set_verdict(Verdict::Dismissed);
        let status = app.status_line().unwrap_or_default().to_string();
        assert!(
            !status.contains("all findings decided"),
            "must not notify again, got: {status}"
        );
    }

    #[test]
    fn an_external_verdict_also_triggers_the_notification() {
        let dir = tempfile::tempdir().unwrap();
        let store = seed(dir.path());
        store
            .update(|s| {
                s.status = crate::model::ReviewStatus::AwaitingReview;
                Ok(())
            })
            .unwrap();
        let mut app = App::new(store).unwrap();
        // Decide the finding outside the TUI (as `co-review verdict` would).
        Store::new(dir.path())
            .update(|s| {
                s.findings[0].verdict = Verdict::Approved;
                Ok(())
            })
            .unwrap();
        app.poll_reload();
        let status = app.status_line().unwrap_or_default().to_string();
        assert!(
            status.contains("all findings decided"),
            "expected a triage-done notification, got: {status}"
        );
    }

    #[test]
    fn no_notification_while_the_agent_is_still_reviewing() {
        let dir = tempfile::tempdir().unwrap();
        let store = seed(dir.path());
        let mut app = App::new(store).unwrap();
        // Status is still `reviewing`: the agent may add more findings, so
        // deciding the only one so far must not trigger the hand-back.
        app.set_verdict(Verdict::Approved);
        let status = app.status_line().unwrap_or_default().to_string();
        assert!(
            !status.contains("all findings decided"),
            "premature notification: {status}"
        );
    }

    #[test]
    fn verdict_key_updates_state_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let store = seed(dir.path());
        let mut app = App::new(store).unwrap();
        app.set_verdict(Verdict::Approved);
        // In-memory reflects it…
        assert_eq!(app.state.findings[0].verdict, Verdict::Approved);
        // …and it's durable.
        let reread = Store::new(dir.path()).read().unwrap();
        assert_eq!(reread.findings[0].verdict, Verdict::Approved);
    }

    /// Not a CI test — a manual snapshot to eyeball the diff-colored code view.
    /// Run with: `cargo test dump_frame -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dump_frame() {
        use crate::exec;
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let run = |a: &[&str]| exec::run("git", a, Some(&repo)).unwrap();
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(
            repo.join("paginate.rs"),
            "fn page(items: &[u32], size: usize) -> &[u32] {\n    let end = size;\n    &items[..end]\n}\n",
        )
        .unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "base"]);
        let base = exec::capture("git", &["rev-parse", "HEAD"], Some(&repo)).unwrap();
        std::fs::write(
            repo.join("paginate.rs"),
            "fn page(items: &[u32], size: usize) -> &[u32] {\n    let end = (size + 1).min(items.len());\n    &items[..end]\n}\n",
        )
        .unwrap();
        run(&["add", "."]);
        run(&["commit", "-qm", "head"]);
        let head = exec::capture("git", &["rev-parse", "HEAD"], Some(&repo)).unwrap();

        let store = Store::new(dir.path());
        let pr = PrInfo {
            owner: "elKei24".into(),
            repo: "herdr-co-review".into(),
            number: 7,
            title: "Fix pagination end index".into(),
            author: "someone".into(),
            base_ref: "main".into(),
            head_ref: "feature".into(),
            base_sha: base,
            head_sha: head,
            url: String::new(),
        };
        let session = SessionMeta {
            id: "s".into(),
            worktree: repo.display().to_string(),
            source_repo: String::new(),
            created_at_ms: 0,
            agent_pane_id: None,
            view_pane_id: None,
            workspace_id: None,
            agent_kind: "claude".into(),
            prompt: String::new(),
        };
        let mut state = State::new(pr, session);
        let mut f = Finding::new("f1".into(), "Off-by-one in end index".into());
        f.severity = Severity::High;
        f.category = Some("correctness".into());
        f.body = "`end` was `size`, dropping the last element for full pages.".into();
        f.locations = vec![Location {
            file: "paginate.rs".into(),
            start_line: 2,
            end_line: None,
            side: Side::Head,
        }];
        state.findings.push(f);
        store.create(&state).unwrap();

        let mut app = App::new(store).unwrap();
        let backend = TestBackend::new(90, 34);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|fr| ui::draw(fr, &mut app)).unwrap();
        println!("\n{}", buffer_text(&term));
    }

    #[test]
    fn empty_session_renders_waiting_message() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path());
        let pr = PrInfo {
            owner: "o".into(),
            repo: "r".into(),
            number: 1,
            title: String::new(),
            author: String::new(),
            base_ref: String::new(),
            head_ref: String::new(),
            base_sha: String::new(),
            head_sha: String::new(),
            url: String::new(),
        };
        let session = SessionMeta {
            id: "s".into(),
            worktree: dir.path().display().to_string(),
            source_repo: String::new(),
            created_at_ms: 0,
            agent_pane_id: None,
            view_pane_id: None,
            workspace_id: None,
            agent_kind: "claude".into(),
            prompt: String::new(),
        };
        store.create(&State::new(pr, session)).unwrap();
        let mut app = App::new(store).unwrap();
        let backend = TestBackend::new(90, 30);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| ui::draw(f, &mut app)).unwrap();
        let text = buffer_text(&term);
        assert!(
            text.contains("waiting for the agent"),
            "waiting msg missing:\n{text}"
        );
    }
}
