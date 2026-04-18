# glum

A reading-focused terminal markdown viewer — more like "Reader Mode in your
terminal" than a markdown-as-markdown renderer.

Glum optimizes for *reading prose*, not decorating syntax. Headings become
typographic styles, not `#` prefixes; links show their URL inline so you can
see and click them; code blocks are framed with rules and highlighted; tables
wrap rather than truncate; the file scrolls a page at a time like `less`.

## Install

From crates.io:

```
cargo install glum
```

From source:

```
cargo build --release
./target/release/glum test.md
```

## Features

- **Narrow, centered measure** (configurable, default 72 cols) so long lines
  don't kill reading. Toggle to left or right anchoring with `A`.
- **Five color themes** — `light`, `dark`, `sepia`, `night`, `plain` — cycled
  with `T` at runtime. Your choice is remembered across runs.
- **Two typographic layouts** — `minimal` (subdued) and `vivid` (strong
  hierarchy with heading prefixes `❯ § ▸ ›` and full-width rules), toggle
  with `L`.
- **Syntax highlighting** for fenced code in 12 languages: Rust, Python,
  JavaScript / TypeScript, Go, Bash, JSON, YAML, TOML, HTML / XML, C / C++,
  Java. Unknown fences pass through cleanly.
- **Code blocks** render as top/bottom-ruled sections with a language label
  and a copy affordance. No side borders, so mouse selection yields clean
  code. Long code lines soft-wrap by default (with a `↪` continuation
  marker); press `W` to flip to truncate-with-`…`.
- **Clipboard copy via OSC 52** — `y` copies the in-view code block to the
  system clipboard. Works over SSH on terminals that support OSC 52; glum
  auto-hides the affordance and disables the key in SSH sessions where it
  often gets stripped by tmux / forwarding.
- **Table rendering** wraps long cells across multiple visual rows, keeps
  column separators aligned on every row, and draws light `╌` separators
  between rows when any row wraps.
- **Inline link URLs** — `[text](https://...)` shows `text (https://...)`
  in dim; autolinks aren't duplicated; anchor / relative links show just
  the text.
- **Smart typography** — straight quotes become curly, `--` becomes em-dash
  `—`, `...` becomes `…`.
- **In-file search** (`/`) with live match count in the overlay, persistent
  `match N/total` counter in the footer, Tab / ↓ next, Shift-Tab / ↑ prev,
  `c` to clear.
- **Table of contents** (`t`) — navigable overlay built from the headings.
- **Position memory** per file, plus remembered theme / layout / align /
  wrap preferences. State lives in `$XDG_STATE_HOME/glum/positions.json`.
- **Follow mode** (`-f` / `--follow`) re-renders when the file changes on
  disk — great paired with an editor in another pane. Scroll position is
  preserved across reloads.
- **Terminal-safe panic handler** — a crash restores raw-mode, alternate
  screen, and cursor so you don't end up with a broken shell.
- **Bounded inputs**: 64 MiB file-size cap; 256-char search query cap;
  atomic writes on the state file with 0600 perms; SHA-256 path hashing so
  the state file doesn't reveal which files you've read.

## Usage

```
glum [OPTIONS] <PATH>

  --measure <N>             column width (20..200, default 72)
  --theme <NAME>            light, dark, sepia, night, plain
  --layout <NAME>           minimal or vivid
  --align <NAME>            center, left, or right
  -s, --search <QUERY>      open with search pre-populated; jump to first match
  -H, --heading <TITLE>     jump to first heading containing TITLE (case-insensitive)
      --toc                 open with the table of contents overlay visible
      --reset-position      ignore saved position; start at the top
      --truncate-code       truncate long code lines with `…` instead of soft-wrapping
      --no-remember         don't persist reading position / preferences across runs
  -f, --follow              re-render when the file changes on disk
```

If `--theme`, `--layout`, or `--align` is omitted, the last value you used is
restored (first-run defaults: `dark` / `minimal` / `center`).

Pass `-` as the path to read from stdin.

Examples:

```
glum README.md                              # just read
glum -s needle test.md                      # open, search for "needle"
glum -H "Code blocks" test.md               # jump to a specific heading
glum --toc test.md                          # open with TOC visible
glum --theme light --measure 80 foo.md
glum --layout vivid --align left doc.md
glum -f README.md                           # auto-reload on save while editing
cat post.md | glum -                        # read from stdin
```

## Keys

| key                   | action                                        |
|-----------------------|-----------------------------------------------|
| j / ↓                 | scroll down                                   |
| k / ↑                 | scroll up                                     |
| space / PgDn          | page down                                     |
| b / PgUp              | page up                                       |
| d / u                 | half page down / up                           |
| g / G                 | top / bottom                                  |
| t                     | table of contents                             |
| /                     | open search                                   |
| n / N / Tab / → / ←   | next / previous search match                  |
| c                     | clear active search                           |
| y                     | copy the in-view code block to clipboard      |
| T                     | cycle theme                                   |
| L                     | cycle layout (minimal ↔ vivid)                |
| A                     | toggle align (center → left → right)          |
| W                     | toggle code wrap / truncate                   |
| ?                     | toggle help overlay                           |
| q / Esc               | quit (reading) or close overlay               |
| Ctrl-C                | force quit from any mode                      |

## Position and preference memory

Glum remembers where you were in each file and what theme / layout / align /
wrap-mode you were using. State lives at `$XDG_STATE_HOME/glum/positions.json`
(or `~/.local/state/glum/` as fallback). Paths are hashed with SHA-256 before
being stored, so the state file does not reveal which files you've read. The
file is written atomically with mode `0600`. Pass `--no-remember` to opt out
of all persistence, or `--reset-position` to ignore the saved offset just for
the current run.

## License

Apache License, Version 2.0. See [LICENSE](./LICENSE) and [NOTICE](./NOTICE).

Copyright © 2026 Jascha Wanger (<https://jascha.me>).
