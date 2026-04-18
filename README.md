# tootles

A reading-focused terminal markdown viewer — more like "Reader Mode in your
terminal" than a markdown-as-markdown renderer.

Tootles prioritizes reading *prose* over showing markdown syntax. That means:

- Narrow, centered measure (configurable; default 72 columns)
- Paged scrolling like `less` — space/PgDn to advance, b/PgUp to go back
- Hidden chrome: headings become typographic styles (no `#` prefixes), links
  become numbered footnotes at the end of the document
- Muted themes (light / dark / sepia / night / plain) with cycling at runtime
- Smart typography: curly quotes, em-dashes, ellipses
- Table of contents, in-file search, position memory across runs
- Light syntax highlighting for fenced code blocks (Rust, Python, JS/TS, Go,
  Bash, JSON, YAML, TOML, HTML/XML, C/C++, Java)

## Install

```
cargo install --path .
```

or just

```
cargo build --release
./target/release/tootles sample.md
```

## Usage

```
tootles [OPTIONS] <PATH>

  --measure <N>       column width (20..200, default 72)
  --theme <NAME>      light, dark, sepia, night, plain (default dark)
  --no-remember       don't persist reading position
```

Pass `-` as the path to read from stdin.

## Keys

| key            | action                  |
|----------------|-------------------------|
| j / ↓          | scroll down             |
| k / ↑          | scroll up               |
| space / PgDn   | page down               |
| b / PgUp       | page up                 |
| d / u          | half page               |
| g / G          | top / bottom            |
| t              | table of contents       |
| T              | cycle theme             |
| /              | search                  |
| n / N          | next / previous match   |
| ?              | help                    |
| q / Esc        | quit / close overlay    |

## Position memory

Tootles remembers where you were in each file. State lives at
`$XDG_STATE_HOME/tootles/positions.json` (or `~/.local/state/tootles/` as
fallback). Paths are hashed with SHA-256 before being stored, so the state
file does not reveal the names of files you've read. Pass `--no-remember`
to opt out.

## License

MIT OR Apache-2.0
