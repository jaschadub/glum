use tootles_lib::render::render;
use tootles_lib::theme::{Theme, ThemeName};

fn plain() -> Theme {
    Theme::resolve(ThemeName::Plain)
}

#[test]
fn complete_document_renders() {
    let md = r#"# Main title

A paragraph with **bold** and *italic*.

## Subsection

- first
- second with `code`
- third

> A wise quote.

```rust
fn main() {
    println!("hello");
}
```

See the [site](https://example.com).
"#;
    let r = render(md, 60, plain());
    let text: String = r
        .lines
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Main title"));
    assert!(text.contains("Subsection"));
    assert!(text.contains("\u{2022}"));
    assert!(text.contains("fn main"));
    assert!(text.contains("https://example.com"));
    assert_eq!(r.toc.len(), 2);
}

#[test]
fn empty_input_is_safe() {
    let r = render("", 60, plain());
    let _ = r; // no panic
}

#[test]
fn very_long_lines_are_wrapped() {
    let md = "word ".repeat(200);
    let r = render(&md, 40, plain());
    for line in &r.lines {
        assert!(line.width() <= 42, "line was {}: {}", line.width(), line);
    }
}
