use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeName {
    Light,
    Dark,
    Sepia,
    Night,
    Plain,
}

impl ThemeName {
    /// Cycle to the next theme (used by `T` keybinding at runtime).
    pub fn next(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::Sepia,
            Self::Sepia => Self::Night,
            Self::Night => Self::Plain,
            Self::Plain => Self::Light,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::Sepia => "sepia",
            Self::Night => "night",
            Self::Plain => "plain",
        }
    }

    /// Parse a label back into a `ThemeName`. Unknown labels return `None`.
    pub fn from_label(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            "sepia" => Some(Self::Sepia),
            "night" => Some(Self::Night),
            "plain" => Some(Self::Plain),
            _ => None,
        }
    }
}

/// Resolved palette for rendering.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub bg: Option<Color>,
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub heading: Color,
    pub code_fg: Color,
    pub code_bg: Option<Color>,
    pub quote: Color,
    pub link: Color,
    pub rule: Color,
    pub syn_keyword: Color,
    pub syn_string: Color,
    pub syn_comment: Color,
    pub syn_number: Color,
    pub syn_type: Color,
    pub syn_fn: Color,
}

impl Theme {
    pub fn resolve(name: ThemeName) -> Self {
        match name {
            ThemeName::Light => Self {
                bg: Some(Color::Rgb(250, 248, 242)),
                fg: Color::Rgb(40, 40, 44),
                dim: Color::Rgb(120, 120, 128),
                accent: Color::Rgb(140, 92, 44),
                heading: Color::Rgb(20, 20, 24),
                code_fg: Color::Rgb(60, 60, 64),
                code_bg: Some(Color::Rgb(238, 234, 224)),
                quote: Color::Rgb(100, 100, 108),
                link: Color::Rgb(54, 92, 150),
                rule: Color::Rgb(196, 192, 180),
                syn_keyword: Color::Rgb(150, 48, 120),
                syn_string: Color::Rgb(96, 112, 40),
                syn_comment: Color::Rgb(150, 150, 156),
                syn_number: Color::Rgb(160, 84, 24),
                syn_type: Color::Rgb(60, 92, 140),
                syn_fn: Color::Rgb(44, 92, 140),
            },
            ThemeName::Dark => Self {
                bg: Some(Color::Rgb(22, 22, 26)),
                fg: Color::Rgb(220, 220, 224),
                dim: Color::Rgb(132, 132, 140),
                accent: Color::Rgb(216, 168, 112),
                heading: Color::Rgb(244, 244, 248),
                code_fg: Color::Rgb(200, 204, 214),
                code_bg: Some(Color::Rgb(32, 32, 38)),
                quote: Color::Rgb(160, 160, 168),
                link: Color::Rgb(132, 168, 222),
                rule: Color::Rgb(78, 78, 86),
                syn_keyword: Color::Rgb(232, 144, 200),
                syn_string: Color::Rgb(184, 210, 128),
                syn_comment: Color::Rgb(124, 124, 132),
                syn_number: Color::Rgb(228, 172, 116),
                syn_type: Color::Rgb(140, 196, 232),
                syn_fn: Color::Rgb(152, 176, 232),
            },
            ThemeName::Sepia => Self {
                bg: Some(Color::Rgb(44, 36, 22)),
                fg: Color::Rgb(230, 214, 184),
                dim: Color::Rgb(160, 144, 116),
                accent: Color::Rgb(198, 146, 94),
                heading: Color::Rgb(244, 224, 184),
                code_fg: Color::Rgb(210, 192, 160),
                code_bg: Some(Color::Rgb(54, 44, 30)),
                quote: Color::Rgb(176, 156, 120),
                link: Color::Rgb(186, 164, 120),
                rule: Color::Rgb(112, 96, 72),
                syn_keyword: Color::Rgb(214, 144, 100),
                syn_string: Color::Rgb(196, 176, 112),
                syn_comment: Color::Rgb(140, 124, 100),
                syn_number: Color::Rgb(220, 168, 120),
                syn_type: Color::Rgb(180, 152, 96),
                syn_fn: Color::Rgb(216, 184, 116),
            },
            ThemeName::Night => Self {
                bg: Some(Color::Rgb(16, 18, 24)),
                fg: Color::Rgb(198, 202, 212),
                dim: Color::Rgb(120, 124, 136),
                accent: Color::Rgb(142, 170, 216),
                heading: Color::Rgb(230, 234, 244),
                code_fg: Color::Rgb(184, 196, 216),
                code_bg: Some(Color::Rgb(26, 30, 38)),
                quote: Color::Rgb(148, 156, 172),
                link: Color::Rgb(148, 170, 214),
                rule: Color::Rgb(72, 78, 92),
                syn_keyword: Color::Rgb(180, 152, 224),
                syn_string: Color::Rgb(152, 200, 180),
                syn_comment: Color::Rgb(104, 112, 124),
                syn_number: Color::Rgb(204, 172, 140),
                syn_type: Color::Rgb(132, 184, 224),
                syn_fn: Color::Rgb(140, 168, 232),
            },
            ThemeName::Plain => Self {
                bg: None,
                fg: Color::Reset,
                dim: Color::DarkGray,
                accent: Color::Yellow,
                heading: Color::Reset,
                code_fg: Color::Reset,
                code_bg: None,
                quote: Color::DarkGray,
                link: Color::Blue,
                rule: Color::DarkGray,
                syn_keyword: Color::Magenta,
                syn_string: Color::Green,
                syn_comment: Color::DarkGray,
                syn_number: Color::Yellow,
                syn_type: Color::Cyan,
                syn_fn: Color::Blue,
            },
        }
    }

    fn syn(self, color: Color) -> Style {
        let mut s = Style::default().fg(color);
        if let Some(bg) = self.code_bg {
            s = s.bg(bg);
        }
        s
    }

    pub fn keyword_style(self) -> Style {
        self.syn(self.syn_keyword).add_modifier(Modifier::BOLD)
    }
    pub fn string_style(self) -> Style {
        self.syn(self.syn_string)
    }
    pub fn comment_style(self) -> Style {
        self.syn(self.syn_comment).add_modifier(Modifier::ITALIC)
    }
    pub fn number_style(self) -> Style {
        self.syn(self.syn_number)
    }
    pub fn type_style(self) -> Style {
        self.syn(self.syn_type)
    }
    pub fn fn_style(self) -> Style {
        self.syn(self.syn_fn)
    }

    pub fn base_style(self) -> Style {
        let mut s = Style::default().fg(self.fg);
        if let Some(bg) = self.bg {
            s = s.bg(bg);
        }
        s
    }

    pub fn heading_style(self, level: u8) -> Style {
        let mut s = Style::default().fg(self.heading).add_modifier(Modifier::BOLD);
        if let Some(bg) = self.bg {
            s = s.bg(bg);
        }
        if level >= 3 {
            s = s.add_modifier(Modifier::DIM);
        }
        s
    }

    pub fn code_style(self) -> Style {
        let mut s = Style::default().fg(self.code_fg);
        if let Some(bg) = self.code_bg {
            s = s.bg(bg);
        }
        s
    }

    pub fn dim_style(self) -> Style {
        let mut s = Style::default().fg(self.dim);
        if let Some(bg) = self.bg {
            s = s.bg(bg);
        }
        s
    }

    pub fn quote_style(self) -> Style {
        let mut s = Style::default()
            .fg(self.quote)
            .add_modifier(Modifier::ITALIC);
        if let Some(bg) = self.bg {
            s = s.bg(bg);
        }
        s
    }

    pub fn link_style(self) -> Style {
        let mut s = Style::default()
            .fg(self.link)
            .add_modifier(Modifier::UNDERLINED);
        if let Some(bg) = self.bg {
            s = s.bg(bg);
        }
        s
    }

    pub fn rule_style(self) -> Style {
        let mut s = Style::default().fg(self.rule);
        if let Some(bg) = self.bg {
            s = s.bg(bg);
        }
        s
    }

    pub fn accent_style(self) -> Style {
        let mut s = Style::default().fg(self.accent);
        if let Some(bg) = self.bg {
            s = s.bg(bg);
        }
        s
    }
}
