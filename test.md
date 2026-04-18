# Markdown Cheat Sheet

A single document exercising every markdown feature Glum renders. Use it to
sanity-check headings, inline styles, lists, tables, code blocks, quotes,
links, typography, and search.

## Headings

# Heading 1
## Heading 2
### Heading 3
#### Heading 4
##### Heading 5
###### Heading 6

## Inline formatting

Plain text is just plain text. You can make it **bold**, *italic*, or
***both***. You can also ~~strike it through~~ when you change your mind.
Inline `code` reads in a monospaced style and stays on the current line.

Typographic substitutions happen automatically: "curly double quotes",
'curly singles', don't-break-apostrophes, em-dashes -- like so -- and
ellipses... are rendered with real glyphs.

Line breaks inside a paragraph are collapsed into spaces, so these
three lines
fold together into a single flowing paragraph when you read them.

Hard break with two spaces at the end.  
Next line starts here.

## Lists

Unordered:

- First item
- Second item with a longer description that demonstrates how wrapped
  continuation lines hang under the item text rather than under the bullet
- Third item
  - Nested child A
  - Nested child B
    - Grand-nested leaf

Ordered:

1. Do the thing
2. Then the other thing
3. Finally the third thing

Task list:

- [x] Parse markdown
- [x] Style it nicely
- [ ] World domination
- [ ] Take a nap first

## Block quotes

> "The only way to do great work is to love what you do."
>
> — Steve Jobs

Nested quote:

> Outer quote.
>
> > Inner quote nested inside the first.
> >
> > Continues on a second line.

## Code

Inline: use `let x = 1;` or `pip install glum` where appropriate.

Fenced, no language:

```
plain fenced block
preserves whitespace
    and leading spaces
```

Rust:

```rust
use std::collections::HashMap;

fn fibonacci(n: u32) -> u64 {
    // classic textbook recurrence
    match n {
        0 => 0,
        1 => 1,
        _ => fibonacci(n - 1) + fibonacci(n - 2),
    }
}

pub struct Point {
    pub x: f64,
    pub y: f64,
}
```

Python:

```python
from dataclasses import dataclass

@dataclass
class Point:
    x: float
    y: float

def distance(a: Point, b: Point) -> float:
    """Euclidean distance between two points."""
    return ((a.x - b.x) ** 2 + (a.y - b.y) ** 2) ** 0.5
```

JavaScript:

```javascript
const fetchUser = async (id) => {
  const res = await fetch(`/api/users/${id}`);
  if (!res.ok) throw new Error("failed");
  return res.json();
};
```

Go:

```go
package main

import "fmt"

func main() {
    nums := []int{1, 2, 3}
    for i, n := range nums {
        fmt.Printf("%d: %d\n", i, n)
    }
}
```

Bash:

```bash
#!/usr/bin/env bash
set -euo pipefail

for file in *.md; do
    echo "processing $file"
    wc -l "$file"
done
```

JSON:

```json
{
  "name": "glum",
  "version": "0.1.0",
  "features": ["reader", "themes", "search"]
}
```

YAML:

```yaml
name: glum
version: 0.1.0
features:
  - reader
  - themes
  - search
enabled: true
```

TOML:

```toml
[package]
name = "glum"
version = "0.1.0"

[dependencies]
ratatui = "0.29"
```

A very long code line that should be clipped rather than wrapped so the reading measure stays intact:

```rust
fn extremely_long_function_name_demonstrating_truncation(arg_one: &str, arg_two: &str, arg_three: &str, arg_four: &str, arg_five: &str) -> Result<String, Box<dyn std::error::Error>> { Ok(String::new()) }
```

## Tables

A small table:

| Name   | Paradigm   | Year |
|--------|------------|------|
| Rust   | systems    | 2010 |
| Python | scripting  | 1991 |
| Go     | systems    | 2009 |
| Elixir | functional | 2011 |

A table with inline code and links:

| Tool     | Language | Repo                                |
|----------|----------|-------------------------------------|
| `glum`| Rust     | [github](https://example.com/toot)  |
| `glow`   | Go       | [github](https://example.com/glow)  |
| `mdcat`  | Rust     | [github](https://example.com/mdcat) |

A big audit-style table with long prose cells (exercises wrapping, column width
heuristics, and the row separators that appear only when cells wrap):

| ID      | Finding                                         | Status & notes |
|---------|-------------------------------------------------|----------------|
| CRIT-1  | Unauthenticated workflow endpoint               | **FIXED** — all mutating routes are behind auth middleware; route inventory verified in §6. |
| CRIT-2  | Non–constant-time token compare                 | **FIXED** — `subtle::ConstantTimeEq` used in `api/middleware.rs`, `http_input/server.rs`, webhook verifier, Slack/Mattermost signature verify. |
| CRIT-3  | `Debug` leaks on `Secret`                       | **FIXED** — `secrets/mod.rs:133` has a redacting `Debug` impl. Residual risk: `VaultAuthConfig` still derives raw `Debug` (see HIGH-4). |
| CRIT-4  | `VaultAuthConfig` debug leak                    | **PARTIAL** — the enum no longer derives `Debug`, but the parent `VaultConfig` can transitively print credential fields via `Display` or error formatting. |
| CRIT-5  | Master encryption key printed to stderr         | **PARTIAL** — the raw `new_key` is no longer printed, but `crypto.rs:353–371` still emits `eprintln!` chatter during the same operation; design still fails to write the key to a 0600 file or require user confirmation. |
| CRIT-6  | `MockSandboxOrchestrator` default               | **RESOLVED** — the production path now routes to real `DockerRunner` / `E2B` / `NativeRunner`. |
| CRIT-7  | Unvalidated sandbox config                      | **PARTIAL** — Docker hardening flags are on by default; volume validation added; but symlink escapes and "named volume" passthrough still exist. |
| CRIT-8  | Native provides no isolation                    | **MITIGATED** — `SYMBIONT_ENV=production` hard-blocks native runner; `SandboxTier::None` has a guard overridable via `SYMBIONT_ALLOW_UNISOLATED=1`. String-based env check is spoofable. |
| CRIT-9  | Docker/gVisor/Firecracker unimplemented         | **PARTIAL** — Docker and E2B are now implemented; gVisor and Firecracker remain stubs reachable from policy parser. |
| CRIT-10 | No process containerisation                     | **RESOLVED for Docker tier**; remains true for native (by design). |
| CRIT-11 | Resource limits only logged                     | **NOT FIXED** — `resource/mod.rs:287–303` still logs violations without killing or throttling. Enforcement is delegated to Docker cgroups when used, absent otherwise. |
| HIGH-1  | 500 vs 401 on missing env                       | **FIXED**. |
| HIGH-2  | `CorsLayer::permissive()`                       | **FIXED** — permissive CORS replaced with allowlist from `SYMBIONT_CORS_ORIGINS`, `allow_credentials(false)`. Wildcard still warned, not rejected. |
| HIGH-3  | `X-Forwarded-For` spoof                         | **FIXED** — trusted-proxy CIDR required via `SYMBIONT_TRUSTED_PROXIES`. |
| HIGH-4  | JWT not implemented                             | **FIXED** — full JWT verifier exists; new HIGH finding: audience is optional. |
| HIGH-5  | No memory zeroization                           | **NOT FIXED** — `zeroize` is not used on `Secret`, KDF-derived keys, or `auth_token`. |
| HIGH-6  | TLS skip-verify                                 | **PARTIAL** — blocked in `SYMBIONT_ENV=production`, allowed elsewhere; still bypassable via typo'd env. |
| HIGH-7  | Argon2 parameters weak                          | **NOT FIXED** — `crypto.rs:172–176, 253–257` still `Params::new(19*1024, 2, 1, …)`. OWASP 2024 recommends 19 MiB + 2 iters for the `m=19456` profile; this code uses it for data-at-rest KDF where 64 MiB / 3 iters is standard. |
| HIGH-8  | Config claims PBKDF2 but uses Argon2            | **NEEDS VERIFY** — not re-verified this round; re-open for triage. |
| HIGH-9  | File permissions not enforced                   | **NOT FIXED** — `secrets/file_backend.rs` opens file without checking mode. |

## Links and images

Inline link to [the Wikipedia article on typography](https://en.wikipedia.org/wiki/Typography).
Autolink: <https://example.com>.
Reference-style link to [Rust][rust-lang] and [another one][rust-lang].

[rust-lang]: https://www.rust-lang.org

An image reference (glum renders this as a placeholder because TUIs can't
show pixels):

![logo placeholder](https://example.com/logo.png)

## Horizontal rule

Above the rule.

---

Below the rule.

## Footnotes

Here is a sentence with a footnote[^note]. And here is another[^second].

[^note]: The footnote text lives at the bottom of the document.
[^second]: A second footnote for good measure.

## Typography torture test

"She said, 'it's -- honestly -- a bit much...'" he replied, a tad wearily.
Em dashes---like this---also collapse to single em dashes. The rule:
non-breaking word-boundary cases should stay intact (well-known, state-of-the-art).

A paragraph mixing **bold with a `code` span** and *italic with a [link](https://example.com) inside* to verify span restyling survives wrapping across lines when a long sentence doesn't quite fit on a single 72-column row.

## Long prose (paging test)

Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut aliquip ex ea commodo consequat. Duis aute irure dolor in reprehenderit in voluptate velit esse cillum dolore eu fugiat nulla pariatur. Excepteur sint occaecat cupidatat non proident, sunt in culpa qui officia deserunt mollit anim id est laborum.

Sed ut perspiciatis unde omnis iste natus error sit voluptatem accusantium doloremque laudantium, totam rem aperiam, eaque ipsa quae ab illo inventore veritatis et quasi architecto beatae vitae dicta sunt explicabo. Nemo enim ipsam voluptatem quia voluptas sit aspernatur aut odit aut fugit, sed quia consequuntur magni dolores eos qui ratione voluptatem sequi nesciunt.

Neque porro quisquam est, qui dolorem ipsum quia dolor sit amet, consectetur, adipisci velit, sed quia non numquam eius modi tempora incidunt ut labore et dolore magnam aliquam quaerat voluptatem. Ut enim ad minima veniam, quis nostrum exercitationem ullam corporis suscipit laboriosam, nisi ut aliquid ex ea commodi consequatur.

At vero eos et accusamus et iusto odio dignissimos ducimus qui blanditiis praesentium voluptatum deleniti atque corrupti quos dolores et quas molestias excepturi sint occaecati cupiditate non provident, similique sunt in culpa qui officia deserunt mollitia animi, id est laborum et dolorum fuga. Et harum quidem rerum facilis est et expedita distinctio.

## Unicode and CJK

Greek: αβγδε ζηθικ λμνξο πρστυ φχψω.
Cyrillic: абвгд еёжзи йклмн опрст уфхцч шщъыь эюя.
Japanese: これは日本語のテストです。読みやすいでしょうか。
Chinese: 终端里的阅读体验也应当舒适。
Arabic (RTL, may render oddly in some terminals): مرحبا بالعالم.
Emoji: 🎉 📚 ✨ 🦀 — terminal support varies.

## Search targets

Look for the word **needle** exactly three times in this section: needle,
needle, and finally needle. Use `/needle` and the `n` key to jump between
the occurrences.

---

End of cheat sheet. Press `q` to quit.
