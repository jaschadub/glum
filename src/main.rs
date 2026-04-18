#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

use tootles_lib::app;
use tootles_lib::cli::Cli;
use tootles_lib::positions::PositionStore;
use tootles_lib::theme::ThemeName;

/// Maximum size of a markdown file we'll load. Refuses larger inputs to cap memory use.
const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024; // 64 MiB

fn main() -> ExitCode {
    match real_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("tootles: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();

    let (path, source) = load_input(&cli.path)?;

    if !io::stdout().is_terminal() {
        anyhow::bail!("stdout is not a terminal; tootles requires a TTY to render");
    }

    let store = if cli.no_remember {
        PositionStore::disabled()
    } else {
        PositionStore::open().unwrap_or_else(|e| {
            eprintln!("tootles: position store unavailable: {e:#}");
            PositionStore::disabled()
        })
    };

    let display_name = app::display_name_for(&path);

    let cfg = app::AppConfig {
        path,
        source,
        measure: cli.measure,
        theme: ThemeName::from(cli.theme),
        store,
        display_name,
    };

    app::run(cfg)
}

fn load_input(p: &Path) -> Result<(PathBuf, String)> {
    if p.as_os_str() == "-" {
        if io::stdin().is_terminal() {
            anyhow::bail!("refusing to read from a TTY stdin; pass a path instead");
        }
        let mut buf = Vec::new();
        io::stdin()
            .take(MAX_INPUT_BYTES + 1)
            .read_to_end(&mut buf)
            .context("reading stdin")?;
        if buf.len() as u64 > MAX_INPUT_BYTES {
            anyhow::bail!("stdin input exceeds {} MiB limit", MAX_INPUT_BYTES / (1024 * 1024));
        }
        let text = String::from_utf8(buf).context("stdin is not valid UTF-8")?;
        let synthetic = PathBuf::from("<stdin>");
        return Ok((synthetic, text));
    }

    let metadata = fs::metadata(p)
        .with_context(|| format!("reading {}", p.display()))?;
    if !metadata.is_file() {
        anyhow::bail!("{} is not a regular file", p.display());
    }
    if metadata.len() > MAX_INPUT_BYTES {
        anyhow::bail!(
            "{} is {} bytes which exceeds the {} MiB limit",
            p.display(),
            metadata.len(),
            MAX_INPUT_BYTES / (1024 * 1024)
        );
    }
    let text = fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
    let canonical = fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    Ok((canonical, text))
}
