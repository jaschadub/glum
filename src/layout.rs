//! Layout presets. A layout controls *typography* — heading hierarchy,
//! section decoration, vertical rhythm — independently of the color theme.
//!
//! Two presets ship:
//!
//! - `minimal` (default): understated. H1 = bold + thin rule underneath;
//!   H2–H6 differ only by weight and dim. Best for long prose where you want
//!   the text, not the chrome, to do the work.
//! - `vivid`: leans hard on hierarchy. H1 is ALL-CAPS, banded by heavy rules
//!   above and below. H2 is prefixed with `§`, H3 with `▸`, H4 with `›`.
//!   Indentation grows slightly at H4/H5/H6. Best for reference docs where
//!   you're scanning for section boundaries.

use ratatui::style::{Modifier, Style};

use crate::theme::Theme;

/// Name of a typographic layout preset. Toggle at runtime with `L`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutName {
    /// Understated typography: quiet hierarchy, single thin rule under H1.
    Minimal,
    /// Strong hierarchy: heading prefixes, heavy rules, per-level indent.
    Vivid,
}

impl LayoutName {
    /// Toggle between the two presets (used by the `L` keybinding).
    pub fn next(self) -> Self {
        match self {
            Self::Minimal => Self::Vivid,
            Self::Vivid => Self::Minimal,
        }
    }

    /// Short lowercase name used on the status bar and in persisted prefs.
    pub fn label(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Vivid => "vivid",
        }
    }

    /// Parse a label back into a `LayoutName`. Unknown labels return `None`.
    pub fn from_label(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "minimal" => Some(Self::Minimal),
            "vivid" => Some(Self::Vivid),
            _ => None,
        }
    }
}

/// Decorations a layout wants drawn around a heading. The renderer consumes
/// these to produce the actual `Line`s; this indirection keeps the render
/// pipeline a dumb consumer of layout-specific choices.
pub struct HeadingDecor {
    /// Blank lines inserted before the heading body.
    pub blank_before: u8,
    /// Blank lines inserted after the heading body.
    pub blank_after: u8,
    /// Optional horizontal rule drawn above the heading.
    pub top_rule: Option<RuleSpec>,
    /// Optional horizontal rule drawn below the heading.
    pub bottom_rule: Option<RuleSpec>,
    /// Prefix glyph + trailing space placed before the title (e.g. `"§ "`).
    pub prefix: String,
    /// Columns of left indent applied to the heading (first + continuation).
    pub indent: usize,
    /// Text style applied to the heading title.
    pub style: Style,
    /// Whether to uppercase the title text before rendering.
    pub uppercase: bool,
}

/// A full-width horizontal rule: a single repeat character plus the style
/// used to color it.
pub struct RuleSpec {
    /// Character repeated across the measure to form the rule.
    pub ch: char,
    /// Style applied to the rule.
    pub style: Style,
}

/// Look up heading decorations for the given layout, level (1–6), and
/// theme. The returned [`HeadingDecor`] fully describes the visual
/// treatment; the renderer only has to execute it.
pub fn decorate_heading(layout: LayoutName, level: u8, theme: Theme) -> HeadingDecor {
    match layout {
        LayoutName::Minimal => decor_minimal(level, theme),
        LayoutName::Vivid => decor_vivid(level, theme),
    }
}

fn decor_minimal(level: u8, theme: Theme) -> HeadingDecor {
    let mut style = Style::default()
        .fg(theme.heading)
        .add_modifier(Modifier::BOLD);
    if let Some(bg) = theme.bg {
        style = style.bg(bg);
    }
    if level >= 3 {
        style = style.add_modifier(Modifier::DIM);
    }
    HeadingDecor {
        blank_before: 0,
        blank_after: 1,
        top_rule: None,
        bottom_rule: if level == 1 {
            Some(RuleSpec {
                ch: '\u{2500}',
                style: theme.rule_style(),
            })
        } else {
            None
        },
        prefix: String::new(),
        indent: 0,
        style,
        uppercase: false,
    }
}

fn decor_vivid(level: u8, theme: Theme) -> HeadingDecor {
    let base_heading = |extra: Modifier| {
        let mut s = Style::default()
            .fg(theme.heading)
            .add_modifier(Modifier::BOLD)
            .add_modifier(extra);
        if let Some(bg) = theme.bg {
            s = s.bg(bg);
        }
        s
    };

    match level {
        1 => HeadingDecor {
            blank_before: 1,
            blank_after: 1,
            top_rule: Some(RuleSpec {
                ch: '\u{2501}', // ━
                style: theme.accent_style(),
            }),
            bottom_rule: Some(RuleSpec {
                ch: '\u{2501}',
                style: theme.accent_style(),
            }),
            prefix: "\u{276F} ".to_string(), // ❯
            indent: 2,
            style: base_heading(Modifier::empty()),
            uppercase: true,
        },
        2 => HeadingDecor {
            blank_before: 1,
            blank_after: 1,
            top_rule: None,
            bottom_rule: Some(RuleSpec {
                ch: '\u{2500}', // ─
                style: theme.rule_style(),
            }),
            prefix: "\u{00A7} ".to_string(), // §
            indent: 0,
            style: base_heading(Modifier::empty()),
            uppercase: false,
        },
        3 => HeadingDecor {
            blank_before: 1,
            blank_after: 0,
            top_rule: None,
            bottom_rule: None,
            prefix: "\u{25B8} ".to_string(), // ▸
            indent: 0,
            style: base_heading(Modifier::empty()),
            uppercase: false,
        },
        4 => HeadingDecor {
            blank_before: 1,
            blank_after: 0,
            top_rule: None,
            bottom_rule: None,
            prefix: "\u{203A} ".to_string(), // ›
            indent: 2,
            style: base_heading(Modifier::empty()),
            uppercase: false,
        },
        _ => HeadingDecor {
            blank_before: 0,
            blank_after: 0,
            top_rule: None,
            bottom_rule: None,
            prefix: "\u{00B7} ".to_string(), // ·
            indent: 4,
            style: base_heading(Modifier::ITALIC | Modifier::DIM),
            uppercase: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_cycles_round_trip() {
        assert_eq!(LayoutName::Minimal.next(), LayoutName::Vivid);
        assert_eq!(LayoutName::Vivid.next(), LayoutName::Minimal);
    }

    #[test]
    fn layout_labels_parse_back() {
        assert_eq!(LayoutName::from_label("minimal"), Some(LayoutName::Minimal));
        assert_eq!(LayoutName::from_label("VIVID"), Some(LayoutName::Vivid));
        assert_eq!(LayoutName::from_label("nope"), None);
    }
}
