//! Colours and the small drawing helpers that give the UI its btop accent.

use ratatui::style::{Color, Modifier, Style};

pub const FG: Color = Color::Rgb(0xc8, 0xd0, 0xda);
pub const DIM: Color = Color::Rgb(0x6b, 0x74, 0x82);
pub const FAINT: Color = Color::Rgb(0x3a, 0x41, 0x4c);
pub const ACCENT: Color = Color::Rgb(0x56, 0xc8, 0xd8);
pub const ACCENT_DK: Color = Color::Rgb(0x1c, 0x4a, 0x52);
pub const GOOD: Color = Color::Rgb(0x7c, 0xd9, 0x92);
pub const WARN: Color = Color::Rgb(0xe6, 0xc3, 0x84);
pub const BAD: Color = Color::Rgb(0xe8, 0x7d, 0x7d);
pub const HILITE: Color = Color::Rgb(0xd2, 0xa8, 0xff);
pub const RUNNING: Color = Color::Rgb(0x8b, 0xe9, 0xa4);

/// mutt's reverse-video status line.
pub fn status_bar() -> Style {
    Style::default().fg(Color::Rgb(0x0d, 0x12, 0x17)).bg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn selected() -> Style {
    Style::default().fg(Color::Rgb(0x0d, 0x12, 0x17)).bg(ACCENT).add_modifier(Modifier::BOLD)
}

pub fn selected_unfocused() -> Style {
    Style::default().fg(FG).bg(ACCENT_DK)
}

pub fn border() -> Style {
    Style::default().fg(FAINT)
}

pub fn border_focused() -> Style {
    Style::default().fg(ACCENT)
}

/// btop's green -> yellow -> red ramp, for a 0..=1 fraction.
pub fn ramp(t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let (r, g, b) = if t < 0.5 {
        let k = t / 0.5;
        lerp3((0x7c, 0xd9, 0x92), (0xe6, 0xc3, 0x84), k)
    } else {
        let k = (t - 0.5) / 0.5;
        lerp3((0xe6, 0xc3, 0x84), (0xe8, 0x5d, 0x5d), k)
    };
    Color::Rgb(r, g, b)
}

fn lerp3(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (f(a.0, b.0), f(a.1, b.1), f(a.2, b.2))
}

/// Eighth-block partial cells, so a meter can resolve below one column.
pub const BLOCKS: [&str; 9] = ["", "▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];

/// Braille levels used by the mini graph, one glyph per two samples.
pub const BRAILLE_DOTS: [[u16; 5]; 2] = [
    [0x0000, 0x0040, 0x0044, 0x0046, 0x0047],
    [0x0000, 0x0080, 0x00a0, 0x00b0, 0x00b8],
];
