use std::path::PathBuf;

use clap::{Parser, ValueEnum};

use crate::layout::LayoutName;
use crate::theme::ThemeName;

/// Glum — a reading-focused terminal markdown viewer.
#[derive(Debug, Parser)]
#[command(name = "glum", version, about, long_about = None)]
pub struct Cli {
    /// Path to a markdown file. Pass `-` to read from stdin.
    pub path: PathBuf,

    /// Target column width for the reading measure.
    #[arg(long, default_value_t = 72, value_parser = parse_measure)]
    pub measure: u16,

    /// Color theme. Press `T` at runtime to cycle themes. If omitted, the
    /// last theme you used is restored (first-run default is `dark`).
    #[arg(long, value_enum)]
    pub theme: Option<ThemeArg>,

    /// Typographic layout. Press `L` at runtime to toggle. If omitted, the
    /// last layout you used is restored (first-run default is `minimal`).
    #[arg(long, value_enum)]
    pub layout: Option<LayoutArg>,

    /// Horizontal alignment of the reading column. `center` leaves symmetric
    /// margins for classic reader-mode feel; `left` anchors the column to the
    /// left edge so code blocks don't appear indented in wide terminals.
    /// Press `A` at runtime to toggle.
    #[arg(long, value_enum)]
    pub align: Option<AlignArg>,

    /// Open with a search pre-populated; jump to the first match. `n`/`N` to
    /// step through as usual.
    #[arg(short = 's', long = "search", value_name = "QUERY")]
    pub search: Option<String>,

    /// Jump to the first heading whose title contains this text
    /// (case-insensitive substring match against the TOC).
    #[arg(short = 'H', long = "heading", value_name = "TITLE")]
    pub heading: Option<String>,

    /// Ignore any remembered position and start at the top of the document.
    /// Future positions are still saved (unless `--no-remember` is given).
    #[arg(long)]
    pub reset_position: bool,

    /// Open with the table of contents overlay already visible.
    #[arg(long)]
    pub toc: bool,

    /// Disable remembering reading position across runs.
    #[arg(long)]
    pub no_remember: bool,

    /// Follow file changes: re-read and re-render when the file on disk is
    /// modified. Scroll position is preserved across reloads.
    #[arg(short = 'f', long = "follow")]
    pub follow: bool,

    /// Truncate long code lines with `…` instead of soft-wrapping them.
    /// Default is to soft-wrap so no code is hidden; pass this flag to
    /// get tight single-line-per-code-line layout. Press `W` at runtime
    /// to toggle.
    #[arg(long = "truncate-code")]
    pub truncate_code: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AlignArg {
    Center,
    Left,
    /// Anchors the reading column to the right margin. Useful as a column
    /// placement hint for RTL scripts; note that glum does not perform
    /// bidirectional layout — individual RTL glyphs still render in terminal
    /// order unless your terminal itself applies `BiDi`.
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum LayoutArg {
    Minimal,
    Vivid,
}

impl From<LayoutArg> for LayoutName {
    fn from(value: LayoutArg) -> Self {
        match value {
            LayoutArg::Minimal => Self::Minimal,
            LayoutArg::Vivid => Self::Vivid,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ThemeArg {
    Light,
    Dark,
    Sepia,
    Night,
    Plain,
}

impl From<ThemeArg> for ThemeName {
    fn from(value: ThemeArg) -> Self {
        match value {
            ThemeArg::Light => Self::Light,
            ThemeArg::Dark => Self::Dark,
            ThemeArg::Sepia => Self::Sepia,
            ThemeArg::Night => Self::Night,
            ThemeArg::Plain => Self::Plain,
        }
    }
}

fn parse_measure(s: &str) -> Result<u16, String> {
    let n: u16 = s.parse().map_err(|e| format!("invalid measure: {e}"))?;
    if !(20..=200).contains(&n) {
        return Err(format!("measure must be between 20 and 200 (got {n})"));
    }
    Ok(n)
}
