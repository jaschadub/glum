//! Command-line argument definitions for the `glum` binary.
//!
//! Parsed with [`clap`]. The theme / layout / align flags use the library's
//! own enums directly ([`ThemeName`], [`LayoutName`], [`Align`]) via
//! `clap::ValueEnum`, so the CLI, completions, and runtime cycling can
//! never drift apart.

use std::path::PathBuf;

use clap::Parser;
use clap_complete::Shell;

use crate::app::Align;
use crate::layout::LayoutName;
use crate::theme::ThemeName;

/// Glum — a reading-focused terminal markdown viewer.
#[derive(Debug, Parser)]
#[command(name = "glum", version, about, long_about = None)]
pub struct Cli {
    /// Markdown file(s) to read. Multiple files open in tabs — switch with
    /// `]` / `[`. Pass `-` to read from stdin. Not required when
    /// `--generate-completions` or `--generate-man` is used, because those
    /// print to stdout and exit.
    #[arg(num_args = 0.., required_unless_present_any(["generate_completions", "generate_man"]))]
    pub paths: Vec<PathBuf>,

    /// Target column width for the reading measure.
    #[arg(long, default_value_t = 72, value_parser = parse_measure)]
    pub measure: u16,

    /// Color theme. Press `T` at runtime to cycle themes. If omitted, the
    /// last theme you used is restored (first-run default is `dark`).
    #[arg(long, value_enum)]
    pub theme: Option<ThemeName>,

    /// Typographic layout. Press `L` at runtime to toggle. If omitted, the
    /// last layout you used is restored (first-run default is `minimal`).
    #[arg(long, value_enum)]
    pub layout: Option<LayoutName>,

    /// Horizontal alignment of the reading column. `center` leaves symmetric
    /// margins for classic reader-mode feel; `left` anchors the column to the
    /// left edge so code blocks don't appear indented in wide terminals.
    /// Press `A` at runtime to toggle.
    #[arg(long, value_enum)]
    pub align: Option<Align>,

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

    /// Enable mouse wheel scrolling. Off by default because mouse capture
    /// disables the terminal's native click-and-drag text selection — which
    /// most readers still want for copying. Turn it on when you prefer
    /// scrolling with the wheel over using `j`/`k`/space.
    #[arg(long)]
    pub mouse: bool,

    /// Enable inline image preview (`i` key) for local images, using the
    /// terminal's best graphics protocol (kitty / sixel / iTerm2, with a
    /// half-block fallback).
    #[arg(long)]
    pub images: bool,

    /// Emit a shell completion script for the given shell on stdout, then
    /// exit. Redirect into the shell's completion directory — examples:
    /// `glum --generate-completions bash > ~/.local/share/bash-completion/completions/glum`,
    /// `glum --generate-completions zsh > "${fpath[1]}/_glum"`,
    /// `glum --generate-completions fish > ~/.config/fish/completions/glum.fish`.
    #[arg(long = "generate-completions", value_name = "SHELL")]
    pub generate_completions: Option<Shell>,

    /// Emit a roff-formatted man page on stdout, then exit.
    /// Typical use: `glum --generate-man > /usr/local/share/man/man1/glum.1`.
    #[arg(long = "generate-man")]
    pub generate_man: bool,
}

fn parse_measure(s: &str) -> Result<u16, String> {
    let n: u16 = s.parse().map_err(|e| format!("invalid measure: {e}"))?;
    if !(20..=200).contains(&n) {
        return Err(format!("measure must be between 20 and 200 (got {n})"));
    }
    Ok(n)
}
