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

use crate::positions::PositionStore;
use crate::render::{self, Rendered, TocEntry};
use crate::theme::{Theme, ThemeName};

pub struct AppConfig {
    pub path: PathBuf,
    pub source: String,
    pub measure: u16,
    pub theme: ThemeName,
    pub store: PositionStore,
    pub display_name: String,
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
    rendered: Rendered,
    offset: usize,
    last_viewport_h: u16,
    mode: Mode,
    search_matches: Vec<usize>,
    search_cursor: usize,
    status: Option<(String, std::time::Instant)>,
}

impl App {
    fn new(cfg: AppConfig) -> Self {
        let theme_name = cfg.theme;
        let theme = Theme::resolve(theme_name);
        let rendered = render::render(&cfg.source, cfg.measure as usize, theme);
        let offset = cfg.store.get(&cfg.path).map_or(0, |e| e.offset).min(rendered.lines.len().saturating_sub(1));
        Self {
            cfg,
            theme,
            theme_name,
            rendered,
            offset,
            last_viewport_h: 0,
            mode: Mode::Reading,
            search_matches: Vec::new(),
            search_cursor: 0,
            status: None,
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
        self.rendered = render::render(&self.cfg.source, self.cfg.measure as usize, self.theme);
        self.offset = self.offset.min(self.total_lines().saturating_sub(1));
        self.set_status(format!("theme: {}", self.theme_name.label()));
    }

    fn percent(&self) -> u16 {
        let total = self.total_lines();
        if total <= 1 {
            return 100;
        }
        let visible_end = (self.offset + self.last_viewport_h.saturating_sub(2) as usize).min(total);
        ((visible_end as f64 / total as f64) * 100.0).round().clamp(0.0, 100.0) as u16
    }

    fn run_search(&mut self, needle: &str) {
        self.search_matches.clear();
        if needle.is_empty() {
            return;
        }
        let n_lower = needle.to_lowercase();
        for (i, line) in self.rendered.lines.iter().enumerate() {
            let s = line.to_string().to_lowercase();
            if s.contains(&n_lower) {
                self.search_matches.push(i);
            }
        }
        self.search_cursor = 0;
        if let Some(&first) = self.search_matches.first() {
            self.jump_to(first);
            self.set_status(format!("match 1/{}", self.search_matches.len()));
        } else {
            self.set_status("no matches");
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
        self.set_status(format!("match {}/{}", self.search_cursor + 1, len));
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

fn run_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, cfg: AppConfig) -> Result<()> {
    let mut app = App::new(cfg);

    loop {
        terminal.draw(|f| draw(f, &mut app))?;

        // Poll with a timeout so status messages can fade.
        if event::poll(Duration::from_millis(500))? {
            match event::read()? {
                Event::Key(key) if (key.kind == KeyEventKind::Press || key.kind == KeyEventKind::Repeat)
                    && handle_key(&mut app, key)? =>
                {
                    break;
                }
                Event::Resize(_, h) => {
                    app.last_viewport_h = h;
                    app.rendered = render::render(&app.cfg.source, app.cfg.measure as usize, app.theme);
                    app.offset = app.offset.min(app.total_lines().saturating_sub(1));
                }
                _ => {}
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
        KeyCode::Char('/') => {
            app.mode = Mode::Search { input: String::new() };
        }
        KeyCode::Char('n') => app.advance_search(true),
        KeyCode::Char('N') => app.advance_search(false),
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
    let Mode::Search { input } = &mut app.mode else {
        return Ok(false);
    };
    match key.code {
        KeyCode::Esc => {
            app.mode = Mode::Reading;
        }
        KeyCode::Enter => {
            let needle = input.clone();
            app.mode = Mode::Reading;
            app.run_search(&needle);
        }
        KeyCode::Backspace => {
            input.pop();
        }
        KeyCode::Char(c) if input.chars().count() < 256 => {
            input.push(c);
        }
        _ => {}
    }
    Ok(false)
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
    let side = (body_rect.width.saturating_sub(wrap_width)) / 2;
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(side),
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

    let status_text = app.status.as_ref().map(|(s, _)| s.clone());

    let name = shorten_middle(&app.cfg.display_name, rect.width.saturating_sub(24) as usize);
    let pct = app.percent();

    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push(Span::styled(name, accent));
    spans.push(Span::styled("  \u{00B7}  ".to_string(), dim));
    spans.push(Span::styled(format!("{pct:>3}%"), dim));
    spans.push(Span::styled("  \u{00B7}  ".to_string(), dim));
    spans.push(Span::styled(app.theme_name.label().to_string(), dim));
    if let Some(s) = status_text {
        spans.push(Span::styled("  \u{00B7}  ".to_string(), dim));
        spans.push(Span::styled(s, accent));
    } else {
        spans.push(Span::styled("  \u{00B7}  ".to_string(), dim));
        spans.push(Span::styled("? help  q quit".to_string(), dim));
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
    let h = 3u16;
    let w = area.width.saturating_sub(8).min(80);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + area.height.saturating_sub(h + 2);
    let rect = Rect { x, y, width: w, height: h };
    f.render_widget(Clear, rect);
    let block = Block::default()
        .title(" Search ")
        .borders(Borders::ALL)
        .style(app.theme.base_style())
        .border_style(app.theme.rule_style());
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let line = Line::from(vec![
        Span::styled("/ ".to_string(), app.theme.dim_style()),
        Span::styled(input.to_string(), app.theme.accent_style()),
        Span::styled("\u{2588}".to_string(), app.theme.dim_style()),
    ]);
    let para = Paragraph::new(line).style(app.theme.base_style());
    f.render_widget(para, inner);
}

fn draw_help_overlay(f: &mut ratatui::Frame<'_>, app: &App, area: Rect) {
    let w = 52u16.min(area.width.saturating_sub(4));
    let h = 18u16.min(area.height.saturating_sub(4));
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
        ("/", "search"),
        ("n / N", "next / prev match"),
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
