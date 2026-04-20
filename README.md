<p align="center">
  <img src="https://raw.githubusercontent.com/jaschadub/glum/main/glum-logo.png" alt="glum" width="480">
</p>

# glum

A reading-focused terminal markdown viewer — more like "Reader Mode in your
terminal" than a markdown-as-markdown renderer.

Glum optimizes for *reading prose*, not decorating syntax. Headings become
typographic styles, not `#` prefixes; links show their URL inline so you can
see and click them; code blocks are framed with rules and highlighted; tables
wrap rather than truncate; the file scrolls a page at a time like `less`.

## Install

### macOS / Linux — one-liner

```bash
curl -fsSL https://raw.githubusercontent.com/jaschadub/glum/main/scripts/install.sh | bash
```

This downloads the latest signed release archive for your platform, verifies
its SHA-256, and installs `glum` into `$HOME/.local/bin`. Override the prefix
with `GLUM_PREFIX=/usr/local`, pin a version with `GLUM_VERSION=v0.2.0`.

### Any platform — via cargo

```bash
cargo install glum
```

### Homebrew, apt, winget, etc.

Not yet. Until then use cargo or the one-liner above. On Windows, grab the
`.zip` from the [releases page](https://github.com/jaschadub/glum/releases)
or use `cargo install glum`.

### From source

```bash
cargo build --release
./target/release/glum test.md
```

### Verify a release download

Every release archive and `checksums.txt` is signed with
[Sigstore cosign](https://docs.sigstore.dev/) via GitHub Actions OIDC
(keyless). Verify with:

```bash
cosign verify-blob \
  --certificate glum-*.pem \
  --signature glum-*.sig \
  --certificate-identity-regexp="https://github.com/jaschadub/glum" \
  --certificate-oidc-issuer="https://token.actions.githubusercontent.com" \
  glum-*.tar.gz
```

## Features

- **Reader layout** — narrow, centered measure (default 72 cols), five
  themes (`T`), two typographic layouts (`L`), three alignments (`A`).
  All toggle at runtime and are remembered across runs.
- **Code blocks** — syntax highlighting for 12 languages, top/bottom
  rules with a language label, no side borders. Long lines soft-wrap
  with `↪` or truncate with `…` (`W`).
- **Copy** — `y` the block, `Y` pick a single source line, `R` open a
  full-screen raw view with horizontal pan. Copies always come from the
  original source, so no `↪` or `…` leaks. Uses native clipboard
  (`pbcopy` / `wl-copy` / `xclip` / `xsel`) when available, OSC 52
  otherwise.
- **Tables** wrap long cells instead of truncating, keep column
  separators aligned, and draw `╌` row separators when any row wraps.
- **Links** render inline URLs for external and relative paths; anchor
  links stay clean.
- **Search** (`/`) with live match count and `n` / `N` / Tab stepping.
- **TOC** (`t`) overlay; jump to first match of a heading title with
  `-H`.
- **Editor handoff** — `e` opens `$EDITOR` / `$VISUAL` at the nearest
  heading's source line, then reloads.
- **Follow mode** (`-f`) re-renders on file change; `r` reloads manually.
- **Per-file position memory**, hashed paths, atomic writes. Opt out
  with `--no-remember`.
- **Optional mouse scroll** (`--mouse`) — off by default so the
  terminal's native text selection keeps working.
- **Smart typography** (`"..."` → `“…”`, `--` → `—`, `...` → `…`).
- **Terminal-safe panic handler** — a crash still restores your shell.
- **Bounded inputs** — 64 MiB file cap; 256-char search cap.

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
      --mouse               enable mouse-wheel scrolling (disables native text selection)
  -f, --follow              re-render when the file changes on disk
```

If `--theme`, `--layout`, or `--align` is omitted, the last value you used is
restored. First-run defaults: `minimal` / `center`, plus `light` or
`dark` based on the terminal's advertised background (via `$COLORFGBG`,
falling back to `dark`).

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
| Y                     | pick a single code line to copy (j/k, Enter)  |
| R                     | raw code view — no wrap, h/l pans, y copies   |
| r                     | reload the current file from disk             |
| e                     | open in `$EDITOR` at the nearest heading      |
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
