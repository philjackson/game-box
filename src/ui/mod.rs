//! Screen layout. mutt's frame — sidebar, index, pager, status line — with a
//! btop-flavoured header and meters.

pub mod detail;
pub mod overlay;
pub mod widgets;

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, Collection, Focus, Mode, MsgKind};
use crate::model::{human_duration, human_size, relative_date};
use crate::theme;
use widgets::*;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(4),    // body
            Constraint::Length(1), // status line
            Constraint::Length(1), // command / hints
        ])
        .split(area);

    header(frame, app, rows[0]);
    body(frame, app, rows[1]);
    status_line(frame, app, rows[2]);
    prompt_line(frame, app, rows[3]);

    if let Mode::Overlay(_) = &app.mode {
        overlay::draw(frame, app, area);
    }
}

// ---------------------------------------------------------------- header ---

fn header(frame: &mut Frame, app: &App, area: Rect) {
    let block = boxed("game-box", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let total: u64 = app.lib.games.iter().filter_map(|g| g.size_bytes).sum();
    let running = app.sessions.len();

    let mut left = vec![
        Span::styled("◆ ", Style::default().fg(theme::ACCENT)),
        Span::styled(
            format!("{}", app.lib.games.len()),
            Style::default().fg(theme::FG).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if app.lib.games.len() == 1 { " game" } else { " games" },
            Style::default().fg(theme::DIM),
        ),
        Span::styled(" · ", Style::default().fg(theme::FAINT)),
        Span::styled(human_size(total), Style::default().fg(theme::FG)),
        Span::styled(" on disk", Style::default().fg(theme::DIM)),
        Span::styled(" · ", Style::default().fg(theme::FAINT)),
        Span::styled(
            format!("{}", app.runners.len()),
            Style::default().fg(theme::FG),
        ),
        Span::styled(
            if app.runners.len() == 1 { " runner" } else { " runners" },
            Style::default().fg(theme::DIM),
        ),
    ];
    if running > 0 {
        left.push(Span::styled(" · ", Style::default().fg(theme::FAINT)));
        left.push(Span::styled(
            format!("{} {} running", spinner(app.tick), running),
            Style::default().fg(theme::RUNNING).add_modifier(Modifier::BOLD),
        ));
    }
    if let Some(job) = &app.install {
        left.push(Span::styled(" · ", Style::default().fg(theme::FAINT)));
        if job.is_live() {
            left.push(Span::styled(
                format!("{} installing", spinner(app.tick)),
                Style::default().fg(theme::HILITE).add_modifier(Modifier::BOLD),
            ));
        } else if job.succeeded() {
            left.push(Span::styled(
                "✓ install ready to register",
                Style::default().fg(theme::GOOD).add_modifier(Modifier::BOLD),
            ));
        }
    }

    // Right-hand meters, btop style: a braille CPU history then a RAM bar.
    let cpu_hist: Vec<f32> = app.sys.cpu_history.iter().copied().collect();
    let graph_w = 14u16.min(inner.width / 4);
    let bar_w = 10u16.min(inner.width / 5);

    let mut right: Vec<Span> = Vec::new();
    right.push(Span::styled("cpu ", Style::default().fg(theme::DIM)));
    right.extend(braille_graph(&cpu_hist, graph_w, 100.0).spans);
    right.push(Span::styled(
        format!(" {:>3.0}%", app.sys.cpu),
        Style::default().fg(theme::ramp(app.sys.cpu / 100.0)),
    ));
    right.push(Span::styled("  mem ", Style::default().fg(theme::DIM)));
    right.extend(meter(bar_w, app.sys.mem_pct() / 100.0).spans);
    right.push(Span::styled(
        format!(" {:>3.0}%", app.sys.mem_pct()),
        Style::default().fg(theme::ramp(app.sys.mem_pct() / 100.0)),
    ));

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(10), Constraint::Length(span_width(&right) as u16)])
        .split(inner);
    frame.render_widget(Paragraph::new(Line::from(left)), cols[0]);
    frame.render_widget(
        Paragraph::new(Line::from(right)).alignment(Alignment::Right),
        cols[1],
    );
}

fn span_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.chars().count()).sum()
}

fn spinner(tick: u64) -> char {
    const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    FRAMES[(tick / 2) as usize % FRAMES.len()]
}

// ------------------------------------------------------------------ body ---

fn body(frame: &mut Frame, app: &mut App, area: Rect) {
    let cols = if app.show_sidebar {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(22), Constraint::Min(30)])
            .split(area)
    } else {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(0), Constraint::Min(30)])
            .split(area)
    };

    if app.show_sidebar {
        sidebar(frame, app, cols[0]);
    }

    let right = if app.show_preview && cols[1].height > 14 {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
            .split(cols[1])
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1)])
            .split(cols[1])
    };

    index(frame, app, right[0]);
    if right.len() > 1 {
        detail::draw(frame, app, right[1]);
    }
}

fn sidebar(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Sidebar;
    let block = boxed("boxes", focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for (i, col) in app.collections.iter().enumerate() {
        let selected = i == app.collection_idx;
        let count = app.count_in(col);
        let icon = match col {
            Collection::All => "▤",
            Collection::Running => "▶",
            Collection::Recent => "◔",
            Collection::Missing => "⚠",
            Collection::Runner(_) => "⚙",
            Collection::Tag(_) => "#",
        };
        let name_style = if selected && focused {
            theme::selected()
        } else if selected {
            theme::selected_unfocused()
        } else if matches!(col, Collection::Missing) {
            Style::default().fg(theme::BAD)
        } else if matches!(col, Collection::Running) && count > 0 {
            Style::default().fg(theme::RUNNING)
        } else {
            Style::default().fg(theme::FG)
        };
        let label = ellipsize(&col.label(), inner.width.saturating_sub(7) as usize);
        let pad = (inner.width as usize)
            .saturating_sub(label.chars().count() + 3 + count.to_string().len() + 1);
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", icon), name_style),
            Span::styled(label, name_style),
            Span::styled(" ".repeat(pad), name_style),
            Span::styled(
                format!("{}", count),
                if selected {
                    name_style
                } else {
                    Style::default().fg(theme::DIM)
                },
            ),
        ]));
        if matches!(col, Collection::Recent) || matches!(col, Collection::Missing) {
            lines.push(Line::from(Span::styled(
                "─".repeat(inner.width as usize),
                Style::default().fg(theme::FAINT),
            )));
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn index(frame: &mut Frame, app: &mut App, area: Rect) {
    let focused = app.focus == Focus::Index;
    let title = format!(
        "{} [{}]",
        app.collection().label(),
        app.view.len()
    );
    let block = boxed(&title, focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }

    // Column widths, mutt index style: flags, name, runner, size, played, time.
    let w = inner.width as usize;
    let runner_w = 16usize.min(w / 5);
    let size_w = 6;
    let played_w = 9;
    let time_w = 6;
    let fixed = 3 + runner_w + size_w + played_w + time_w + 4;
    let name_w = w.saturating_sub(fixed).max(8);

    let head = Line::from(vec![
        Span::styled("   ", Style::default()),
        Span::styled(format!("{:<w$} ", "GAME", w = name_w), Style::default().fg(theme::DIM)),
        Span::styled(format!("{:<w$} ", "RUNNER", w = runner_w), Style::default().fg(theme::DIM)),
        Span::styled(format!("{:>w$} ", "SIZE", w = size_w), Style::default().fg(theme::DIM)),
        Span::styled(format!("{:>w$} ", "PLAYED", w = played_w), Style::default().fg(theme::DIM)),
        Span::styled(format!("{:>w$}", "TIME", w = time_w), Style::default().fg(theme::DIM)),
    ]);

    let rows_h = inner.height.saturating_sub(1) as usize;
    if rows_h == 0 {
        frame.render_widget(Paragraph::new(head), inner);
        return;
    }

    // Keep the cursor in view without re-centring on every frame.
    if app.cursor < app.index_offset {
        app.index_offset = app.cursor;
    } else if app.cursor >= app.index_offset + rows_h {
        app.index_offset = app.cursor + 1 - rows_h;
    }
    if app.index_offset + rows_h > app.view.len() {
        app.index_offset = app.view.len().saturating_sub(rows_h);
    }

    let mut lines: Vec<Line> = vec![head];

    if app.view.is_empty() {
        lines.push(Line::from(Span::styled(
            if app.query.is_empty() {
                "  nothing in this box yet — press 'a' to add a game from disk"
            } else {
                "  no games match the current search — Esc to clear it"
            },
            Style::default().fg(theme::DIM),
        )));
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    for (row, &gi) in app
        .view
        .iter()
        .enumerate()
        .skip(app.index_offset)
        .take(rows_h)
        .map(|(i, gi)| (i, gi))
    {
        let g = &app.lib.games[gi];
        let selected = row == app.cursor;
        let running = app.is_running(&g.id);
        let missing = !g.exists();

        let (flag, flag_style) = if running {
            ("▶", Style::default().fg(theme::RUNNING).add_modifier(Modifier::BOLD))
        } else if missing {
            ("!", Style::default().fg(theme::BAD).add_modifier(Modifier::BOLD))
        } else if g.last_played.is_none() {
            ("N", Style::default().fg(theme::HILITE))
        } else {
            (" ", Style::default())
        };

        let base = if selected && focused {
            theme::selected()
        } else if selected {
            theme::selected_unfocused()
        } else if missing {
            Style::default().fg(theme::DIM)
        } else {
            Style::default().fg(theme::FG)
        };
        let sub = if selected { base } else { Style::default().fg(theme::DIM) };

        let cursor_mark = if selected { "▌" } else { " " };
        let size = g.size_bytes.map(human_size).unwrap_or_else(|| "·".into());
        let played = relative_date(g.last_played);
        let time = if g.playtime_secs == 0 {
            "—".to_string()
        } else {
            human_duration(g.playtime_secs)
        };

        lines.push(Line::from(vec![
            Span::styled(cursor_mark, Style::default().fg(theme::ACCENT)),
            Span::styled(flag, if selected { base } else { flag_style }),
            Span::styled(" ", base),
            Span::styled(
                format!("{:<w$} ", ellipsize(&g.name, name_w), w = name_w),
                if selected {
                    base
                } else {
                    base.add_modifier(Modifier::BOLD)
                },
            ),
            Span::styled(
                format!("{:<w$} ", ellipsize(&g.runner.name, runner_w), w = runner_w),
                sub,
            ),
            Span::styled(format!("{:>w$} ", size, w = size_w), sub),
            Span::styled(format!("{:>w$} ", ellipsize(&played, played_w), w = played_w), sub),
            Span::styled(format!("{:>w$}", ellipsize(&time, time_w), w = time_w), sub),
        ]));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

// ------------------------------------------------------------ status bar ---

fn status_line(frame: &mut Frame, app: &App, area: Rect) {
    let pos = if app.view.is_empty() {
        "0/0".to_string()
    } else {
        format!("{}/{}", app.cursor + 1, app.view.len())
    };
    let mut left = format!(
        " game-box {}: {} [{}]  sort:{}{}",
        VERSION,
        app.collection().label(),
        pos,
        app.sort.label(),
        if app.sort_rev { "↑" } else { "↓" },
    );
    if !app.query.is_empty() {
        left.push_str(&format!("  /{}", app.query));
    }
    let right = match app.selected() {
        Some(g) => ellipsize_left(&g.exe.to_string_lossy(), (area.width as usize) / 2),
        None => String::new(),
    };
    let pad = (area.width as usize)
        .saturating_sub(left.chars().count() + right.chars().count() + 1);
    let text = format!("{}{}{} ", left, "─".repeat(pad), right);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(text, theme::status_bar()))),
        area,
    );
}

fn prompt_line(frame: &mut Frame, app: &App, area: Rect) {
    match &app.mode {
        Mode::Command(p) | Mode::Search(p) => {
            let text = format!("{}{}", p.label, p.value);
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    text,
                    Style::default().fg(theme::FG),
                ))),
                area,
            );
            let x = area.x + (p.label.chars().count() + p.cursor) as u16;
            frame.set_cursor_position((x.min(area.right().saturating_sub(1)), area.y));
        }
        _ => {
            if let Some((msg, kind)) = &app.message {
                if app.tick.saturating_sub(app.message_tick) < 60 {
                    let style = match kind {
                        MsgKind::Info => Style::default().fg(theme::GOOD),
                        MsgKind::Error => Style::default().fg(theme::BAD).add_modifier(Modifier::BOLD),
                    };
                    let prefix = match kind {
                        MsgKind::Info => " ✓ ",
                        MsgKind::Error => " ✗ ",
                    };
                    frame.render_widget(
                        Paragraph::new(Line::from(vec![
                            Span::styled(prefix, style),
                            Span::styled(msg.clone(), style),
                        ])),
                        area,
                    );
                    return;
                }
            }
            frame.render_widget(Paragraph::new(hints()), area);
        }
    }
}

fn hints() -> Line<'static> {
    let key = Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD);
    let txt = Style::default().fg(theme::DIM);
    let mut spans = vec![Span::styled(" ", txt)];
    for (k, label) in [
        ("a", "add"),
        ("i", "install"),
        ("↵", "play"),
        ("x", "kill"),
        ("R", "runner"),
        ("o", "log"),
        ("d", "remove"),
        ("/", "search"),
        (":", "cmd"),
        ("?", "help"),
        ("q", "quit"),
    ] {
        spans.push(Span::styled(k, key));
        spans.push(Span::styled(format!(":{}  ", label), txt));
    }
    Line::from(spans)
}
