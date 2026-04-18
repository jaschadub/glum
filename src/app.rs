//! TUI application loop: rendering, paging, theme cycling, TOC, search.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{cursor, execute, terminal};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use ratatui::Terminal;

use crate::clipboard;
use crate::layout::LayoutName;
use crate::positions::PositionStore;
use crate::render::{self, CodeBlockEntry, Rendered, TocEntry};
use crate::theme::{Theme, ThemeName};
use crate::watch::FileWatcher;

pub struct AppConfig {
    pub path: PathBuf,
    pub source: String,
    pub measure: u16,
    pub theme: ThemeName,
    pub layout: LayoutName,
    pub align: Align,
    /// When true, long code lines soft-wrap; when false, they truncate with `…`.
    pub wrap_code: bool,
    pub store: PositionStore,
    pub display_name: String,
    /// Optional opening behavior set from CLI flags.
    pub initial: InitialState,
    /// Enabled when `--follow` is active and the input is a real file.
    pub watcher: Option<FileWatcher>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Center,
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

    pub fn label(self) -> &'static str {
        match self {
            Self::Center => "center",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

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

#[derive(Debug, Default, Clone)]
pub struct InitialState {
    pub search: Option<String>,
    pub heading: Option<String>,
    pub reset_position: bool,
    pub open_toc: bool,
}

enum Mode {
    Reading,
    Toc { selected: usize },
    Search { input: String },
    Help,
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
    mode: Mode,
    search_matches: Vec<usize>,
    search_cursor: usize,
    status: Option<(String, std::time::Instant)>,
    /// Time of the last detected filesystem change; used to settle bursty
    /// editor writes before triggering a reload.
    pending_reload_at: Option<std::time::Instant>,
}

impl App {
    fn new(cfg: AppConfig) -> Self {
        let theme_name = cfg.theme;
        let theme = Theme::resolve(theme_name);
        let layout_name = cfg.layout;
        let align = cfg.align;
        let wrap_code = cfg.wrap_code;
        let rendered = render::render(
            &cfg.source,
            cfg.measure as usize,
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
            mode: Mode::Reading,
            search_matches: Vec::new(),
            search_cursor: 0,
            status: None,
            pending_reload_at: None,
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
                    self.theme,
                    self.layout_name,
                    self.wrap_code,
                );
                self.offset = self.offset.min(self.total_lines().saturating_sub(1));
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
        self.status = Some((msg.into(), std::time::Instant::now()));
    }

    fn jump_to(&mut self, line: usize) {
        let max = self.total_lines().saturating_sub(1);
        self.offset = line.min(max);
    }

    fn page_size(&self) -> usize {
        // Reserve 2 lines for footer/status, overlap by 2 for orientation.
        let body = self.last_viewport_h.saturating_sub(2) as usize;
        body.saturating_sub(2).max(1)
    }

    fn scroll(&mut self, delta: isize) {
        let max = self.total_lines().saturating_sub(1);
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
            self.theme,
            self.layout_name,
            self.wrap_code,
        );
        self.offset = self.offset.min(self.total_lines().saturating_sub(1));
    }

    fn toggle_wrap_code(&mut self) {
        self.wrap_code = !self.wrap_code;
        self.re_render();
        self.cfg
            .store
            .set_wrap_code(self.wrap_code)
            .ok();
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
        let visible_end = (self.offset + self.last_viewport_h.saturating_sub(2) as usize).min(total);
        ((visible_end as f64 / total as f64) * 100.0).round().clamp(0.0, 100.0) as u16
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
        if self.rendered.code_blocks.is_empty() {
            self.set_status("no code blocks");
            return;
        }
        let viewport_end = self
            .offset
            .saturating_add(self.last_viewport_h.saturating_sub(1) as usize);
        let block = pick_code_block(&self.rendered.code_blocks, self.offset, viewport_end);
        let Some(block) = block else {
            self.set_status("no code blocks");
            return;
        };

        match clipboard::copy(&block.code) {
            Ok(Some(n)) => self.set_status(format!("copied {n} bytes ({})", block.lang)),
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

pub fn run(cfg: AppConfig) -> Result<()> {
    // Install a panic hook so a crash in the TUI still restores the terminal
    // before the default hook prints the message. This prevents a panic from
    // leaving the user in raw mode with no cursor.
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore_raw_terminal();
        prev_hook(info);
    }));

    let mut guard = TerminalGuard::new()?;
    let result = run_loop(&mut guard.terminal, cfg);
    // TerminalGuard::drop will restore the terminal whether we succeeded or errored.
    drop(guard);

    // Leave the panic hook in place: tootles is a CLI, main() will exit promptly.
    result
}

struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalGuard {
    fn new() -> Result<Self> {
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
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend).context("building terminal")?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        terminal::disable_raw_mode().ok();
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

        if event::poll(poll_interval)? {
            match event::read()? {
                Event::Key(key)
                    if (key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat)
                        && handle_key(&mut app, key)? =>
                {
                    break;
                }
                Event::Resize(_, h) => {
                    app.last_viewport_h = h;
                    app.re_render();
                }
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
        if let Some((_, at)) = app.status {
            if at.elapsed() > Duration::from_secs(3) {
                app.status = None;
            }
        }
    }

    // Persist position on exit.
    app.cfg.store.set(&app.cfg.path, app.offset).ok();
    Ok(())
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
            app.mode = Mode::Search { input: String::new() };
        }
        KeyCode::Char('n') | KeyCode::Tab | KeyCode::Right => app.advance_search(true),
        KeyCode::Char('N') | KeyCode::BackTab | KeyCode::Left => app.advance_search(false),
        KeyCode::Char('c') if !app.search_matches.is_empty() => app.clear_search(),
        KeyCode::Char('y') => app.copy_current_code_block(),
        KeyCode::Char('?' | 'h') => {
            app.mode = Mode::Help;
        }
        _ => {}
    }
    Ok(false)
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
        KeyCode::Char('j') | KeyCode::Down
            if toc_len > 0 => {
                *selected = (*selected + 1).min(toc_len - 1);
                }
        KeyCode::Char('k') | KeyCode::Up => {
            *selected = selected.saturating_sub(1);
        }
        KeyCode::Char('g') | KeyCode::Home => {
            *selected = 0;
        }
        KeyCode::Char('G') | KeyCode::End
            if toc_len > 0 => {
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
fn pick_code_block(
    blocks: &[CodeBlockEntry],
    view_top: usize,
    view_bottom: usize,
) -> Option<&CodeBlockEntry> {
    // Intersects the viewport.
    if let Some(b) = blocks
        .iter()
        .find(|b| b.start_line <= view_bottom && b.end_line >= view_top)
    {
        return Some(b);
    }
    // Nearest above.
    let above = blocks.iter().rev().find(|b| b.end_line < view_top);
    if above.is_some() {
        return above;
    }
    blocks.iter().find(|b| b.start_line > view_bottom)
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
    toc.iter().find(|e| e.title.to_lowercase().contains(&q)).map(|e| e.line)
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
    let wrap_width = target.min(body_rect.width.saturating_sub(2));
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
        Mode::Reading => {}
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
                let hl_style = Style::default()
                    .add_modifier(Modifier::REVERSED);
                let marked: Vec<Span<'static>> = original
                    .spans
                    .into_iter()
                    .map(|s| Span::styled(s.content.into_owned(), s.style.patch(hl_style)))
                    .collect();
                display[rel] = Line::from(marked).style(app.theme.base_style());
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

    let name = shorten_middle(&app.cfg.display_name, rect.width.saturating_sub(32) as usize);
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
    //   1. Active search match counter (always show while matches exist).
    //   2. Transient status message.
    //   3. Help hint.
    let trailing = if !app.search_matches.is_empty() {
        Some((
            format!(
                "match {}/{}  (n/N or Tab/\u{2190}\u{2192}, c clear)",
                app.search_cursor + 1,
                app.search_matches.len()
            ),
            accent,
        ))
    } else if let Some((s, _)) = app.status.as_ref() {
        Some((s.clone(), accent))
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
    let rect = Rect { x, y, width: w, height: h };

    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(" Table of contents ")
        .borders(Borders::ALL)
        .style(app.theme.base_style())
        .border_style(app.theme.rule_style());
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let start = selected.saturating_sub(inner.height as usize / 2);
    let end = (start + inner.height as usize).min(app.rendered.toc.len());
    for (i, entry) in app.rendered.toc[start..end].iter().enumerate() {
        let abs = start + i;
        let indent = "  ".repeat(entry.level.saturating_sub(1) as usize);
        let content = format!("{indent}{}", entry.title);
        let mut style = app.theme.base_style();
        if abs == selected {
            style = style.add_modifier(Modifier::REVERSED);
        }
        lines.push(Line::styled(content, style));
    }
    let para = Paragraph::new(lines).style(app.theme.base_style());
    f.render_widget(para, inner);
}

fn draw_search_overlay(f: &mut ratatui::Frame<'_>, app: &App, input: &str, area: Rect) {
    let h = 4u16;
    let w = area.width.saturating_sub(8).min(80);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + area.height.saturating_sub(h + 2);
    let rect = Rect { x, y, width: w, height: h };
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
            if app.search_matches.len() == 1 { "" } else { "es" }
        )
    };
    let title = format!(" Search \u{2014} {count_text} ");

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
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

fn draw_help_overlay(f: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let w = 52u16.min(area.width.saturating_sub(4));
    let h = 22u16.min(area.height.saturating_sub(4));
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    let rect = Rect { x, y, width: w, height: h };
    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
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
            CodeBlockEntry { start_line: 10, end_line: 20, lang: "rust".into(), code: "a".into() },
            CodeBlockEntry { start_line: 30, end_line: 40, lang: "py".into(), code: "b".into() },
        ];
        let b = pick_code_block(&blocks, 15, 25).unwrap();
        assert_eq!(b.lang, "rust");
    }

    #[test]
    fn pick_code_block_falls_back_above() {
        let blocks = vec![
            CodeBlockEntry { start_line: 5, end_line: 7, lang: "rust".into(), code: "a".into() },
            CodeBlockEntry { start_line: 50, end_line: 60, lang: "py".into(), code: "b".into() },
        ];
        let b = pick_code_block(&blocks, 20, 25).unwrap();
        assert_eq!(b.lang, "rust");
    }

    #[test]
    fn pick_code_block_falls_back_below() {
        let blocks = vec![
            CodeBlockEntry { start_line: 50, end_line: 60, lang: "py".into(), code: "b".into() },
        ];
        let b = pick_code_block(&blocks, 0, 10).unwrap();
        assert_eq!(b.lang, "py");
    }

    #[test]
    fn find_heading_exact_wins_over_substring() {
        let toc = vec![
            TocEntry { level: 1, title: "Installation".into(), line: 10 },
            TocEntry { level: 2, title: "Install".into(), line: 42 },
        ];
        // "install" is a substring of both but an exact case-insensitive match
        // of the second → second wins.
        assert_eq!(find_heading(&toc, "install"), Some(42));
    }

    #[test]
    fn find_heading_substring_when_no_exact() {
        let toc = vec![
            TocEntry { level: 1, title: "Code blocks".into(), line: 100 },
            TocEntry { level: 2, title: "Quoting text".into(), line: 200 },
        ];
        assert_eq!(find_heading(&toc, "quot"), Some(200));
        assert_eq!(find_heading(&toc, "nonexistent"), None);
        assert_eq!(find_heading(&toc, ""), None);
    }

    #[test]
    fn current_toc_index_finds_nearest_heading_above() {
        let toc = vec![
            TocEntry { level: 1, title: "A".into(), line: 0 },
            TocEntry { level: 2, title: "B".into(), line: 10 },
            TocEntry { level: 2, title: "C".into(), line: 20 },
        ];
        assert_eq!(current_toc_index(&toc, 0), 0);
        assert_eq!(current_toc_index(&toc, 15), 1);
        assert_eq!(current_toc_index(&toc, 100), 2);
    }
}
