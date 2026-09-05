//! Modal overlays: help, the add-a-game wizard, confirmations, runner picker
//! and the log viewer.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::widgets::*;
use crate::app::{AddSource, AddStep, AddWizard, App, Mode, Overlay, Prompt};
use crate::install::{Arch, Phase};
use crate::launch;
use crate::model::*;
use crate::theme;

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let Mode::Overlay(overlay) = &app.mode else { return };
    match overlay {
        Overlay::Help { scroll } => help(frame, area, *scroll),
        Overlay::Add(w) => add(frame, app, area, w),
        Overlay::Confirm { message, .. } => confirm(frame, area, message),
        Overlay::RunnerPick { idx, .. } => runner_pick(frame, app, area, *idx),
        Overlay::Log { title, lines, scroll } => log(frame, area, title, lines, *scroll),
    }
}

fn panel(frame: &mut Frame, area: Rect, title: &str) -> Rect {
    clear(frame, area);
    let block = boxed(title, true);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

// ------------------------------------------------------------------ help ---

fn help(frame: &mut Frame, area: Rect, scroll: u16) {
    let area = centered(72, 82, area);
    let inner = panel(frame, area, "help");

    let mut lines: Vec<Line> = Vec::new();
    let section = |lines: &mut Vec<Line>, name: &str| {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(" {}", name),
            Style::default().fg(theme::HILITE).add_modifier(Modifier::BOLD),
        )));
    };
    let row = |lines: &mut Vec<Line>, keys: &str, what: &str| {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<12}", keys),
                Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(what.to_string(), Style::default().fg(theme::FG)),
        ]));
    };

    section(&mut lines, "moving around");
    row(&mut lines, "j / k", "next / previous game");
    row(&mut lines, "g / G", "first / last");
    row(&mut lines, "^d / ^u", "half page down / up");
    row(&mut lines, "Tab", "cycle sidebar → index → preview");
    row(&mut lines, "b / v", "toggle sidebar / preview pane");

    section(&mut lines, "games");
    row(&mut lines, "a", "add a game — pick one of the two ways in");
    row(&mut lines, "i", "install from a setup .exe / .msi");
    row(&mut lines, "Enter / p", "play the selected game");
    row(&mut lines, "x", "terminate the running game");
    row(&mut lines, "R", "change the wine/proton runner");
    row(&mut lines, "C", "open winecfg on the prefix");
    row(&mut lines, "o", "read the last launch log");
    row(&mut lines, "d", "remove from library (files stay)");
    row(&mut lines, "s / S", "cycle sort key / reverse");
    row(&mut lines, "$", "rescan runners and prefix sizes");

    section(&mut lines, "finding things");
    row(&mut lines, "/", "search name, runner, path, tag");
    row(&mut lines, "Tab ^j ^k", "step through a path prompt's entries");
    row(&mut lines, "Esc", "clear the search");

    section(&mut lines, "commands  (:)");
    row(&mut lines, ":add [path]", "add a game already on disk");
    row(&mut lines, ":install <e>", "run an installer in a new prefix");
    row(&mut lines, ":rename <s>", "rename the selected game");
    row(&mut lines, ":tag a,b", "set tags (they become sidebar boxes)");
    row(&mut lines, ":runner", "runner picker");
    row(&mut lines, ":sort <k>", "name | played | time | size | added");
    row(&mut lines, ":log", "launch log");
    row(&mut lines, ":winecfg", "winecfg on the prefix");
    row(&mut lines, ":q", "quit");

    section(&mut lines, "flags in the index");
    row(&mut lines, "▶", "running right now");
    row(&mut lines, "N", "never played");
    row(&mut lines, "!", "executable is missing from disk");

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("  library: {}", crate::library::library_path().display()),
        Style::default().fg(theme::DIM),
    )));
    lines.push(Line::from(Span::styled(
        format!("  logs:    {}", crate::library::log_dir().display()),
        Style::default().fg(theme::DIM),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  q or Esc to close",
        Style::default().fg(theme::DIM),
    )));

    frame.render_widget(Paragraph::new(lines).scroll((scroll, 0)), inner);
}

// --------------------------------------------------------------- confirm ---

fn confirm(frame: &mut Frame, area: Rect, message: &str) {
    let area = centered_fixed((message.chars().count() as u16 + 8).min(area.width), 7, area);
    let inner = panel(frame, area, "confirm");
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {}", message),
            Style::default().fg(theme::FG),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  y", Style::default().fg(theme::GOOD).add_modifier(Modifier::BOLD)),
            Span::styled("es    ", Style::default().fg(theme::DIM)),
            Span::styled("n", Style::default().fg(theme::BAD).add_modifier(Modifier::BOLD)),
            Span::styled("o / Esc", Style::default().fg(theme::DIM)),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

// ---------------------------------------------------------- runner picker ---

fn runner_pick(frame: &mut Frame, app: &App, area: Rect, idx: usize) {
    let h = (app.runners.len() as u16 + 4).min(area.height.saturating_sub(4));
    let area = centered_fixed(70.min(area.width), h.max(6), area);
    let inner = panel(frame, area, "runner");

    let mut lines: Vec<Line> = Vec::new();
    if app.runners.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no proton or wine installations found",
            Style::default().fg(theme::BAD),
        )));
    }
    for (i, r) in app.runners.iter().enumerate() {
        let style = if i == idx {
            theme::selected()
        } else {
            Style::default().fg(theme::FG)
        };
        let kind_style = if i == idx {
            style
        } else if r.kind == RunnerKind::Proton {
            Style::default().fg(theme::HILITE)
        } else {
            Style::default().fg(theme::ACCENT)
        };
        lines.push(Line::from(vec![
            Span::styled(if i == idx { "▌" } else { " " }, Style::default().fg(theme::ACCENT)),
            Span::styled(format!("{:<8}", r.kind.label()), kind_style),
            Span::styled(format!("{:<28}", ellipsize(&r.name, 27)), style),
            Span::styled(
                format!("{}", r.origin),
                if i == idx { style } else { Style::default().fg(theme::DIM) },
            ),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(footer("↵ select   Esc cancel"));
    frame.render_widget(Paragraph::new(lines), inner);
}

// -------------------------------------------------------------------- log ---

fn log(frame: &mut Frame, area: Rect, title: &str, lines: &[String], scroll: usize) {
    let area = centered(88, 85, area);
    let inner = panel(frame, area, &ellipsize(title, area.width.saturating_sub(8) as usize));
    let height = inner.height.saturating_sub(1) as usize;
    let start = scroll.min(lines.len().saturating_sub(1));
    let mut out: Vec<Line> = lines
        .iter()
        .skip(start)
        .take(height)
        .map(|l| {
            let style = if l.starts_with('#') {
                Style::default().fg(theme::DIM)
            } else if l.contains("err:") || l.to_lowercase().contains("error") {
                Style::default().fg(theme::BAD)
            } else if l.contains("fixme:") || l.contains("warn:") {
                Style::default().fg(theme::WARN)
            } else {
                Style::default().fg(theme::FG)
            };
            Line::from(Span::styled(
                ellipsize(l, inner.width as usize),
                style,
            ))
        })
        .collect();
    out.push(footer(&format!(
        "line {}/{}   j/k scroll   g/G ends   Esc close",
        start + 1,
        lines.len()
    )));
    frame.render_widget(Paragraph::new(out), inner);
}

// ----------------------------------------------------------------- wizard ---

fn add(frame: &mut Frame, app: &App, area: Rect, w: &AddWizard) {
    let area = centered(78, 80, area);
    let title = match w.step {
        AddStep::Source => "add game".to_string(),
        _ => format!("add game · {}", w.source.title()),
    };
    let inner = panel(frame, area, &title);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // breadcrumb
            Constraint::Min(3),    // step body
            Constraint::Length(2), // error + footer
        ])
        .split(inner);

    breadcrumb(frame, rows[0], w);

    // Each step that draws a text field reports where its cursor belongs, so
    // the two can never drift apart.
    let cursor: Option<(u16, u16)> = match w.step {
        AddStep::Source => {
            step_source(frame, rows[1], w);
            None
        }
        AddStep::Path => step_path(frame, rows[1], w),
        AddStep::Installer => step_installer(frame, rows[1], w),
        AddStep::Dest => step_dest(frame, rows[1], w),
        AddStep::Arch => {
            step_arch(frame, rows[1], app, w);
            None
        }
        AddStep::Installing => {
            step_installing(frame, rows[1], app);
            None
        }
        AddStep::Scanning => {
            step_scanning(frame, rows[1], app.tick, w);
            None
        }
        AddStep::Exe => {
            step_exe(frame, rows[1], w);
            None
        }
        AddStep::Runner => {
            step_runner(frame, rows[1], app, w);
            None
        }
        AddStep::Name => step_name(frame, rows[1], w),
        AddStep::Confirm => {
            step_confirm(frame, rows[1], app, w);
            None
        }
    };

    let installing_live = app.install.as_ref().is_some_and(|j| j.is_live());
    let hint = match w.step {
        AddStep::Source => "j/k choose   ↵ continue   Esc cancel",
        AddStep::Path => "↵ scan   Tab/^j/^k step through folders   ↑ back   ^u clear   Esc",
        AddStep::Installer => "↵ next   Tab/^j/^k step through entries   ^u clear   Esc cancel",
        AddStep::Dest => "↵ next   Tab/^j/^k step through folders   ↑ back   ^u clear   Esc",
        AddStep::Arch => "space toggle   ↵ start installing   ⌫ back   Esc cancel",
        AddStep::Installing if installing_live => {
            "x cancel install   o full log   Esc leave it running"
        }
        AddStep::Installing => "↵ find what it installed   o full log   Esc close",
        AddStep::Scanning => "Esc cancel",
        AddStep::Exe => "j/k choose   ↵ next   ⌫ back   Esc cancel",
        AddStep::Runner => "j/k choose   ↵ next   ⌫ back   Esc cancel",
        AddStep::Name => "↵ next   ⌫ back   Esc cancel",
        AddStep::Confirm => "↵ add to library   ⌫ back   Esc cancel",
    };
    let mut tail: Vec<Line> = Vec::new();
    if let Some(err) = &w.error {
        tail.push(Line::from(Span::styled(
            format!(" ✗ {}", err),
            Style::default().fg(theme::BAD).add_modifier(Modifier::BOLD),
        )));
    } else {
        tail.push(Line::from(""));
    }
    tail.push(footer(hint));
    frame.render_widget(Paragraph::new(tail), rows[2]);

    // Park the terminal cursor in the active text field.
    if let Some((cx, cy)) = cursor {
        frame.set_cursor_position((
            (rows[1].x + cx).min(rows[1].right().saturating_sub(1)),
            (rows[1].y + cy).min(rows[1].bottom().saturating_sub(1)),
        ));
    }
}

fn breadcrumb(frame: &mut Frame, area: Rect, w: &AddWizard) {
    // Each source walks a different path through the wizard.
    let steps: &[(AddStep, &str)] = match w.source {
        AddSource::Existing => &[
            (AddStep::Path, "path"),
            (AddStep::Exe, "executable"),
            (AddStep::Runner, "runner"),
            (AddStep::Name, "name"),
            (AddStep::Confirm, "confirm"),
        ],
        AddSource::Installer => &[
            (AddStep::Installer, "installer"),
            (AddStep::Dest, "prefix"),
            (AddStep::Runner, "runner"),
            (AddStep::Arch, "arch"),
            (AddStep::Installing, "install"),
            (AddStep::Exe, "executable"),
            (AddStep::Name, "name"),
            (AddStep::Confirm, "confirm"),
        ],
    };
    let order = |s: AddStep| -> usize {
        steps
            .iter()
            .position(|(step, _)| *step == s)
            .unwrap_or(match s {
                // Steps that have no chip of their own borrow their neighbour's.
                AddStep::Scanning => match w.source {
                    AddSource::Existing => 1,
                    AddSource::Installer => 5,
                },
                _ => 0,
            })
    };

    if w.step == AddStep::Source {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " how would you like to add a game?",
                Style::default().fg(theme::DIM),
            ))),
            area,
        );
        return;
    }

    let cur = order(w.step);
    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    for (i, (s, label)) in steps.iter().enumerate() {
        let n = order(*s);
        let (mark, style) = if n < cur {
            ("●", Style::default().fg(theme::GOOD))
        } else if n == cur {
            ("◉", Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD))
        } else {
            ("○", Style::default().fg(theme::FAINT))
        };
        spans.push(Span::styled(format!("{} ", mark), style));
        spans.push(Span::styled(
            label.to_string(),
            if n == cur {
                Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::DIM)
            },
        ));
        if i + 1 < steps.len() {
            spans.push(Span::styled(" ─ ", Style::default().fg(theme::FAINT)));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn step_source(frame: &mut Frame, area: Rect, w: &AddWizard) {
    let sources = [AddSource::Existing, AddSource::Installer];
    let mut lines: Vec<Line> = vec![Line::from("")];
    for (i, src) in sources.iter().enumerate() {
        let selected = i == w.source_idx;
        let style = if selected {
            theme::selected()
        } else {
            Style::default().fg(theme::FG)
        };
        lines.push(Line::from(vec![
            Span::styled(if selected { "▌" } else { " " }, Style::default().fg(theme::ACCENT)),
            Span::styled(
                format!(" {}. ", i + 1),
                if selected { style } else { Style::default().fg(theme::DIM) },
            ),
            Span::styled(
                format!("{:<24}", src.title()),
                if selected { style } else { style.add_modifier(Modifier::BOLD) },
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!("     {}", src.blurb()),
            Style::default().fg(theme::DIM),
        )));
        lines.push(Line::from(""));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn step_installer(frame: &mut Frame, area: Rect, w: &AddWizard) -> Option<(u16, u16)> {
    let (input, cursor_x) = input_line(&w.prompt, area.width);
    let mut lines = vec![
        Line::from(Span::styled(
            " The setup .exe (or .msi) to run:",
            Style::default().fg(theme::DIM),
        )),
        input,
        Line::from(""),
    ];
    let used = lines.len();
    lines.extend(path_listing(
        &w.prompt,
        true,
        (area.height as usize).saturating_sub(used),
        area.width,
    ));
    frame.render_widget(Paragraph::new(lines), area);
    Some((cursor_x, 1))
}

fn step_dest(frame: &mut Frame, area: Rect, w: &AddWizard) -> Option<(u16, u16)> {
    let vw = area.width.saturating_sub(13) as usize;
    let (input, cursor_x) = input_line(&w.prompt, area.width);
    let mut lines = vec![
        Line::from(Span::styled(
            " A new, empty directory — the game and its wine prefix live here together:",
            Style::default().fg(theme::DIM),
        )),
        input,
        Line::from(""),
        field(
            "installing",
            ellipsize_left(
                &w.installer.clone().unwrap_or_default().to_string_lossy(),
                vw,
            ),
        ),
        Line::from(""),
    ];
    let used = lines.len();
    lines.extend(path_listing(
        &w.prompt,
        false,
        (area.height as usize).saturating_sub(used),
        area.width,
    ));
    frame.render_widget(Paragraph::new(lines), area);
    Some((cursor_x, 1))
}

fn step_arch(frame: &mut Frame, area: Rect, app: &App, w: &AddWizard) {
    let proton = app
        .runners
        .get(w.runner_idx)
        .map(|r| r.kind == RunnerKind::Proton)
        .unwrap_or(false);

    let mut lines = vec![Line::from(Span::styled(
        " Prefix architecture:",
        Style::default().fg(theme::DIM),
    ))];
    lines.push(Line::from(""));
    for arch in [Arch::Win64, Arch::Win32] {
        let selected = arch == w.arch;
        let style = if selected { theme::selected() } else { Style::default().fg(theme::FG) };
        lines.push(Line::from(vec![
            Span::styled(if selected { "▌" } else { " " }, Style::default().fg(theme::ACCENT)),
            Span::styled(format!(" {:<8}", arch.label()), style),
            Span::styled(
                arch.describe().to_string(),
                if selected { style } else { Style::default().fg(theme::DIM) },
            ),
        ]));
    }
    if proton {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " ⓘ proton always builds a 64-bit prefix with a 32-bit layer, so this",
            Style::default().fg(theme::WARN),
        )));
        lines.push(Line::from(Span::styled(
            "   setting is ignored unless you switch to a wine runner.",
            Style::default().fg(theme::WARN),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ↵ starts the installer. Its windows open on your desktop as usual.",
        Style::default().fg(theme::DIM),
    )));
    frame.render_widget(Paragraph::new(lines), area);
}

fn step_installing(frame: &mut Frame, area: Rect, app: &App) {
    let Some(job) = &app.install else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " no install is running",
                Style::default().fg(theme::DIM),
            ))),
            area,
        );
        return;
    };
    const FRAMES: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
    let phase = job.phase();
    let live = phase.is_live();

    let (mark, mark_style) = if live {
        (
            FRAMES[(app.tick / 2) as usize % FRAMES.len()].to_string(),
            Style::default().fg(theme::ACCENT),
        )
    } else {
        match &phase {
            Phase::Done { code: 0 } => ("✓".into(), Style::default().fg(theme::GOOD)),
            Phase::Cancelled => ("⊘".into(), Style::default().fg(theme::WARN)),
            _ => ("✗".into(), Style::default().fg(theme::BAD)),
        }
    };

    let vw = area.width.saturating_sub(13) as usize;
    let mut lines = vec![
        Line::from(vec![
            Span::styled(format!(" {} ", mark), mark_style),
            Span::styled(
                phase.label(),
                Style::default()
                    .fg(if live { theme::FG } else { mark_style.fg.unwrap_or(theme::FG) })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("   {} elapsed", human_duration(job.started.elapsed().as_secs())),
                Style::default().fg(theme::DIM),
            ),
        ]),
        Line::from(""),
        field("installer", ellipsize_left(&job.installer.to_string_lossy(), vw)),
        field("prefix", ellipsize_left(&job.prefix.to_string_lossy(), vw)),
        field(
            "runner",
            format!("{} ({}, {})", job.runner.name, job.runner.kind.label(), job.arch.label()),
        ),
        Line::from(""),
    ];

    if live {
        // Indeterminate: an installer cannot tell us how far along it is.
        let width = area.width.saturating_sub(4).min(48);
        let sweep = ((app.tick as f32 / 6.0).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
        let mut bar = vec![Span::styled("   ", Style::default())];
        bar.extend(meter(width, sweep).spans);
        lines.push(Line::from(bar));
        let elapsed = job.started.elapsed().as_secs();
        let note = match phase {
            Phase::Installing => {
                "   the installer's own windows are on your desktop — click through them there"
            }
            _ => "   wine is building the prefix; the first time takes a minute",
        };
        lines.push(Line::from(Span::styled(note, Style::default().fg(theme::DIM))));
        if elapsed > 90 {
            // Almost always a modal wine dialog nobody noticed.
            lines.push(Line::from(Span::styled(
                "   ⚠ taking a while — check your desktop for a dialog waiting on you, or press x",
                Style::default().fg(theme::WARN),
            )));
        }
    } else if matches!(phase, Phase::Done { .. }) {
        lines.push(Line::from(Span::styled(
            "   ↵ to scan the new prefix and pick the game's executable",
            Style::default().fg(theme::GOOD),
        )));
    }
    lines.push(Line::from(""));

    // Live tail of the wine output.
    let used = lines.len() as u16;
    let room = area.height.saturating_sub(used + 1) as usize;
    if room > 1 {
        lines.push(Line::from(Span::styled(
            " log",
            Style::default().fg(theme::DIM),
        )));
        for line in crate::install::tail(&job.log, room.saturating_sub(1)) {
            let style = if line.starts_with('#') {
                Style::default().fg(theme::ACCENT_DK)
            } else if line.contains("err:") {
                Style::default().fg(theme::BAD)
            } else if line.contains("warn:") {
                Style::default().fg(theme::WARN)
            } else {
                Style::default().fg(theme::FAINT)
            };
            lines.push(Line::from(Span::styled(
                format!("   {}", ellipsize(&line, area.width.saturating_sub(4) as usize)),
                style,
            )));
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}

/// The marker in front of a text field. Its width is also the cursor's x
/// offset inside the field.
const FIELD_GUTTER: u16 = 3;

/// Render an editable value, scrolled so the cursor is always on screen.
/// Returns the line plus the cursor's column within `width`.
fn input_line(p: &Prompt, width: u16) -> (Line<'static>, u16) {
    let field_w = width.saturating_sub(FIELD_GUTTER + 1).max(1) as usize;
    let chars: Vec<char> = p.value.chars().collect();
    // Keep the cursor inside the window; long paths scroll their head off.
    let start = p.cursor.saturating_sub(field_w.saturating_sub(1));
    let end = (start + field_w).min(chars.len());
    let visible: String = chars[start.min(end)..end].iter().collect();
    let line = Line::from(vec![
        Span::styled(" ▸ ", Style::default().fg(theme::ACCENT)),
        Span::styled(visible, Style::default().fg(theme::FG).add_modifier(Modifier::BOLD)),
    ]);
    (line, FIELD_GUTTER + (p.cursor - start) as u16)
}

/// The directory listing under a path prompt. Once Tab or ^J/^K have built a
/// completion list that list is what is shown, with the active entry marked,
/// scrolled to keep it visible.
fn path_listing(p: &Prompt, with_exes: bool, rows: usize, width: u16) -> Vec<Line<'static>> {
    if rows == 0 {
        return Vec::new();
    }
    let (entries, selected): (Vec<std::path::PathBuf>, Option<usize>) = if p.completions.is_empty()
    {
        (crate::scan::complete_entries(&p.value, with_exes), None)
    } else {
        (
            p.completions
                .iter()
                .map(|c| std::path::PathBuf::from(c.trim_end_matches('/')))
                .collect(),
            Some(p.completion_idx.min(p.completions.len().saturating_sub(1))),
        )
    };
    if entries.is_empty() {
        // Say *why* there is nothing — on the destination prompt the answer is
        // usually "because you are naming a folder that does not exist yet".
        let needle = if p.value.ends_with('/') {
            String::new()
        } else {
            std::path::Path::new(&p.value)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default()
        };
        let msg = if needle.is_empty() {
            " nothing here to complete".to_string()
        } else {
            format!(" nothing here starts with \"{}\"", needle)
        };
        return vec![Line::from(Span::styled(msg, Style::default().fg(theme::DIM)))];
    }

    let body = rows.saturating_sub(1).max(1);
    let start = match selected {
        Some(i) if i >= body => (i + 1 - body).min(entries.len().saturating_sub(body)),
        _ => 0,
    };

    let mut lines = vec![Line::from(vec![
        Span::styled(
            format!(" {} entries", entries.len()),
            Style::default().fg(theme::DIM),
        ),
        Span::styled(
            if entries.len() > body {
                format!("  ({}–{})", start + 1, (start + body).min(entries.len()))
            } else {
                String::new()
            },
            Style::default().fg(theme::FAINT),
        ),
        Span::styled("   ^j/^k or Tab to step through", Style::default().fg(theme::FAINT)),
    ])];

    let name_w = (width as usize).saturating_sub(20).clamp(12, 60);
    for (i, path) in entries.iter().enumerate().skip(start).take(body) {
        let picked = selected == Some(i);
        let is_dir = path.is_dir();
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let annotation = if is_dir {
            if crate::scan::is_prefix(path) || crate::scan::is_prefix(&path.join("pfx")) {
                "wine prefix".to_string()
            } else {
                String::new()
            }
        } else {
            std::fs::metadata(path).map(|m| human_size(m.len())).unwrap_or_default()
        };
        let name_style = if picked {
            theme::selected()
        } else if is_dir {
            Style::default().fg(theme::FG)
        } else {
            Style::default().fg(theme::HILITE)
        };
        lines.push(Line::from(vec![
            Span::styled(
                if picked { " ▌" } else { "  " },
                Style::default().fg(theme::ACCENT),
            ),
            Span::styled(
                format!("{:<w$}", ellipsize(&name, name_w), w = name_w),
                name_style,
            ),
            Span::styled(
                format!(" {}", annotation),
                if picked {
                    name_style
                } else if is_dir {
                    Style::default().fg(theme::GOOD)
                } else {
                    Style::default().fg(theme::DIM)
                },
            ),
        ]));
    }
    lines
}

fn step_path(frame: &mut Frame, area: Rect, w: &AddWizard) -> Option<(u16, u16)> {
    let (input, cursor_x) = input_line(&w.prompt, area.width);
    let mut lines = vec![
        Line::from(Span::styled(
            " Directory holding the game (and usually its wine prefix):",
            Style::default().fg(theme::DIM),
        )),
        input,
        Line::from(""),
    ];
    let used = lines.len();
    lines.extend(path_listing(
        &w.prompt,
        false,
        (area.height as usize).saturating_sub(used),
        area.width,
    ));
    frame.render_widget(Paragraph::new(lines), area);
    Some((cursor_x, 1))
}

fn step_scanning(frame: &mut Frame, area: Rect, tick: u64, w: &AddWizard) {
    const FRAMES: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
    let f = FRAMES[(tick / 2) as usize % FRAMES.len()];
    let width = area.width.saturating_sub(4).min(40);
    let phase = ((tick as f32 / 8.0).sin() * 0.5 + 0.5).clamp(0.0, 1.0);
    let mut bar = vec![Span::styled("   ", Style::default())];
    bar.extend(meter(width, phase).spans);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(vec![
                Span::styled(format!(" {} ", f), Style::default().fg(theme::ACCENT)),
                Span::styled(
                    format!("scanning {}", ellipsize(&w.prompt.value, area.width.saturating_sub(14) as usize)),
                    Style::default().fg(theme::FG),
                ),
            ]),
            Line::from(""),
            Line::from(bar),
            Line::from(""),
            Line::from(Span::styled(
                "   looking for drive_c, a version file, and launchable .exes",
                Style::default().fg(theme::DIM),
            )),
        ]),
        area,
    );
}

fn step_exe(frame: &mut Frame, area: Rect, w: &AddWizard) {
    let Some(scan) = &w.scan else { return };
    let mut lines: Vec<Line> = Vec::new();

    let prefix_desc = scan
        .prefix
        .as_ref()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "none".into());
    lines.push(Line::from(vec![
        Span::styled(" prefix  ", Style::default().fg(theme::DIM)),
        Span::styled(
            ellipsize_left(&prefix_desc, area.width.saturating_sub(11) as usize),
            Style::default().fg(theme::GOOD),
        ),
    ]));
    if let Some(hint) = &scan.runner_hint {
        lines.push(Line::from(vec![
            Span::styled(" built by", Style::default().fg(theme::DIM)),
            Span::styled(format!(" {}", hint), Style::default().fg(theme::HILITE)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            " {} candidate executables, best guess first{}:",
            scan.candidates.len(),
            if scan.truncated { " (list trimmed)" } else { "" }
        ),
        Style::default().fg(theme::DIM),
    )));

    let visible = area.height.saturating_sub(lines.len() as u16 + 1) as usize;
    let start = w.exe_idx.saturating_sub(visible.saturating_sub(1));
    for (i, c) in scan.candidates.iter().enumerate().skip(start).take(visible) {
        let selected = i == w.exe_idx;
        let style = if selected { theme::selected() } else { Style::default().fg(theme::FG) };
        let sub = if selected { style } else { Style::default().fg(theme::DIM) };
        let path_w = area.width.saturating_sub(14) as usize;
        lines.push(Line::from(vec![
            Span::styled(if selected { "▌" } else { " " }, Style::default().fg(theme::ACCENT)),
            Span::styled(
                format!("{:<w$}", ellipsize_left(&c.rel, path_w), w = path_w),
                style,
            ),
            Span::styled(format!("{:>8}", human_size(c.size)), sub),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn step_runner(frame: &mut Frame, area: Rect, app: &App, w: &AddWizard) {
    let mut lines = vec![Line::from(Span::styled(
        " Which wine or proton build should run it?",
        Style::default().fg(theme::DIM),
    ))];
    if let Some(hint) = w.scan.as_ref().and_then(|s| s.runner_hint.clone()) {
        lines.push(Line::from(vec![
            Span::styled(" the prefix was last built by ", Style::default().fg(theme::DIM)),
            Span::styled(hint, Style::default().fg(theme::HILITE).add_modifier(Modifier::BOLD)),
            Span::styled(" — matching it avoids a prefix rebuild", Style::default().fg(theme::DIM)),
        ]));
    }
    lines.push(Line::from(""));

    let visible = area.height.saturating_sub(lines.len() as u16) as usize;
    let start = w.runner_idx.saturating_sub(visible.saturating_sub(1));
    for (i, r) in app.runners.iter().enumerate().skip(start).take(visible) {
        let selected = i == w.runner_idx;
        let style = if selected { theme::selected() } else { Style::default().fg(theme::FG) };
        let kind_style = if selected {
            style
        } else if r.kind == RunnerKind::Proton {
            Style::default().fg(theme::HILITE)
        } else {
            Style::default().fg(theme::ACCENT)
        };
        lines.push(Line::from(vec![
            Span::styled(if selected { "▌" } else { " " }, Style::default().fg(theme::ACCENT)),
            Span::styled(format!("{:<8}", r.kind.label()), kind_style),
            Span::styled(format!("{:<30}", ellipsize(&r.name, 29)), style),
            Span::styled(
                r.origin.clone(),
                if selected { style } else { Style::default().fg(theme::DIM) },
            ),
        ]));
    }
    if app.runners.is_empty() {
        lines.push(Line::from(Span::styled(
            " no runners found — install a proton build into ~/.local/share/Steam/compatibilitytools.d",
            Style::default().fg(theme::BAD),
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn step_name(frame: &mut Frame, area: Rect, w: &AddWizard) -> Option<(u16, u16)> {
    let (input, cursor_x) = input_line(&w.prompt, area.width);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                " Name it (this is what shows in the index):",
                Style::default().fg(theme::DIM),
            )),
            input,
        ]),
        area,
    );
    Some((cursor_x, 1))
}

fn step_confirm(frame: &mut Frame, area: Rect, app: &App, w: &AddWizard) {
    let mut lines: Vec<Line> = Vec::new();
    let exe = w.chosen_exe().unwrap_or_default();
    let prefix = w
        .scan
        .as_ref()
        .and_then(|s| s.prefix.clone())
        .unwrap_or_default();
    let vw = area.width.saturating_sub(12) as usize;

    lines.push(field("name", w.name.clone()));
    lines.push(field("executable", ellipsize_left(&exe.to_string_lossy(), vw)));
    lines.push(field("prefix", ellipsize_left(&prefix.to_string_lossy(), vw)));
    if let Some(r) = app.runners.get(w.runner_idx) {
        lines.push(field("runner", format!("{} ({})", r.name, r.kind.label())));

        // Preview the exact command so nothing about the launch is a surprise.
        let probe = Game {
            id: "preview".into(),
            name: w.name.clone(),
            exe: exe.clone(),
            prefix: prefix.clone(),
            runner: RunnerRef::of(r),
            args: Vec::new(),
            env: Default::default(),
            working_dir: None,
            tags: Vec::new(),
            notes: String::new(),
            added: chrono::Utc::now(),
            last_played: None,
            playtime_secs: 0,
            sessions: Vec::new(),
            size_bytes: None,
        };
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " command",
            Style::default().fg(theme::DIM),
        )));
        let cmd = launch::command_preview(&probe, r);
        for chunk in cmd
            .chars()
            .collect::<Vec<_>>()
            .chunks(area.width.saturating_sub(4).max(1) as usize)
        {
            lines.push(Line::from(Span::styled(
                format!("   {}", chunk.iter().collect::<String>()),
                Style::default().fg(theme::FAINT),
            )));
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn footer(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {}", text),
        Style::default().fg(theme::DIM),
    ))
}
