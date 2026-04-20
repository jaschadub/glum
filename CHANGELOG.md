# Changelog

All notable changes to glum are documented in this file. Format based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `--generate-completions <shell>` emits a shell-completion script for
  `bash`, `zsh`, `fish`, `elvish`, or `powershell` to stdout. Built from
  the actual CLI definition, so completions stay in sync automatically.
- `--generate-man` emits a roff man page to stdout.
- Release tarballs now include completions and the man page laid out in
  the conventional XDG directories (`share/bash-completion/completions/`,
  `share/zsh/site-functions/`, `share/fish/vendor_completions.d/`,
  `share/man/man1/`), so distro packagers can drop `share/` under a
  prefix without renames.

## [0.2.1] — 2026-04-19

### Changed

- Overlays (TOC, Search, Help, Raw code) now use rounded borders — a
  softer frame that matches glum's reader-first tone.
- TOC is rendered as a tree with `│ ├ └` connectors instead of plain
  indentation, so deep documents stay scannable.
- Vivid layout uses a heavy top rule (`━`) on code blocks to echo the
  heading hierarchy; minimal keeps the lighter `─`.
- Unordered list bullets graduate by depth in vivid layout
  (`• → ◦ → ▫`); minimal stays with a single `•` for quieter pages.
- First-run theme is now picked from the terminal's advertised
  background (`$COLORFGBG`) — light terminals open with `light`
  instead of `dark`. `--theme` and the remembered theme still win.

### Docs

- Full library-surface documentation at
  [docs.rs/glum](https://docs.rs/glum). Added a crate-level landing
  page with a `no_run` example of rendering markdown to ratatui lines,
  plus doc comments on every public item across `theme`, `layout`,
  `cli`, `app`, `render`, and `positions`. `[package.metadata.docs.rs]`
  now sets `all-features` and `--cfg docsrs`.

[0.2.1]: https://github.com/jaschadub/glum/releases/tag/v0.2.1

## [0.2.0] — 2026-04-19

### Added

- `Y` — per-line copy mode inside a code block. Moves a line cursor with
  `j`/`k`/`↑`/`↓`, highlights the selected source line (including any
  soft-wrapped continuations), and copies just that line on `y`/`Enter`.
  `Y` again copies the whole block; `R` jumps into the raw view; `Esc`
  exits.
- `R` — full-screen raw code overlay. Renders the current block with no
  wrap and horizontal pan (`h`/`l`/`←`/`→`, `0`/`$`). Per-line cursor
  with `y`/`Enter` copy; `Y` copies the whole block. `#` toggles a
  source-line-number gutter (on by default).
- Both new modes copy from the original unwrapped source, so copied text
  never contains the `↪` continuation marker or a truncation `…`.
- `e` — suspend the TUI and open `$VISUAL` / `$EDITOR` (default `vi`)
  at the source line of the nearest heading. Handles `$EDITOR` values
  with args (e.g. `"nvim --clean"`), sends `+<line>` only to editors
  that accept it (vi / vim / nvim / nano / emacs / kak / helix /
  micro / …), and reloads the file on exit.
- `r` — manually reload the current file (works with or without
  `--follow`).
- Native clipboard fallback. Local copy operations now prefer `pbcopy`
  (macOS), `wl-copy` (Wayland), `xclip`, or `xsel` when available; OSC
  52 remains the fallback and the only transport used in SSH sessions.
- `--mouse` — opt-in mouse-wheel scrolling. Left off by default so the
  terminal's native click-and-drag text selection keeps working.
- Status-bar flash: successful copies and edits briefly reverse the
  accent color so the confirmation is hard to miss.

### Changed

- README Features section simplified — the long per-feature prose was
  collapsed into a compact list so the project's shape is easier to
  scan.

[0.2.0]: https://github.com/jaschadub/glum/releases/tag/v0.2.0

## [0.1.1] — 2026-04-18

First release with pre-built binaries and a curl-install one-liner.
Code is functionally identical to 0.1.0; this release exists to kick
off the release-binaries CI pipeline and produce signed binary
artifacts on GitHub Releases.

### Added

- Pre-built binaries for x86_64/aarch64 Linux, x86_64/aarch64 macOS,
  and x86_64 Windows, attached to the GitHub Release.
- Sigstore cosign signatures (keyless GitHub OIDC) on every archive
  plus a signed `checksums.txt`.
- `scripts/install.sh` one-liner installer for macOS / Linux with
  SHA-256 verification against the signed checksums.
- `.github/workflows/release-binaries.yml` — release pipeline.
- `.github/workflows/publish-crates.yml` — automatic crates.io
  publish on GitHub Release (with tag/version guard and idempotent
  re-run behavior).
- `.github/workflows/test.yml` — clippy + rustfmt + build + test
  matrix across Linux / macOS / Windows.
- Expanded README covering every runtime toggle (theme / layout /
  align / code-wrap) and install path.

### Fixed

- Applied `cargo fmt` across the codebase so the CI fmt-check step
  stays green.

[0.1.1]: https://github.com/jaschadub/glum/releases/tag/v0.1.1

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
