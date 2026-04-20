//! glum — a reading-focused terminal markdown viewer.
//!
//! This crate is the library half of the `glum` binary: the markdown
//! renderer, the TUI application loop, the clipboard transport, and the
//! persistence store that remembers your reading position. It's published
//! mostly so users can read the source as documentation and so contributors
//! can navigate on [docs.rs]; you usually want the [`glum`] binary, not a
//! direct dependency on this library.
//!
//! # What's inside
//!
//! - [`render`] — parses `CommonMark` with
//!   [`pulldown_cmark`](https://docs.rs/pulldown-cmark) and produces
//!   pre-styled, pre-wrapped [`ratatui`](https://docs.rs/ratatui) lines.
//!   Central entry point: [`render::render`].
//! - [`app`] — the `ratatui` application loop: paging, theme cycling, table of
//!   contents, search, code-block copy, external-editor handoff. Entry
//!   point: [`app::run`].
//! - [`theme`] — five color themes (`light`, `dark`, `sepia`, `night`,
//!   `plain`) plus their per-role styles (headings, code, quotes, rules,
//!   links). See [`theme::Theme::resolve`].
//! - [`layout`] — two typographic layouts (`minimal`, `vivid`) that
//!   drive heading decorations and rule heaviness.
//! - [`highlight`] — small per-language token highlighters for fenced code
//!   blocks. Intentionally lightweight: keyword sets, generic string /
//!   comment / number rules. See [`highlight::highlight_line`].
//! - [`cli`] — [`clap`] argument definitions for the `glum` binary.
//! - [`clipboard`] — OSC 52 copy with native (`pbcopy`, `wl-copy`, `xclip`,
//!   `xsel`) fallbacks. See [`clipboard::copy`].
//! - [`positions`] — on-disk store for reading positions and remembered
//!   preferences, with SHA-256-hashed paths so the store doesn't reveal
//!   which files have been read.
//! - [`typography`] — "smart quote" typographic substitution
//!   (`--` → `—`, `...` → `…`, straight → curly quotes).
//! - [`watch`] — debounced filesystem watcher used by `--follow`.
//!
//! # Example: render a markdown string to styled lines
//!
//! ```no_run
//! use glum_lib::layout::LayoutName;
//! use glum_lib::render;
//! use glum_lib::theme::{Theme, ThemeName};
//!
//! let md = "# Hello\n\nA paragraph with `code` and a [link](https://example.com).";
//! let r = render::render(
//!     md,
//!     /* measure */ 72,
//!     Theme::resolve(ThemeName::Plain),
//!     LayoutName::Minimal,
//!     /* wrap_code */ true,
//! );
//! // `r.lines` is a `Vec<ratatui::text::Line<'static>>` ready to render;
//! // `r.toc` is the heading outline; `r.code_blocks` is the set of fenced
//! // blocks, each carrying its raw source text plus visual-row ranges.
//! for line in &r.lines {
//!     println!("{}", line.to_string());
//! }
//! ```
//!
//! # Stability
//!
//! This library's API is not stabilized. It tracks glum's binary; minor
//! releases may break library consumers. Pin exact versions if you depend
//! on it.
//!
//! [`glum`]: https://crates.io/crates/glum
//! [docs.rs]: https://docs.rs/glum
//! [`clap`]: https://docs.rs/clap

#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

pub mod app;
pub mod cli;
pub mod clipboard;
pub mod highlight;
pub mod layout;
pub mod positions;
pub mod render;
pub mod theme;
pub mod typography;
pub mod watch;
