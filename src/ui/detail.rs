//! The preview pane: everything we know about the selected game.

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::widgets::*;
use crate::app::{App, Focus};
use crate::launch;
use crate::model::*;
use crate::runners;
use crate::theme;

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let focused = app.focus == Focus::Detail;
    let Some(game) = app.selected() else {
        let block = boxed("detail", focused);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  Nothing selected.",
                    Style::default().fg(theme::DIM),
                )),
                Line::from(""),
                Line::from(vec![
                    Span::styled("  Press ", Style::default().fg(theme::DIM)),
                    Span::styled("a", Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)),
                    Span::styled(
                        " to add a game that already exists on disk.",
                        Style::default().fg(theme::DIM),
                    ),
                ]),
            ]),
            inner,
        );
        return;
    };

    let block = boxed(&ellipsize(&game.name, 30), focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width < 12 {
        return;
    }

    let runner = runners::resolve(&app.runners, &game.runner);
    let running = app.session_for(&game.id);
    let bar_w = (inner.width.saturating_sub(24)).clamp(8, 40);

    let mut lines: Vec<Line> = Vec::new();

    // --- headline -----------------------------------------------------
    let mut head = vec![Span::styled(
        format!(" {}", game.name),
        Style::default().fg(theme::HILITE).add_modifier(Modifier::BOLD),
    )];
    if running.is_some() {
        head.push(Span::styled("  ▶ RUNNING", Style::default().fg(theme::RUNNING).add_modifier(Modifier::BOLD)));
    } else if !game.exists() {
        head.push(Span::styled("  ! MISSING", Style::default().fg(theme::BAD).add_modifier(Modifier::BOLD)));
    }
    lines.push(Line::from(head));
    lines.push(Line::from(Span::styled(
        "─".repeat(inner.width as usize),
        Style::default().fg(theme::FAINT),
    )));

    // --- live meters for a running game -------------------------------
    if let Some(session) = running {
        let (cpu, rss) = app.proc_stats.get(&game.id).copied().unwrap_or((0.0, 0));
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f32;
        let cpu_frac = (cpu / (cores * 100.0)).clamp(0.0, 1.0);
        let mem_frac = if app.sys.mem_total > 0 {
            rss as f32 / app.sys.mem_total as f32
        } else {
            0.0
        };

        let mut cpu_line = vec![Span::styled(format!(" {:<11}", "cpu"), Style::default().fg(theme::DIM))];
        cpu_line.extend(meter(bar_w, cpu_frac).spans);
        cpu_line.push(Span::styled(
            format!(" {:>5.1}%", cpu),
            Style::default().fg(theme::ramp(cpu_frac)),
        ));
        lines.push(Line::from(cpu_line));

        let mut mem_line = vec![Span::styled(format!(" {:<11}", "memory"), Style::default().fg(theme::DIM))];
        mem_line.extend(meter(bar_w, mem_frac).spans);
        mem_line.push(Span::styled(
            format!(" {:>6}", human_size(rss)),
            Style::default().fg(theme::ramp(mem_frac)),
        ));
        lines.push(Line::from(mem_line));

        lines.push(field(
            "session",
            format!(
                "{} elapsed · pid {} · runner {}",
                human_duration(session.started.elapsed().as_secs()),
                session.pid,
                session.runner
            ),
        ));
        lines.push(field(
            "log",
            ellipsize_left(&session.log.to_string_lossy(), inner.width.saturating_sub(11) as usize),
        ));
        lines.push(Line::from(""));
    }

    // --- static facts --------------------------------------------------
    let exe_w = inner.width.saturating_sub(11) as usize;
    lines.push(field("executable", ellipsize_left(&game.exe.to_string_lossy(), exe_w)));
    lines.push(field("prefix", ellipsize_left(&game.prefix.to_string_lossy(), exe_w)));

    let runner_span = match runner {
        Some(r) if r.name == game.runner.name => Span::styled(
            format!("{} ({})", r.name, r.kind.label()),
            Style::default().fg(theme::GOOD),
        ),
        Some(r) => Span::styled(
            format!("{} missing → falling back to {}", game.runner.name, r.name),
            Style::default().fg(theme::WARN),
        ),
        None => Span::styled(
            format!("{} — not installed", game.runner.name),
            Style::default().fg(theme::BAD),
        ),
    };
    lines.push(Line::from(vec![
        Span::styled(format!(" {:<11}", "runner"), Style::default().fg(theme::DIM)),
        runner_span,
    ]));

    let gs_style = if game.gamescope.enabled {
        Style::default().fg(theme::HILITE)
    } else {
        Style::default().fg(theme::DIM)
    };
    lines.push(Line::from(vec![
        Span::styled(format!(" {:<11}", "gamescope"), Style::default().fg(theme::DIM)),
        Span::styled(game.gamescope.summary(), gs_style),
    ]));

    // --- disk usage, scaled against the biggest game in the library ----
    let biggest = app
        .lib
        .games
        .iter()
        .filter_map(|g| g.size_bytes)
        .max()
        .unwrap_or(1)
        .max(1);
    match game.size_bytes {
        Some(size) => {
            let mut l = vec![Span::styled(format!(" {:<11}", "disk"), Style::default().fg(theme::DIM))];
            l.extend(meter(bar_w, size as f32 / biggest as f32).spans);
            l.push(Span::styled(
                format!(" {:>6}", human_size(size)),
                Style::default().fg(theme::FG),
            ));
            lines.push(Line::from(l));
        }
        None => lines.push(field("disk", "measuring…")),
    }

    // --- play history ---------------------------------------------------
    lines.push(field(
        "playtime",
        match game.playtime_secs {
            0 => "never played".to_string(),
            secs => format!(
                "{} over {} session(s) · last {}",
                human_duration(secs),
                game.sessions.len(),
                relative_date(game.last_played)
            ),
        },
    ));
    let mut hist = vec![Span::styled(format!(" {:<11}", "sessions"), Style::default().fg(theme::DIM))];
    hist.extend(sparkline(&game.sessions, bar_w).spans);
    hist.push(Span::styled(
        format!(
            " {}",
            game.sessions.iter().max().map(|m| format!("peak {}m", m)).unwrap_or_default()
        ),
        Style::default().fg(theme::DIM),
    ));
    lines.push(Line::from(hist));

    if !game.tags.is_empty() {
        lines.push(field("tags", game.tags.join(", ")));
    }
    lines.push(field("added", game.added.format("%Y-%m-%d %H:%M").to_string()));

    // --- warnings and the exact command line -----------------------------
    if let Some(r) = runner {
        if r.kind == RunnerKind::Proton && !game.is_proton_ready() {
            lines.push(Line::from(Span::styled(
                format!(
                    "  ⚠ proton wants {}/pfx — add a `pfx -> .` symlink",
                    game.compat_data_path().display()
                ),
                Style::default().fg(theme::WARN),
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " command",
            Style::default().fg(theme::DIM),
        )));
        let cmd = launch::command_preview(game, r);
        for chunk in wrap(&cmd, inner.width.saturating_sub(2) as usize) {
            lines.push(Line::from(Span::styled(
                format!("   {}", chunk),
                Style::default().fg(theme::FAINT),
            )));
        }
    }

    let max_scroll = lines.len().saturating_sub(inner.height as usize) as u16;
    let scroll = app.detail_scroll.min(max_scroll);
    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

/// Hard-wrap on width; the command line has no useful break points.
fn wrap(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![];
    }
    let chars: Vec<char> = s.chars().collect();
    chars.chunks(width).map(|c| c.iter().collect()).collect()
}
