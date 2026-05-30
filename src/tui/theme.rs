use ratatui::style::Color;

/// Adaptive TUI color theme based on detected terminal background.
///
/// Detection: `COLORFGBG` env var → default dark.
pub struct TuiTheme {
    is_dark: bool,
}

impl TuiTheme {
    pub fn detect() -> Self {
        Self {
            is_dark: detect_dark_terminal(),
        }
    }

    pub fn fg_primary(&self) -> Color {
        Color::Reset
    }

    pub fn fg_secondary(&self) -> Color {
        if self.is_dark { Color::Gray } else { Color::DarkGray }
    }

    pub fn fg_dim(&self) -> Color {
        if self.is_dark { Color::DarkGray } else { Color::Gray }
    }

    pub fn fg_empty(&self) -> Color {
        self.fg_secondary()
    }
}

fn detect_dark_terminal() -> bool {
    // COLORFGBG: "fg_color;bg_color" with ANSI color indices (xterm, rxvt, kitty, etc.)
    if let Ok(val) = std::env::var("COLORFGBG") {
        if let Some(bg) = val.split(';').next_back().and_then(|s| s.trim().parse::<u8>().ok()) {
            // 0-7: standard (0=black..7=white), 8-15: bright (8=darkgray..15=bright white)
            return bg <= 8;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_theme_colors() {
        let t = TuiTheme { is_dark: true };
        assert_eq!(t.fg_primary(), Color::Reset);
        assert_eq!(t.fg_secondary(), Color::Gray);
        assert_eq!(t.fg_dim(), Color::DarkGray);
        assert_eq!(t.fg_empty(), Color::Gray);
    }

    #[test]
    fn light_theme_colors() {
        let t = TuiTheme { is_dark: false };
        assert_eq!(t.fg_primary(), Color::Reset);
        assert_eq!(t.fg_secondary(), Color::DarkGray);
        assert_eq!(t.fg_dim(), Color::Gray);
        assert_eq!(t.fg_empty(), Color::DarkGray);
    }
}
