# Tootles Sample

A terminal markdown reader that prioritizes *reading prose* over showing markdown-as-markdown.

## Basic features

The reader handles the usual stuff:

- **Bold**, *italic*, and `inline code`
- Paged scrolling like `less` — press space to advance
- Smart typography: "double quotes", 'single', and em-dashes -- like this
- Links become footnotes: [the homepage](https://example.com)

Long paragraphs wrap to a narrow measure. A wise person once said that line length is the most important single typographic choice, because too long and the eye loses its place on return, while too short and the rhythm is broken every few words.

## Code

Code blocks do *not* wrap. Language fences enable syntax highlighting:

```rust
fn fibonacci(n: u32) -> u64 {
    // classic textbook recurrence
    match n {
        0 => 0,
        1 => 1,
        _ => fibonacci(n - 1) + fibonacci(n - 2),
    }
}
```

```python
def greet(name: str) -> None:
    # say hello
    print(f"hello, {name}!")
```

## Quoting

> All truly great thoughts are conceived while walking.
>
> — Friedrich Nietzsche

## Tables

| language | paradigm   | year |
|----------|------------|------|
| Rust     | systems    | 2010 |
| Python   | scripting  | 1991 |
| Go       | systems    | 2009 |

## Tasks

- [x] Parse markdown
- [x] Style it
- [ ] World domination

---

That's the sample. Press `q` to quit, `T` to cycle themes.
