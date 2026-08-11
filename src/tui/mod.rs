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
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::cli::SessionArgs;
use crate::model::Verdict;
use crate::store::Store;
use app::{App, Input};

type Tui = Terminal<CrosstermBackend<Stdout>>;

pub fn view(args: &SessionArgs) -> Result<()> {
    let dir = crate::paths::resolve_session_dir(args.session.as_deref())?;
    let store = Store::new(dir);
    let mut app = App::new(store).context("loading co-review session")?;

    let mut terminal = setup_terminal().context("initializing terminal")?;
    let result = run_loop(&mut terminal, &mut app);
    restore_terminal(&mut terminal).ok();
    result
}

fn setup_terminal() -> Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Tui) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
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
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Release {
                    handle_key(app, key);
                    app.dirty = true;
                }
            }
        }

        app.tick_status();
        app.poll_reload();

        if app.should_quit {
            return Ok(());
        }
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

    #[test]
    fn renders_header_list_and_detail() {
        let dir = tempfile::tempdir().unwrap();
        let store = seed(dir.path());
        let app = App::new(store).unwrap();
        let backend = TestBackend::new(100, 40);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| ui::draw(f, &app)).unwrap();
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

        let app = App::new(store).unwrap();
        let backend = TestBackend::new(90, 34);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|fr| ui::draw(fr, &app)).unwrap();
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
        let app = App::new(store).unwrap();
        let backend = TestBackend::new(90, 30);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| ui::draw(f, &app)).unwrap();
        let text = buffer_text(&term);
        assert!(
            text.contains("waiting for the agent"),
            "waiting msg missing:\n{text}"
        );
    }
}
