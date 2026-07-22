# Changelog

All notable changes to glum are documented in this file. Format based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); project follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.3.0] — 2026-07-22

### Added

- **Two vibrant themes**: `meadow` (light) and `aurora` (dark) join the
  cycle — seven themes total.
- **Wide tables escape the measure**: a table that cannot fit the prose
  column may use the full terminal width before cells start wrapping.

- **Link opening** (`o`): pick any link with j/k (its line highlights,
  the footer shows the destination) and Enter opens it — http(s)/mailto
  via the OS opener, `#anchors` jump to their heading in-document,
  relative file paths open with the default app. Non-navigable schemes
  (`javascript:`, `data:`, …) are refused.
- **Image preview** (`i`, opt-in via `--images`): local images render
  full-screen using the terminal's best graphics protocol (kitty /
  sixel / iTerm2), with a half-block fallback everywhere else.
- **Tabs**: `glum a.md b.md c.md` opens every file; `]` / `[` switch
  documents, a tab bar appears across the top, and each file keeps its
  own remembered reading position. `--follow` re-points the watcher at
  the visible file.
- **Scrollbar** with search-match tick marks: a dim track on the right
  edge shows where you are, and every search match is marked on it in
  accent — the `5/12` counter now has a spatial meaning. The raw code
  view gets vertical and horizontal scrollbars on its frame.
- **Reading-progress gauge**: the footer's bare percentage is now a
  subtle line gauge with the percent as its label.
- **Search prompt editing**: Left/Right/Home/End move a real caret;
  Ctrl-A/E jump, Ctrl-W deletes the previous word, Ctrl-U clears to the
  start, Delete works, and pastes insert at the caret.
- **True background detection**: the first-run theme now asks the
  terminal for its actual background color via OSC 11
  (`terminal-colorsaurus`), falling back to `$COLORFGBG`, then `dark`.

### Fixed

- `--follow` no longer crashes when a reload removes or shrinks the code
  block open in the line-pick or raw-code view — stale block indices now
  drop back to reading mode.
- Terminals narrower than the measure: the document re-renders at the
  effective width instead of letting the widget re-wrap lines, so paging,
  the percent indicator, and search highlighting stay correct in narrow
  panes. Resizing now actually re-flows the text.
- Search matches text altered by smart typography: queries like `don't`,
  `--follow`, or `...` now match their curly/em-dash/ellipsis renderings
  (both sides are normalized before comparison).
- Pasting into the search prompt works (bracketed paste events were
  silently discarded).
- Cancelling a search (`Esc`) restores the scroll position from before the
  live preview moved it; a committed search jumps to the first match at or
  after the reading position (like `less`), not the top of the document.
- A panic while `--mouse` is active no longer leaves the terminal spewing
  mouse escape sequences.
- Squeezing the terminal very small with the raw-code or search overlay
  open no longer panics.
- Deeply nested blockquotes at narrow measures shorten the gutter instead
  of silently dropping the quoted text.
- Very long tokens (URLs) hard-break at the measure instead of overflowing
  and desyncing scroll math.
- Reading position is autosaved every couple of seconds, so a crash or
  SIGKILL no longer loses it.
- Footer filename shortening is display-width aware (CJK names no longer
  overflow the footer).
- Help overlay no longer clips its last line; on short terminals it
  truncates with an indicator instead of clipping silently.

### Changed

- Copy status is honest about its transport: only native-tool copies
  (`pbcopy`/`wl-copy`/`xclip`/`xsel`) claim success; the OSC 52 fallback
  reports "sent via OSC 52 — if paste fails, install xclip", since some
  terminals (VTE-based ones in particular) silently ignore the sequence.
- `Esc` in reading mode clears the active search (or does nothing) instead
  of quitting; `q` and `Ctrl-C` quit. Overlays still close on `Esc`.
- Ctrl-chords are real bindings now: `Ctrl-f`/`Ctrl-b` page, `Ctrl-d`/
  `Ctrl-u` half-page, `Ctrl-e`/`Ctrl-y` scroll one line. Ctrl/Alt chords no
  longer accidentally trigger plain-key actions (Ctrl-E used to open
  `$EDITOR`).
- `G` / `End` jumps to the last page, not a nearly blank screen with the
  last line at the top; scrolling clamps the same way.
- All search matches on screen are now marked (underlined accent), with the
  current match reversed — and only the matched substring, not the whole
  line. `n`/`N` announce when the search wraps around.
- Copy/reload confirmations are no longer hidden by the match counter while
  a search is active.
- The `sepia` theme is now the conventional light cream e-reader palette
  (it was previously a dark brown theme).
- Inline code renders bold in the `plain` theme so it is distinguishable
  without colors; inline-code padding uses plain spaces instead of
  no-break spaces, so mouse-selected text pastes cleanly into a shell.
- Minimal layout gains a quiet heading hierarchy: H2 is underlined, H3
  bold, H4+ dim — H2 was previously indistinguishable from bold body text.
- Blockquotes: the quoted text itself is styled (not just the `│` bar), and
  the bar continues across blank lines between paragraphs of one quote.
- Footnote definitions are labeled (`[1]: …`); images show their URL
  (`[image: alt — url]`).
- Horizontal rules span the full measure, matching heading and code rules.
- TOC overlay supports paging (`d`/`u`/`PgUp`/`PgDn`/`space`/`b`) and
  type-to-filter: `/` starts a live case-insensitive title filter
  (arrows/Tab move, Enter jumps, Esc returns to the full tree). The mouse
  wheel moves the selection instead of scrolling the hidden document.
- `R` opened from the line picker returns to the picker on `Esc`, not to
  reading mode.
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

[0.3.0]: https://github.com/jaschadub/glum/releases/tag/v0.3.0

## [0.2.2] — 2026-04-20

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
- **Signed `.deb` and `.rpm` packages** for `x86_64` and `aarch64` Linux
  attached to every GitHub Release (via `cargo-deb` and
  `cargo-generate-rpm`). Packages install the binary to `/usr/bin/glum`
  and land completions + man page in the right XDG paths. Each `.deb` /
  `.rpm` is cosign-signed alongside the existing archives.

[0.2.2]: https://github.com/jaschadub/glum/releases/tag/v0.2.2

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
