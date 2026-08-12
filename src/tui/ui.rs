//! Rendering the navigator: header, findings list, detail + related code,
//! footer/status, input line, and a help overlay.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::model::{Severity, Verdict};
use crate::tui::app::{App, Input};

pub fn draw(f: &mut Frame, app: &App) {
    let size = f.area();
    let footer_h = if app.input.is_some() { 3 } else { 1 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),        // header
            Constraint::Percentage(34),   // list
            Constraint::Min(6),           // detail
            Constraint::Length(footer_h), // footer / input
        ])
        .split(size);

    draw_header(f, app, chunks[0]);
    draw_list(f, app, chunks[1]);
    draw_detail(f, app, chunks[2]);
    if app.input.is_some() {
        draw_input(f, app, chunks[3]);
    } else {
        draw_footer(f, app, chunks[3]);
    }

    if app.show_help {
        draw_help(f, size);
    }
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let s = &app.state;
    let c = s.counts();

    let title = if s.pr.title.is_empty() {
        format!("{}/{} #{}", s.pr.owner, s.pr.repo, s.pr.number)
    } else {
        format!(
            "{}/{} #{} — {}",
            s.pr.owner, s.pr.repo, s.pr.number, s.pr.title
        )
    };

    let mut spans = vec![Span::styled(
        format!(" {} ", s.status.label()),
        Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    )];
    if let Some(agent) = app.agent_state() {
        spans.push(Span::styled(
            format!(" agent: {agent} "),
            Style::default().fg(agent_state_color(agent)),
        ));
    }
    spans.extend([
        Span::raw(format!("  {} findings", c.total)),
        Span::styled(
            format!("  {} pending", c.pending),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(
            format!("  {} approved", c.approved),
            Style::default().fg(Color::Green),
        ),
        Span::styled(
            format!("  {} dismissed", c.dismissed),
            Style::default().fg(Color::Red),
        ),
        Span::styled(
            format!("  {} posted", c.posted),
            Style::default().fg(Color::Cyan),
        ),
    ]);
    let line2 = Line::from(spans);

    let block = Block::default().borders(Borders::ALL).title(" co-review ");
    let para = Paragraph::new(vec![
        Line::from(Span::styled(
            title,
            Style::default().add_modifier(Modifier::BOLD),
        )),
        line2,
    ])
    .block(block);
    f.render_widget(para, area);
}

fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = if app.state.findings.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "  waiting for the agent's findings…",
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC),
        )))]
    } else {
        app.state
            .findings
            .iter()
            .map(|fd| {
                let loc = fd
                    .primary_location()
                    .map(|l| l.label())
                    .unwrap_or_else(|| "—".into());
                let line = Line::from(vec![
                    Span::styled(
                        format!("{} ", fd.severity.glyph()),
                        Style::default().fg(severity_color(fd.severity)),
                    ),
                    verdict_badge(fd.verdict),
                    Span::raw(" "),
                    Span::styled(
                        fd.title.clone(),
                        if fd.posted {
                            Style::default().add_modifier(Modifier::DIM)
                        } else {
                            Style::default()
                        },
                    ),
                    Span::styled(format!("  {loc}"), Style::default().fg(Color::DarkGray)),
                ]);
                ListItem::new(line)
            })
            .collect()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" findings (j/k) ");
    let list = List::default()
        .items(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(40, 40, 55))
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▌");

    let mut state = ListState::default();
    if !app.state.findings.is_empty() {
        state.select(Some(app.selected));
    }
    f.render_stateful_widget(list, area, &mut state);
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    if let Some(fd) = app.state.findings.get(app.selected) {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", fd.severity.glyph()),
                Style::default().fg(severity_color(fd.severity)),
            ),
            Span::styled(
                fd.severity.label().to_uppercase(),
                Style::default()
                    .fg(severity_color(fd.severity))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            verdict_badge(fd.verdict),
            Span::raw("  "),
            Span::styled(fd.id.clone(), Style::default().fg(Color::DarkGray)),
        ]));
        lines.push(Line::from(Span::styled(
            fd.title.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        if let Some(cat) = &fd.category {
            lines.push(Line::from(Span::styled(
                format!("category: {cat}"),
                Style::default().fg(Color::DarkGray),
            )));
        }
        if fd.posted {
            let url = fd.posted_url.clone().unwrap_or_default();
            lines.push(Line::from(Span::styled(
                format!("✓ posted {url}"),
                Style::default().fg(Color::Cyan),
            )));
        }
        lines.push(Line::from(""));
        for bl in fd.body.lines() {
            lines.push(Line::from(bl.to_string()));
        }
        if let Some(sug) = &fd.suggestion {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "suggestion:",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));
            for sl in sug.lines() {
                lines.push(Line::from(Span::styled(
                    sl.to_string(),
                    Style::default().fg(Color::Green),
                )));
            }
        }
        if let Some(note) = &fd.user_note {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(
                    "your note: ",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(note.clone(), Style::default().fg(Color::Yellow)),
            ]));
        }

        // Related code blocks.
        for block in app.code_blocks() {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("── {} ──", block.header),
                Style::default()
                    .fg(Color::Rgb(130, 170, 255))
                    .add_modifier(Modifier::BOLD),
            )));
            lines.extend(block.lines.iter().cloned());
        }
    } else {
        lines.push(Line::from(Span::styled(
            "No finding selected.",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let total = lines.len() as u16;
    let max_scroll = total.saturating_sub(1);
    let scroll = app.detail_scroll.min(max_scroll);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" detail & related code (J/K scroll) ");
    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    f.render_widget(para, area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let hint = "j/k move  J/K scroll  a approve  d dismiss  x discuss  u reset  n note  c chat  P post  r refresh  ? help  q quit";
    let content = match app.status_line() {
        Some(msg) => Line::from(Span::styled(
            format!(" {msg}"),
            Style::default().fg(Color::Black).bg(Color::Yellow),
        )),
        None => Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray))),
    };
    f.render_widget(Paragraph::new(content), area);
}

fn draw_input(f: &mut Frame, app: &App, area: Rect) {
    let (title, prefix) = match app.input {
        Some(Input::Note) => (" note (Enter save · Esc cancel) ", "note> "),
        Some(Input::Chat) => (" message to agent (Enter send · Esc cancel) ", "chat> "),
        None => (" input ", "> "),
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    let line = Line::from(vec![
        Span::styled(prefix, Style::default().fg(Color::Cyan)),
        Span::raw(app.input_buffer.clone()),
        Span::styled("▏", Style::default().fg(Color::Cyan)),
    ]);
    f.render_widget(Paragraph::new(line).block(block), area);
}

fn draw_help(f: &mut Frame, size: Rect) {
    let area = centered_rect(70, 70, size);
    f.render_widget(Clear, area);
    let help = vec![
        Line::from(Span::styled(
            "co-review navigator",
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Navigation"),
        Line::from("  j / k / ↓ / ↑     move between findings"),
        Line::from("  g / G             first / last finding"),
        Line::from("  J / K / PgDn/PgUp scroll the detail & code"),
        Line::from(""),
        Line::from("Triage (acts on the selected finding)"),
        Line::from("  a  approve        d  dismiss"),
        Line::from("  x  needs-discussion   u  reset to pending"),
        Line::from("  e  approve as edited"),
        Line::from("  n  add / edit your note"),
        Line::from(""),
        Line::from("Collaboration"),
        Line::from("  c  message the agent about this finding (into its pane)"),
        Line::from("  P  ask the agent to post the approved findings"),
        Line::from(""),
        Line::from("Other"),
        Line::from("  r  force refresh    ?  toggle this help    q / Esc  quit"),
        Line::from(""),
        Line::from(Span::styled(
            "Findings appear live as the agent records them; your verdicts and notes",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "are visible to the agent immediately.",
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let block = Block::default().borders(Borders::ALL).title(" help ");
    f.render_widget(
        Paragraph::new(help)
            .block(block)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn agent_state_color(state: &str) -> Color {
    match state {
        "working" | "running" | "thinking" => Color::Green,
        "blocked" | "waiting" => Color::Yellow,
        "error" => Color::Red,
        "done" => Color::Cyan,
        _ => Color::Gray,
    }
}

fn severity_color(s: Severity) -> Color {
    match s {
        Severity::Critical => Color::Rgb(255, 85, 85),
        Severity::High => Color::Rgb(255, 135, 95),
        Severity::Medium => Color::Rgb(240, 200, 90),
        Severity::Low => Color::Rgb(120, 170, 255),
        Severity::Nit => Color::Rgb(140, 140, 150),
    }
}

fn verdict_badge(v: Verdict) -> Span<'static> {
    let (text, color) = match v {
        Verdict::Pending => ("pending ", Color::DarkGray),
        Verdict::Approved => ("approved", Color::Green),
        Verdict::Dismissed => ("dismissed", Color::Red),
        Verdict::NeedsDiscussion => ("discuss ", Color::Yellow),
        Verdict::Edited => ("edited  ", Color::Cyan),
    };
    Span::styled(format!("[{text}]"), Style::default().fg(color))
}

/// A centered rectangle `pct_x`% by `pct_y`% of `r`.
fn centered_rect(pct_x: u16, pct_y: u16, r: Rect) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(v[1])[1]
}
