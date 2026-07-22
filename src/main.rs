#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser};
use clap_complete::generate as generate_completions;

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

    // Generator shortcuts: emit completions or a man page to stdout and
    // exit before doing anything terminal-related. Order matters — these
    // can't require a PATH (`required_unless_present_any` handles that).
    if let Some(shell) = cli.generate_completions {
        let mut cmd = Cli::command();
        let bin = cmd.get_name().to_string();
        generate_completions(shell, &mut cmd, bin, &mut io::stdout());
        return Ok(());
    }
    if cli.generate_man {
        let cmd = Cli::command();
        let man = clap_mangen::Man::new(cmd);
        let mut out = Vec::new();
        man.render(&mut out).context("rendering man page")?;
        io::stdout()
            .write_all(&out)
            .context("writing man page to stdout")?;
        return Ok(());
    }

    // clap guaranteed at least one path via `required_unless_present_any`.
    let mut docs: Vec<app::DocInput> = Vec::with_capacity(cli.paths.len());
    for p in &cli.paths {
        let (path, source) = load_input(p)?;
        let display_name = app::display_name_for(&path);
        docs.push(app::DocInput {
            path,
            source,
            display_name,
            offset: 0,
        });
    }
    let (path, source, display_name) = {
        let d = &docs[0];
        (d.path.clone(), d.source.clone(), d.display_name.clone())
    };

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

    // Explicit --theme wins. Otherwise fall back to the remembered theme,
    // then to a first-run default picked from the terminal's advertised
    // background ($COLORFGBG) so light terminals don't open with dark glum.
    let theme = cli
        .theme
        .or_else(|| store.theme().and_then(ThemeName::from_label))
        .unwrap_or_else(adaptive_first_run_theme);

    let layout = cli
        .layout
        .or_else(|| store.layout().and_then(LayoutName::from_label))
        .unwrap_or(LayoutName::Minimal);

    let align = cli
        .align
        .or_else(|| store.align().and_then(Align::from_label))
        .unwrap_or(Align::Center);

    // Default is soft-wrap. --truncate-code flips it off; otherwise the
    // remembered preference wins, and first-run default is wrap.
    let wrap_code = if cli.truncate_code {
        false
    } else {
        store.wrap_code().unwrap_or(true)
    };

    // Image preview needs a terminal-graphics handshake, which must happen
    // before raw mode / the alternate screen. Query failure falls back to
    // half-block rendering, which works everywhere.
    let picker = if cli.images {
        Some(
            ratatui_image::picker::Picker::from_query_stdio()
                .unwrap_or_else(|_| ratatui_image::picker::Picker::from_fontsize((8, 16))),
        )
    } else {
        None
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
        docs,
        current: 0,
        follow: cli.follow,
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
        mouse: cli.mouse,
        picker,
    };

    app::run(cfg)
}

/// First-run theme pick. Ask the terminal for its actual background color
/// via OSC 11 (`terminal-colorsaurus` handles the query, tmux passthrough,
/// and timeouts) — must run before raw mode / alternate screen. Falls back
/// to `$COLORFGBG`, then `dark`.
fn adaptive_first_run_theme() -> ThemeName {
    if let Ok(mode) =
        terminal_colorsaurus::theme_mode(terminal_colorsaurus::QueryOptions::default())
    {
        return match mode {
            terminal_colorsaurus::ThemeMode::Light => ThemeName::Light,
            terminal_colorsaurus::ThemeMode::Dark => ThemeName::Dark,
        };
    }
    if let Ok(val) = std::env::var("COLORFGBG") {
        // Last token is the background ANSI index; terminals use 7 or 15 for
        // light backgrounds.
        if let Some(bg) = val
            .rsplit(';')
            .next()
            .and_then(|s| s.trim().parse::<u32>().ok())
        {
            if matches!(bg, 7 | 15) {
                return ThemeName::Light;
            }
            return ThemeName::Dark;
        }
    }
    ThemeName::Dark
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
            anyhow::bail!(
                "stdin input exceeds {} MiB limit",
                MAX_INPUT_BYTES / (1024 * 1024)
            );
        }
        let text = String::from_utf8(buf).context("stdin is not valid UTF-8")?;
        let synthetic = PathBuf::from("<stdin>");
        return Ok((synthetic, text));
    }

    let metadata = fs::metadata(p).with_context(|| format!("reading {}", p.display()))?;
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
