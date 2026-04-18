# Changelog

All notable changes to glum are documented in this file. Format based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] — 2026-04-18

First public release. Published to crates.io.

### Added

- Reading-focused markdown TUI with narrow centered measure, paged
  navigation (`space` / `PgDn` / `b` / `PgUp` / `d` / `u` / `g` / `G`), and
  smart typography (curly quotes, em-dash, ellipsis).
- Five color themes: `light`, `dark`, `sepia`, `night`, `plain`. Cycle with
  `T` at runtime; choice persists across runs.
- Two typographic layouts: `minimal` (subdued) and `vivid` (strong heading
  hierarchy with `❯ § ▸ ›` prefixes and rules). Toggle with `L`.
- Column alignment: `center`, `left`, or `right`. Toggle with `A`.
- Syntax highlighting for 12 languages: Rust, Python, JS / TS, Go, Bash,
  JSON, YAML, TOML, HTML / XML, C / C++, Java.
- Code blocks render as top/bottom-ruled sections with a language label and
  a copy affordance. No side borders — mouse selection yields clean code.
- Long code lines soft-wrap by default with a `↪` continuation marker.
  Toggle truncate-with-`…` with `W` or `--truncate-code`.
- Clipboard copy via OSC 52 (`y` key). Auto-hidden and disabled in SSH
  sessions where OSC 52 often gets stripped.
- Table rendering with per-cell wrapping, smart column-width allocation,
  and light `╌` row separators when any row wraps.
- Inline link URLs — `[text](url)` renders as `text (url)` in dim. Autolinks
  aren't duplicated; anchor and relative links show just the text.
- In-file search (`/`) with live match count, persistent footer counter,
  `n` / `N` / `Tab` / `Shift-Tab` / `→` / `←` navigation, and `c` to clear.
- Table of contents overlay (`t`).
- Position memory per file (SHA-256-hashed paths), plus remembered theme /
  layout / align / code-wrap preferences.
- `--follow` / `-f` auto-reload on file change, debounced 120ms, scroll
  position preserved across reloads.
- CLI pre-seeding flags: `--search` / `-s`, `--heading` / `-H`, `--toc`,
  `--reset-position`, `--truncate-code`, `--no-remember`.
- Terminal-safe panic handler restores raw-mode / alternate screen / cursor
  on crash.

### Security

- 64 MiB input-file size cap.
- 256-char search query cap.
- Atomic writes on state file, mode `0600`.
- SHA-256 path hashing so the state file does not reveal which files have
  been read.
- No `unsafe` code (`#![forbid(unsafe_code)]`).

### License

- Apache-2.0. See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).

[0.1.0]: https://github.com/jaschadub/glum/releases/tag/v0.1.0
