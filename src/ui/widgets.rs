//! Small drawing helpers: gradient meters, braille graphs and box chrome.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear};
use ratatui::Frame;

use crate::theme;

/// A rounded box with a btop-style `┤ title ├` chip in the top border.
pub fn boxed(title: &str, focused: bool) -> Block<'static> {
    let style = if focused { theme::border_focused() } else { theme::border() };
    let chip = Line::from(vec![
        Span::styled("┤", style),
        Span::styled(
            format!(" {} ", title),
            if focused {
                Style::default().fg(theme::ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::DIM)
            },
        ),
        Span::styled("├", style),
    ]);
    Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(style)
        .title(chip)
}

/// A horizontal bar whose colour follows btop's green→yellow→red ramp.
/// Resolution is 1/8 of a cell.
pub fn meter(width: u16, frac: f32) -> Line<'static> {
    let width = width.max(1) as usize;
    let frac = frac.clamp(0.0, 1.0);
    let eighths = (frac * (width * 8) as f32).round() as usize;
    let full = eighths / 8;
    let rem = eighths % 8;

    let mut spans: Vec<Span> = Vec::with_capacity(width);
    for i in 0..width {
        let t = (i as f32 + 0.5) / width as f32;
        let color = theme::ramp(t);
        if i < full {
            spans.push(Span::styled("█", Style::default().fg(color)));
        } else if i == full && rem > 0 {
            spans.push(Span::styled(theme::BLOCKS[rem], Style::default().fg(color)));
        } else {
            spans.push(Span::styled("·", Style::default().fg(theme::FAINT)));
        }
    }
    Line::from(spans)
}

/// A one-row braille graph: two samples per cell, four vertical levels,
/// coloured by the value it is drawing.
pub fn braille_graph(samples: &[f32], width: u16, max: f32) -> Line<'static> {
    let width = width.max(1) as usize;
    let want = width * 2;
    let start = samples.len().saturating_sub(want);
    let recent = &samples[start..];

    let level = |v: f32| -> usize {
        if max <= 0.0 {
            0
        } else {
            ((v / max) * 4.0).ceil().clamp(0.0, 4.0) as usize
        }
    };

    let mut spans: Vec<Span> = Vec::with_capacity(width);
    let pad = want.saturating_sub(recent.len());
    for cell in 0..width {
        let i0 = cell * 2;
        let get = |i: usize| -> f32 {
            if i < pad {
                0.0
            } else {
                recent.get(i - pad).copied().unwrap_or(0.0)
            }
        };
        let (a, b) = (get(i0), get(i0 + 1));
        let bits = theme::BRAILLE_DOTS[0][level(a)] | theme::BRAILLE_DOTS[1][level(b)];
        let ch = char::from_u32(0x2800 + bits as u32).unwrap_or(' ');
        let peak = a.max(b);
        let color = if bits == 0 {
            theme::FAINT
        } else {
            theme::ramp(if max > 0.0 { peak / max } else { 0.0 })
        };
        spans.push(Span::styled(ch.to_string(), Style::default().fg(color)));
    }
    Line::from(spans)
}

/// Column-chart sparkline for per-session play lengths.
pub fn sparkline(values: &[u32], width: u16) -> Line<'static> {
    const BARS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let width = width.max(1) as usize;
    if values.is_empty() {
        return Line::from(Span::styled(
            "·".repeat(width),
            Style::default().fg(theme::FAINT),
        ));
    }
    let start = values.len().saturating_sub(width);
    let recent = &values[start..];
    let max = recent.iter().copied().max().unwrap_or(1).max(1) as f32;
    let mut spans: Vec<Span> = Vec::new();
    for v in recent {
        let t = *v as f32 / max;
        let idx = ((t * 7.0).round() as usize).min(7);
        spans.push(Span::styled(BARS[idx], Style::default().fg(theme::ramp(t * 0.7))));
    }
    let used = recent.len();
    if used < width {
        spans.insert(
            0,
            Span::styled("·".repeat(width - used), Style::default().fg(theme::FAINT)),
        );
    }
    Line::from(spans)
}

/// A key/value line for the detail pane.
pub fn field<'a>(key: &'a str, value: impl Into<String>) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!(" {:<11}", key), Style::default().fg(theme::DIM)),
        Span::styled(value.into(), Style::default().fg(theme::FG)),
    ])
}

/// A centred rectangle, in percent of the parent.
pub fn centered(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(v[1])[1]
}

/// A centred rectangle of a fixed size, clamped to the parent.
pub fn centered_fixed(w: u16, h: u16, area: Rect) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

pub fn clear(frame: &mut Frame, area: Rect) {
    frame.render_widget(Clear, area);
}

/// Truncate to `width` columns, adding an ellipsis when it does not fit.
pub fn ellipsize(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    if width <= 1 {
        return "…".into();
    }
    let head: String = s.chars().take(width - 1).collect();
    format!("{}…", head)
}

/// Keep the tail of a path — the interesting end — when it is too long.
pub fn ellipsize_left(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n <= width {
        return s.to_string();
    }
    if width <= 1 {
        return "…".into();
    }
    let tail: String = s.chars().skip(n - (width - 1)).collect();
    format!("…{}", tail)
}
