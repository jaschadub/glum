#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

use glum_lib::app::{self, Align, InitialState};
use glum_lib::cli::Cli;
use glum_lib::layout::LayoutName;
use glum_lib::positions::PositionStore;
use glum_lib::theme::ThemeName;
use glum_lib::watch::FileWatcher;

/// Maximum size of a markdown file we'll load. Refuses larger inputs to cap memory use.
const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024; // 64 MiB

fn main() -> ExitCode {
    match real_main() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("glum: {e:#}");
            ExitCode::from(1)
        }
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();

    let (path, source) = load_input(&cli.path)?;

    if !io::stdout().is_terminal() {
        anyhow::bail!("stdout is not a terminal; glum requires a TTY to render");
    }

    let store = if cli.no_remember {
        PositionStore::disabled()
    } else {
        PositionStore::open().unwrap_or_else(|e| {
            eprintln!("glum: position store unavailable: {e:#}");
            PositionStore::disabled()
        })
    };

    let display_name = app::display_name_for(&path);

    // Explicit --theme wins. Otherwise fall back to the remembered theme, then
    // to `dark` on first run.
    let theme = cli
        .theme
        .map(ThemeName::from)
        .or_else(|| store.theme().and_then(ThemeName::from_label))
        .unwrap_or(ThemeName::Dark);

    let layout = cli
        .layout
        .map(LayoutName::from)
        .or_else(|| store.layout().and_then(LayoutName::from_label))
        .unwrap_or(LayoutName::Minimal);

    let align = cli
        .align
        .map(Align::from)
        .or_else(|| store.align().and_then(Align::from_label))
        .unwrap_or(Align::Center);

    // Default is soft-wrap. --truncate-code flips it off; otherwise the
    // remembered preference wins, and first-run default is wrap.
    let wrap_code = if cli.truncate_code {
        false
    } else {
        store.wrap_code().unwrap_or(true)
    };

    // --follow only makes sense for a real file on disk. For stdin input
    // (path == "<stdin>") we silently fall through without a watcher.
    let watcher = if cli.follow && path.as_os_str() != "<stdin>" {
        match FileWatcher::start(&path) {
            Ok(w) => Some(w),
            Err(e) => {
                eprintln!("glum: --follow unavailable: {e:#}");
                None
            }
        }
    } else {
        None
    };

    let cfg = app::AppConfig {
        path,
        source,
        measure: cli.measure,
        theme,
        layout,
        align,
        wrap_code,
        store,
        display_name,
        initial: InitialState {
            search: cli.search,
            heading: cli.heading,
            reset_position: cli.reset_position,
            open_toc: cli.toc,
        },
        watcher,
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
