//! Parse markdown and lay it out into pre-styled, pre-wrapped lines for paging.
//!
//! Strategy: walk the pulldown-cmark event stream with a small state stack.
//! Inline text accumulates into a run list (text + style). On block boundaries,
//! the runs get concatenated, typographically smartened, word-wrapped to the
//! current measure, and then span styling is restitched onto the wrapped output.
//!
//! Code blocks, block quotes, and lists have dedicated prefixes/gutters and
//! their own wrap behavior so they read well without any visible markdown syntax.

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::highlight::highlight_line;
use crate::theme::Theme;
use crate::typography::smarten;

/// A single rendered document, consisting of styled lines plus a table of contents.
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    pub toc: Vec<TocEntry>,
}

#[derive(Debug, Clone)]
pub struct TocEntry {
    pub level: u8,
    pub title: String,
    /// Index into `Rendered::lines` where this heading starts.
    pub line: usize,
}

/// Entry point. Produces styled lines wrapped to `measure` columns.
pub fn render(md: &str, measure: usize, theme: Theme) -> Rendered {
    let measure = measure.max(20);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_TASKLISTS);

    let parser = Parser::new_ext(md, opts);
    let mut r = Renderer::new(measure, theme);
    for event in parser {
        r.handle(event);
    }
    r.finish()
}

/// One styled run of inline text inside a block.
struct Run {
    text: String,
    style: Style,
}

/// List context: ordered counter and marker indent (prefix width).
#[derive(Debug, Clone, Copy)]
struct ListCtx {
    ordered: Option<u64>,
    /// Columns of indentation before the marker.
    indent: usize,
}

struct Renderer {
    measure: usize,
    theme: Theme,
    out: Vec<Line<'static>>,
    toc: Vec<TocEntry>,

    // Inline accumulation
    runs: Vec<Run>,
    style_stack: Vec<Style>,
    current_style: Style,

    // Block state
    in_heading: Option<u8>,
    heading_text: String,
    in_code_block: bool,
    code_lang: String,
    code_buf: String,
    blockquote_depth: usize,
    list_stack: Vec<ListCtx>,
    pending_list_marker: Option<String>,
    in_link: Option<String>,
    link_refs: Vec<(String, String)>, // (label, url)

    // Tables
    in_table_head: bool,
    in_table_cell: bool,
    table_cell_buf: Vec<Run>,
    table_rows: Vec<Vec<Vec<Run>>>,
    table_head: Vec<Vec<Run>>,
}

impl Renderer {
    fn new(measure: usize, theme: Theme) -> Self {
        Self {
            measure,
            theme,
            out: Vec::new(),
            toc: Vec::new(),
            runs: Vec::new(),
            style_stack: Vec::new(),
            current_style: theme.base_style(),
            in_heading: None,
            heading_text: String::new(),
            in_code_block: false,
            code_lang: String::new(),
            code_buf: String::new(),
            blockquote_depth: 0,
            list_stack: Vec::new(),
            pending_list_marker: None,
            in_link: None,
            link_refs: Vec::new(),
            in_table_head: false,
            in_table_cell: false,
            table_cell_buf: Vec::new(),
            table_rows: Vec::new(),
            table_head: Vec::new(),
        }
    }

    fn push_style(&mut self, add: impl FnOnce(Style) -> Style) {
        self.style_stack.push(self.current_style);
        self.current_style = add(self.current_style);
    }

    fn pop_style(&mut self) {
        if let Some(s) = self.style_stack.pop() {
            self.current_style = s;
        }
    }

    fn push_text(&mut self, t: &str) {
        if self.in_code_block {
            self.code_buf.push_str(t);
            return;
        }
        if self.in_heading.is_some() {
            self.heading_text.push_str(t);
        }
        self.push_run(Run {
            text: t.to_string(),
            style: self.current_style,
        });
    }

    /// Route a styled run to either the current table cell or the paragraph buffer.
    fn push_run(&mut self, run: Run) {
        if self.in_table_cell {
            self.table_cell_buf.push(run);
        } else {
            self.runs.push(run);
        }
    }

    fn blank(&mut self) {
        self.out.push(Line::styled("", self.theme.base_style()));
    }

    fn handle(&mut self, event: Event<'_>) {
        match event {
            Event::Start(tag) => self.start_tag(tag),
            Event::End(tag) => self.end_tag(tag),
            Event::Text(t) => self.push_text(&t),
            Event::Code(t) => {
                let styled = format!("\u{00A0}{t}\u{00A0}");
                self.push_run(Run {
                    text: styled,
                    style: self.theme.code_style(),
                });
            }
            Event::SoftBreak => self.push_text(" "),
            Event::HardBreak if !self.in_table_cell => {
                self.flush_paragraph();
            }
            Event::HardBreak => {}
            Event::Rule => {
                if self.in_table_cell {
                    return;
                }
                self.blank();
                let rule: String = "\u{2500}".repeat(self.measure.min(64));
                self.out.push(Line::styled(rule, self.theme.rule_style()));
                self.blank();
            }
            Event::TaskListMarker(checked) => {
                let box_char = if checked { "[\u{2713}] " } else { "[ ] " };
                self.push_run(Run {
                    text: box_char.to_string(),
                    style: self.theme.accent_style(),
                });
            }
            Event::Html(html) | Event::InlineHtml(html) => {
                // Don't render HTML — skip silently. Markdown-as-prose should
                // not expose raw <tag> noise to the reader.
                let _ = html;
            }
            Event::FootnoteReference(label) => {
                self.push_run(Run {
                    text: format!("[{label}]"),
                    style: self.theme.accent_style(),
                });
            }
            _ => {}
        }
    }

    fn start_tag(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                let n: u8 = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                self.in_heading = Some(n);
                self.heading_text.clear();
                let hs = self.theme.heading_style(n);
                self.push_style(|_| hs);
            }
            Tag::BlockQuote(_) => {
                self.blockquote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.in_code_block = true;
                self.code_buf.clear();
                self.code_lang.clear();
                if let CodeBlockKind::Fenced(info) = kind {
                    self.code_lang = info.to_string();
                }
            }
            Tag::List(first) => {
                let indent = self.list_stack.len() * 2;
                self.list_stack.push(ListCtx {
                    ordered: first,
                    indent,
                });
            }
            Tag::Item => {
                if let Some(ctx) = self.list_stack.last_mut() {
                    let prefix = if let Some(n) = ctx.ordered {
                        let p = format!("{n}. ");
                        ctx.ordered = Some(n + 1);
                        p
                    } else {
                        "\u{2022} ".to_string()
                    };
                    self.pending_list_marker = Some(prefix);
                }
            }
            Tag::Emphasis => self.push_style(|s| s.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(|s| s.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } => {
                self.in_link = Some(dest_url.to_string());
                let ls = self.theme.link_style();
                self.push_style(|_| ls);
            }
            Tag::Image { .. } => {
                self.push_run(Run {
                    text: "[image: ".to_string(),
                    style: self.theme.dim_style(),
                });
            }
            Tag::Table(_) => {
                self.table_rows.clear();
                self.table_head.clear();
            }
            Tag::TableHead => {
                self.in_table_head = true;
            }
            Tag::TableRow => {
                self.table_rows.push(Vec::new());
            }
            Tag::TableCell => {
                self.in_table_cell = true;
                self.table_cell_buf.clear();
            }
            Tag::FootnoteDefinition(_) => {}
            _ => {}
        }
    }

    fn end_tag(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                if self.in_table_cell {
                    return;
                }
                self.flush_paragraph();
                // Vertical rhythm: one blank between paragraphs (but not inside list items with tight layout).
                if self.list_stack.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Heading(level) => {
                let n: u8 = match level {
                    HeadingLevel::H1 => 1,
                    HeadingLevel::H2 => 2,
                    HeadingLevel::H3 => 3,
                    HeadingLevel::H4 => 4,
                    HeadingLevel::H5 => 5,
                    HeadingLevel::H6 => 6,
                };
                self.pop_style();
                // Space before heading (unless at doc start).
                if !self.out.is_empty()
                    && !self
                        .out
                        .last()
                        .is_some_and(|l| l.width() == 0)
                {
                    self.blank();
                }
                let start_line = self.out.len();
                let heading = std::mem::take(&mut self.heading_text);
                self.flush_heading(n, &heading);
                let title = heading.trim().to_string();
                if !title.is_empty() {
                    self.toc.push(TocEntry {
                        level: n,
                        title,
                        line: start_line,
                    });
                }
                self.in_heading = None;
            }
            TagEnd::BlockQuote(_) => {
                self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
            }
            TagEnd::CodeBlock => {
                self.flush_code_block();
                self.in_code_block = false;
                self.blank();
            }
            TagEnd::List(_) => {
                self.list_stack.pop();
                if self.list_stack.is_empty() {
                    self.blank();
                }
            }
            TagEnd::Item => {
                self.flush_paragraph();
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough => self.pop_style(),
            TagEnd::Link => {
                self.pop_style();
                if let Some(url) = self.in_link.take() {
                    // Inside table cells, don't footnote — inline the label visually.
                    if !self.in_table_cell {
                        let n = self.link_refs.len() + 1;
                        self.push_run(Run {
                            text: format!("[{n}]"),
                            style: self.theme.dim_style(),
                        });
                        self.link_refs.push((n.to_string(), url));
                    }
                }
            }
            TagEnd::Image => {
                self.push_run(Run {
                    text: "]".to_string(),
                    style: self.theme.dim_style(),
                });
            }
            TagEnd::Table => {
                self.flush_table();
                self.blank();
            }
            TagEnd::TableHead => {
                self.in_table_head = false;
            }
            TagEnd::TableRow => {}
            TagEnd::TableCell => {
                self.in_table_cell = false;
                let cell = std::mem::take(&mut self.table_cell_buf);
                if self.in_table_head {
                    self.table_head.push(cell);
                } else if let Some(row) = self.table_rows.last_mut() {
                    row.push(cell);
                }
            }
            TagEnd::FootnoteDefinition => {
                self.flush_paragraph();
            }
            _ => {}
        }
    }

    fn flush_heading(&mut self, level: u8, text: &str) {
        let text = smarten(text);
        let wrap_width = self.inner_width();
        let style = self.theme.heading_style(level);
        for wrapped in textwrap::wrap(&text, wrap_width) {
            self.out.push(Line::styled(wrapped.into_owned(), style));
        }
        if level == 1 {
            let rule: String = "\u{2500}".repeat(wrap_width.min(self.measure));
            self.out.push(Line::styled(rule, self.theme.rule_style()));
        }
        self.blank();
    }

    fn flush_paragraph(&mut self) {
        if self.runs.is_empty() && self.pending_list_marker.is_none() {
            return;
        }

        // Flatten runs into (String, Vec<(range, Style)>).
        let mut combined = String::new();
        let mut ranges: Vec<(usize, usize, Style)> = Vec::new();
        for run in self.runs.drain(..) {
            let start = combined.len();
            combined.push_str(&run.text);
            let end = combined.len();
            ranges.push((start, end, run.style));
        }

        let smart = smarten(&combined);
        let (smart, ranges) = if smart == combined {
            (smart, ranges)
        } else {
            // Typographic substitution shifted byte offsets — we can't restitch
            // inline styles precisely. Fall back to applying the paragraph base
            // style to the entire block. Pure-text paragraphs are unaffected;
            // the rare paragraph that mixes bold/italic with `--` or `...`
            // loses its inline emphasis, which is an acceptable trade-off.
            let end = smart.len();
            (smart, vec![(0, end, self.theme.base_style())])
        };

        let gutter_str = self.block_gutter();
        let gutter_style = self.theme.quote_style();
        let first_indent = self
            .pending_list_marker
            .take()
            .unwrap_or_default();
        let list_indent = self
            .list_stack
            .last()
            .map_or(0, |c| c.indent + first_indent.chars().count());
        let subsequent_indent = " ".repeat(list_indent);
        let first_line_indent = match self.list_stack.last() {
            Some(c) => {
                let mut s = " ".repeat(c.indent);
                s.push_str(&first_indent);
                s
            }
            None => String::new(),
        };

        let inner = self
            .inner_width()
            .saturating_sub(gutter_str.chars().count());
        if inner < 4 {
            return;
        }

        let opts = textwrap::Options::new(inner)
            .initial_indent(&first_line_indent)
            .subsequent_indent(&subsequent_indent)
            .break_words(false);
        let wrapped = textwrap::wrap(&smart, &opts);

        // We wrapped with textwrap; it may have consumed leading spaces we added.
        // To restyle, we locate each wrapped line's chars within `smart` by a
        // greedy advancing pointer over non-whitespace content, ignoring injected
        // indentation. For robustness and simplicity when ranges are rich,
        // we apply style to the wrapped text by matching against the source:
        // any characters not present in ranges get base style.

        let mut source_cursor = 0usize;
        for wline in wrapped {
            let s = wline.into_owned();
            let line = self.styled_line_from_source(&s, &smart, &mut source_cursor, &ranges, &gutter_str, gutter_style, &first_indent);
            self.out.push(line);
        }
    }

    /// Build a styled line by aligning wrapped text back to the source string byte ranges.
    fn styled_line_from_source(
        &self,
        wrapped: &str,
        source: &str,
        cursor: &mut usize,
        ranges: &[(usize, usize, Style)],
        gutter: &str,
        gutter_style: Style,
        list_marker: &str,
    ) -> Line<'static> {
        let base = self.theme.base_style();
        let mut spans: Vec<Span<'static>> = Vec::new();
        if !gutter.is_empty() {
            spans.push(Span::styled(gutter.to_string(), gutter_style));
        }

        // Identify the list marker prefix, if present at the start of the wrapped line.
        // The wrapped line begins with our indent + first-line-indent on the first paragraph line,
        // or with subsequent-indent on continuation lines. The list marker characters are not in
        // the source, so we style them with accent and advance visibly.
        let mut wrapped_cursor = 0usize;
        let wrapped_bytes = wrapped.as_bytes();

        // Leading ASCII spaces: indent.
        let mut leading_spaces = 0;
        while wrapped_cursor < wrapped.len() && wrapped_bytes[wrapped_cursor] == b' ' {
            leading_spaces += 1;
            wrapped_cursor += 1;
        }
        if leading_spaces > 0 {
            spans.push(Span::styled(" ".repeat(leading_spaces), base));
        }

        // List marker: only on the first wrapped line of a paragraph we haven't rendered yet.
        // We detect by prefix string match.
        if !list_marker.is_empty()
            && wrapped[wrapped_cursor..].starts_with(list_marker) {
                spans.push(Span::styled(list_marker.to_string(), self.theme.accent_style()));
                wrapped_cursor += list_marker.len();
            }

        // For the remaining content of the wrapped line, walk character by character
        // against the source, matching visible characters to recover their style.
        let mut buf = String::new();
        let mut buf_style: Option<Style> = None;

        let rest = &wrapped[wrapped_cursor..];
        for ch in rest.chars() {
            let style = if ch.is_whitespace() {
                // Whitespace takes on surrounding base style by default.
                base
            } else {
                // Advance source cursor to the next occurrence of ch and pick up its style.
                let style = loop {
                    if *cursor >= source.len() {
                        break base;
                    }
                    let src_ch = source[*cursor..].chars().next().unwrap();
                    let src_ch_len = src_ch.len_utf8();
                    if src_ch == ch {
                        let byte_pos = *cursor;
                        *cursor += src_ch_len;
                        // Look up which range contains byte_pos.
                        let style = ranges
                            .iter()
                            .rev()
                            .find(|(a, b, _)| byte_pos >= *a && byte_pos < *b)
                            .map_or(base, |(_, _, s)| *s);
                        break style;
                    }
                    *cursor += src_ch_len;
                };
                style
            };

            match buf_style {
                Some(s) if s == style => buf.push(ch),
                _ => {
                    if !buf.is_empty() {
                        spans.push(Span::styled(std::mem::take(&mut buf), buf_style.unwrap_or(base)));
                    }
                    buf.push(ch);
                    buf_style = Some(style);
                }
            }
        }
        if !buf.is_empty() {
            spans.push(Span::styled(buf, buf_style.unwrap_or(base)));
        }

        Line::from(spans).style(base)
    }

    fn flush_code_block(&mut self) {
        let gutter = "\u{2502} "; // │
        let gutter_style = self.theme.dim_style();
        let inner = self.inner_width().saturating_sub(2);
        let code = std::mem::take(&mut self.code_buf);
        let lang = std::mem::take(&mut self.code_lang);
        let trimmed = code.trim_end_matches('\n');
        for raw_line in trimmed.split('\n') {
            let normalized = raw_line.replace('\t', "    ");
            // Truncate for display so a single long code line can't break layout.
            let visible = truncate_to_width(&normalized, inner);
            let mut spans: Vec<Span<'static>> = Vec::new();
            spans.push(Span::styled(gutter.to_string(), gutter_style));
            let mut tokens = highlight_line(&visible, &lang, self.theme);
            spans.append(&mut tokens);
            self.out.push(Line::from(spans).style(self.theme.base_style()));
        }
    }

    fn flush_table(&mut self) {
        if self.table_head.is_empty() && self.table_rows.is_empty() {
            return;
        }
        // Render table as plain text rows separated by " | ", wrapped overall.
        let cell_to_text = |cell: &[Run]| -> String {
            let mut s = String::new();
            for r in cell {
                s.push_str(&r.text);
            }
            s
        };
        let head: Vec<String> = self.table_head.iter().map(|c| cell_to_text(c)).collect();
        let rows: Vec<Vec<String>> = self
            .table_rows
            .iter()
            .map(|row| row.iter().map(|c| cell_to_text(c)).collect())
            .collect();

        let all_rows = std::iter::once(head.clone()).chain(rows.iter().cloned());
        let col_count = head.len().max(rows.iter().map(std::vec::Vec::len).max().unwrap_or(0));
        if col_count == 0 {
            return;
        }
        let mut widths = vec![0usize; col_count];
        for row in all_rows {
            for (i, cell) in row.iter().enumerate() {
                if i < col_count {
                    let w = unicode_width::UnicodeWidthStr::width(cell.as_str());
                    widths[i] = widths[i].max(w);
                }
            }
        }
        let total_width: usize = widths.iter().sum::<usize>() + 3 * col_count.saturating_sub(1);
        let inner = self.inner_width();
        if total_width > inner {
            // Scale down proportionally.
            let factor = inner as f64 / total_width.max(1) as f64;
            for w in &mut widths {
                *w = ((*w as f64) * factor).floor().max(3.0) as usize;
            }
        }

        let render_row = |row: &[String], widths: &[usize], style: Style, theme: Theme| -> Line<'static> {
            let mut spans = Vec::new();
            for (i, w) in widths.iter().enumerate() {
                let text = row.get(i).cloned().unwrap_or_default();
                let truncated = truncate_to_width(&text, *w);
                let pad = w.saturating_sub(unicode_width::UnicodeWidthStr::width(truncated.as_str()));
                spans.push(Span::styled(truncated, style));
                if pad > 0 {
                    spans.push(Span::styled(" ".repeat(pad), theme.base_style()));
                }
                if i + 1 < widths.len() {
                    spans.push(Span::styled(" \u{2502} ".to_string(), theme.dim_style()));
                }
            }
            Line::from(spans).style(theme.base_style())
        };

        let head_style = self
            .theme
            .base_style()
            .add_modifier(Modifier::BOLD);
        if !head.is_empty() {
            self.out.push(render_row(&head, &widths, head_style, self.theme));
            let sep: String = widths
                .iter()
                .enumerate()
                .map(|(i, w)| {
                    let mut s = "\u{2500}".repeat(*w);
                    if i + 1 < widths.len() {
                        s.push_str("\u{2500}\u{253C}\u{2500}");
                    }
                    s
                })
                .collect();
            self.out.push(Line::styled(sep, self.theme.rule_style()));
        }
        for row in &rows {
            self.out.push(render_row(row, &widths, self.theme.base_style(), self.theme));
        }
    }

    fn block_gutter(&self) -> String {
        if self.blockquote_depth > 0 {
            "\u{2502} ".repeat(self.blockquote_depth)
        } else {
            String::new()
        }
    }

    fn inner_width(&self) -> usize {
        self.measure
    }

    fn finish(mut self) -> Rendered {
        // Flush anything dangling.
        self.flush_paragraph();

        // If there were any links, emit them as a footnote list at the end.
        if !self.link_refs.is_empty() {
            self.blank();
            self.out.push(Line::styled(
                "Links".to_string(),
                self.theme.heading_style(3),
            ));
            self.blank();
            let refs = std::mem::take(&mut self.link_refs);
            for (n, url) in refs {
                let label = format!("[{n}] ");
                let label_style = self.theme.dim_style();
                let link_style = self.theme.link_style();
                let spans = vec![
                    Span::styled(label, label_style),
                    Span::styled(url, link_style),
                ];
                self.out.push(Line::from(spans).style(self.theme.base_style()));
            }
        }

        Rendered {
            lines: self.out,
            toc: self.toc,
        }
    }
}

fn truncate_to_width(s: &str, max_cols: usize) -> String {
    let mut out = String::new();
    let mut width = 0;
    for ch in s.chars() {
        let w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + w > max_cols {
            // append ellipsis if room
            if max_cols >= 1 && !out.is_empty() {
                let need = 1;
                while unicode_width::UnicodeWidthStr::width(out.as_str()) + need > max_cols {
                    out.pop();
                }
                out.push('\u{2026}');
            }
            return out;
        }
        out.push(ch);
        width += w;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Theme, ThemeName};

    fn plain() -> Theme {
        Theme::resolve(ThemeName::Plain)
    }

    #[test]
    fn renders_paragraph() {
        let r = render("Hello world.", 40, plain());
        assert!(!r.lines.is_empty());
        assert!(r.lines[0].width() > 0);
    }

    #[test]
    fn wraps_long_paragraph() {
        let text = "word ".repeat(50);
        let r = render(&text, 20, plain());
        assert!(r.lines.len() > 2);
        for line in &r.lines {
            assert!(line.width() <= 20 + 1, "line too wide: {}", line.width());
        }
    }

    #[test]
    fn headings_in_toc() {
        let md = "# Top\n\nSome text.\n\n## Sub\n\nMore.\n";
        let r = render(md, 60, plain());
        assert_eq!(r.toc.len(), 2);
        assert_eq!(r.toc[0].title, "Top");
        assert_eq!(r.toc[0].level, 1);
        assert_eq!(r.toc[1].level, 2);
    }

    #[test]
    fn code_blocks_render() {
        let md = "```\nfn main() {}\n```\n";
        let r = render(md, 60, plain());
        let any_code_line = r.lines.iter().any(|l| l.to_string().contains("fn main"));
        assert!(any_code_line);
    }

    #[test]
    fn lists_use_bullet() {
        let md = "- one\n- two\n";
        let r = render(md, 60, plain());
        let text: String = r.lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("\u{2022}"), "expected bullet in: {text}");
    }

    #[test]
    fn links_become_footnotes() {
        let md = "See [here](https://example.com) for details.\n";
        let r = render(md, 60, plain());
        let text: String = r.lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("https://example.com"));
        assert!(text.contains("[1]"));
    }

    #[test]
    fn tables_render_header_and_rows() {
        let md = "| A | B |\n|---|---|\n| one | two |\n| three | four |\n";
        let r = render(md, 80, plain());
        let text: String = r.lines.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
        for s in ["A", "B", "one", "two", "three", "four"] {
            assert!(text.contains(s), "expected `{s}` in rendered table:\n{text}");
        }
    }

    #[test]
    fn tables_with_inline_code_cells() {
        let md = "| name | value |\n|------|-------|\n| `x` | 1 |\n";
        let r = render(md, 80, plain());
        let text: String = r.lines.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
        // The inline-code text "x" should be in the cell (same line as "name" row
        // content), and NOT appear as a stray post-table paragraph.
        assert!(text.contains("name"));
        let mut cell_line = None;
        for line in &r.lines {
            let s = line.to_string();
            if s.contains('x') && s.contains('1') {
                cell_line = Some(s);
                break;
            }
        }
        assert!(cell_line.is_some(), "x and 1 should share a single row line:\n{text}");
    }

    #[test]
    fn blockquote_has_gutter() {
        let md = "> quoted.\n";
        let r = render(md, 60, plain());
        let text: String = r.lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(text.contains("\u{2502}"));
    }

    #[test]
    fn smart_substitution_preserves_text() {
        let md = "He said \"yes\" -- probably...\n";
        let r = render(md, 60, plain());
        let text: String = r.lines.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n");
        assert!(text.contains("\u{201C}yes\u{201D}"));
        assert!(text.contains("\u{2014}"));
        assert!(text.contains("\u{2026}"));
    }

    #[test]
    fn html_tags_are_hidden() {
        let md = "Hello <em>world</em> and <div>x</div>.\n";
        let r = render(md, 60, plain());
        let text: String = r.lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(!text.contains("<em>"));
        assert!(!text.contains("</em>"));
        assert!(!text.contains("<div>"));
        assert!(!text.contains("</div>"));
    }
}
