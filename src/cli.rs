use std::path::PathBuf;

use clap::{Parser, ValueEnum};

use crate::theme::ThemeName;

/// Tootles — a reading-focused terminal markdown viewer.
#[derive(Debug, Parser)]
#[command(name = "tootles", version, about, long_about = None)]
pub struct Cli {
    /// Path to a markdown file. Pass `-` to read from stdin.
    pub path: PathBuf,

    /// Target column width for the reading measure.
    #[arg(long, default_value_t = 72, value_parser = parse_measure)]
    pub measure: u16,

    /// Color theme. Press `T` at runtime to cycle themes.
    #[arg(long, value_enum, default_value_t = ThemeArg::Dark)]
    pub theme: ThemeArg,

    /// Disable remembering reading position across runs.
    #[arg(long)]
    pub no_remember: bool,
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
