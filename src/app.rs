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
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, LineGauge, List, ListItem, ListState, Paragraph, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Tabs,
};
use ratatui::Terminal;

use crate::clipboard::{self, CopyOutcome};
use crate::highlight::highlight_line;
use crate::layout::LayoutName;
use crate::positions::PositionStore;
use crate::render::{self, CodeBlockEntry, Rendered, TocEntry};
use crate::theme::{Theme, ThemeName};
use crate::watch::FileWatcher;

/// One loaded input document. `AppConfig.path/source/display_name` mirror
/// the *current* doc; the vector holds every doc for tab switching.
pub struct DocInput {
    /// Canonical path (or `<stdin>`).
    pub path: PathBuf,
    /// Full markdown text of the document.
    pub source: String,
    /// Path shown in the footer and tab bar.
    pub display_name: String,
    /// In-memory fallback offset for `--no-remember` sessions.
    pub offset: usize,
}

/// Full configuration needed to launch the TUI. Built by `main.rs` from the
/// parsed CLI plus the persistence store, then handed to [`run`].
pub struct AppConfig {
    /// Canonical path of the file being read (or `<stdin>` for piped input).
    pub path: PathBuf,
    /// File contents already loaded into memory; the renderer reads this
    /// string, not the path.
    pub source: String,
    /// Every input document, in CLI order (tabs).
    pub docs: Vec<DocInput>,
    /// Index into `docs` of the document currently shown.
    pub current: usize,
    /// True when `--follow` was passed; the watcher is recreated to track
    /// the visible document on tab switches.
    pub follow: bool,
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
    /// Terminal graphics negotiation for `i` image preview. `Some` only when
    /// `--images` was passed (the query must happen before raw mode).
    pub picker: Option<ratatui_image::picker::Picker>,
}

/// Horizontal alignment of the reading column within the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
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
        /// Index into the *filtered* entry list (the full TOC when no filter
        /// is active).
        selected: usize,
        /// `Some` while type-to-filter is active (`/` inside the overlay):
        /// printable keys edit it, Esc clears it, Enter jumps.
        filter: Option<String>,
    },
    Search {
        input: String,
        /// Caret position in chars (0..=input len).
        cursor: usize,
    },
    Help,
    /// Inline line-pick: the selected source line of a specific code block is
    /// highlighted in the main view; j/k move, y/Enter copies that line.
    LinePick {
        block_idx: usize,
        line_idx: usize,
    },
    /// Link picker: j/k steps through the document's links (highlighting
    /// their line), Enter opens — external URLs via the OS opener, `#anchor`
    /// jumps to the heading, relative paths open with the OS default app.
    LinkPick {
        idx: usize,
    },
    /// Full-screen preview of a local image (`--images`, `i` key).
    ImageView {
        protocol: Box<ratatui_image::protocol::StatefulProtocol>,
        title: String,
    },
    /// Full-screen raw view of a code block: no wrap, horizontal pan, per-line
    /// cursor — lets the reader see full long lines and copy a single one.
    RawCode {
        block_idx: usize,
        line_idx: usize,
        h_off: usize,
        /// Entered from `LinePick` — Esc returns there instead of Reading.
        from_pick: bool,
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
    /// Measure the current `rendered` was produced at: the configured measure
    /// clamped to the terminal width. Re-rendering at the effective width keeps
    /// every logical line ≤ one screen row, which all scroll math relies on.
    render_measure: usize,
    offset: usize,
    last_viewport_h: u16,
    /// Last terminal width used for rendering. Tables are sized against
    /// this (not just the prose measure), so a meaningful resize triggers
    /// a re-render to recompute table column widths against the new budget.
    last_render_width: u16,
    mode: Mode,
    search_matches: Vec<usize>,
    search_cursor: usize,
    /// The committed/live query, normalized (see `normalize_for_search`).
    search_query: String,
    /// Normalized text of each rendered line, built lazily on first search
    /// and invalidated on re-render — avoids re-allocating the whole document
    /// per keystroke.
    search_cache: Option<Vec<String>>,
    /// Scroll offset when the search prompt opened; restored on cancel and
    /// used to pick the first match at-or-after the reading position.
    search_origin: Option<usize>,
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
        let render_measure = cfg.measure as usize;
        let mut app = Self {
            cfg,
            theme,
            theme_name,
            layout_name,
            align,
            wrap_code,
            rendered,
            render_measure,
            offset: saved_offset,
            last_viewport_h: 0,
            last_render_width: term_w,
            mode: Mode::Reading,
            search_matches: Vec::new(),
            search_cursor: 0,
            search_query: String::new(),
            search_cache: None,
            search_origin: None,
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
                    self.render_measure,
                    table_budget(self.cfg.measure, self.last_render_width),
                    self.theme,
                    self.layout_name,
                    self.wrap_code,
                );
                self.search_cache = None;
                self.offset = self.offset.min(self.max_offset());
                // Invalidate any pinned search match (line indices are stale).
                if !self.search_matches.is_empty() {
                    self.search_matches.clear();
                    self.search_cursor = 0;
                }
                self.validate_block_mode();
                self.set_status("reloaded");
            }
            Err(e) => {
                self.set_status(format!("reload failed: {e}"));
            }
        }
    }

    /// A `--follow` reload can shrink or remove code blocks while a
    /// block-bound mode (`LinePick`/`RawCode`) holds indices into them.
    /// Drop back to reading mode if those indices no longer resolve.
    fn validate_block_mode(&mut self) {
        let valid = match &self.mode {
            Mode::LinePick {
                block_idx,
                line_idx,
            } => self
                .rendered
                .code_blocks
                .get(*block_idx)
                .is_some_and(|b| *line_idx < b.line_visuals.len().max(1)),
            Mode::RawCode {
                block_idx,
                line_idx,
                ..
            } => self
                .rendered
                .code_blocks
                .get(*block_idx)
                .is_some_and(|b| *line_idx < b.code.split('\n').count().max(1)),
            _ => true,
        };
        if !valid {
            self.mode = Mode::Reading;
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
            self.search_origin = Some(self.offset);
            self.update_matches(&query);
            self.snap_to_match_near(self.offset);
            let cursor = query.chars().count();
            self.mode = Mode::Search {
                input: query,
                cursor,
            };
        }
        if self.cfg.initial.open_toc {
            if self.rendered.toc.is_empty() {
                self.set_status("no headings");
            } else {
                // --toc takes precedence over an opened search prompt.
                let selected = current_toc_index(&self.rendered.toc, self.offset);
                self.mode = Mode::Toc {
                    selected,
                    filter: None,
                };
            }
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
        // Reserve 2 lines for footer/status (+ tab bar), overlap by 2 for
        // orientation.
        let body = self.last_viewport_h.saturating_sub(2 + self.tab_rows()) as usize;
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
        let body = self.last_viewport_h.saturating_sub(2 + self.tab_rows()) as usize;
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
            self.render_measure,
            table_budget(self.cfg.measure, self.last_render_width),
            self.theme,
            self.layout_name,
            self.wrap_code,
        );
        self.search_cache = None;
        self.offset = self.offset.min(self.max_offset());
    }

    /// Clamp the render measure to the terminal width and re-render when
    /// either the effective measure or the terminal width changed (the
    /// latter also feeds the table budget). Keeps the
    /// one-logical-line-per-screen-row invariant that all paging math
    /// depends on when the terminal is narrower than the measure.
    fn sync_measure(&mut self, term_width: u16) {
        let effective = (self.cfg.measure.min(term_width.saturating_sub(2)) as usize).max(20);
        if effective != self.render_measure || term_width != self.last_render_width {
            self.render_measure = effective;
            self.last_render_width = term_width;
            self.re_render();
            // Match line indices belong to the old wrap; recompute.
            if !self.search_query.is_empty() {
                let q = self.search_query.clone();
                self.update_matches(&q);
            }
            self.validate_block_mode();
        }
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
        let visible_end = (self.offset
            + self.last_viewport_h.saturating_sub(2 + self.tab_rows()) as usize)
            .min(total);
        ((visible_end as f64 / total as f64) * 100.0)
            .round()
            .clamp(0.0, 100.0) as u16
    }

    /// Recompute match line indices for `needle`. Keeps the current cursor if
    /// possible, otherwise snaps to match 0. Called live as the user types.
    ///
    /// Both needle and document text are normalized (smart quotes, dashes,
    /// ellipsis, NBSP folded back to ASCII) so a query typed as it appears in
    /// the *source* still matches the smartened rendering.
    fn update_matches(&mut self, needle: &str) {
        self.search_matches.clear();
        self.search_query = normalize_for_search(needle);
        if self.search_query.is_empty() {
            self.search_cursor = 0;
            return;
        }
        if self.search_cache.is_none() {
            self.search_cache = Some(
                self.rendered
                    .lines
                    .iter()
                    .map(|l| normalize_for_search(&l.to_string()))
                    .collect(),
            );
        }
        if let Some(cache) = self.search_cache.as_ref() {
            for (i, s) in cache.iter().enumerate() {
                if s.contains(&self.search_query) {
                    self.search_matches.push(i);
                }
            }
        }
        self.search_cursor = self
            .search_cursor
            .min(self.search_matches.len().saturating_sub(1));
    }

    /// Move the cursor to the first match at or after line `from` (wrapping
    /// to the first match overall), like `less` — search continues from the
    /// reading position instead of yanking to the top of the document.
    fn snap_to_match_near(&mut self, from: usize) {
        if self.search_matches.is_empty() {
            return;
        }
        let idx = self
            .search_matches
            .iter()
            .position(|&l| l >= from)
            .unwrap_or(0);
        self.search_cursor = idx;
        self.jump_to(self.search_matches[idx]);
    }

    /// Run a committed search: update matches and scroll to the nearest one.
    fn commit_search(&mut self, needle: &str) {
        let origin = self.search_origin.take().unwrap_or(0);
        self.update_matches(needle);
        if self.search_matches.is_empty() {
            if !needle.is_empty() {
                // Failed search: return to where the reader was before the
                // live preview moved the view.
                self.jump_to(origin);
                self.set_status("no matches");
            }
        } else {
            self.snap_to_match_near(origin);
        }
    }

    fn clear_search(&mut self) {
        self.search_matches.clear();
        self.search_cursor = 0;
        self.search_query.clear();
    }

    /// Index of the code block to act on for the current viewport — see
    /// `pick_code_block_idx` for the selection logic.
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
            from_pick: false,
        };
    }

    /// Scroll the reader so the visual rows of `(block_idx, line_idx)` are
    /// inside the viewport. Used by `LinePick` when the user moves the cursor.
    fn ensure_code_line_visible(&mut self, block_idx: usize, line_idx: usize) {
        let Some(block) = self.rendered.code_blocks.get(block_idx) else {
            return;
        };
        let Some(&(vs, ve)) = block.line_visuals.get(line_idx) else {
            return;
        };
        let body_h = self.last_viewport_h.saturating_sub(2 + self.tab_rows()) as usize;
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
        let Some(block) = self.rendered.code_blocks.get(block_idx) else {
            self.set_status("no such block");
            return;
        };
        let Some(line) = block.code.split('\n').nth(line_idx) else {
            self.set_status("no such line");
            return;
        };
        let payload = line.to_string();
        let total = block.line_visuals.len().max(1);
        let pos = line_idx + 1;
        match clipboard::copy(&payload) {
            Ok(CopyOutcome::Native(n)) => {
                self.set_status_success(format!("copied line {pos}/{total} \u{2014} {n}B"));
            }
            Ok(CopyOutcome::Osc52(_)) => self.set_status(format!(
                "line {pos}/{total} sent via OSC 52 \u{2014} if paste fails, install xclip"
            )),
            Ok(CopyOutcome::TooLarge) => self.set_status("line too large to copy"),
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
        let Some(block) = self.rendered.code_blocks.get(block_idx) else {
            self.set_status("no such block");
            return;
        };
        match clipboard::copy(&block.code) {
            Ok(CopyOutcome::Native(n)) => {
                self.set_status_success(format!("copied {n} bytes ({})", block.lang));
            }
            Ok(CopyOutcome::Osc52(n)) => self.set_status(format!(
                "sent {n}B via OSC 52 \u{2014} if paste fails, install xclip"
            )),
            Ok(CopyOutcome::TooLarge) => self.set_status("block too large to copy"),
            Err(_) => self.set_status("copy failed"),
        }
    }

    /// One row for the tab bar when more than one file is open.
    fn tab_rows(&self) -> u16 {
        u16::from(self.cfg.docs.len() > 1)
    }

    /// Switch to the next/previous open document (`]` / `[`), saving the
    /// current position and re-pointing the file watcher.
    fn switch_doc(&mut self, delta: isize) {
        let n = self.cfg.docs.len();
        if n < 2 {
            return;
        }
        let cur = self.cfg.current;
        self.cfg.docs[cur].offset = self.offset;
        self.cfg.docs[cur].source = std::mem::take(&mut self.cfg.source);
        self.cfg.store.set(&self.cfg.path, self.offset).ok();

        let next = (cur as isize + delta).rem_euclid(n as isize) as usize;
        self.cfg.current = next;
        let d = &self.cfg.docs[next];
        self.cfg.path = d.path.clone();
        self.cfg.display_name = d.display_name.clone();
        self.cfg.source = d.source.clone();
        self.mode = Mode::Reading;
        self.clear_search();
        self.re_render();
        let saved = self.cfg.store.get(&self.cfg.path).map(|e| e.offset);
        self.offset = saved
            .unwrap_or(self.cfg.docs[next].offset)
            .min(self.max_offset());
        self.cfg.watcher = if self.cfg.follow && self.cfg.path.as_os_str() != "<stdin>" {
            FileWatcher::start(&self.cfg.path).ok()
        } else {
            None
        };
        self.set_status(format!("file {}/{n}", next + 1));
    }

    /// Enter the link picker on the first link at/after the viewport top.
    fn enter_link_pick(&mut self) {
        if self.rendered.links.is_empty() {
            self.set_status("no links");
            return;
        }
        let idx = self
            .rendered
            .links
            .iter()
            .position(|l| l.line >= self.offset)
            .unwrap_or(0);
        self.mode = Mode::LinkPick { idx };
        self.ensure_link_visible(idx);
    }

    fn ensure_link_visible(&mut self, idx: usize) {
        let Some(link) = self.rendered.links.get(idx) else {
            return;
        };
        let line = link.line;
        let body_h = self.last_viewport_h.saturating_sub(2 + self.tab_rows()) as usize;
        if body_h == 0 {
            return;
        }
        if line >= self.offset + body_h {
            self.offset = line + 1 - body_h;
        }
        if line < self.offset {
            self.offset = line;
        }
        self.offset = self.offset.min(self.total_lines().saturating_sub(1));
    }

    /// Open the selected link: `#anchor` jumps to its heading, http(s)/
    /// mailto go to the OS opener, relative paths resolve against the
    /// document's directory. Anything else (javascript:, data:, …) is
    /// refused.
    fn open_link(&mut self, idx: usize) {
        let Some(link) = self.rendered.links.get(idx).cloned() else {
            return;
        };
        let url = link.url.trim().to_string();
        if let Some(anchor) = url.strip_prefix('#') {
            if let Some(line) = heading_line_for_anchor(&self.rendered.toc, anchor) {
                self.mode = Mode::Reading;
                self.jump_to(line);
                self.set_status(format!("jumped to #{anchor}"));
            } else {
                self.set_status(format!("no heading for #{anchor}"));
            }
            return;
        }
        let lower = url.to_ascii_lowercase();
        if lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("mailto:")
        {
            match open_external(&url) {
                Ok(()) => {
                    self.mode = Mode::Reading;
                    self.set_status_success(format!("opened {}", shorten_middle(&url, 48)));
                }
                Err(e) => self.set_status(format!("open failed: {e}")),
            }
            return;
        }
        if !url.contains("://") {
            // Relative file reference; strip any #fragment.
            let path_part = url.split('#').next().unwrap_or("");
            let base = self
                .cfg
                .path
                .parent()
                .map_or_else(PathBuf::new, Path::to_path_buf);
            let target = base.join(path_part);
            if target.exists() {
                match open_external(&target.to_string_lossy()) {
                    Ok(()) => {
                        self.mode = Mode::Reading;
                        self.set_status_success(format!("opened {path_part}"));
                    }
                    Err(e) => self.set_status(format!("open failed: {e}")),
                }
            } else {
                self.set_status(format!("not found: {}", target.display()));
            }
            return;
        }
        self.set_status("unsupported link scheme");
    }

    /// Preview the nearest image in view (`i`, requires `--images`).
    fn enter_image_view(&mut self) {
        if self.rendered.images.is_empty() {
            self.set_status("no images");
            return;
        }
        if self.cfg.picker.is_none() {
            self.set_status("image preview off \u{2014} run with --images");
            return;
        }
        let entry = self
            .rendered
            .images
            .iter()
            .find(|e| e.line >= self.offset)
            .or_else(|| self.rendered.images.last())
            .cloned();
        let Some(entry) = entry else {
            return;
        };
        if entry.url.contains("://") {
            self.set_status("remote image \u{2014} press o to open its link");
            return;
        }
        let base = self
            .cfg
            .path
            .parent()
            .map_or_else(PathBuf::new, Path::to_path_buf);
        let path = base.join(entry.url.split('#').next().unwrap_or(""));
        match image::open(&path) {
            Ok(img) => {
                let Some(picker) = self.cfg.picker.as_mut() else {
                    return;
                };
                let protocol = picker.new_resize_protocol(img);
                self.mode = Mode::ImageView {
                    protocol: Box::new(protocol),
                    title: entry.url,
                };
            }
            Err(e) => self.set_status(format!("image load failed: {e}")),
        }
    }

    fn advance_search(&mut self, forward: bool) {
        if self.search_matches.is_empty() {
            return;
        }
        let len = self.search_matches.len();
        if forward {
            if self.search_cursor + 1 == len {
                self.set_status("search wrapped to top");
            }
            self.search_cursor = (self.search_cursor + 1) % len;
        } else {
            if self.search_cursor == 0 {
                self.set_status("search wrapped to bottom");
            }
            self.search_cursor = (self.search_cursor + len - 1) % len;
        }
        let target = self.search_matches[self.search_cursor];
        self.jump_to(target);
    }
}

/// Launch the OS default opener for a URL or file path, detached from the
/// TUI (no suspend needed — the opener returns immediately).
fn open_external(target: &str) -> io::Result<()> {
    use std::process::Stdio;
    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("open");
        c.arg(target);
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", target]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = {
        let mut c = Command::new("xdg-open");
        c.arg(target);
        c
    };
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

/// GitHub-style anchor slug: lowercase, spaces → `-`, drop everything that
/// isn't alphanumeric or a hyphen.
fn slugify(title: &str) -> String {
    title
        .trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| {
            if c.is_alphanumeric() {
                Some(c)
            } else if c == ' ' || c == '-' {
                Some('-')
            } else {
                None
            }
        })
        .collect()
}

/// Resolve `#anchor` against the TOC by slug, falling back to a
/// case-insensitive substring match on titles.
fn heading_line_for_anchor(toc: &[TocEntry], anchor: &str) -> Option<usize> {
    let want = anchor.trim().to_lowercase();
    if want.is_empty() {
        return None;
    }
    if let Some(e) = toc.iter().find(|e| slugify(&e.title) == want) {
        return Some(e.line);
    }
    let loose = want.replace('-', " ");
    toc.iter()
        .find(|e| e.title.to_lowercase().contains(&loose))
        .map(|e| e.line)
}

/// Fold the substitutions made by `typography::smarten` (plus the NBSP pads
/// around inline code) back to the ASCII a user would type, lowercased —
/// applied to both needle and haystack so `don't`, `--follow`, or `...`
/// match their smartened renderings.
fn normalize_for_search(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\u{2018}' | '\u{2019}' => out.push('\''),
            '\u{201C}' | '\u{201D}' => out.push('"'),
            '\u{2014}' => out.push_str("--"),
            '\u{2013}' => out.push('-'),
            '\u{2026}' => out.push_str("..."),
            '\u{00A0}' => out.push(' '),
            c => out.extend(c.to_lowercase()),
        }
    }
    out
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
        // Harmless if mouse capture was never enabled; forgetting it after a
        // panic under --mouse leaves the shell spewing escape sequences.
        DisableMouseCapture,
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

    // Throttled autosave so a crash or SIGKILL doesn't lose the reading
    // position the tool exists to remember.
    let mut saved_offset = app.offset;
    let mut last_save = std::time::Instant::now();

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
                Event::Resize(_, h) => {
                    // Width handling (measure clamp + table budget) happens
                    // in `draw` via `sync_measure`, which sees the new frame
                    // size on the next iteration.
                    app.last_viewport_h = h;
                }
                Event::Mouse(m) => handle_mouse(&mut app, m),
                Event::Paste(s) => handle_paste(&mut app, &s),
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

        if app.offset != saved_offset && last_save.elapsed() > Duration::from_secs(2) {
            app.cfg.store.set(&app.cfg.path, app.offset).ok();
            saved_offset = app.offset;
            last_save = std::time::Instant::now();
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
        // Wheeling over the TOC moves its selection, not the hidden document.
        (MouseEventKind::ScrollUp | MouseEventKind::ScrollDown, Mode::Toc { filter, .. }) => {
            let len = toc_filter_indices(&app.rendered.toc, filter.as_deref().unwrap_or("")).len();
            let delta: isize = if matches!(ev.kind, MouseEventKind::ScrollUp) {
                -1
            } else {
                1
            };
            if let Mode::Toc { selected, .. } = &mut app.mode {
                let max = len.saturating_sub(1) as isize;
                *selected = (*selected as isize + delta).clamp(0, max) as usize;
            }
        }
        (MouseEventKind::ScrollUp | MouseEventKind::ScrollDown, Mode::LinkPick { idx }) => {
            let len = app.rendered.links.len();
            let delta: isize = if matches!(ev.kind, MouseEventKind::ScrollUp) {
                -1
            } else {
                1
            };
            let new = (*idx as isize + delta).clamp(0, len.saturating_sub(1) as isize) as usize;
            app.mode = Mode::LinkPick { idx: new };
            app.ensure_link_visible(new);
        }
        // Modal overlays without their own scroll: don't move the document
        // underneath — the reader would lose their place invisibly.
        (_, Mode::Help | Mode::Search { .. } | Mode::ImageView { .. }) => {}
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
        from_pick,
    } = &app.mode
    else {
        return;
    };
    let (block_idx, from_pick) = (*block_idx, *from_pick);
    let Some(block) = app.rendered.code_blocks.get(block_idx) else {
        app.mode = Mode::Reading;
        return;
    };
    let total = block.code.split('\n').count().max(1);
    let max_w = max_source_line_width(block);
    let new_line = (*line_idx as isize + dy).clamp(0, total as isize - 1) as usize;
    let new_off = (*h_off as isize + dx).clamp(0, max_w.saturating_sub(1) as isize) as usize;
    app.mode = Mode::RawCode {
        block_idx,
        line_idx: new_line,
        h_off: new_off,
        from_pick,
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
        Mode::LinkPick { .. } => handle_key_link_pick(app, key),
        Mode::RawCode { .. } => handle_key_raw_code(app, key),
        Mode::ImageView { .. } => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('q' | 'i')) {
                app.mode = Mode::Reading;
            }
            Ok(false)
        }
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
    // Ctrl chords get their own (less/vim-style) bindings; without this gate
    // crossterm's Char('e') + CONTROL would fall through to plain `e` and,
    // say, launch $EDITOR on Ctrl-E.
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('d') => app.scroll(page / 2),
            KeyCode::Char('u') => app.scroll(-(page / 2)),
            KeyCode::Char('f') => app.scroll(page),
            KeyCode::Char('b') => app.scroll(-page),
            KeyCode::Char('e') => app.scroll(1),
            KeyCode::Char('y') => app.scroll(-1),
            _ => {}
        }
        return Ok(false);
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        return Ok(false);
    }
    match key.code {
        KeyCode::Char('q') => return Ok(true),
        // Esc means "dismiss" everywhere else in the app; at top level it
        // dismisses the active search highlight rather than quitting.
        KeyCode::Esc => app.clear_search(),
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
                app.mode = Mode::Toc {
                    selected,
                    filter: None,
                };
            }
        }
        KeyCode::Char('T') => app.cycle_theme(),
        KeyCode::Char('L') => app.cycle_layout(),
        KeyCode::Char('A') => app.toggle_align(),
        KeyCode::Char('W') => app.toggle_wrap_code(),
        KeyCode::Char('/') => {
            app.search_origin = Some(app.offset);
            app.mode = Mode::Search {
                input: String::new(),
                cursor: 0,
            };
        }
        KeyCode::Char('n') | KeyCode::Tab | KeyCode::Right => app.advance_search(true),
        KeyCode::Char('N') | KeyCode::BackTab | KeyCode::Left => app.advance_search(false),
        KeyCode::Char('c') if !app.search_matches.is_empty() => app.clear_search(),
        KeyCode::Char('y') => app.copy_current_code_block(),
        KeyCode::Char('Y') => app.enter_line_pick(),
        KeyCode::Char('R') => app.enter_raw_code(),
        KeyCode::Char('r') => app.reload_from_disk(),
        KeyCode::Char('o') => app.enter_link_pick(),
        KeyCode::Char('i') => app.enter_image_view(),
        KeyCode::Char(']') => app.switch_doc(1),
        KeyCode::Char('[') => app.switch_doc(-1),
        KeyCode::Char('e') => app.pending_editor = true,
        KeyCode::Char('?') => {
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
    let line_count = app
        .rendered
        .code_blocks
        .get(block_idx)
        .map_or(0, |b| b.line_visuals.len());
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
                from_pick: true,
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

fn handle_key_link_pick(app: &mut App, key: KeyEvent) -> Result<bool> {
    let Mode::LinkPick { idx } = &app.mode else {
        return Ok(false);
    };
    let mut idx = *idx;
    let len = app.rendered.links.len();
    if len == 0 {
        app.mode = Mode::Reading;
        return Ok(false);
    }
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.mode = Mode::Reading;
            return Ok(false);
        }
        KeyCode::Char('j' | 'n') | KeyCode::Down | KeyCode::Tab => {
            idx = (idx + 1).min(len - 1);
        }
        KeyCode::Char('k' | 'N') | KeyCode::Up | KeyCode::BackTab => {
            idx = idx.saturating_sub(1);
        }
        KeyCode::Char('g') | KeyCode::Home => idx = 0,
        KeyCode::Char('G') | KeyCode::End => idx = len - 1,
        KeyCode::Enter | KeyCode::Char('o') => {
            app.open_link(idx);
            return Ok(false);
        }
        _ => {}
    }
    app.mode = Mode::LinkPick { idx };
    app.ensure_link_visible(idx);
    Ok(false)
}

fn handle_key_raw_code(app: &mut App, key: KeyEvent) -> Result<bool> {
    let (block_idx, mut line_idx, mut h_off, from_pick) = {
        let Mode::RawCode {
            block_idx,
            line_idx,
            h_off,
            from_pick,
        } = &app.mode
        else {
            return Ok(false);
        };
        (*block_idx, *line_idx, *h_off, *from_pick)
    };
    let Some(block) = app.rendered.code_blocks.get(block_idx) else {
        app.mode = Mode::Reading;
        return Ok(false);
    };
    let total_lines = block.code.split('\n').count().max(1);
    let max_line_w = max_source_line_width(block);
    let pan_step: usize = 8;

    match key.code {
        KeyCode::Esc | KeyCode::Char('q' | 'R') => {
            // Return to where the overlay was opened from.
            app.mode = if from_pick {
                Mode::LinePick {
                    block_idx,
                    line_idx,
                }
            } else {
                Mode::Reading
            };
            if from_pick {
                app.ensure_code_line_visible(block_idx, line_idx);
            }
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
        }
        KeyCode::Char('y') | KeyCode::Enter => {
            app.copy_source_line(block_idx, line_idx);
        }
        KeyCode::Char('Y') => {
            app.copy_whole_block(block_idx);
        }
        _ => {}
    }

    app.mode = Mode::RawCode {
        block_idx,
        line_idx,
        h_off,
        from_pick,
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

/// Indices of TOC entries whose titles contain `filter`
/// (case-insensitive); all entries when the filter is empty.
fn toc_filter_indices(toc: &[TocEntry], filter: &str) -> Vec<usize> {
    let f = filter.trim().to_lowercase();
    if f.is_empty() {
        return (0..toc.len()).collect();
    }
    toc.iter()
        .enumerate()
        .filter(|(_, e)| e.title.to_lowercase().contains(&f))
        .map(|(i, _)| i)
        .collect()
}

fn handle_key_toc(app: &mut App, key: KeyEvent) -> Result<bool> {
    let (mut selected, mut filter) = {
        let Mode::Toc { selected, filter } = &mut app.mode else {
            return Ok(false);
        };
        (*selected, filter.take())
    };
    let indices = toc_filter_indices(&app.rendered.toc, filter.as_deref().unwrap_or(""));
    let len = indices.len();

    // Type-to-filter: printable keys edit the filter; navigation stays on
    // the arrows (j/k would collide with typing).
    if let Some(fl) = filter.as_mut() {
        let chord = key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => {
                // Back to the unfiltered list, keeping the current pick.
                let abs = indices.get(selected).copied().unwrap_or(0);
                app.mode = Mode::Toc {
                    selected: abs,
                    filter: None,
                };
                return Ok(false);
            }
            KeyCode::Enter => {
                if let Some(&abs) = indices.get(selected) {
                    let line = app.rendered.toc[abs].line;
                    app.jump_to(line);
                    app.mode = Mode::Reading;
                    return Ok(false);
                }
            }
            KeyCode::Backspace => {
                fl.pop();
                let new_len = toc_filter_indices(&app.rendered.toc, fl).len();
                selected = selected.min(new_len.saturating_sub(1));
            }
            KeyCode::Down | KeyCode::Tab if len > 0 => {
                selected = (selected + 1).min(len - 1);
            }
            KeyCode::Up | KeyCode::BackTab => {
                selected = selected.saturating_sub(1);
            }
            KeyCode::PageDown if len > 0 => {
                selected = (selected + 10).min(len - 1);
            }
            KeyCode::PageUp => {
                selected = selected.saturating_sub(10);
            }
            KeyCode::Char(c) if !chord && fl.chars().count() < 64 => {
                fl.push(c);
                selected = 0;
            }
            _ => {}
        }
        app.mode = Mode::Toc { selected, filter };
        return Ok(false);
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q' | 't') => {
            app.mode = Mode::Reading;
            return Ok(false);
        }
        KeyCode::Char('/') => {
            filter = Some(String::new());
        }
        KeyCode::Char('j') | KeyCode::Down if len > 0 => {
            selected = (selected + 1).min(len - 1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            selected = selected.saturating_sub(1);
        }
        KeyCode::Char('d' | ' ') | KeyCode::PageDown if len > 0 => {
            selected = (selected + 10).min(len - 1);
        }
        KeyCode::Char('u' | 'b') | KeyCode::PageUp => {
            selected = selected.saturating_sub(10);
        }
        KeyCode::Char('g') | KeyCode::Home => {
            selected = 0;
        }
        KeyCode::Char('G') | KeyCode::End if len > 0 => {
            selected = len - 1;
        }
        KeyCode::Enter => {
            if let Some(e) = app.rendered.toc.get(selected) {
                let line = e.line;
                app.jump_to(line);
                app.mode = Mode::Reading;
                return Ok(false);
            }
        }
        _ => {}
    }
    app.mode = Mode::Toc { selected, filter };
    Ok(false)
}

/// Byte offset of char index `cursor` in `s` (clamped to the end).
fn byte_at(s: &str, cursor: usize) -> usize {
    s.char_indices().nth(cursor).map_or(s.len(), |(i, _)| i)
}

fn handle_key_search(app: &mut App, key: KeyEvent) -> Result<bool> {
    // Extract the current input as owned data so we can mutate App freely.
    let (mut input, mut cursor) = {
        let Mode::Search { input, cursor } = &mut app.mode else {
            return Ok(false);
        };
        (std::mem::take(input), *cursor)
    };
    let len = input.chars().count();
    cursor = cursor.min(len);
    let mut changed = false;
    let mut action = SearchAction::Continue;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let chord = key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
    match key.code {
        KeyCode::Esc => {
            action = SearchAction::Cancel;
        }
        KeyCode::Enter => {
            action = SearchAction::Commit;
        }
        KeyCode::Backspace => {
            if cursor > 0 {
                input.remove(byte_at(&input, cursor - 1));
                cursor -= 1;
                changed = true;
            }
        }
        KeyCode::Delete => {
            if cursor < len {
                input.remove(byte_at(&input, cursor));
                changed = true;
            }
        }
        KeyCode::Left => cursor = cursor.saturating_sub(1),
        KeyCode::Right => cursor = (cursor + 1).min(len),
        KeyCode::Home => cursor = 0,
        KeyCode::End => cursor = len,
        KeyCode::Char('a') if ctrl => cursor = 0,
        KeyCode::Char('e') if ctrl => cursor = len,
        KeyCode::Char('u') if ctrl => {
            // Delete from start of input to the caret.
            input = input.chars().skip(cursor).collect();
            cursor = 0;
            changed = true;
        }
        KeyCode::Char('w') if ctrl => {
            // Delete the word before the caret.
            let head: Vec<char> = input.chars().take(cursor).collect();
            let tail: String = input.chars().skip(cursor).collect();
            let mut keep = head.len();
            while keep > 0 && head[keep - 1].is_whitespace() {
                keep -= 1;
            }
            while keep > 0 && !head[keep - 1].is_whitespace() {
                keep -= 1;
            }
            input = head[..keep].iter().collect::<String>() + &tail;
            changed = cursor != keep;
            cursor = keep;
        }
        KeyCode::Tab | KeyCode::Down => {
            app.advance_search(true);
        }
        KeyCode::BackTab | KeyCode::Up => {
            app.advance_search(false);
        }
        KeyCode::Char(c) if !chord && len < 256 => {
            input.insert(byte_at(&input, cursor), c);
            cursor += 1;
            changed = true;
        }
        _ => {}
    }

    match action {
        SearchAction::Continue => {
            if changed {
                // Preview: update matches live and scroll to the nearest one.
                let preview = input.clone();
                app.update_matches(&preview);
                app.snap_to_match_near(app.search_origin.unwrap_or(0));
            }
            app.mode = Mode::Search { input, cursor };
        }
        SearchAction::Commit => {
            app.mode = Mode::Reading;
            app.commit_search(&input);
        }
        SearchAction::Cancel => {
            app.mode = Mode::Reading;
            app.clear_search();
            // Put the reader back where they were before the live preview
            // moved the view.
            if let Some(origin) = app.search_origin.take() {
                app.jump_to(origin);
            }
        }
    }
    Ok(false)
}

/// Bracketed paste: only meaningful in the search prompt, where the natural
/// flow is copy an error message, `/`, paste. Newlines/tabs fold to spaces;
/// text is inserted at the caret.
fn handle_paste(app: &mut App, pasted: &str) {
    let (mut input, mut cursor) = {
        let Mode::Search { input, cursor } = &mut app.mode else {
            return;
        };
        (std::mem::take(input), *cursor)
    };
    cursor = cursor.min(input.chars().count());
    for c in pasted.chars() {
        let c = if matches!(c, '\n' | '\r' | '\t') {
            ' '
        } else {
            c
        };
        if c.is_control() {
            continue;
        }
        if input.chars().count() >= 256 {
            break;
        }
        input.insert(byte_at(&input, cursor), c);
        cursor += 1;
    }
    app.update_matches(&input);
    app.snap_to_match_near(app.search_origin.unwrap_or(0));
    app.mode = Mode::Search { input, cursor };
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

/// Case-insensitive match against TOC entry titles. An exact title match
/// wins; otherwise the first entry (in document order) whose title contains
/// the query is returned.
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
    app.sync_measure(size.width);
    app.offset = app.offset.min(app.max_offset());

    // Paint the whole background so terminal defaults don't leak through.
    let bg_block = Block::default().style(app.theme.base_style());
    f.render_widget(bg_block, size);

    let tab_rows = app.tab_rows();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(tab_rows),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(size);
    let tabs_rect = vertical[0];
    let body_rect = vertical[1];
    let footer_rect = vertical[2];

    if tab_rows > 0 {
        let titles: Vec<Line<'static>> = app
            .cfg
            .docs
            .iter()
            .map(|d| Line::from(shorten_middle(&d.display_name, 24)))
            .collect();
        let tabs = Tabs::new(titles)
            .select(app.cfg.current)
            .style(app.theme.dim_style())
            .highlight_style(app.theme.accent_style().add_modifier(Modifier::BOLD))
            .divider(Span::styled("\u{2502}", app.theme.rule_style()));
        f.render_widget(tabs, tabs_rect);
    }

    let target = app.render_measure as u16;
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
    draw_scrollbar(f, app, body_rect);
    draw_footer(f, app, footer_rect);

    if matches!(app.mode, Mode::ImageView { .. }) {
        draw_image_overlay(f, app, size);
        return;
    }
    match &app.mode {
        Mode::Toc { selected, filter } => {
            draw_toc_overlay(f, app, *selected, filter.as_deref(), size);
        }
        Mode::Search { input, cursor } => draw_search_overlay(f, app, input, *cursor, size),
        Mode::Help => draw_help_overlay(f, app, size),
        Mode::RawCode {
            block_idx,
            line_idx,
            h_off,
            ..
        } => draw_raw_code_overlay(f, app, *block_idx, *line_idx, *h_off, size),
        Mode::Reading | Mode::LinePick { .. } | Mode::LinkPick { .. } | Mode::ImageView { .. } => {}
    }
}

/// Full-screen image preview. The protocol object encodes the picture for
/// the terminal's best supported transport (kitty/sixel/iTerm2/halfblocks).
fn draw_image_overlay(f: &mut ratatui::Frame<'_>, app: &mut App, area: Rect) {
    let base = app.theme.base_style();
    let rule = app.theme.rule_style();
    let Mode::ImageView { protocol, title } = &mut app.mode else {
        return;
    };
    let margin_x: u16 = 2;
    let margin_y: u16 = 1;
    let rect = Rect {
        x: area.x + margin_x,
        y: area.y + margin_y,
        width: area.width.saturating_sub(margin_x * 2).max(10),
        height: area.height.saturating_sub(margin_y * 2).max(4),
    }
    .intersection(area);
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(format!(" {title} \u{2014} Esc close "))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(base)
        .border_style(rule);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let widget = ratatui_image::StatefulImage::default();
    f.render_stateful_widget(widget, inner, protocol.as_mut());
}

/// Thin scrollbar on the right edge of the body, with search-match tick
/// marks so the counter (`5/12`) has a spatial meaning. Hidden when the
/// whole document fits.
fn draw_scrollbar(f: &mut ratatui::Frame<'_>, app: &App, body: Rect) {
    let total = app.rendered.lines.len();
    let view = body.height as usize;
    if total <= view || body.width == 0 || view == 0 {
        return;
    }
    let bar = Rect {
        x: body.right().saturating_sub(1),
        y: body.y,
        width: 1,
        height: body.height,
    };
    let mut state = ScrollbarState::new(total.saturating_sub(view))
        .position(app.offset)
        .viewport_content_length(view);
    let sb = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("\u{2502}"))
        .track_style(app.theme.rule_style())
        .thumb_symbol("\u{2503}")
        .thumb_style(app.theme.dim_style());
    f.render_stateful_widget(sb, bar, &mut state);

    // Search-match ticks: one accent mark per match, mapped onto the track.
    if !app.search_matches.is_empty() {
        let buf = f.buffer_mut();
        let denom = total.saturating_sub(1).max(1);
        for &m in &app.search_matches {
            let y = bar.y + ((m * (view - 1)) / denom).min(view - 1) as u16;
            if let Some(cell) = buf.cell_mut((bar.x, y)) {
                cell.set_symbol("\u{25AA}");
                cell.set_style(app.theme.accent_style());
            }
        }
    }
}

fn draw_body(f: &mut ratatui::Frame<'_>, app: &App, rect: Rect) {
    let total = app.rendered.lines.len();
    let start = app.offset.min(total);
    let end = (start + rect.height as usize).min(total);

    let mut display: Vec<Line<'static>> = app.rendered.lines[start..end].to_vec();

    // Mark every visible match: the current one reversed, the others with an
    // underlined accent — so `5/12 matches` has a visible spatial meaning.
    if !app.search_query.is_empty() && !app.search_matches.is_empty() {
        let current = app.search_matches.get(app.search_cursor).copied();
        let other_style = Style::default()
            .fg(app.theme.accent)
            .add_modifier(Modifier::UNDERLINED);
        let current_style = Style::default().add_modifier(Modifier::REVERSED);
        for row in start..end {
            if app.search_matches.binary_search(&row).is_ok() {
                let hl = if Some(row) == current {
                    current_style
                } else {
                    other_style
                };
                display[row - start] = highlight_query_in_line(
                    &display[row - start],
                    &app.search_query,
                    hl,
                    app.theme.base_style(),
                );
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
                        display[rel] = patch_line(&display[rel], hl_style, app.theme.base_style());
                    }
                }
            }
        }
    }

    // LinkPick: highlight the selected link's line.
    if let Mode::LinkPick { idx } = &app.mode {
        if let Some(link) = app.rendered.links.get(*idx) {
            if (start..end).contains(&link.line) {
                let rel = link.line - start;
                let hl = Style::default().add_modifier(Modifier::REVERSED);
                display[rel] = patch_line(&display[rel], hl, app.theme.base_style());
            }
        }
    }

    // No widget-level wrap: lines are pre-wrapped to the effective measure,
    // and re-wrapping here would break the line-per-row scroll math.
    let para = Paragraph::new(display).style(app.theme.base_style());
    f.render_widget(para, rect);
}

/// Re-style every span of a line with `patch` applied on top (used for
/// whole-line selection highlights).
fn patch_line(line: &Line<'static>, patch: Style, base: Style) -> Line<'static> {
    let spans: Vec<Span<'static>> = line
        .spans
        .iter()
        .map(|s| Span::styled(s.content.clone().into_owned(), s.style.patch(patch)))
        .collect();
    Line::from(spans).style(base)
}

/// Apply `hl` to just the substrings of `line` that match the normalized
/// query. Works in normalized-character space: each display char expands to
/// its `normalize_for_search` form with a back-pointer, the query is located
/// in that space, and the matched display chars are re-styled.
fn highlight_query_in_line(
    line: &Line<'static>,
    query_norm: &str,
    hl: Style,
    base: Style,
) -> Line<'static> {
    let needle: Vec<char> = query_norm.chars().collect();
    if needle.is_empty() {
        return line.clone();
    }

    // Flatten to (char, style) and build the normalized view with owners.
    let mut display: Vec<(char, Style)> = Vec::new();
    for span in &line.spans {
        for ch in span.content.chars() {
            display.push((ch, span.style));
        }
    }
    let mut norm: Vec<char> = Vec::with_capacity(display.len());
    let mut owner: Vec<usize> = Vec::with_capacity(display.len());
    let mut one = [0u8; 4];
    for (d, (ch, _)) in display.iter().enumerate() {
        for nc in normalize_for_search(ch.encode_utf8(&mut one)).chars() {
            norm.push(nc);
            owner.push(d);
        }
    }

    let mut marked = vec![false; display.len()];
    let mut i = 0usize;
    while i + needle.len() <= norm.len() {
        if norm[i..i + needle.len()] == needle[..] {
            for &o in &owner[i..i + needle.len()] {
                marked[o] = true;
            }
            i += needle.len();
        } else {
            i += 1;
        }
    }

    // Rebuild spans, grouping runs of identical (style, marked).
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut buf = String::new();
    let mut cur: Option<(Style, bool)> = None;
    for (d, (ch, style)) in display.iter().enumerate() {
        let key = (*style, marked[d]);
        if cur != Some(key) {
            if let Some((s, m)) = cur.take() {
                let st = if m { s.patch(hl) } else { s };
                spans.push(Span::styled(std::mem::take(&mut buf), st));
            }
            cur = Some(key);
        }
        buf.push(*ch);
    }
    if let Some((s, m)) = cur {
        let st = if m { s.patch(hl) } else { s };
        spans.push(Span::styled(buf, st));
    }
    Line::from(spans).style(base)
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
    //   2. Transient status message (fades after 3s) — above the match
    //      counter so copy/reload feedback isn't invisible during a search.
    //   3. Active search match counter.
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
    } else if let Mode::LinkPick { idx } = &app.mode {
        let n = app.rendered.links.len().max(1);
        let url = app
            .rendered
            .links
            .get(*idx)
            .map_or(String::new(), |l| shorten_middle(&l.url, 32));
        Some((
            format!(
                "link {}/{n}: {url}  (Enter open \u{00B7} Esc exit)",
                idx + 1
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
    } else if !app.search_matches.is_empty() {
        Some((
            format!(
                "match {}/{}  (n/N or Tab/\u{2190}\u{2192}, c clear)",
                app.search_cursor + 1,
                app.search_matches.len()
            ),
            accent,
        ))
    } else {
        Some(("? help  q quit".to_string(), dim))
    };

    if let Some((text, style)) = trailing {
        spans.push(Span::styled("  \u{00B7}  ".to_string(), dim));
        spans.push(Span::styled(text, style));
    }

    // Right side: reading progress as a subtle line gauge with the percent
    // as its label (replaces the bare `42%` text).
    let gauge_w = 16u16.min(rect.width / 3);
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(gauge_w)])
        .split(rect);

    let para = Paragraph::new(Line::from(spans)).style(app.theme.base_style());
    f.render_widget(para, cols[0]);

    if gauge_w >= 8 {
        let gauge = LineGauge::default()
            .ratio(f64::from(pct) / 100.0)
            .label(Span::styled(format!("{pct:>3}%"), dim))
            .filled_style(accent)
            .unfilled_style(app.theme.rule_style())
            .style(app.theme.base_style());
        f.render_widget(gauge, cols[1]);
    }
}

fn draw_toc_overlay(
    f: &mut ratatui::Frame<'_>,
    app: &App,
    selected: usize,
    filter: Option<&str>,
    area: Rect,
) {
    let w = (f32::from(area.width) * 0.7) as u16;
    let h = (f32::from(area.height) * 0.7) as u16;
    let x = (area.width - w) / 2 + area.x;
    let y = (area.height - h) / 2 + area.y;
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    }
    .intersection(area);
    if rect.width == 0 || rect.height == 0 {
        return;
    }

    let indices = toc_filter_indices(&app.rendered.toc, filter.unwrap_or(""));
    let title = match filter {
        Some(fl) => format!(
            " Table of contents \u{2014} /{fl}\u{2588} ({}) ",
            indices.len()
        ),
        None => " Table of contents ".to_string(),
    };

    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(app.theme.base_style())
        .border_style(app.theme.rule_style());
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let dim = app.theme.dim_style();
    let base = app.theme.base_style();
    if indices.is_empty() {
        let para = Paragraph::new(Line::styled("  no matching headings".to_string(), dim));
        f.render_widget(para.style(base), inner);
        return;
    }

    let items: Vec<ListItem<'static>> = indices
        .iter()
        .map(|&abs| {
            let entry = &app.rendered.toc[abs];
            // The tree connectors only make sense on the full list; a
            // filtered subset renders flat with a level indent instead.
            let (guides, branch) = if filter.is_none() {
                toc_tree_prefix(&app.rendered.toc, abs)
            } else {
                (
                    " ".repeat((entry.level.saturating_sub(1) as usize) * 2),
                    String::new(),
                )
            };
            let mut spans: Vec<Span<'static>> = Vec::new();
            if !guides.is_empty() {
                spans.push(Span::styled(guides, dim));
            }
            if !branch.is_empty() {
                spans.push(Span::styled(branch, dim));
            }
            spans.push(Span::styled(entry.title.clone(), base));
            ListItem::new(Line::from(spans).style(base))
        })
        .collect();

    // Keep the selection roughly centered, like a pager.
    let view = inner.height as usize;
    let offset = selected
        .saturating_sub(view / 2)
        .min(indices.len().saturating_sub(view));
    let mut state = ListState::default()
        .with_selected(Some(selected.min(indices.len() - 1)))
        .with_offset(offset);
    let list = List::new(items)
        .style(base)
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, inner, &mut state);

    if indices.len() > view {
        let mut sb_state = ScrollbarState::new(indices.len().saturating_sub(view))
            .position(state.offset())
            .viewport_content_length(view);
        let sb = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_style(app.theme.rule_style())
            .thumb_style(dim);
        f.render_stateful_widget(sb, rect, &mut sb_state);
    }
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

fn draw_search_overlay(
    f: &mut ratatui::Frame<'_>,
    app: &App,
    input: &str,
    cursor: usize,
    area: Rect,
) {
    let h = 4u16;
    let w = area.width.saturating_sub(8).min(80);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + area.height.saturating_sub(h + 2);
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    }
    .intersection(area);
    if rect.width == 0 || rect.height == 0 {
        return;
    }
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
    let hint = "Enter=confirm  Esc=cancel  Tab/\u{2193}=next  Shift-Tab/\u{2191}=prev";
    let mut hint_line = String::new();
    if hint.chars().count() <= hint_width {
        hint_line = hint.to_string();
    }
    // Caret-aware rendering: text before the caret, the char under the caret
    // reversed (or a block at the end), then the rest.
    let accent = app.theme.accent_style();
    let chars: Vec<char> = input.chars().collect();
    let cur = cursor.min(chars.len());
    let before: String = chars[..cur].iter().collect();
    let (at, after): (String, String) = if cur < chars.len() {
        (chars[cur].to_string(), chars[cur + 1..].iter().collect())
    } else {
        (String::new(), String::new())
    };
    let mut input_spans = vec![
        Span::styled("/ ".to_string(), app.theme.dim_style()),
        Span::styled(before, accent),
    ];
    if at.is_empty() {
        input_spans.push(Span::styled("\u{2588}".to_string(), app.theme.dim_style()));
    } else {
        input_spans.push(Span::styled(at, accent.add_modifier(Modifier::REVERSED)));
        input_spans.push(Span::styled(after, accent));
    }
    let input_line = Line::from(input_spans);
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
    // The .max floors can exceed a tiny frame; clamp so ratatui's buffer
    // writes stay in bounds instead of panicking.
    let rect = Rect {
        x: area.x + margin_x,
        y: area.y + margin_y,
        width: w,
        height: h,
    }
    .intersection(area);
    if rect.width == 0 || rect.height == 0 {
        return;
    }
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
        let mut line = Line::from(spans).style(base);
        if i == line_idx {
            line = patch_line(&line, hl_style, base);
        }
        lines.push(line);
    }
    // Pad out empty rows so the overlay fills its box consistently.
    while lines.len() < content_rows {
        lines.push(Line::styled(" ".repeat(full_cols), code_bg));
    }

    let hint = "j/k line  h/l pan  0/$ home/end  # line-nums  y copy line  Y all  Esc close";
    let hint_text = render::truncate_to_width(hint, full_cols);
    lines.push(Line::styled(hint_text, dim));

    let para = Paragraph::new(lines).style(base);
    f.render_widget(para, inner);

    // Scrollbars on the border: vertical for the line cursor, horizontal
    // for the pan offset. Drawn over the block border, standard ratatui
    // placement.
    if total > content_rows {
        let mut vs = ScrollbarState::new(total.saturating_sub(content_rows))
            .position(top)
            .viewport_content_length(content_rows);
        let sb = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_style(app.theme.rule_style())
            .thumb_style(dim);
        f.render_stateful_widget(sb, rect, &mut vs);
    }
    let max_w = max_source_line_width(block);
    if max_w > content_cols {
        let mut hs = ScrollbarState::new(max_w.saturating_sub(content_cols))
            .position(h_off.min(max_w))
            .viewport_content_length(content_cols);
        let sb = Scrollbar::new(ScrollbarOrientation::HorizontalBottom)
            .begin_symbol(None)
            .end_symbol(None)
            .track_style(app.theme.rule_style())
            .thumb_style(dim);
        f.render_stateful_widget(sb, rect, &mut hs);
    }
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

fn draw_help_overlay(f: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let rows: &[(&str, &str)] = &[
        ("j / \u{2193}", "scroll down"),
        ("k / \u{2191}", "scroll up"),
        ("space / PgDn", "page down"),
        ("b / PgUp", "page up"),
        ("d / u", "half page"),
        ("Ctrl-f/b/d/u", "page / half page"),
        ("g / Home", "top"),
        ("G / End", "bottom"),
        ("t", "table of contents (/ filters)"),
        ("T", "cycle theme"),
        ("L", "cycle layout"),
        ("A", "toggle align (center/left/right)"),
        ("W", "toggle code wrap / truncate"),
        ("/", "search"),
        ("n / N", "next / prev match"),
        ("Tab / \u{2192}", "next match"),
        ("Shift-Tab / \u{2190}", "prev match"),
        ("c / Esc", "clear active search"),
        ("y", "copy code block in view"),
        ("Y", "pick & copy a single code line"),
        ("R", "raw code view (no wrap, h/l pan)"),
        ("o", "pick & open a link"),
        ("i", "preview image (needs --images)"),
        ("] / [", "next / prev file (tabs)"),
        ("r", "reload current file"),
        ("e", "open in $EDITOR at this heading"),
        ("?", "toggle this help"),
        ("q", "quit"),
    ];
    // 2 border rows + rows + blank + close hint.
    let content_h = rows.len() as u16 + 2;
    let w = 58u16.min(area.width.saturating_sub(4));
    let h = (content_h + 2).min(area.height.saturating_sub(2));
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    let rect = Rect {
        x,
        y,
        width: w,
        height: h,
    }
    .intersection(area);
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(app.theme.base_style())
        .border_style(app.theme.rule_style());
    let inner = block.inner(rect);
    f.render_widget(block, rect);

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
    // Terminal too short for the full list: truncate with an indicator
    // instead of silently clipping mid-table.
    let visible = inner.height as usize;
    if lines.len() > visible && visible > 0 {
        lines.truncate(visible - 1);
        lines.push(Line::styled(
            "  \u{2026} resize the terminal for more".to_string(),
            dim,
        ));
    }
    let para = Paragraph::new(lines).style(base);
    f.render_widget(para, inner);
}

/// Middle-truncate `s` to at most `max` display columns (not chars — CJK
/// filenames are two columns per char and would otherwise blow the budget).
fn shorten_middle(s: &str, max: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if max < 5 {
        return String::new();
    }
    if UnicodeWidthStr::width(s) <= max {
        return s.to_string();
    }
    let keep = max - 3;
    let head_budget = keep / 2;
    let tail_budget = keep - head_budget;

    let mut head = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if w + cw > head_budget {
            break;
        }
        head.push(ch);
        w += cw;
    }
    let mut tail_rev: Vec<char> = Vec::new();
    let mut tw = 0usize;
    for ch in s.chars().rev() {
        let cw = UnicodeWidthChar::width(ch).unwrap_or(0);
        if tw + cw > tail_budget {
            break;
        }
        tail_rev.push(ch);
        tw += cw;
    }
    head.push_str("...");
    head.extend(tail_rev.iter().rev());
    head
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

    fn test_app(md: &str) -> App {
        App::new(AppConfig {
            path: PathBuf::from("/nonexistent/glum-test.md"),
            source: md.to_string(),
            docs: Vec::new(),
            current: 0,
            follow: false,
            measure: 40,
            theme: ThemeName::Plain,
            layout: LayoutName::Minimal,
            align: Align::Center,
            wrap_code: true,
            store: PositionStore::disabled(),
            display_name: "test.md".into(),
            initial: InitialState::default(),
            watcher: None,
            mouse: false,
            picker: None,
        })
    }

    #[test]
    fn normalize_folds_smart_typography() {
        assert_eq!(normalize_for_search("don\u{2019}t"), "don't");
        assert_eq!(normalize_for_search("\u{201C}Yes\u{201D}"), "\"yes\"");
        assert_eq!(normalize_for_search("a\u{2014}b"), "a--b");
        assert_eq!(normalize_for_search("wait\u{2026}"), "wait...");
        assert_eq!(normalize_for_search("\u{00A0}x\u{00A0}"), " x ");
    }

    #[test]
    fn search_matches_despite_smart_typography() {
        // The renderer smartens `don't` → `don’t` and `--` → `—`; queries
        // typed as the source must still match.
        let mut app = test_app("He said \"yes\" -- don't wait...\n");
        app.update_matches("don't");
        assert!(
            !app.search_matches.is_empty(),
            "apostrophe query must match"
        );
        app.update_matches("--");
        assert!(!app.search_matches.is_empty(), "dash query must match");
        app.update_matches("\"yes\"");
        assert!(!app.search_matches.is_empty(), "quote query must match");
    }

    #[test]
    fn search_matches_inline_code() {
        let mut app = test_app("run `glum` now\n");
        app.update_matches("glum");
        assert!(!app.search_matches.is_empty());
    }

    #[test]
    fn snap_prefers_match_at_or_after_origin() {
        let md = "needle\n\nfiller\n\nfiller\n\nneedle again\n";
        let mut app = test_app(md);
        app.update_matches("needle");
        assert_eq!(app.search_matches.len(), 2);
        let second = app.search_matches[1];
        app.snap_to_match_near(second);
        assert_eq!(app.search_cursor, 1);
        // Past the last match → wraps to the first.
        app.snap_to_match_near(second + 100);
        assert_eq!(app.search_cursor, 0);
    }

    #[test]
    fn g_jumps_to_last_page_not_last_line() {
        let md = "line\n\n".repeat(100);
        let mut app = test_app(&md);
        app.last_viewport_h = 12; // body rows = 10
        app.jump_to(app.total_lines());
        // Clamped so the last line sits at the bottom of the body, not the
        // top of an otherwise blank screen.
        assert_eq!(app.offset, app.total_lines() - 10);
        // Scrolling can't exceed the same bound.
        app.scroll(1000);
        assert_eq!(app.offset, app.max_offset());
    }

    #[test]
    fn stale_block_mode_resets_to_reading() {
        let mut app = test_app("```\ncode\n```\n");
        app.mode = Mode::RawCode {
            block_idx: 5,
            line_idx: 0,
            h_off: 0,
            from_pick: false,
        };
        app.validate_block_mode();
        assert!(matches!(app.mode, Mode::Reading));

        app.mode = Mode::LinePick {
            block_idx: 0,
            line_idx: 99,
        };
        app.validate_block_mode();
        assert!(matches!(app.mode, Mode::Reading));

        // Valid indices survive.
        app.mode = Mode::LinePick {
            block_idx: 0,
            line_idx: 0,
        };
        app.validate_block_mode();
        assert!(matches!(app.mode, Mode::LinePick { .. }));
    }

    #[test]
    fn esc_in_reading_clears_search_instead_of_quitting() {
        let mut app = test_app("needle\n");
        app.update_matches("needle");
        assert!(!app.search_matches.is_empty());
        let quit =
            handle_key_reading(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).unwrap();
        assert!(!quit, "Esc must not quit from reading mode");
        assert!(app.search_matches.is_empty(), "Esc clears the search");
    }

    #[test]
    fn ctrl_chords_do_not_trigger_plain_bindings() {
        let mut app = test_app("hello\n");
        // Ctrl-E must scroll, not request $EDITOR.
        let quit = handle_key_reading(
            &mut app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::CONTROL),
        )
        .unwrap();
        assert!(!quit);
        assert!(!app.pending_editor, "Ctrl-E must not open the editor");
        // Ctrl-Q must not quit (only q and Ctrl-C quit).
        let quit = handle_key_reading(
            &mut app,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
        )
        .unwrap();
        assert!(!quit);
    }

    #[test]
    fn narrow_terminal_clamps_render_measure() {
        let mut app = test_app(&"word ".repeat(200));
        assert_eq!(app.render_measure, 40);
        app.sync_measure(30); // narrower than the measure
        assert_eq!(app.render_measure, 28);
        for line in &app.rendered.lines {
            assert!(line.width() <= 28 + 1, "line too wide: {}", line.width());
        }
        app.sync_measure(120); // back to wide → full measure restored
        assert_eq!(app.render_measure, 40);
    }

    #[test]
    fn toc_filter_matches_case_insensitive_substring() {
        let toc = vec![
            TocEntry {
                level: 1,
                title: "Installation".into(),
                line: 0,
                source_line: 1,
            },
            TocEntry {
                level: 2,
                title: "Code blocks".into(),
                line: 10,
                source_line: 2,
            },
            TocEntry {
                level: 2,
                title: "Copy".into(),
                line: 20,
                source_line: 3,
            },
        ];
        assert_eq!(toc_filter_indices(&toc, ""), vec![0, 1, 2]);
        assert_eq!(toc_filter_indices(&toc, "co"), vec![1, 2]);
        assert_eq!(toc_filter_indices(&toc, "BLOCKS"), vec![1]);
        assert!(toc_filter_indices(&toc, "zzz").is_empty());
    }

    #[test]
    fn toc_type_to_filter_narrows_and_jumps() {
        let md = "# Alpha\n\ntext\n\n# Beta\n\ntext\n\n# Alpha two\n\ntext\n";
        let mut app = test_app(md);
        app.mode = Mode::Toc {
            selected: 0,
            filter: None,
        };
        let key = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        handle_key_toc(&mut app, key('/')).unwrap();
        handle_key_toc(&mut app, key('b')).unwrap();
        handle_key_toc(&mut app, key('e')).unwrap();
        let Mode::Toc {
            selected,
            filter: Some(f),
        } = &app.mode
        else {
            panic!("expected filtering TOC mode");
        };
        assert_eq!(f, "be");
        assert_eq!(*selected, 0);
        // Enter jumps to "Beta" (the only match) and closes the overlay.
        handle_key_toc(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();
        assert!(matches!(app.mode, Mode::Reading));
        assert_eq!(app.offset, app.rendered.toc[1].line.min(app.max_offset()));
    }

    #[test]
    fn slugify_matches_github_anchors() {
        assert_eq!(slugify("Code blocks"), "code-blocks");
        assert_eq!(
            slugify("Position and preference memory"),
            "position-and-preference-memory"
        );
        assert_eq!(slugify("What's new?"), "whats-new");
    }

    #[test]
    fn anchor_link_jumps_to_heading() {
        let md = "# Intro\n\nJump to [usage](#code-blocks).\n\ntext\n\n# Code blocks\n\nbody\n";
        let mut app = test_app(md);
        assert_eq!(app.rendered.links.len(), 1);
        app.mode = Mode::LinkPick { idx: 0 };
        app.open_link(0);
        assert!(matches!(app.mode, Mode::Reading));
        let heading_line = app.rendered.toc[1].line;
        assert_eq!(app.offset, heading_line.min(app.max_offset()));
    }

    #[test]
    fn highlight_query_marks_substring_only() {
        let line = Line::from(vec![Span::styled(
            "find the needle here".to_string(),
            Style::default(),
        )]);
        let hl = Style::default().add_modifier(Modifier::REVERSED);
        let out = highlight_query_in_line(&line, "needle", hl, Style::default());
        let marked: String = out
            .spans
            .iter()
            .filter(|s| s.style.add_modifier.contains(Modifier::REVERSED))
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(marked, "needle");
        assert_eq!(out.to_string(), "find the needle here");
    }
}
