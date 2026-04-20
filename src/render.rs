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
use crate::layout::{decorate_heading, LayoutName, RuleSpec};
use crate::theme::Theme;
use crate::typography::smarten;

/// A single rendered document, consisting of styled lines plus a table of
/// contents and a list of code blocks (for clipboard copy).
pub struct Rendered {
    pub lines: Vec<Line<'static>>,
    pub toc: Vec<TocEntry>,
    pub code_blocks: Vec<CodeBlockEntry>,
}

/// A recorded fenced code block: its visual line range in `Rendered::lines`
/// and the raw (unhighlighted, untruncated) source text.
#[derive(Debug, Clone)]
pub struct CodeBlockEntry {
    pub start_line: usize,
    pub end_line: usize,
    pub lang: String,
    pub code: String,
    /// Inclusive `(start, end)` visual-row range in `Rendered::lines` for each
    /// source line of `code` (as split on `\n`, after trimming trailing
    /// newlines). A line that soft-wraps spans multiple visual rows; a line
    /// rendered unwrapped has `start == end`. Enables per-source-line
    /// navigation and copy independent of visual wrap state.
    pub line_visuals: Vec<(usize, usize)>,
}

#[derive(Debug, Clone)]
pub struct TocEntry {
    pub level: u8,
    pub title: String,
    /// Index into `Rendered::lines` where this heading starts.
    pub line: usize,
    /// 1-based line number in the *source* markdown. Used by the external-
    /// editor handoff so `$EDITOR +<n> <path>` lands on the heading itself.
    pub source_line: usize,
}

/// Entry point. Produces styled lines wrapped to `measure` columns.
pub fn render(
    md: &str,
    measure: usize,
    theme: Theme,
    layout: LayoutName,
    wrap_code: bool,
) -> Rendered {
    let measure = measure.max(20);
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_FOOTNOTES);
    opts.insert(Options::ENABLE_TASKLISTS);

    // Byte offsets of each `\n` in the source — used to translate the byte
    // offsets reported by pulldown-cmark back to 1-based source line numbers
    // for TOC entries (so `e` can open the editor at the right line).
    let newline_offsets: Vec<usize> = md
        .char_indices()
        .filter_map(|(i, c)| (c == '\n').then_some(i))
        .collect();
    let source_line_at =
        |byte_offset: usize| -> usize { newline_offsets.partition_point(|&n| n < byte_offset) + 1 };

    let parser = Parser::new_ext(md, opts).into_offset_iter();
    let mut r = Renderer::new(measure, theme, layout, wrap_code);
    for (event, range) in parser {
        if matches!(event, Event::Start(Tag::Heading { .. })) {
            r.pending_heading_src_line = Some(source_line_at(range.start));
        }
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
    layout: LayoutName,
    wrap_code: bool,
    out: Vec<Line<'static>>,
    toc: Vec<TocEntry>,
    code_blocks: Vec<CodeBlockEntry>,

    // Inline accumulation
    runs: Vec<Run>,
    style_stack: Vec<Style>,
    current_style: Style,

    // Block state
    in_heading: Option<u8>,
    heading_text: String,
    /// Latched by the caller before `handle` sees a Heading Start event.
    pending_heading_src_line: Option<usize>,
    in_code_block: bool,
    code_lang: String,
    code_buf: String,
    blockquote_depth: usize,
    list_stack: Vec<ListCtx>,
    pending_list_marker: Option<String>,
    /// Active link URL and the `runs` index where the link text began.
    in_link: Option<(usize, String)>,

    // Tables
    in_table_head: bool,
    in_table_cell: bool,
    table_cell_buf: Vec<Run>,
    table_rows: Vec<Vec<Vec<Run>>>,
    table_head: Vec<Vec<Run>>,
}

impl Renderer {
    fn new(measure: usize, theme: Theme, layout: LayoutName, wrap_code: bool) -> Self {
        Self {
            measure,
            theme,
            layout,
            wrap_code,
            out: Vec::new(),
            toc: Vec::new(),
            code_blocks: Vec::new(),
            runs: Vec::new(),
            style_stack: Vec::new(),
            current_style: theme.base_style(),
            in_heading: None,
            heading_text: String::new(),
            pending_heading_src_line: None,
            in_code_block: false,
            code_lang: String::new(),
            code_buf: String::new(),
            blockquote_depth: 0,
            list_stack: Vec::new(),
            pending_list_marker: None,
            in_link: None,
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
            // Heading text flushes through `heading_text` on TagEnd::Heading.
            // It must NOT also land in `runs`, or it leaks into the next
            // paragraph/list item and appears duplicated.
            self.heading_text.push_str(t);
            return;
        }
        self.push_run(Run {
            text: t.to_string(),
            style: self.current_style,
        });
    }

    /// Route a styled run to the right buffer for the current block context.
    /// Headings flatten everything into `heading_text` (styles are applied when
    /// the heading flushes). Table cells accumulate into their own buffer.
    /// Otherwise the run joins the paragraph buffer.
    fn push_run(&mut self, run: Run) {
        if self.in_heading.is_some() {
            self.heading_text.push_str(&run.text);
        } else if self.in_table_cell {
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
                let depth = self.list_stack.len();
                let vivid = matches!(self.layout, LayoutName::Vivid);
                if let Some(ctx) = self.list_stack.last_mut() {
                    let prefix = if let Some(n) = ctx.ordered {
                        let p = format!("{n}. ");
                        ctx.ordered = Some(n + 1);
                        p
                    } else {
                        // Vivid layout graduates unordered bullets by depth so
                        // nested lists read as a hierarchy at a glance; minimal
                        // keeps a single glyph for a quieter page.
                        let glyph = if vivid {
                            match depth.saturating_sub(1) {
                                0 => "\u{2022}", // •
                                1 => "\u{25E6}", // ◦
                                _ => "\u{25AB}", // ▫
                            }
                        } else {
                            "\u{2022}"
                        };
                        format!("{glyph} ")
                    };
                    self.pending_list_marker = Some(prefix);
                }
            }
            Tag::Emphasis => self.push_style(|s| s.add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(|s| s.add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => self.push_style(|s| s.add_modifier(Modifier::CROSSED_OUT)),
            Tag::Link { dest_url, .. } => {
                // Remember where the link text starts in runs so we can detect
                // autolinks (where text == URL) at TagEnd::Link and skip the
                // duplicated inline URL.
                self.in_link = Some((self.runs.len(), dest_url.to_string()));
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
                let start_line = self.out.len();
                let heading = std::mem::take(&mut self.heading_text);
                self.flush_heading(n, &heading);
                let title = heading.trim().to_string();
                let source_line = self.pending_heading_src_line.take().unwrap_or(1);
                if !title.is_empty() {
                    self.toc.push(TocEntry {
                        level: n,
                        title,
                        line: start_line,
                        source_line,
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
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => self.pop_style(),
            TagEnd::Link => {
                self.pop_style();
                if let Some((link_start, url)) = self.in_link.take() {
                    // Autolink detection: <https://example.com> emits text that
                    // is identical to the URL. Don't duplicate it.
                    let link_text: String = self.runs[link_start..]
                        .iter()
                        .map(|r| r.text.as_str())
                        .collect();
                    // Show URLs inline for http(s)/mailto and for relative file
                    // references (e.g. `README.es.md`, `docs/foo.md`, `LICENSE`),
                    // since the path is the only cue that the text is a link to
                    // another file. Hide them for in-document anchors (`#section`),
                    // which add no information beyond the link text.
                    let worth_showing =
                        is_inline_worthy_url(&url) && link_text.trim() != url.trim();
                    // Inside table cells we skip inline URLs to keep columns
                    // compact; the link text still has the underline style.
                    if worth_showing && !self.in_table_cell {
                        self.push_run(Run {
                            text: " (".to_string(),
                            style: self.theme.dim_style(),
                        });
                        self.push_run(Run {
                            text: url,
                            style: self.theme.dim_style(),
                        });
                        self.push_run(Run {
                            text: ")".to_string(),
                            style: self.theme.dim_style(),
                        });
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
        let decor = decorate_heading(self.layout, level, self.theme);
        let wrap_width = self.inner_width();

        for _ in 0..decor.blank_before {
            // Avoid stacking multiple blanks when the previous block already
            // ended with one.
            if !self.out.last().is_some_and(|l| l.width() == 0) {
                self.blank();
            }
        }

        if let Some(rule) = decor.top_rule.as_ref() {
            self.push_full_rule(rule, wrap_width);
        }

        let smart = smarten(text);
        let display_text = if decor.uppercase {
            smart.to_uppercase()
        } else {
            smart
        };
        // Indent + prefix budget cuts into the wrap width so prefixed long
        // headings don't blow past the measure.
        let prefix_w = unicode_width::UnicodeWidthStr::width(decor.prefix.as_str());
        let body_width = wrap_width.saturating_sub(decor.indent + prefix_w).max(10);
        let wrapped_lines: Vec<String> = textwrap::wrap(&display_text, body_width)
            .into_iter()
            .map(|s| s.into_owned())
            .collect();

        let indent_str = " ".repeat(decor.indent);
        let base = self.theme.base_style();
        for (i, line) in wrapped_lines.iter().enumerate() {
            let mut spans: Vec<Span<'static>> = Vec::new();
            if decor.indent > 0 {
                spans.push(Span::styled(indent_str.clone(), base));
            }
            // Prefix only on the first line; continuation lines align under
            // the heading text, not the prefix.
            if i == 0 && !decor.prefix.is_empty() {
                spans.push(Span::styled(decor.prefix.clone(), decor.style));
            } else if i > 0 && !decor.prefix.is_empty() {
                spans.push(Span::styled(" ".repeat(prefix_w), base));
            }
            spans.push(Span::styled(line.clone(), decor.style));
            self.out.push(Line::from(spans).style(base));
        }

        if let Some(rule) = decor.bottom_rule.as_ref() {
            self.push_full_rule(rule, wrap_width);
        }

        for _ in 0..decor.blank_after {
            self.blank();
        }
    }

    fn push_full_rule(&mut self, rule: &RuleSpec, width: usize) {
        let s: String = rule.ch.to_string().repeat(width);
        self.out.push(Line::styled(s, rule.style));
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
        let first_indent = self.pending_list_marker.take().unwrap_or_default();
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
            let line = self.styled_line_from_source(
                &s,
                &smart,
                &mut source_cursor,
                &ranges,
                &gutter_str,
                gutter_style,
                &first_indent,
            );
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
        if !list_marker.is_empty() && wrapped[wrapped_cursor..].starts_with(list_marker) {
            spans.push(Span::styled(
                list_marker.to_string(),
                self.theme.accent_style(),
            ));
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
                        spans.push(Span::styled(
                            std::mem::take(&mut buf),
                            buf_style.unwrap_or(base),
                        ));
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
        // Code blocks render with top + bottom rules only, no side borders.
        // Side borders would get picked up by terminal mouse selection and
        // pollute pasted code, so we keep the rules plus the `code_bg` fill
        // for visual separation and let the content be cleanly copyable.
        //
        //   ─── rust ───────────────────────── ⎘ y ───
        //    fn main() {
        //        println!("hi");
        //    }
        //   ──────────────────────────────────────────
        //
        // The `⎘ y` copy-hint is only shown when OSC 52 has a reasonable
        // chance of reaching the clipboard (i.e. not an SSH session, where
        // tmux/forwarding often strips the escape sequence).
        let code = std::mem::take(&mut self.code_buf);
        let lang = std::mem::take(&mut self.code_lang);
        let lang_label = lang
            .split(|c: char| c.is_whitespace() || c == ',')
            .next()
            .unwrap_or("")
            .to_string();

        let width = self.inner_width();
        // One-column left padding keeps code slightly inset so the block
        // reads as distinct. Mouse-selecting will grab the leading space,
        // which is harmless in pasted code.
        let left_pad = 1usize;
        let code_cols = width.saturating_sub(left_pad).max(1);

        let rule_style = self.theme.rule_style();
        let pad_style = self.theme.code_style();
        let label_style = self.theme.dim_style();
        let base_style = self.theme.base_style();

        let show_copy_hint = !crate::clipboard::is_ssh_session();
        let copy_hint = " \u{2398} y ";
        let copy_hint_w = if show_copy_hint {
            unicode_width::UnicodeWidthStr::width(copy_hint)
        } else {
            0
        };

        // Vivid layout uses a heavy top rule (━) to reinforce the hierarchy
        // — the heading rules already use heavier glyphs in vivid, so code
        // blocks should match. Minimal keeps the lighter ─ on both rules.
        let top_rule_ch = if matches!(self.layout, LayoutName::Vivid) {
            "\u{2501}"
        } else {
            "\u{2500}"
        };

        // Top rule with optional language label and optional copy hint.
        let mut top_spans: Vec<Span<'static>> = Vec::new();
        if lang_label.is_empty() {
            let dashes_w = width.saturating_sub(copy_hint_w);
            top_spans.push(Span::styled(top_rule_ch.repeat(dashes_w), rule_style));
            if show_copy_hint {
                top_spans.push(Span::styled(copy_hint.to_string(), label_style));
            }
        } else {
            let lbl = format!(" {lang_label} ");
            let lbl_w = unicode_width::UnicodeWidthStr::width(lbl.as_str());
            let leading_w = 3;
            let mid_w = width.saturating_sub(leading_w + lbl_w + copy_hint_w).max(1);
            top_spans.push(Span::styled(top_rule_ch.repeat(leading_w), rule_style));
            top_spans.push(Span::styled(lbl, label_style));
            top_spans.push(Span::styled(top_rule_ch.repeat(mid_w), rule_style));
            if show_copy_hint {
                top_spans.push(Span::styled(copy_hint.to_string(), label_style));
            }
        }
        let start_line = self.out.len();
        self.out.push(Line::from(top_spans).style(base_style));

        let pad_str = " ".repeat(left_pad);
        let trimmed = code.trim_end_matches('\n');
        // Continuation lines of a soft-wrapped code line start with a small
        // dim arrow so the reader can tell the line is wrapped rather than
        // a genuine new code line.
        let cont_marker = " \u{21AA} "; //  ↪
        let cont_w = unicode_width::UnicodeWidthStr::width(cont_marker);

        let mut line_visuals: Vec<(usize, usize)> = Vec::new();

        for raw_line in trimmed.split('\n') {
            let src_visual_start = self.out.len();
            let normalized = raw_line.replace('\t', "    ");
            let line_w = unicode_width::UnicodeWidthStr::width(normalized.as_str());

            // Three cases:
            //   1. Fits in code_cols → render as-is.
            //   2. Too long + wrap_code on → split into chunks and emit
            //      multiple visual lines with a continuation marker.
            //   3. Too long + wrap_code off → truncate with `…`.
            if line_w <= code_cols {
                self.push_code_line(
                    &pad_str,
                    &normalized,
                    code_cols,
                    &lang,
                    pad_style,
                    base_style,
                );
                line_visuals.push((src_visual_start, self.out.len() - 1));
                continue;
            }

            if self.wrap_code {
                // First chunk occupies the full width; continuations lose
                // `cont_w` columns to the marker.
                let first_chunk_w = code_cols;
                let cont_chunk_w = code_cols.saturating_sub(cont_w).max(1);
                let chunks = wrap_code_line(&normalized, first_chunk_w, cont_chunk_w);
                for (i, chunk) in chunks.iter().enumerate() {
                    if i == 0 {
                        self.push_code_line(
                            &pad_str, chunk, code_cols, &lang, pad_style, base_style,
                        );
                    } else {
                        // Render: <pad><cont_marker><highlighted chunk><trailing pad>
                        let chunk_w = unicode_width::UnicodeWidthStr::width(chunk.as_str());
                        let trailing = code_cols.saturating_sub(cont_w).saturating_sub(chunk_w);
                        let mut spans: Vec<Span<'static>> = Vec::new();
                        spans.push(Span::styled(pad_str.clone(), pad_style));
                        spans.push(Span::styled(cont_marker.to_string(), label_style));
                        spans.extend(highlight_line(chunk, &lang, self.theme));
                        if trailing > 0 {
                            spans.push(Span::styled(" ".repeat(trailing), pad_style));
                        }
                        self.out.push(Line::from(spans).style(base_style));
                    }
                }
            } else {
                let visible = truncate_to_width(&normalized, code_cols);
                self.push_code_line(&pad_str, &visible, code_cols, &lang, pad_style, base_style);
            }
            line_visuals.push((src_visual_start, self.out.len() - 1));
        }

        self.out
            .push(Line::styled("\u{2500}".repeat(width), rule_style));

        let end_line = self.out.len() - 1;
        self.code_blocks.push(CodeBlockEntry {
            start_line,
            end_line,
            lang: lang_label,
            code: code.trim_end_matches('\n').to_string(),
            line_visuals,
        });
    }

    /// Render one code line (already known to fit within `width`) with the
    /// left pad, highlighted tokens, and trailing code-bg fill.
    fn push_code_line(
        &mut self,
        pad: &str,
        text: &str,
        width: usize,
        lang: &str,
        pad_style: Style,
        base_style: Style,
    ) {
        let content_w = unicode_width::UnicodeWidthStr::width(text);
        let trailing = width.saturating_sub(content_w);
        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled(pad.to_string(), pad_style));
        spans.extend(highlight_line(text, lang, self.theme));
        if trailing > 0 {
            spans.push(Span::styled(" ".repeat(trailing), pad_style));
        }
        self.out.push(Line::from(spans).style(base_style));
    }

    fn flush_table(&mut self) {
        if self.table_head.is_empty() && self.table_rows.is_empty() {
            return;
        }
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

        let col_count = head
            .len()
            .max(rows.iter().map(std::vec::Vec::len).max().unwrap_or(0));
        if col_count == 0 {
            return;
        }

        let inner = self.inner_width();
        let widths = compute_column_widths(&head, &rows, col_count, inner);

        let theme = self.theme;
        let head_style = theme.base_style().add_modifier(Modifier::BOLD);
        let body_style = theme.base_style();
        let sep_style = theme.dim_style();
        let rule_style = theme.rule_style();

        // Wrap every row up front so we know whether any row spans multiple
        // visual lines. If so, draw a thin separator between body rows to
        // keep row boundaries legible when cells wrap.
        let head_wrapped = if head.is_empty() {
            Vec::new()
        } else {
            render_wrapped_row(&head, &widths, head_style, sep_style, body_style)
        };
        let body_wrapped: Vec<Vec<Line<'static>>> = rows
            .iter()
            .map(|row| render_wrapped_row(row, &widths, body_style, sep_style, body_style))
            .collect();

        let any_wrapping_row = body_wrapped.iter().any(|r| r.len() > 1) || head_wrapped.len() > 1;

        let heavy_sep = {
            let mut s = String::new();
            for (i, w) in widths.iter().enumerate() {
                s.push_str(&"\u{2500}".repeat(*w));
                if i + 1 < widths.len() {
                    s.push_str("\u{2500}\u{253C}\u{2500}");
                }
            }
            s
        };
        // Light row separator uses `╌` when available so row splits read as
        // lighter than the header rule.
        let light_sep = {
            let mut s = String::new();
            for (i, w) in widths.iter().enumerate() {
                s.push_str(&"\u{254C}".repeat(*w));
                if i + 1 < widths.len() {
                    s.push_str("\u{254C}\u{253C}\u{254C}");
                }
            }
            s
        };

        // Render header rows (one logical row may wrap into multiple visual lines).
        if !head_wrapped.is_empty() {
            for line in head_wrapped {
                self.out.push(line);
            }
            self.out.push(Line::styled(heavy_sep, rule_style));
        }

        let last_idx = body_wrapped.len().saturating_sub(1);
        for (row_idx, row_lines) in body_wrapped.into_iter().enumerate() {
            for line in row_lines {
                self.out.push(line);
            }
            if any_wrapping_row && row_idx != last_idx {
                self.out.push(Line::styled(light_sep.clone(), sep_style));
            }
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

        Rendered {
            lines: self.out,
            toc: self.toc,
            code_blocks: self.code_blocks,
        }
    }
}

/// Compute column widths that fit within `total` columns. Start from each
/// column's natural max width (longest cell), reserve 3 cols per separator, and
/// if the sum exceeds the available space, shrink columns iteratively until
/// everything fits — always shrinking the currently-widest column. This keeps
/// narrow columns intact and compresses long prose columns instead of
/// chopping every column proportionally.
fn compute_column_widths(
    head: &[String],
    rows: &[Vec<String>],
    col_count: usize,
    total: usize,
) -> Vec<usize> {
    const MIN_COL: usize = 6;

    let mut widths = vec![0usize; col_count];
    for row in std::iter::once(head).chain(rows.iter().map(std::vec::Vec::as_slice)) {
        for (i, cell) in row.iter().enumerate() {
            if i < col_count {
                let w = cell
                    .split('\n')
                    .map(unicode_width::UnicodeWidthStr::width)
                    .max()
                    .unwrap_or(0);
                widths[i] = widths[i].max(w);
            }
        }
    }
    let sep_cost = 3 * col_count.saturating_sub(1);
    let budget = total.saturating_sub(sep_cost);

    // Bottom-out: if we can't give MIN_COL to every column, distribute as
    // evenly as we can and let textwrap hard-break if necessary.
    let min_total = MIN_COL * col_count;
    if budget < min_total {
        let each = (budget / col_count).max(1);
        return vec![each; col_count];
    }

    // Shrink the widest column one unit at a time until we fit.
    while widths.iter().sum::<usize>() > budget {
        let Some((i, _)) = widths.iter().enumerate().max_by_key(|(_, w)| **w) else {
            break;
        };
        if widths[i] <= MIN_COL {
            break;
        }
        widths[i] -= 1;
    }
    widths
}

/// Render one logical table row as one or more visual lines by wrapping each
/// cell to its column width. Short cells are padded with blank lines so the
/// `│` separators align across the whole logical row.
fn render_wrapped_row(
    row: &[String],
    widths: &[usize],
    text_style: Style,
    sep_style: Style,
    fill_style: Style,
) -> Vec<Line<'static>> {
    // Wrap each cell into a vec of lines.
    let wrapped: Vec<Vec<String>> = widths
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let cell = row.get(i).cloned().unwrap_or_default();
            if cell.is_empty() {
                return vec![String::new()];
            }
            let mut out: Vec<String> = Vec::new();
            // Respect explicit newlines in the cell first, then wrap each segment.
            for segment in cell.split('\n') {
                if segment.is_empty() {
                    out.push(String::new());
                    continue;
                }
                let opts = textwrap::Options::new(*w).break_words(true);
                for wrapped_line in textwrap::wrap(segment, &opts) {
                    out.push(wrapped_line.into_owned());
                }
            }
            if out.is_empty() {
                out.push(String::new());
            }
            out
        })
        .collect();

    let row_height = wrapped.iter().map(std::vec::Vec::len).max().unwrap_or(1);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(row_height);
    for line_idx in 0..row_height {
        let mut spans: Vec<Span<'static>> = Vec::new();
        for (i, w) in widths.iter().enumerate() {
            let text = wrapped[i].get(line_idx).cloned().unwrap_or_default();
            let truncated = truncate_to_width(&text, *w);
            let pad = w.saturating_sub(unicode_width::UnicodeWidthStr::width(truncated.as_str()));
            spans.push(Span::styled(truncated, text_style));
            if pad > 0 {
                spans.push(Span::styled(" ".repeat(pad), fill_style));
            }
            if i + 1 < widths.len() {
                spans.push(Span::styled(" \u{2502} ".to_string(), sep_style));
            }
        }
        lines.push(Line::from(spans).style(fill_style));
    }
    lines
}

/// Split a code line into column-sized chunks. Unlike prose wrapping, code
/// wrapping must be *column*-bounded (not word-bounded) — a single token can
/// easily be longer than the column, and breaking on whitespace would leave
/// the tail trailing past the right edge. We walk char-by-char counting
/// display width (CJK, emoji) and emit chunks when we hit the budget.
///
/// The first chunk uses `first_w`; subsequent chunks use `rest_w` (which is
/// narrower to make room for the continuation marker).
fn wrap_code_line(line: &str, first_w: usize, rest_w: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut current_w = 0usize;
    let mut budget = first_w.max(1);

    for ch in line.chars() {
        let ch_w = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_w + ch_w > budget && !current.is_empty() {
            out.push(std::mem::take(&mut current));
            current_w = 0;
            budget = rest_w.max(1);
        }
        current.push(ch);
        current_w += ch_w;
    }
    if !current.is_empty() || out.is_empty() {
        out.push(current);
    }
    out
}

/// A link URL is "external" if it has a scheme we can plausibly click in a
/// terminal: http, https, or mailto. In-document anchors (`#section`),
/// relative paths, and unknown schemes are not shown inline.
fn is_external_url(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    u.starts_with("http://") || u.starts_with("https://") || u.starts_with("mailto:")
}

/// Whether to render the URL inline next to the link text. True for external
/// URLs and for relative file references (which encode the only hint of where
/// the link points). False for in-document anchors and unsupported schemes
/// like `javascript:` / `data:`.
fn is_inline_worthy_url(url: &str) -> bool {
    let u = url.trim();
    if u.is_empty() || u.starts_with('#') {
        return false;
    }
    if is_external_url(u) {
        return true;
    }
    let lower = u.to_ascii_lowercase();
    // Block known non-navigable schemes. Anything else without a scheme is a
    // relative path (file reference), which is worth showing.
    for bad in ["javascript:", "data:", "vbscript:", "file:"] {
        if lower.starts_with(bad) {
            return false;
        }
    }
    // Treat strings containing "://" but not matched above as foreign schemes
    // we can't usefully click — still show them, since hiding makes the text
    // ambiguous, but the user gets the URL to copy.
    true
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
        let r = render("Hello world.", 40, plain(), LayoutName::Minimal, true);
        assert!(!r.lines.is_empty());
        assert!(r.lines[0].width() > 0);
    }

    #[test]
    fn wraps_long_paragraph() {
        let text = "word ".repeat(50);
        let r = render(&text, 20, plain(), LayoutName::Minimal, true);
        assert!(r.lines.len() > 2);
        for line in &r.lines {
            assert!(line.width() <= 20 + 1, "line too wide: {}", line.width());
        }
    }

    #[test]
    fn headings_in_toc() {
        let md = "# Top\n\nSome text.\n\n## Sub\n\nMore.\n";
        let r = render(md, 60, plain(), LayoutName::Minimal, true);
        assert_eq!(r.toc.len(), 2);
        assert_eq!(r.toc[0].title, "Top");
        assert_eq!(r.toc[0].level, 1);
        assert_eq!(r.toc[1].level, 2);
    }

    #[test]
    fn code_blocks_render() {
        let md = "```\nfn main() {}\n```\n";
        let r = render(md, 60, plain(), LayoutName::Minimal, true);
        let any_code_line = r.lines.iter().any(|l| l.to_string().contains("fn main"));
        assert!(any_code_line);
    }

    #[test]
    fn code_block_line_visuals_track_source_lines() {
        // Short lines: one visual row per source line, all rows inside the
        // block's [start_line+1, end_line-1] range (rules excluded).
        let md = "```\nfn main() {\n    println!(\"hi\");\n}\n```\n";
        let r = render(md, 60, plain(), LayoutName::Minimal, true);
        let b = r.code_blocks.first().expect("one code block");
        assert_eq!(b.line_visuals.len(), 3, "three source lines");
        for (vs, ve) in &b.line_visuals {
            assert_eq!(vs, ve, "short line should occupy exactly one visual row");
            assert!(*vs > b.start_line && *ve < b.end_line);
        }
        // Source lines recoverable from code.
        let src: Vec<&str> = b.code.split('\n').collect();
        assert_eq!(src.len(), 3);
    }

    #[test]
    fn code_block_line_visuals_span_wrapped_rows() {
        // A very long single line with wrap on: one source line but the
        // visual range must span multiple rows.
        let long = "x".repeat(200);
        let md = format!("```\n{long}\n```\n");
        let r = render(&md, 40, plain(), LayoutName::Minimal, true);
        let b = r.code_blocks.first().expect("one code block");
        assert_eq!(b.line_visuals.len(), 1, "single source line");
        let (vs, ve) = b.line_visuals[0];
        assert!(ve > vs, "wrapped line should span multiple visual rows");
        // The raw code still stores the full, unbroken source.
        assert_eq!(b.code, long);
    }

    #[test]
    fn lists_use_bullet() {
        let md = "- one\n- two\n";
        let r = render(md, 60, plain(), LayoutName::Minimal, true);
        let text: String = r
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("\u{2022}"), "expected bullet in: {text}");
    }

    #[test]
    fn links_inline_url_after_text() {
        let md = "See [here](https://example.com) for details.\n";
        let r = render(md, 60, plain(), LayoutName::Minimal, true);
        let text: String = r
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        // Link text is rendered, and the URL appears inline in parentheses.
        assert!(text.contains("here"));
        assert!(text.contains("(https://example.com)"));
        // No footnote markers anymore.
        assert!(!text.contains("[1]"));
        assert!(!text.to_lowercase().contains("\nlinks\n"));
    }

    #[test]
    fn autolinks_do_not_duplicate_url() {
        let md = "Visit <https://example.com> today.\n";
        let r = render(md, 80, plain(), LayoutName::Minimal, true);
        let text: String = r
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        // The bare URL appears exactly once, not as "url (url)".
        assert_eq!(
            text.matches("https://example.com").count(),
            1,
            "text was:\n{text}"
        );
    }

    #[test]
    fn anchor_links_do_not_show_inline_url() {
        let md = "See [the section](#intro) for details.\n";
        let r = render(md, 80, plain(), LayoutName::Minimal, true);
        let text: String = r
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !text.contains("#intro"),
            "anchor link should not render inline: {text}"
        );
        assert!(text.contains("the section"));
    }

    #[test]
    fn relative_file_links_show_inline_url() {
        // Translation links and similar relative file references — the path is
        // the only cue that the text points to another file. We render the URL
        // inline so the reader can see (and the terminal can hyperlink) it.
        let md = "[Español](README.es.md) | [中文](README.zh-cn.md)\n";
        let r = render(md, 80, plain(), LayoutName::Minimal, true);
        let text: String = r
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("(README.es.md)"),
            "relative file link should render its URL inline: {text}"
        );
        assert!(
            text.contains("(README.zh-cn.md)"),
            "relative file link should render its URL inline: {text}"
        );
    }

    #[test]
    fn tables_render_header_and_rows() {
        let md = "| A | B |\n|---|---|\n| one | two |\n| three | four |\n";
        let r = render(md, 80, plain(), LayoutName::Minimal, true);
        let text: String = r
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        for s in ["A", "B", "one", "two", "three", "four"] {
            assert!(
                text.contains(s),
                "expected `{s}` in rendered table:\n{text}"
            );
        }
    }

    #[test]
    fn tables_insert_row_separators_when_rows_wrap() {
        let md = "\
| id | description |
|----|-------------|
| a | one two three four five six seven eight nine ten |
| b | short |
";
        let r = render(md, 40, plain(), LayoutName::Minimal, true);
        let text: String = r
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        // A light ╌ separator must appear between the two body rows.
        assert!(
            text.contains('\u{254C}'),
            "expected light row separator when any row wraps:\n{text}"
        );
    }

    #[test]
    fn compact_tables_have_no_row_separators() {
        let md = "\
| A | B |
|---|---|
| 1 | 2 |
| 3 | 4 |
";
        let r = render(md, 80, plain(), LayoutName::Minimal, true);
        let text: String = r
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !text.contains('\u{254C}'),
            "no row separator expected for single-line rows:\n{text}"
        );
    }

    #[test]
    fn tables_wrap_long_cells_instead_of_truncating() {
        let md = "| id | description |\n|----|-------------|\n| a | one two three four five six seven eight nine ten |\n";
        let r = render(md, 40, plain(), LayoutName::Minimal, true);
        let text: String = r
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        // All ten content words must appear somewhere — nothing gets dropped.
        for w in [
            "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
        ] {
            assert!(text.contains(w), "word {w} missing in:\n{text}");
        }
        // Wrapped into multiple visual rows: at least one line has the
        // separator but not the id 'a' (continuation of the description cell).
        let any_continuation = r
            .lines
            .iter()
            .map(Line::to_string)
            .any(|l| l.contains('\u{2502}') && !l.contains(" a "));
        assert!(
            any_continuation,
            "expected cell content to wrap onto continuation lines:\n{text}"
        );
    }

    #[test]
    fn tables_with_inline_code_cells() {
        let md = "| name | value |\n|------|-------|\n| `x` | 1 |\n";
        let r = render(md, 80, plain(), LayoutName::Minimal, true);
        let text: String = r
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
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
        assert!(
            cell_line.is_some(),
            "x and 1 should share a single row line:\n{text}"
        );
    }

    #[test]
    fn blockquote_has_gutter() {
        let md = "> quoted.\n";
        let r = render(md, 60, plain(), LayoutName::Minimal, true);
        let text: String = r
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("\u{2502}"));
    }

    #[test]
    fn heading_text_does_not_leak_into_following_paragraph() {
        let md = "### C-3. panics in enterprise context compaction\n\n- **C-3.** `todo!()` panics in enterprise context compaction\n";
        let r = render(md, 80, plain(), LayoutName::Minimal, true);
        let lines: Vec<String> = r.lines.iter().map(ToString::to_string).collect();
        // First rendered non-empty line should be the heading text, clean.
        let heading_line = lines.iter().find(|l| !l.trim().is_empty()).unwrap();
        assert_eq!(
            heading_line.trim(),
            "C-3. panics in enterprise context compaction"
        );

        // The bullet line must contain the item's body exactly once, with no
        // leading copy of the heading text.
        let bullet_line = lines
            .iter()
            .find(|l| l.contains('\u{2022}'))
            .expect("bullet line not found");
        // The heading text should not appear on the bullet line.
        assert!(
            !bullet_line.contains("C-3. panics in enterprise context compaction"),
            "heading text leaked into bullet: {bullet_line}"
        );
        assert!(bullet_line.contains("todo!()"));
    }

    #[test]
    fn heading_with_inline_code_does_not_leak() {
        let md = "### A `todo!()` heading\n\nBody paragraph.\n";
        let r = render(md, 60, plain(), LayoutName::Minimal, true);
        let lines: Vec<String> = r.lines.iter().map(ToString::to_string).collect();
        let body = lines.iter().find(|l| l.contains("Body")).unwrap();
        assert!(!body.contains("todo!()"), "heading code leaked: {body}");
        assert!(!body.contains("heading"), "heading text leaked: {body}");
    }

    #[test]
    fn smart_substitution_preserves_text() {
        let md = "He said \"yes\" -- probably...\n";
        let r = render(md, 60, plain(), LayoutName::Minimal, true);
        let text: String = r
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("\u{201C}yes\u{201D}"));
        assert!(text.contains("\u{2014}"));
        assert!(text.contains("\u{2026}"));
    }

    #[test]
    fn html_tags_are_hidden() {
        let md = "Hello <em>world</em> and <div>x</div>.\n";
        let r = render(md, 60, plain(), LayoutName::Minimal, true);
        let text: String = r
            .lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!text.contains("<em>"));
        assert!(!text.contains("</em>"));
        assert!(!text.contains("<div>"));
        assert!(!text.contains("</div>"));
    }
}
