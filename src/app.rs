//! TUI application loop: rendering, paging, theme cycling, TOC, search.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseEvent, MouseEventKind,
};
use crossterm::{cursor, execute, terminal};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Terminal;

use crate::clipboard;
use crate::highlight::highlight_line;
use crate::layout::LayoutName;
use crate::positions::PositionStore;
use crate::render::{self, CodeBlockEntry, Rendered, TocEntry};
use crate::theme::{Theme, ThemeName};
use crate::watch::FileWatcher;

/// Full configuration needed to launch the TUI. Built by `main.rs` from the
/// parsed CLI plus the persistence store, then handed to [`run`].
pub struct AppConfig {
    /// Canonical path of the file being read (or `<stdin>` for piped input).
    pub path: PathBuf,
    /// File contents already loaded into memory; the renderer reads this
    /// string, not the path.
    pub source: String,
    /// Target reading column width (clap-validated to 20..=200).
    pub measure: u16,
    /// Initial color theme (may be restored from the persistence store).
    pub theme: ThemeName,
    /// Initial typographic layout.
    pub layout: LayoutName,
    /// Initial horizontal alignment of the reading column.
    pub align: Align,
    /// When true, long code lines soft-wrap; when false, they truncate with `…`.
    pub wrap_code: bool,
    /// Persistence handle for reading position and remembered preferences.
    pub store: PositionStore,
    /// Path shown in the footer (typically the relative-to-cwd form).
    pub display_name: String,
    /// Optional opening behavior set from CLI flags.
    pub initial: InitialState,
    /// Enabled when `--follow` is active and the input is a real file.
    pub watcher: Option<FileWatcher>,
    /// When true, enable mouse capture so the wheel scrolls the reader.
    /// Comes at the cost of losing the terminal's native drag-select, so
    /// it's opt-in via `--mouse`.
    pub mouse: bool,
}

/// Horizontal alignment of the reading column within the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// Symmetric margins — classic reader-mode feel.
    Center,
    /// Column anchored to the left (leaves a 2-col gutter on the left).
    Left,
    /// Anchors the column to the right margin. Column placement only — does
    /// not apply bidirectional text layout to RTL scripts.
    Right,
}

impl Align {
    /// Cycle: center → left → right → center.
    pub fn toggle(self) -> Self {
        match self {
            Self::Center => Self::Left,
            Self::Left => Self::Right,
            Self::Right => Self::Center,
        }
    }

    /// Short lowercase name for the status bar and persisted prefs.
    pub fn label(self) -> &'static str {
        match self {
            Self::Center => "center",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    /// Parse a label back into an `Align`. Accepts `centre` as an alias.
    pub fn from_label(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "center" | "centre" => Some(Self::Center),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

impl From<crate::cli::AlignArg> for Align {
    fn from(a: crate::cli::AlignArg) -> Self {
        match a {
            crate::cli::AlignArg::Center => Self::Center,
            crate::cli::AlignArg::Left => Self::Left,
            crate::cli::AlignArg::Right => Self::Right,
        }
    }
}

/// Table width budget: how many display columns tables may use. The reading
/// `measure` is a prose legibility choice — artificially narrowing a wide
/// table to it forces headers like "passed" to break character-by-character.
/// So tables are allowed to grow toward the terminal width, capped to keep
/// column arithmetic sane and to leave a small visual gutter.
fn table_budget(measure: u16, term_w: u16) -> usize {
    const MARGIN: u16 = 4;
    const CAP: u16 = 200;
    let usable = term_w.saturating_sub(MARGIN).min(CAP);
    usable.max(measure) as usize
}

/// Optional opening-state overrides set from CLI flags — applied once after
/// the initial render so the reader lands where the user asked.
#[derive(Debug, Default, Clone)]
pub struct InitialState {
    /// Pre-populated search query (opens the search overlay with matches).
    pub search: Option<String>,
    /// Case-insensitive substring of a heading title to jump to.
    pub heading: Option<String>,
    /// If true, ignore any saved scroll position and start at the top.
    pub reset_position: bool,
    /// If true, open with the TOC overlay visible.
    pub open_toc: bool,
}

enum Mode {
    Reading,
    Toc {
        selected: usize,
    },
    Search {
        input: String,
    },
    Help,
    /// Inline line-pick: the selected source line of a specific code block is
    /// highlighted in the main view; j/k move, y/Enter copies that line.
    LinePick {
        block_idx: usize,
        line_idx: usize,
    },
    /// Full-screen raw view of a code block: no wrap, horizontal pan, per-line
    /// cursor — lets the reader see full long lines and copy a single one.
    RawCode {
        block_idx: usize,
        line_idx: usize,
        h_off: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusKind {
    Info,
    Success,
}

struct App {
    cfg: AppConfig,
    theme: Theme,
    theme_name: ThemeName,
    layout_name: LayoutName,
    align: Align,
    wrap_code: bool,
    rendered: Rendered,
    offset: usize,
    last_viewport_h: u16,
    /// Last terminal width used for rendering. Tables are sized against
    /// this (not just the prose measure), so a meaningful resize triggers
    /// a re-render to recompute table column widths against the new budget.
    last_render_width: u16,
    mode: Mode,
    search_matches: Vec<usize>,
    search_cursor: usize,
    status: Option<(String, std::time::Instant, StatusKind)>,
    /// Time of the last detected filesystem change; used to settle bursty
    /// editor writes before triggering a reload.
    pending_reload_at: Option<std::time::Instant>,
    /// Set by the `e` keybind; consumed by the main loop, which owns the
    /// ratatui `Terminal` handle needed to suspend/restore the TUI.
    pending_editor: bool,
    /// When `true`, the raw-code overlay prepends each row with a dim source
    /// line-number gutter. Toggled inside the overlay with `#`.
    raw_show_line_nums: bool,
}

impl App {
    fn new(cfg: AppConfig) -> Self {
        let theme_name = cfg.theme;
        let theme = Theme::resolve(theme_name);
        let layout_name = cfg.layout;
        let align = cfg.align;
        let wrap_code = cfg.wrap_code;
        let term_w = terminal::size().map_or(cfg.measure, |(w, _)| w);
        let rendered = render::render(
            &cfg.source,
            cfg.measure as usize,
            table_budget(cfg.measure, term_w),
            theme,
            layout_name,
            wrap_code,
        );
        let saved_offset = if cfg.initial.reset_position {
            0
        } else {
            cfg.store
                .get(&cfg.path)
                .map_or(0, |e| e.offset)
                .min(rendered.lines.len().saturating_sub(1))
        };
        let mut app = Self {
            cfg,
            theme,
            theme_name,
            layout_name,
            align,
            wrap_code,
            rendered,
            offset: saved_offset,
            last_viewport_h: 0,
            last_render_width: term_w,
            mode: Mode::Reading,
            search_matches: Vec::new(),
            search_cursor: 0,
            status: None,
            pending_reload_at: None,
            pending_editor: false,
            raw_show_line_nums: true,
        };
        app.apply_initial();
        app
    }

    /// Re-read the source file and re-render. Preserves the scroll offset
    /// where possible; clamps if the file has shrunk.
    fn reload_from_disk(&mut self) {
        match std::fs::read_to_string(&self.cfg.path) {
            Ok(text) => {
                self.cfg.source = text;
                self.rendered = render::render(
                    &self.cfg.source,
                    self.cfg.measure as usize,
                    table_budget(self.cfg.measure, self.last_render_width),
                    self.theme,
                    self.layout_name,
                    self.wrap_code,
                );
                self.offset = self.offset.min(self.max_offset());
                // Invalidate any pinned search match (line indices are stale).
                if !self.search_matches.is_empty() {
                    self.search_matches.clear();
                    self.search_cursor = 0;
                }
                self.set_status("reloaded");
            }
            Err(e) => {
                self.set_status(format!("reload failed: {e}"));
            }
        }
    }

    /// Source-file line of the heading nearest above the current viewport.
    /// Used by the external-editor handoff so the editor lands roughly where
    /// the reader was. Falls back to line 1 when there are no headings.
    fn nearest_heading_source_line(&self) -> usize {
        if self.rendered.toc.is_empty() {
            return 1;
        }
        let idx = current_toc_index(&self.rendered.toc, self.offset);
        self.rendered
            .toc
            .get(idx)
            .map_or(1, |e| e.source_line.max(1))
    }

    /// Apply CLI-provided opening state: jump to heading, run search, open TOC.
    fn apply_initial(&mut self) {
        if let Some(title) = self.cfg.initial.heading.clone() {
            if let Some(line) = find_heading(&self.rendered.toc, &title) {
                self.jump_to(line);
            } else {
                self.set_status(format!("no heading matches \"{title}\""));
            }
        }
        if let Some(query) = self.cfg.initial.search.clone() {
            // Pre-seed the same UX as if the user had typed `/query`: leave
            // the search prompt visible with the query filled in and matches
            // live. Enter commits to reading mode, Esc cancels.
            self.update_matches(&query);
            if let Some(&first) = self.search_matches.first() {
                self.jump_to(first);
            }
            self.mode = Mode::Search { input: query };
        }
        if self.cfg.initial.open_toc && !self.rendered.toc.is_empty() {
            // --toc takes precedence over an opened search prompt.
            let selected = current_toc_index(&self.rendered.toc, self.offset);
            self.mode = Mode::Toc { selected };
        }
    }

    fn total_lines(&self) -> usize {
        self.rendered.lines.len()
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), std::time::Instant::now(), StatusKind::Info));
    }

    /// Same as `set_status` but marks the message as a success so the footer
    /// briefly flashes it (reversed accent) before fading to the normal tone.
    fn set_status_success(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), std::time::Instant::now(), StatusKind::Success));
    }

    fn jump_to(&mut self, line: usize) {
        self.offset = line.min(self.max_offset());
    }

    fn page_size(&self) -> usize {
        // Reserve 2 lines for footer/status, overlap by 2 for orientation.
        let body = self.last_viewport_h.saturating_sub(2) as usize;
        body.saturating_sub(2).max(1)
    }

    /// Largest `offset` that still keeps the last document line visible at
    /// the bottom of the viewport. Scrolling past this would leave empty
    /// rows below the document — `percent()` already hits 100% there, and
    /// the standard pager contract is to stop at that point rather than
    /// letting the user drift into blank space.
    ///
    /// Before the first draw `last_viewport_h` is 0; fall back to
    /// `total - 1` so pre-draw clamps (restored saved offset, jump-to-line)
    /// don't get collapsed to zero.
    fn max_offset(&self) -> usize {
        let total = self.total_lines();
        let body = self.last_viewport_h.saturating_sub(2) as usize;
        if body == 0 {
            total.saturating_sub(1)
        } else {
            total.saturating_sub(body)
        }
    }

    fn scroll(&mut self, delta: isize) {
        let max = self.max_offset();
        let new = (self.offset as isize + delta).clamp(0, max as isize) as usize;
        if new != self.offset {
            self.offset = new;
        }
    }

    fn cycle_theme(&mut self) {
        self.theme_name = self.theme_name.next();
        self.theme = Theme::resolve(self.theme_name);
        self.re_render();
        // Best-effort persistence — a store write failure should not interrupt
        // reading, so we swallow the error and keep going.
        self.cfg.store.set_theme(self.theme_name.label()).ok();
        self.set_status(format!("theme: {}", self.theme_name.label()));
    }

    fn cycle_layout(&mut self) {
        self.layout_name = self.layout_name.next();
        self.re_render();
        self.cfg.store.set_layout(self.layout_name.label()).ok();
        self.set_status(format!("layout: {}", self.layout_name.label()));
    }

    fn toggle_align(&mut self) {
        self.align = self.align.toggle();
        self.cfg.store.set_align(self.align.label()).ok();
        self.set_status(format!("align: {}", self.align.label()));
    }

    fn re_render(&mut self) {
        self.rendered = render::render(
            &self.cfg.source,
            self.cfg.measure as usize,
            table_budget(self.cfg.measure, self.last_render_width),
            self.theme,
            self.layout_name,
            self.wrap_code,
        );
        self.offset = self.offset.min(self.max_offset());
    }

    fn toggle_wrap_code(&mut self) {
        self.wrap_code = !self.wrap_code;
        self.re_render();
        self.cfg.store.set_wrap_code(self.wrap_code).ok();
        self.set_status(if self.wrap_code {
            "code: wrap"
        } else {
            "code: truncate"
        });
    }

    fn percent(&self) -> u16 {
        let total = self.total_lines();
        if total <= 1 {
            return 100;
        }
        let visible_end =
            (self.offset + self.last_viewport_h.saturating_sub(2) as usize).min(total);
        ((visible_end as f64 / total as f64) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u16
    }

    /// Recompute match line indices for `needle`. Keeps the current cursor if
    /// possible, otherwise snaps to match 0. Called live as the user types.
    fn update_matches(&mut self, needle: &str) {
        self.search_matches.clear();
        if needle.is_empty() {
            self.search_cursor = 0;
            return;
        }
        let n_lower = needle.to_lowercase();
        for (i, line) in self.rendered.lines.iter().enumerate() {
            let s = line.to_string().to_lowercase();
            if s.contains(&n_lower) {
                self.search_matches.push(i);
            }
        }
        self.search_cursor = self
            .search_cursor
            .min(self.search_matches.len().saturating_sub(1));
    }

    /// Run a committed search: update matches and scroll to the first one.
    fn commit_search(&mut self, needle: &str) {
        self.update_matches(needle);
        if let Some(&first) = self.search_matches.first() {
            self.search_cursor = 0;
            self.jump_to(first);
        } else if !needle.is_empty() {
            self.set_status("no matches");
        }
    }

    fn clear_search(&mut self) {
        self.search_matches.clear();
        self.search_cursor = 0;
    }

    /// Index of the code block to act on for the current viewport — same
    /// selection logic as `pick_code_block` but returns the `Vec` index so
    /// mode state can hold a stable reference.
    fn current_code_block_idx(&self) -> Option<usize> {
        let view_top = self.offset;
        let view_bottom = self
            .offset
            .saturating_add(self.last_viewport_h.saturating_sub(1) as usize);
        pick_code_block_idx(&self.rendered.code_blocks, view_top, view_bottom)
    }

    /// Default source-line index when entering a line-picker: the first line
    /// whose visual span starts at or below the viewport top — so the cursor
    /// lands on something the reader can already see. Falls back to 0.
    fn initial_line_idx(&self, block_idx: usize) -> usize {
        let block = &self.rendered.code_blocks[block_idx];
        if block.line_visuals.is_empty() {
            return 0;
        }
        block
            .line_visuals
            .iter()
            .position(|(vs, _)| *vs >= self.offset)
            .unwrap_or(0)
            .min(block.line_visuals.len() - 1)
    }

    fn enter_line_pick(&mut self) {
        let Some(block_idx) = self.current_code_block_idx() else {
            self.set_status("no code blocks");
            return;
        };
        let line_idx = self.initial_line_idx(block_idx);
        self.mode = Mode::LinePick {
            block_idx,
            line_idx,
        };
        self.ensure_code_line_visible(block_idx, line_idx);
    }

    fn enter_raw_code(&mut self) {
        let Some(block_idx) = self.current_code_block_idx() else {
            self.set_status("no code blocks");
            return;
        };
        let line_idx = self.initial_line_idx(block_idx);
        self.mode = Mode::RawCode {
            block_idx,
            line_idx,
            h_off: 0,
        };
    }

    /// Scroll the reader so the visual rows of `(block_idx, line_idx)` are
    /// inside the viewport. Used by `LinePick` when the user moves the cursor.
    fn ensure_code_line_visible(&mut self, block_idx: usize, line_idx: usize) {
        let block = &self.rendered.code_blocks[block_idx];
        let Some(&(vs, ve)) = block.line_visuals.get(line_idx) else {
            return;
        };
        let body_h = self.last_viewport_h.saturating_sub(2) as usize;
        if body_h == 0 {
            return;
        }
        if ve >= self.offset + body_h {
            self.offset = ve + 1 - body_h;
        }
        if vs < self.offset {
            self.offset = vs;
        }
        self.offset = self.offset.min(self.max_offset());
    }

    /// Copy a single source line of a code block to the clipboard.
    fn copy_source_line(&mut self, block_idx: usize, line_idx: usize) {
        if clipboard::is_ssh_session() {
            self.set_status("copy unavailable in SSH session");
            return;
        }
        let block = &self.rendered.code_blocks[block_idx];
        let Some(line) = block.code.split('\n').nth(line_idx) else {
            self.set_status("no such line");
            return;
        };
        let payload = line.to_string();
        let total = block.line_visuals.len().max(1);
        let pos = line_idx + 1;
        match clipboard::copy(&payload) {
            Ok(Some(n)) => self.set_status_success(format!("copied line {pos}/{total} — {n}B")),
            Ok(None) => self.set_status("line too large to copy"),
            Err(_) => self.set_status("copy failed"),
        }
    }

    /// Copy the code block currently in view (or nearest above if none are
    /// on-screen) to the system clipboard via OSC 52.
    fn copy_current_code_block(&mut self) {
        if clipboard::is_ssh_session() {
            // OSC 52 often doesn't survive SSH + tmux; we don't advertise the
            // hint in SSH sessions, so pressing `y` there would silently lie
            // about success. Surface the reason instead.
            self.set_status("copy unavailable in SSH session");
            return;
        }
        let Some(block_idx) = self.current_code_block_idx() else {
            self.set_status("no code blocks");
            return;
        };
        self.copy_whole_block(block_idx);
    }

    fn copy_whole_block(&mut self, block_idx: usize) {
        if clipboard::is_ssh_session() {
            self.set_status("copy unavailable in SSH session");
            return;
        }
        let block = &self.rendered.code_blocks[block_idx];
        match clipboard::copy(&block.code) {
            Ok(Some(n)) => self.set_status_success(format!("copied {n} bytes ({})", block.lang)),
            Ok(None) => self.set_status("block too large to copy"),
            Err(_) => self.set_status("copy failed"),
        }
    }

    fn advance_search(&mut self, forward: bool) {
        if self.search_matches.is_empty() {
            return;
        }
        let len = self.search_matches.len();
        if forward {
            self.search_cursor = (self.search_cursor + 1) % len;
        } else {
            self.search_cursor = (self.search_cursor + len - 1) % len;
        }
        let target = self.search_matches[self.search_cursor];
        self.jump_to(target);
    }
}

/// Entry point: take over the terminal, run the reader loop until the user
/// quits (or `Ctrl-C`), and restore the terminal on exit. Installs a panic
/// hook so a crash inside the TUI still exits raw mode cleanly. Returns
/// `Err` only on irrecoverable terminal I/O failure.
pub fn run(cfg: AppConfig) -> Result<()> {
    // Install a panic hook so a crash in the TUI still restores the terminal
    // before the default hook prints the message. This prevents a panic from
    // leaving the user in raw mode with no cursor.
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_raw_terminal();
        prev_hook(info);
    }));

    let mouse = cfg.mouse;
    let mut guard = TerminalGuard::new(mouse)?;
    let result = run_loop(&mut guard.terminal, cfg);
    // TerminalGuard::drop will restore the terminal whether we succeeded or errored.
    drop(guard);

    // Leave the panic hook in place: glum is a CLI, main() will exit promptly.
    result
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
    mouse: bool,
}

impl TerminalGuard {
    fn new(mouse: bool) -> Result<Self> {
        terminal::enable_raw_mode().context("enabling raw mode")?;
        let mut stdout = io::stdout();
        if let Err(e) = execute!(
            stdout,
            terminal::EnterAlternateScreen,
            cursor::Hide,
            event::EnableBracketedPaste,
        ) {
            terminal::disable_raw_mode().ok();
            return Err(anyhow::Error::from(e).context("entering alternate screen"));
        }
        if mouse {
            // Best-effort: if the terminal rejects mouse capture we still
            // want to run, just without wheel scrolling.
            execute!(stdout, EnableMouseCapture).ok();
        }
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("building terminal")?;
        Ok(Self { terminal, mouse })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        terminal::disable_raw_mode().ok();
        if self.mouse {
            execute!(self.terminal.backend_mut(), DisableMouseCapture).ok();
        }
        execute!(
            self.terminal.backend_mut(),
            event::DisableBracketedPaste,
            terminal::LeaveAlternateScreen,
            cursor::Show,
        )
        .ok();
        self.terminal.show_cursor().ok();
    }
}

/// Suspend the TUI, run `$VISUAL` / `$EDITOR` (default `vi`) on the current
/// file, then resume. Called from the main loop so we can invalidate
/// ratatui's diff buffer (`terminal.clear()`) after re-entering the alternate
/// screen — without that, the reader comes back blank until the user types a
/// key that forces a state change.
fn run_external_editor(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> Result<()> {
    if app.cfg.path.as_os_str() == "<stdin>" {
        app.set_status("cannot edit stdin");
        return Ok(());
    }
    let editor_raw = std::env::var("VISUAL")
        .ok()
        .or_else(|| std::env::var("EDITOR").ok())
        .unwrap_or_else(|| "vi".to_string());
    // Handle `$EDITOR="nvim --clean"` etc. by splitting on whitespace; the
    // first token is the command, the rest are prefix args.
    let mut parts = editor_raw.split_whitespace();
    let cmd_name = parts.next().unwrap_or("vi").to_string();
    let prefix_args: Vec<String> = parts.map(str::to_string).collect();
    let line = app.nearest_heading_source_line();

    // --- Suspend ---
    terminal::disable_raw_mode().ok();
    if app.cfg.mouse {
        execute!(terminal.backend_mut(), DisableMouseCapture).ok();
    }
    execute!(
        terminal.backend_mut(),
        event::DisableBracketedPaste,
        terminal::LeaveAlternateScreen,
        cursor::Show,
    )?;

    let mut cmd = Command::new(&cmd_name);
    cmd.args(&prefix_args);
    if uses_plus_line_arg(&cmd_name) {
        cmd.arg(format!("+{line}"));
    }
    cmd.arg(&app.cfg.path);
    let status = cmd.status();

    // --- Resume ---
    terminal::enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        terminal::EnterAlternateScreen,
        cursor::Hide,
        event::EnableBracketedPaste,
    )?;
    if app.cfg.mouse {
        execute!(terminal.backend_mut(), EnableMouseCapture).ok();
    }
    // Reset ratatui's last-known frame so the very next draw is a full
    // repaint — we changed terminal state behind its back.
    terminal.clear()?;

    match status {
        Ok(s) if s.success() => {
            app.reload_from_disk();
            app.set_status_success(format!("edited in {cmd_name}"));
        }
        Ok(_) => app.set_status(format!("{cmd_name}: exited with error")),
        Err(e) => app.set_status(format!("{cmd_name} failed: {e}")),
    }
    Ok(())
}

/// Editors that accept the classic `+<line>` cursor-position argument. Anything
/// outside this list is invoked without a line hint — better than passing a
/// flag the editor will treat as a filename.
fn uses_plus_line_arg(cmd: &str) -> bool {
    let base = std::path::Path::new(cmd)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    matches!(
        base,
        "vi" | "vim"
            | "nvim"
            | "gvim"
            | "mvim"
            | "nano"
            | "pico"
            | "ex"
            | "view"
            | "emacs"
            | "emacsclient"
            | "joe"
            | "ne"
            | "mg"
            | "micro"
            | "kak"
            | "helix"
            | "hx"
    )
}

/// Emergency terminal restore for use inside a panic hook, which has no access
/// to the `TerminalGuard` instance.
fn restore_raw_terminal() -> io::Result<()> {
    terminal::disable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        event::DisableBracketedPaste,
        terminal::LeaveAlternateScreen,
        cursor::Show,
    )?;
    Ok(())
}

/// Settle window: wait this long after the last filesystem event before
/// reloading, so that multi-step writes (atomic rename + modify) collapse
/// into a single reload.
const RELOAD_SETTLE: Duration = Duration::from_millis(120);

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, cfg: AppConfig) -> Result<()> {
    let mut app = App::new(cfg);

    // Shorter poll window when following a file so filesystem events are
    // noticed quickly; otherwise keep the longer window for lower idle CPU.
    let poll_interval = if app.cfg.watcher.is_some() {
        Duration::from_millis(100)
    } else {
        Duration::from_millis(500)
    };

    loop {
        terminal.draw(|f| draw(f, &mut app))?;

        // Pending editor request: handled here (not inside handle_key) because
        // suspending the TUI needs the `terminal` handle, and re-entering the
        // alternate screen must be followed by `terminal.clear()` to reset
        // ratatui's diff buffer — otherwise the screen stays blank on return.
        if app.pending_editor {
            app.pending_editor = false;
            if let Err(e) = run_external_editor(terminal, &mut app) {
                app.set_status(format!("editor error: {e}"));
            }
            continue;
        }

        if event::poll(poll_interval)? {
            match event::read()? {
                Event::Key(key)
                    if (key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat)
                        && handle_key(&mut app, key)? =>
                {
                    break;
                }
                Event::Resize(w, h) => {
                    app.last_viewport_h = h;
                    // Re-render on any width change so table column widths
                    // get recomputed against the new terminal budget. The
                    // rendered output itself doesn't depend on height, so
                    // height-only resizes could skip this — but re_render is
                    // cheap and makes the common "drag to resize" path feel
                    // consistent.
                    if w != app.last_render_width {
                        app.last_render_width = w;
                        app.re_render();
                    }
                }
                Event::Mouse(m) => handle_mouse(&mut app, m),
                _ => {}
            }
        }

        // File-change handling (only when --follow is active).
        if let Some(watcher) = app.cfg.watcher.as_ref() {
            if watcher.drain() {
                app.pending_reload_at = Some(std::time::Instant::now());
            }
        }
        if let Some(at) = app.pending_reload_at {
            if at.elapsed() >= RELOAD_SETTLE {
                app.pending_reload_at = None;
                app.reload_from_disk();
            }
        }

        // Fade status after a couple seconds.
        if let Some((_, at, _)) = app.status {
            if at.elapsed() > Duration::from_secs(3) {
                app.status = None;
            }
        }
    }

    // Persist position on exit.
    app.cfg.store.set(&app.cfg.path, app.offset).ok();
    Ok(())
}

/// Translate wheel events into scroll deltas. In `RawCode` mode, Shift+wheel
/// pans horizontally — the rest of the time the wheel scrolls the reader
/// (or moves the line cursor inside `LinePick`).
fn handle_mouse(app: &mut App, ev: MouseEvent) {
    let step: isize = 3;
    match (ev.kind, &app.mode) {
        (MouseEventKind::ScrollUp, Mode::LinePick { .. }) => {
            // Synthesize a `k` press-equivalent for line-pick.
            move_line_pick(app, -1);
        }
        (MouseEventKind::ScrollDown, Mode::LinePick { .. }) => {
            move_line_pick(app, 1);
        }
        (MouseEventKind::ScrollUp, Mode::RawCode { .. }) => move_raw_code(app, -1, 0),
        (MouseEventKind::ScrollDown, Mode::RawCode { .. }) => move_raw_code(app, 1, 0),
        (MouseEventKind::ScrollLeft, Mode::RawCode { .. }) => move_raw_code(app, 0, -8),
        (MouseEventKind::ScrollRight, Mode::RawCode { .. }) => move_raw_code(app, 0, 8),
        (MouseEventKind::ScrollUp, _) => app.scroll(-step),
        (MouseEventKind::ScrollDown, _) => app.scroll(step),
        _ => {}
    }
}

/// Shared line-cursor mover for `LinePick` (mouse + keyboard paths share it).
fn move_line_pick(app: &mut App, delta: isize) {
    let Mode::LinePick {
        block_idx,
        line_idx,
    } = &app.mode
    else {
        return;
    };
    let block_idx = *block_idx;
    let old = *line_idx;
    let len = app.rendered.code_blocks[block_idx].line_visuals.len();
    if len == 0 {
        return;
    }
    let new = (old as isize + delta).clamp(0, len as isize - 1) as usize;
    app.mode = Mode::LinePick {
        block_idx,
        line_idx: new,
    };
    app.ensure_code_line_visible(block_idx, new);
}

fn move_raw_code(app: &mut App, dy: isize, dx: isize) {
    let Mode::RawCode {
        block_idx,
        line_idx,
        h_off,
    } = &app.mode
    else {
        return;
    };
    let block_idx = *block_idx;
    let block = &app.rendered.code_blocks[block_idx];
    let total = block.code.split('\n').count().max(1);
    let max_w = block
        .code
        .split('\n')
        .map(|l| unicode_width::UnicodeWidthStr::width(l.replace('\t', "    ").as_str()))
        .max()
        .unwrap_or(0);
    let new_line = (*line_idx as isize + dy).clamp(0, total as isize - 1) as usize;
    let new_off = (*h_off as isize + dx).clamp(0, max_w.saturating_sub(1) as isize) as usize;
    app.mode = Mode::RawCode {
        block_idx,
        line_idx: new_line,
        h_off: new_off,
    };
}

fn handle_key(app: &mut App, key: KeyEvent) -> Result<bool> {
    // Global: ctrl-c always quits.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Ok(true);
    }

    match &mut app.mode {
        Mode::Reading => handle_key_reading(app, key),
        Mode::Toc { .. } => handle_key_toc(app, key),
        Mode::Search { .. } => handle_key_search(app, key),
        Mode::LinePick { .. } => handle_key_line_pick(app, key),
        Mode::RawCode { .. } => handle_key_raw_code(app, key),
        Mode::Help => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('q' | '?')) {
                app.mode = Mode::Reading;
            }
            Ok(false)
        }
    }
}

fn handle_key_reading(app: &mut App, key: KeyEvent) -> Result<bool> {
    let page = app.page_size() as isize;
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Char('j') | KeyCode::Down => app.scroll(1),
        KeyCode::Char('k') | KeyCode::Up => app.scroll(-1),
        KeyCode::Char(' ') | KeyCode::PageDown => app.scroll(page),
        KeyCode::Char('b') | KeyCode::PageUp => app.scroll(-page),
        KeyCode::Char('d') => app.scroll(page / 2),
        KeyCode::Char('u') => app.scroll(-(page / 2)),
        KeyCode::Char('g') | KeyCode::Home => app.jump_to(0),
        KeyCode::Char('G') | KeyCode::End => app.jump_to(app.total_lines()),
        KeyCode::Char('t') => {
            if app.rendered.toc.is_empty() {
                app.set_status("no headings");
            } else {
                let selected = current_toc_index(&app.rendered.toc, app.offset);
                app.mode = Mode::Toc { selected };
            }
        }
        KeyCode::Char('T') => app.cycle_theme(),
        KeyCode::Char('L') => app.cycle_layout(),
        KeyCode::Char('A') => app.toggle_align(),
        KeyCode::Char('W') => app.toggle_wrap_code(),
        KeyCode::Char('/') => {
            app.mode = Mode::Search {
                input: String::new(),
            };
        }
        KeyCode::Char('n') | KeyCode::Tab | KeyCode::Right => app.advance_search(true),
        KeyCode::Char('N') | KeyCode::BackTab | KeyCode::Left => app.advance_search(false),
        KeyCode::Char('c') if !app.search_matches.is_empty() => app.clear_search(),
        KeyCode::Char('y') => app.copy_current_code_block(),
        KeyCode::Char('Y') => app.enter_line_pick(),
        KeyCode::Char('R') => app.enter_raw_code(),
        KeyCode::Char('r') => app.reload_from_disk(),
        KeyCode::Char('e') => app.pending_editor = true,
        KeyCode::Char('?' | 'h') => {
            app.mode = Mode::Help;
        }
        _ => {}
    }
    Ok(false)
}

fn handle_key_line_pick(app: &mut App, key: KeyEvent) -> Result<bool> {
    let (block_idx, mut line_idx) = {
        let Mode::LinePick {
            block_idx,
            line_idx,
        } = &app.mode
        else {
            return Ok(false);
        };
        (*block_idx, *line_idx)
    };
    let line_count = app.rendered.code_blocks[block_idx].line_visuals.len();
    if line_count == 0 {
        app.mode = Mode::Reading;
        return Ok(false);
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.mode = Mode::Reading;
            return Ok(false);
        }
        KeyCode::Char('j') | KeyCode::Down => {
            line_idx = (line_idx + 1).min(line_count - 1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            line_idx = line_idx.saturating_sub(1);
        }
        KeyCode::Char('g') | KeyCode::Home => {
            line_idx = 0;
        }
        KeyCode::Char('G') | KeyCode::End => {
            line_idx = line_count - 1;
        }
        KeyCode::Char('y') | KeyCode::Enter => {
            app.copy_source_line(block_idx, line_idx);
            app.mode = Mode::LinePick {
                block_idx,
                line_idx,
            };
            return Ok(false);
        }
        KeyCode::Char('Y') => {
            app.copy_whole_block(block_idx);
            app.mode = Mode::LinePick {
                block_idx,
                line_idx,
            };
            return Ok(false);
        }
        KeyCode::Char('R') => {
            app.mode = Mode::RawCode {
                block_idx,
                line_idx,
                h_off: 0,
            };
            return Ok(false);
        }
        _ => {}
    }

    app.mode = Mode::LinePick {
        block_idx,
        line_idx,
    };
    app.ensure_code_line_visible(block_idx, line_idx);
    Ok(false)
}

fn handle_key_raw_code(app: &mut App, key: KeyEvent) -> Result<bool> {
    let (block_idx, mut line_idx, mut h_off) = {
        let Mode::RawCode {
            block_idx,
            line_idx,
            h_off,
        } = &app.mode
        else {
            return Ok(false);
        };
        (*block_idx, *line_idx, *h_off)
    };
    let block = &app.rendered.code_blocks[block_idx];
    let total_lines = block.code.split('\n').count().max(1);
    let max_line_w = max_source_line_width(block);
    let pan_step: usize = 8;

    match key.code {
        KeyCode::Esc | KeyCode::Char('q' | 'R') => {
            app.mode = Mode::Reading;
            return Ok(false);
        }
        KeyCode::Char('j') | KeyCode::Down => {
            line_idx = (line_idx + 1).min(total_lines - 1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            line_idx = line_idx.saturating_sub(1);
        }
        KeyCode::Char('g') | KeyCode::Home => {
            line_idx = 0;
        }
        KeyCode::Char('G') | KeyCode::End => {
            line_idx = total_lines - 1;
        }
        KeyCode::Char('l') | KeyCode::Right => {
            let max_off = max_line_w.saturating_sub(1);
            h_off = (h_off + pan_step).min(max_off);
        }
        KeyCode::Char('h') | KeyCode::Left => {
            h_off = h_off.saturating_sub(pan_step);
        }
        KeyCode::Char('0') => {
            h_off = 0;
        }
        KeyCode::Char('$') => {
            h_off = max_line_w.saturating_sub(1);
        }
        KeyCode::Char('#') => {
            app.raw_show_line_nums = !app.raw_show_line_nums;
            app.mode = Mode::RawCode {
                block_idx,
                line_idx,
                h_off,
            };
            return Ok(false);
        }
        KeyCode::Char('y') | KeyCode::Enter => {
            app.copy_source_line(block_idx, line_idx);
            app.mode = Mode::RawCode {
                block_idx,
                line_idx,
                h_off,
            };
            return Ok(false);
        }
        KeyCode::Char('Y') => {
            app.copy_whole_block(block_idx);
            app.mode = Mode::RawCode {
                block_idx,
                line_idx,
                h_off,
            };
            return Ok(false);
        }
        _ => {}
    }

    app.mode = Mode::RawCode {
        block_idx,
        line_idx,
        h_off,
    };
    Ok(false)
}

fn max_source_line_width(block: &CodeBlockEntry) -> usize {
    block
        .code
        .split('\n')
        .map(|l| unicode_width::UnicodeWidthStr::width(l.replace('\t', "    ").as_str()))
        .max()
        .unwrap_or(0)
}

fn handle_key_toc(app: &mut App, key: KeyEvent) -> Result<bool> {
    let toc_len = app.rendered.toc.len();
    let Mode::Toc { selected } = &mut app.mode else {
        return Ok(false);
    };
    match key.code {
        KeyCode::Esc | KeyCode::Char('q' | 't') => {
            app.mode = Mode::Reading;
        }
        KeyCode::Char('j') | KeyCode::Down if toc_len > 0 => {
            *selected = (*selected + 1).min(toc_len - 1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            *selected = selected.saturating_sub(1);
        }
        KeyCode::Char('g') | KeyCode::Home => {
            *selected = 0;
        }
        KeyCode::Char('G') | KeyCode::End if toc_len > 0 => {
            *selected = toc_len - 1;
        }
        KeyCode::Enter => {
            if let Some(e) = app.rendered.toc.get(*selected) {
                let line = e.line;
                app.jump_to(line);
                app.mode = Mode::Reading;
            }
        }
        _ => {}
    }
    Ok(false)
}

fn handle_key_search(app: &mut App, key: KeyEvent) -> Result<bool> {
    // Extract the current input as owned data so we can mutate App freely.
    let mut input = {
        let Mode::Search { input } = &mut app.mode else {
            return Ok(false);
        };
        std::mem::take(input)
    };
    let mut changed = false;
    let mut action = SearchAction::Continue;
    match key.code {
        KeyCode::Esc => {
            action = SearchAction::Cancel;
        }
        KeyCode::Enter => {
            action = SearchAction::Commit;
        }
        KeyCode::Backspace => {
            let popped = input.pop();
            changed = popped.is_some();
        }
        KeyCode::Tab | KeyCode::Down => {
            app.advance_search(true);
        }
        KeyCode::BackTab | KeyCode::Up => {
            app.advance_search(false);
        }
        KeyCode::Char(c) if input.chars().count() < 256 => {
            input.push(c);
            changed = true;
        }
        _ => {}
    }

    match action {
        SearchAction::Continue => {
            if changed {
                // Preview: update matches live and scroll to the first one.
                let preview = input.clone();
                app.update_matches(&preview);
                if let Some(&first) = app.search_matches.first() {
                    let target = first;
                    app.jump_to(target);
                }
            }
            app.mode = Mode::Search { input };
        }
        SearchAction::Commit => {
            app.mode = Mode::Reading;
            app.commit_search(&input);
        }
        SearchAction::Cancel => {
            app.mode = Mode::Reading;
            app.clear_search();
        }
    }
    Ok(false)
}

enum SearchAction {
    Continue,
    Commit,
    Cancel,
}

/// Pick the code block to copy for the current viewport: first prefer the
/// topmost block whose line range intersects the viewport. If none is on
/// screen, fall back to the nearest block above the viewport top, then the
/// nearest one below.
fn pick_code_block_idx(
    blocks: &[CodeBlockEntry],
    view_top: usize,
    view_bottom: usize,
) -> Option<usize> {
    if blocks.is_empty() {
        return None;
    }
    if let Some((i, _)) = blocks
        .iter()
        .enumerate()
        .find(|(_, b)| b.start_line <= view_bottom && b.end_line >= view_top)
    {
        return Some(i);
    }
    if let Some((i, _)) = blocks
        .iter()
        .enumerate()
        .rev()
        .find(|(_, b)| b.end_line < view_top)
    {
        return Some(i);
    }
    blocks
        .iter()
        .enumerate()
        .find(|(_, b)| b.start_line > view_bottom)
        .map(|(i, _)| i)
}

/// Case-insensitive substring match against TOC entry titles. Returns the
/// line offset of the first hit, preferring earlier entries and shallower
/// heading levels on tie (so a top-level heading beats a nested one with the
/// same text).
fn find_heading(toc: &[TocEntry], query: &str) -> Option<usize> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return None;
    }
    // Exact case-insensitive match wins if one exists.
    if let Some(e) = toc.iter().find(|e| e.title.to_lowercase() == q) {
        return Some(e.line);
    }
    toc.iter()
        .find(|e| e.title.to_lowercase().contains(&q))
        .map(|e| e.line)
}

fn current_toc_index(toc: &[TocEntry], offset: usize) -> usize {
    let mut idx = 0;
    for (i, e) in toc.iter().enumerate() {
        if e.line <= offset {
            idx = i;
        } else {
            break;
        }
    }
    idx
}

fn draw(f: &mut ratatui::Frame<'_>, app: &mut App) {
    let size = f.area();
    app.last_viewport_h = size.height;

    // Paint the whole background so terminal defaults don't leak through.
    let bg_block = Block::default().style(app.theme.base_style());
    f.render_widget(bg_block, size);

    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(size);
    let body_rect = vertical[0];
    let footer_rect = vertical[1];

    let target = app.cfg.measure;
    let body_w = body_rect.width.saturating_sub(2);
    // Grow the drawing rect beyond the reading measure when any rendered
    // row (typically a table) needs it. Prose rows stay shorter than the
    // measure so the widened rect only affects layouts with wide tables.
    let rendered_max = u16::try_from(app.rendered.max_width).unwrap_or(u16::MAX);
    let wrap_width = target.max(rendered_max).min(body_w);
    let remaining = body_rect.width.saturating_sub(wrap_width);
    let left_margin = match app.align {
        Align::Center => remaining / 2,
        // Left and right still leave a 2-col gutter so text isn't flush
        // against the terminal edge — easier on the eyes.
        Align::Left => 2u16.min(remaining),
        Align::Right => remaining.saturating_sub(2u16.min(remaining)),
    };
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(left_margin),
            Constraint::Length(wrap_width),
            Constraint::Min(0),
        ])
        .split(body_rect);
    let text_rect = horizontal[1];

    draw_body(f, app, text_rect);
    draw_footer(f, app, footer_rect);

    match &app.mode {
        Mode::Toc { selected } => draw_toc_overlay(f, app, *selected, size),
        Mode::Search { input } => draw_search_overlay(f, app, input, size),
        Mode::Help => draw_help_overlay(f, app, size),
        Mode::RawCode {
            block_idx,
            line_idx,
            h_off,
        } => draw_raw_code_overlay(f, app, *block_idx, *line_idx, *h_off, size),
        Mode::Reading | Mode::LinePick { .. } => {}
    }
}

fn draw_body(f: &mut ratatui::Frame<'_>, app: &App, rect: Rect) {
    let total = app.rendered.lines.len();
    let start = app.offset.min(total);
    let end = (start + rect.height as usize).min(total);

    let mut display: Vec<Line<'static>> = app.rendered.lines[start..end].to_vec();

    // Highlight current search match if any.
    if !app.search_matches.is_empty() {
        if let Some(&m) = app.search_matches.get(app.search_cursor) {
            if (start..end).contains(&m) {
                let rel = m - start;
                let original = display[rel].clone();
                let hl_style = Style::default().add_modifier(Modifier::REVERSED);
                let marked: Vec<Span<'static>> = original
                    .spans
                    .into_iter()
                    .map(|s| Span::styled(s.content.into_owned(), s.style.patch(hl_style)))
                    .collect();
                display[rel] = Line::from(marked).style(app.theme.base_style());
            }
        }
    }

    // LinePick: reverse-highlight every visual row of the selected source line
    // that intersects the viewport. The block may soft-wrap, so a single
    // source line can cover multiple rows — highlight them all so the wrapped
    // continuation (`↪ …`) is visually part of the same selection.
    if let Mode::LinePick {
        block_idx,
        line_idx,
    } = &app.mode
    {
        if let Some(block) = app.rendered.code_blocks.get(*block_idx) {
            if let Some(&(vs, ve)) = block.line_visuals.get(*line_idx) {
                let hl_style = Style::default().add_modifier(Modifier::REVERSED);
                for row in vs..=ve {
                    if (start..end).contains(&row) {
                        let rel = row - start;
                        let original = display[rel].clone();
                        let marked: Vec<Span<'static>> = original
                            .spans
                            .into_iter()
                            .map(|s| Span::styled(s.content.into_owned(), s.style.patch(hl_style)))
                            .collect();
                        display[rel] = Line::from(marked).style(app.theme.base_style());
                    }
                }
            }
        }
    }

    let para = Paragraph::new(display)
        .style(app.theme.base_style())
        .wrap(Wrap { trim: false });
    f.render_widget(para, rect);
}

fn draw_footer(f: &mut ratatui::Frame<'_>, app: &App, rect: Rect) {
    let dim = app.theme.dim_style();
    let accent = app.theme.accent_style();

    let name = shorten_middle(
        &app.cfg.display_name,
        rect.width.saturating_sub(32) as usize,
    );
    let pct = app.percent();

    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(name, accent));
    spans.push(Span::styled("  \u{00B7}  ".to_string(), dim));
    spans.push(Span::styled(format!("{pct:>3}%"), dim));
    spans.push(Span::styled("  \u{00B7}  ".to_string(), dim));
    spans.push(Span::styled(
        format!(
            "{}/{}/{}",
            app.theme_name.label(),
            app.layout_name.label(),
            app.align.label(),
        ),
        dim,
    ));

    // Priority for the trailing slot:
    //   1. LinePick mode hint (context-specific — always show while picking).
    //   2. Active search match counter.
    //   3. Transient status message.
    //   4. Default help hint.
    let trailing = if let Mode::LinePick {
        block_idx,
        line_idx,
    } = &app.mode
    {
        let total = app
            .rendered
            .code_blocks
            .get(*block_idx)
            .map_or(0, |b| b.line_visuals.len());
        Some((
            format!(
                "line {}/{}  (j/k move · y copy · Y all · R raw · Esc exit)",
                line_idx + 1,
                total.max(1),
            ),
            accent,
        ))
    } else if !app.search_matches.is_empty() {
        Some((
            format!(
                "match {}/{}  (n/N or Tab/\u{2190}\u{2192}, c clear)",
                app.search_cursor + 1,
                app.search_matches.len()
            ),
            accent,
        ))
    } else if let Some((s, at, kind)) = app.status.as_ref() {
        // Success messages flash with a reversed accent for ~700ms so a
        // successful copy or edit feels confirmed, then fade to plain accent.
        let style = if *kind == StatusKind::Success && at.elapsed() < Duration::from_millis(700) {
            accent.add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            accent
        };
        Some((s.clone(), style))
    } else {
        Some(("? help  q quit".to_string(), dim))
    };

    if let Some((text, style)) = trailing {
        spans.push(Span::styled("  \u{00B7}  ".to_string(), dim));
        spans.push(Span::styled(text, style));
    }

    let para = Paragraph::new(Line::from(spans)).style(app.theme.base_style());
    f.render_widget(para, rect);
}

fn draw_toc_overlay(f: &mut ratatui::Frame<'_>, app: &App, selected: usize, area: Rect) {
    let w = (f32::from(area.width) * 0.7) as u16;
    let h = (f32::from(area.height) * 0.7) as u16;
    let x = (area.width - w) / 2 + area.x;
    let y = (area.height - h) / 2 + area.y;
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };

    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(" Table of contents ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(app.theme.base_style())
        .border_style(app.theme.rule_style());
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let start = selected.saturating_sub(inner.height as usize / 2);
    let end = (start + inner.height as usize).min(app.rendered.toc.len());
    let dim = app.theme.dim_style();
    let base = app.theme.base_style();
    for (i, entry) in app.rendered.toc[start..end].iter().enumerate() {
        let abs = start + i;
        let (guides, branch) = toc_tree_prefix(&app.rendered.toc, abs);
        let hl = abs == selected;
        let title_style = if hl {
            base.add_modifier(Modifier::REVERSED)
        } else {
            base
        };
        let guide_style = if hl {
            dim.add_modifier(Modifier::REVERSED)
        } else {
            dim
        };
        let mut spans: Vec<Span<'static>> = Vec::new();
        if !guides.is_empty() {
            spans.push(Span::styled(guides, guide_style));
        }
        if !branch.is_empty() {
            spans.push(Span::styled(branch, guide_style));
        }
        spans.push(Span::styled(entry.title.clone(), title_style));
        lines.push(Line::from(spans).style(base));
    }
    let para = Paragraph::new(lines).style(base);
    f.render_widget(para, inner);
}

/// Compute the tree-style prefix for TOC entry `i`: `(guides, branch)` where
/// `guides` is the vertical-bars column showing which ancestor levels still
/// have unprocessed siblings, and `branch` is the `├ ` or `└ ` connector for
/// the entry itself. Glum's TOC is a flat list ordered by document order, so
/// we can resolve both with forward scans.
fn toc_tree_prefix(toc: &[TocEntry], i: usize) -> (String, String) {
    let entry = &toc[i];
    let lvl = entry.level as usize;
    if lvl == 0 {
        return (String::new(), String::new());
    }
    // For each ancestor level `l` strictly above this entry, a `│ ` column
    // when any later entry has the same level `l` *before* something shallower
    // closes the section — that means the ancestor at `l` still has siblings.
    let mut guides = String::new();
    for l in 1..lvl {
        let mut has_sibling = false;
        for entry_j in toc.iter().skip(i + 1) {
            let lj = entry_j.level as usize;
            if lj < l {
                break;
            }
            if lj == l {
                has_sibling = true;
                break;
            }
        }
        guides.push_str(if has_sibling { "\u{2502} " } else { "  " });
    }
    // The branch glyph at this entry's own level depends on whether *this*
    // entry is the last of its siblings at `lvl` (before the next shallower
    // heading). `└ ` for last, `├ ` otherwise.
    let mut is_last = true;
    for entry_j in toc.iter().skip(i + 1) {
        let lj = entry_j.level as usize;
        if lj < lvl {
            break;
        }
        if lj == lvl {
            is_last = false;
            break;
        }
    }
    let branch = if is_last { "\u{2514} " } else { "\u{251C} " }.to_string();
    (guides, branch)
}

fn draw_search_overlay(f: &mut ratatui::Frame<'_>, app: &App, input: &str, area: Rect) {
    let h = 4u16;
    let w = area.width.saturating_sub(8).min(80);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + area.height.saturating_sub(h + 2);
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);

    let count_text = if input.is_empty() {
        "type to search".to_string()
    } else if app.search_matches.is_empty() {
        "no matches".to_string()
    } else {
        format!(
            "{}/{} match{}",
            app.search_cursor + 1,
            app.search_matches.len(),
            if app.search_matches.len() == 1 {
                ""
            } else {
                "es"
            }
        )
    };
    let title = format!(" Search \u{2014} {count_text} ");

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(app.theme.base_style())
        .border_style(app.theme.rule_style());
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let hint_width = inner.width.saturating_sub(1) as usize;
    let hint = "Enter=confirm  Esc=cancel  Tab/\u{2191}\u{2193}=next/prev";
    let mut hint_line = String::new();
    if hint.chars().count() <= hint_width {
        hint_line = hint.to_string();
    }
    let input_line = Line::from(vec![
        Span::styled("/ ".to_string(), app.theme.dim_style()),
        Span::styled(input.to_string(), app.theme.accent_style()),
        Span::styled("\u{2588}".to_string(), app.theme.dim_style()),
    ]);
    let hint_styled = Line::styled(hint_line, app.theme.dim_style());
    let para = Paragraph::new(vec![input_line, hint_styled]).style(app.theme.base_style());
    f.render_widget(para, inner);
}

fn draw_raw_code_overlay(
    f: &mut ratatui::Frame<'_>,
    app: &App,
    block_idx: usize,
    line_idx: usize,
    h_off: usize,
    area: Rect,
) {
    let Some(block) = app.rendered.code_blocks.get(block_idx) else {
        return;
    };
    let source_lines: Vec<String> = block.code.split('\n').map(String::from).collect();
    let total = source_lines.len().max(1);

    let margin_x: u16 = 2;
    let margin_y: u16 = 1;
    let w = area.width.saturating_sub(margin_x * 2).max(10);
    let h = area.height.saturating_sub(margin_y * 2).max(4);
    let rect = Rect {
        x: area.x + margin_x,
        y: area.y + margin_y,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);

    let lang_label = if block.lang.is_empty() {
        "code".to_string()
    } else {
        block.lang.clone()
    };
    let title = format!(
        " {lang_label} \u{2014} line {}/{} \u{2014} col {} ",
        line_idx + 1,
        total,
        h_off + 1,
    );
    let block_widget = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(app.theme.base_style())
        .border_style(app.theme.rule_style());
    let inner = block_widget.inner(rect);
    f.render_widget(block_widget, rect);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    // Reserve the final row inside the border for the hint line.
    let content_rows = inner.height.saturating_sub(1) as usize;
    let full_cols = inner.width as usize;
    // Reserve a gutter for source-line numbers when enabled. Width is
    // `digits(total) + 2` to leave a one-column visual gap between the
    // number and the code.
    let gutter_w = if app.raw_show_line_nums {
        digit_count(total) + 2
    } else {
        0
    };
    let content_cols = full_cols.saturating_sub(gutter_w);
    if content_rows == 0 || content_cols == 0 {
        return;
    }

    // Vertical scroll: try to center the cursor line; otherwise clamp so
    // scrolling off the ends still shows the full window.
    let top = if total <= content_rows {
        0
    } else {
        let half = content_rows / 2;
        line_idx
            .saturating_sub(half)
            .min(total.saturating_sub(content_rows))
    };

    let hl_style = Style::default().add_modifier(Modifier::REVERSED);
    let base = app.theme.base_style();
    let dim = app.theme.dim_style();
    let code_bg = app.theme.code_style();

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(content_rows + 1);
    for (i, src) in source_lines.iter().enumerate().skip(top).take(content_rows) {
        let normalized = src.replace('\t', "    ");
        let visible = slice_by_display_cols(&normalized, h_off, content_cols);
        let vis_w = unicode_width::UnicodeWidthStr::width(visible.as_str());
        let mut spans: Vec<Span<'static>> = Vec::new();
        if gutter_w > 0 {
            let num = format!("{:>w$}  ", i + 1, w = gutter_w - 2);
            spans.push(Span::styled(num, dim));
        }
        spans.extend(highlight_line(&visible, &block.lang, app.theme));
        let pad_cols = content_cols.saturating_sub(vis_w);
        if pad_cols > 0 {
            spans.push(Span::styled(" ".repeat(pad_cols), code_bg));
        }
        if i == line_idx {
            spans = spans
                .into_iter()
                .map(|s| Span::styled(s.content.into_owned(), s.style.patch(hl_style)))
                .collect();
        }
        lines.push(Line::from(spans).style(base));
    }
    // Pad out empty rows so the overlay fills its box consistently.
    while lines.len() < content_rows {
        lines.push(Line::styled(" ".repeat(full_cols), code_bg));
    }

    let hint = "j/k line  h/l pan  0/$ home/end  # line-nums  y copy line  Y all  Esc close";
    let hint_text = truncate_display(hint, full_cols);
    lines.push(Line::styled(hint_text, dim));

    let para = Paragraph::new(lines).style(base);
    f.render_widget(para, inner);
}

/// Skip `skip` display columns, then keep up to `keep` display columns.
/// Wide characters that straddle `skip` are dropped entirely (no partial
/// char); a leading space is inserted if the skip cut mid-wide-char so
/// columns still line up.
fn slice_by_display_cols(s: &str, skip: usize, keep: usize) -> String {
    let mut out = String::new();
    let mut skipped = 0usize;
    let mut kept = 0usize;
    let mut chars = s.chars().peekable();
    while let Some(&ch) = chars.peek() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if skipped < skip {
            if skipped + w > skip {
                // A wide char straddles the skip boundary; drop it and pad.
                let pad = skipped + w - skip;
                for _ in 0..pad.min(keep.saturating_sub(kept)) {
                    out.push(' ');
                    kept += 1;
                }
                chars.next();
                skipped += w;
                continue;
            }
            skipped += w;
            chars.next();
            continue;
        }
        if kept + w > keep {
            break;
        }
        out.push(ch);
        kept += w;
        chars.next();
    }
    out
}

/// Number of decimal digits needed to display `n` (minimum 1 — so zero
/// still reserves a visible column).
fn digit_count(n: usize) -> usize {
    if n == 0 {
        1
    } else {
        (n as f64).log10().floor() as usize + 1
    }
}

fn truncate_display(s: &str, max_cols: usize) -> String {
    if unicode_width::UnicodeWidthStr::width(s) <= max_cols {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > max_cols {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out
}

fn draw_help_overlay(f: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let w = 58u16.min(area.width.saturating_sub(4));
    let h = 26u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    };
    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(app.theme.base_style())
        .border_style(app.theme.rule_style());
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let rows: &[(&str, &str)] = &[
        ("j / \u{2193}", "scroll down"),
        ("k / \u{2191}", "scroll up"),
        ("space / PgDn", "page down"),
        ("b / PgUp", "page up"),
        ("d / u", "half page"),
        ("g / G", "top / bottom"),
        ("t", "table of contents"),
        ("T", "cycle theme"),
        ("L", "cycle layout"),
        ("A", "toggle align (center/left/right)"),
        ("W", "toggle code wrap / truncate"),
        ("/", "search"),
        ("n / N", "next / prev match"),
        ("Tab / \u{2192}", "next match"),
        ("Shift-Tab / \u{2190}", "prev match"),
        ("c", "clear active search"),
        ("y", "copy code block in view"),
        ("Y", "pick & copy a single code line"),
        ("R", "raw code view (no wrap, h/l pan)"),
        ("r", "reload current file"),
        ("e", "open in $EDITOR at this heading"),
        ("?", "toggle this help"),
        ("q / Esc", "quit / close overlay"),
    ];
    let dim = app.theme.dim_style();
    let base = app.theme.base_style();
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (k, v) in rows {
        lines.push(Line::from(vec![
            Span::styled(format!("  {k:<14}"), app.theme.accent_style()),
            Span::styled((*v).to_string(), base),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::styled("  press ? or Esc to close".to_string(), dim));
    let para = Paragraph::new(lines).style(base);
    f.render_widget(para, inner);
}

fn shorten_middle(s: &str, max: usize) -> String {
    if max < 5 {
        return String::new();
    }
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max - 3;
    let head = keep / 2;
    let tail = keep - head;
    let chars: Vec<char> = s.chars().collect();
    let mut out: String = chars[..head].iter().collect();
    out.push_str("...");
    out.extend(&chars[chars.len() - tail..]);
    out
}

/// Short, cwd-relative form of `path` suitable for the status bar. Falls
/// back to the full path when it can't be made relative.
pub fn display_name_for(path: &Path) -> String {
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(stripped) = path.strip_prefix(&cwd) {
            return stripped.display().to_string();
        }
    }
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorten_middle_keeps_edges() {
        assert_eq!(shorten_middle("abcdefghij", 10), "abcdefghij");
        let s = shorten_middle("0123456789abcdef", 9);
        assert_eq!(s.chars().count(), 9);
        assert!(s.contains("..."));
    }

    #[test]
    fn pick_code_block_prefers_intersection() {
        let blocks = vec![
            CodeBlockEntry {
                start_line: 10,
                end_line: 20,
                lang: "rust".into(),
                code: "a".into(),
                line_visuals: vec![],
            },
            CodeBlockEntry {
                start_line: 30,
                end_line: 40,
                lang: "py".into(),
                code: "b".into(),
                line_visuals: vec![],
            },
        ];
        let i = pick_code_block_idx(&blocks, 15, 25).unwrap();
        assert_eq!(blocks[i].lang, "rust");
    }

    #[test]
    fn pick_code_block_falls_back_above() {
        let blocks = vec![
            CodeBlockEntry {
                start_line: 5,
                end_line: 7,
                lang: "rust".into(),
                code: "a".into(),
                line_visuals: vec![],
            },
            CodeBlockEntry {
                start_line: 50,
                end_line: 60,
                lang: "py".into(),
                code: "b".into(),
                line_visuals: vec![],
            },
        ];
        let i = pick_code_block_idx(&blocks, 20, 25).unwrap();
        assert_eq!(blocks[i].lang, "rust");
    }

    #[test]
    fn pick_code_block_falls_back_below() {
        let blocks = vec![CodeBlockEntry {
            start_line: 50,
            end_line: 60,
            lang: "py".into(),
            code: "b".into(),
            line_visuals: vec![],
        }];
        let i = pick_code_block_idx(&blocks, 0, 10).unwrap();
        assert_eq!(blocks[i].lang, "py");
    }

    #[test]
    fn find_heading_exact_wins_over_substring() {
        let toc = vec![
            TocEntry {
                level: 1,
                title: "Installation".into(),
                line: 10,
                source_line: 1,
            },
            TocEntry {
                level: 2,
                title: "Install".into(),
                line: 42,
                source_line: 1,
            },
        ];
        // "install" is a substring of both but an exact case-insensitive match
        // of the second → second wins.
        assert_eq!(find_heading(&toc, "install"), Some(42));
    }

    #[test]
    fn find_heading_substring_when_no_exact() {
        let toc = vec![
            TocEntry {
                level: 1,
                title: "Code blocks".into(),
                line: 100,
                source_line: 1,
            },
            TocEntry {
                level: 2,
                title: "Quoting text".into(),
                line: 200,
                source_line: 1,
            },
        ];
        assert_eq!(find_heading(&toc, "quot"), Some(200));
        assert_eq!(find_heading(&toc, "nonexistent"), None);
        assert_eq!(find_heading(&toc, ""), None);
    }

    #[test]
    fn slice_by_display_cols_skip_and_keep() {
        // Skip 5 cols, keep 3 cols out of a long ASCII line.
        assert_eq!(slice_by_display_cols("abcdefghij", 5, 3), "fgh");
        // Skip past end → empty.
        assert_eq!(slice_by_display_cols("abc", 10, 5), "");
        // Keep exceeds line length → returns full tail.
        assert_eq!(slice_by_display_cols("abc", 1, 10), "bc");
        // Zero-width skip returns the prefix up to `keep`.
        assert_eq!(slice_by_display_cols("hello world", 0, 5), "hello");
    }

    #[test]
    fn toc_tree_prefix_draws_guides_and_branches() {
        let toc = vec![
            TocEntry {
                level: 1,
                title: "A".into(),
                line: 0,
                source_line: 1,
            },
            TocEntry {
                level: 2,
                title: "A.1".into(),
                line: 1,
                source_line: 2,
            },
            TocEntry {
                level: 2,
                title: "A.2".into(),
                line: 2,
                source_line: 3,
            },
            TocEntry {
                level: 1,
                title: "B".into(),
                line: 3,
                source_line: 4,
            },
        ];
        // A is the first of two level-1 items → ├
        assert_eq!(
            toc_tree_prefix(&toc, 0),
            (String::new(), "\u{251C} ".into())
        );
        // A.1 has a later sibling (A.2) before a shallower heading → ├; guide
        // shows A's column as a vertical bar (A still has a later sibling B).
        assert_eq!(
            toc_tree_prefix(&toc, 1),
            ("\u{2502} ".into(), "\u{251C} ".into())
        );
        // A.2 is the last child of A (next heading is level 1) → └
        assert_eq!(
            toc_tree_prefix(&toc, 2),
            ("\u{2502} ".into(), "\u{2514} ".into())
        );
        // B is the last level-1 → └
        assert_eq!(
            toc_tree_prefix(&toc, 3),
            (String::new(), "\u{2514} ".into())
        );
    }

    #[test]
    fn current_toc_index_finds_nearest_heading_above() {
        let toc = vec![
            TocEntry {
                level: 1,
                title: "A".into(),
                line: 0,
                source_line: 1,
            },
            TocEntry {
                level: 2,
                title: "B".into(),
                line: 10,
                source_line: 1,
            },
            TocEntry {
                level: 2,
                title: "C".into(),
                line: 20,
                source_line: 1,
            },
        ];
        assert_eq!(current_toc_index(&toc, 0), 0);
        assert_eq!(current_toc_index(&toc, 15), 1);
        assert_eq!(current_toc_index(&toc, 100), 2);
    }
}
