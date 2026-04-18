//! Lightweight token-based syntax highlighting for code blocks.
//!
//! Intentionally simple: per-language keyword sets, plus generic rules for
//! line comments, block comments, strings, and numbers. Good enough to give
//! code readable structure without dragging in a full regex grammar engine.
//!
//! Supported languages (by fenced info string): `rust`, `python`/`py`,
//! `javascript`/`js`/`ts`/`typescript`, `go`, `bash`/`sh`/`shell`,
//! `json`, `yaml`/`yml`, `toml`, `html`/`xml`, `c`/`cpp`/`c++`/`h`, `java`.
//! Unknown languages fall back to plain code-style spans.
//!
//! The highlighter operates per-line (so it can be wired into paged display).
//! Multi-line block comments and strings are not tracked across lines; this
//! is a deliberate tradeoff for simplicity and avoids pathological inputs
//! that would make a stateful scanner slow.

use ratatui::style::Style;
use ratatui::text::Span;

use crate::theme::Theme;

/// Produce styled spans for a single line of code in the given language.
pub fn highlight_line(line: &str, lang: &str, theme: Theme) -> Vec<Span<'static>> {
    let grammar = Grammar::for_lang(lang);
    if grammar.is_none() {
        return vec![Span::styled(line.to_string(), theme.code_style())];
    }
    let grammar = grammar.unwrap();
    scan(line, grammar, theme)
}

#[derive(Debug, Clone, Copy)]
struct Grammar {
    keywords: &'static [&'static str],
    types: &'static [&'static str],
    /// Line-comment prefixes (tried in order).
    line_comments: &'static [&'static str],
    /// String delimiters; both ends are the same character.
    strings: &'static [char],
    /// Whether `#` starts a line comment (shell/python/yaml/toml style).
    /// This is covered by `line_comments` but flagged explicitly for fn-call heuristic.
    case_sensitive: bool,
    /// Permit a function-call highlight: `identifier(` → identifier colored as fn.
    fn_call_highlight: bool,
}

impl Grammar {
    fn for_lang(raw: &str) -> Option<&'static Self> {
        let lang = raw.trim().to_ascii_lowercase();
        let key = lang
            .split(|c: char| c == ',' || c.is_whitespace())
            .next()
            .unwrap_or("");
        match key {
            "rust" | "rs" => Some(&RUST),
            "python" | "py" => Some(&PYTHON),
            "js" | "javascript" | "jsx" | "ts" | "typescript" | "tsx" => Some(&JS),
            "go" => Some(&GO),
            "bash" | "sh" | "shell" | "zsh" => Some(&BASH),
            "json" => Some(&JSON),
            "yaml" | "yml" => Some(&YAML),
            "toml" | "ini" => Some(&TOML),
            "html" | "xml" | "svg" => Some(&HTML),
            "c" | "h" => Some(&C),
            "cpp" | "c++" | "hpp" | "cxx" | "hxx" => Some(&CPP),
            "java" => Some(&JAVA),
            _ => None,
        }
    }
}

static RUST: Grammar = Grammar {
    keywords: &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
        "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
        "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super", "trait",
        "true", "type", "union", "unsafe", "use", "where", "while", "yield",
    ],
    types: &[
        "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16",
        "u32", "u64", "u128", "usize", "str", "String", "Vec", "Option", "Result", "Box", "Rc",
        "Arc", "HashMap", "BTreeMap",
    ],
    line_comments: &["//"],
    strings: &['"'],
    case_sensitive: true,
    fn_call_highlight: true,
};

static PYTHON: Grammar = Grammar {
    keywords: &[
        "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class",
        "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global",
        "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise", "return",
        "try", "while", "with", "yield", "match", "case",
    ],
    types: &[
        "int", "float", "str", "bool", "list", "dict", "tuple", "set", "bytes",
    ],
    line_comments: &["#"],
    strings: &['"', '\''],
    case_sensitive: true,
    fn_call_highlight: true,
};

static JS: Grammar = Grammar {
    keywords: &[
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "function",
        "if",
        "import",
        "in",
        "instanceof",
        "interface",
        "let",
        "new",
        "null",
        "of",
        "return",
        "static",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "type",
        "typeof",
        "undefined",
        "var",
        "void",
        "while",
        "with",
        "yield",
    ],
    types: &[
        "boolean", "number", "string", "object", "symbol", "bigint", "any", "unknown", "never",
        "void",
    ],
    line_comments: &["//"],
    strings: &['"', '\'', '`'],
    case_sensitive: true,
    fn_call_highlight: true,
};

static GO: Grammar = Grammar {
    keywords: &[
        "break",
        "case",
        "chan",
        "const",
        "continue",
        "default",
        "defer",
        "else",
        "fallthrough",
        "for",
        "func",
        "go",
        "goto",
        "if",
        "import",
        "interface",
        "map",
        "package",
        "range",
        "return",
        "select",
        "struct",
        "switch",
        "type",
        "var",
        "true",
        "false",
        "nil",
    ],
    types: &[
        "bool",
        "byte",
        "rune",
        "string",
        "error",
        "int",
        "int8",
        "int16",
        "int32",
        "int64",
        "uint",
        "uint8",
        "uint16",
        "uint32",
        "uint64",
        "uintptr",
        "float32",
        "float64",
        "complex64",
        "complex128",
        "any",
    ],
    line_comments: &["//"],
    strings: &['"', '`'],
    case_sensitive: true,
    fn_call_highlight: true,
};

static BASH: Grammar = Grammar {
    keywords: &[
        "if", "then", "else", "elif", "fi", "case", "esac", "for", "select", "while", "until",
        "do", "done", "function", "in", "time", "return", "break", "continue", "export", "local",
        "readonly", "source", "alias", "unset", "trap",
    ],
    types: &[],
    line_comments: &["#"],
    strings: &['"', '\''],
    case_sensitive: true,
    fn_call_highlight: false,
};

static JSON: Grammar = Grammar {
    keywords: &["true", "false", "null"],
    types: &[],
    line_comments: &[],
    strings: &['"'],
    case_sensitive: true,
    fn_call_highlight: false,
};

static YAML: Grammar = Grammar {
    keywords: &["true", "false", "null", "yes", "no", "on", "off"],
    types: &[],
    line_comments: &["#"],
    strings: &['"', '\''],
    case_sensitive: false,
    fn_call_highlight: false,
};

static TOML: Grammar = Grammar {
    keywords: &["true", "false"],
    types: &[],
    line_comments: &["#"],
    strings: &['"', '\''],
    case_sensitive: true,
    fn_call_highlight: false,
};

static HTML: Grammar = Grammar {
    keywords: &[],
    types: &[],
    line_comments: &[],
    strings: &['"', '\''],
    case_sensitive: false,
    fn_call_highlight: false,
};

static C: Grammar = Grammar {
    keywords: &[
        "auto", "break", "case", "char", "const", "continue", "default", "do", "double", "else",
        "enum", "extern", "float", "for", "goto", "if", "inline", "int", "long", "register",
        "restrict", "return", "short", "signed", "sizeof", "static", "struct", "switch", "typedef",
        "union", "unsigned", "void", "volatile", "while",
    ],
    types: &[
        "int8_t",
        "int16_t",
        "int32_t",
        "int64_t",
        "uint8_t",
        "uint16_t",
        "uint32_t",
        "uint64_t",
        "size_t",
        "ssize_t",
        "ptrdiff_t",
        "intptr_t",
        "uintptr_t",
        "bool",
        "FILE",
    ],
    line_comments: &["//"],
    strings: &['"', '\''],
    case_sensitive: true,
    fn_call_highlight: true,
};

static CPP: Grammar = Grammar {
    keywords: &[
        "alignas",
        "alignof",
        "and",
        "auto",
        "bool",
        "break",
        "case",
        "catch",
        "char",
        "class",
        "const",
        "constexpr",
        "continue",
        "decltype",
        "default",
        "delete",
        "do",
        "double",
        "else",
        "enum",
        "explicit",
        "export",
        "extern",
        "false",
        "final",
        "float",
        "for",
        "friend",
        "goto",
        "if",
        "inline",
        "int",
        "long",
        "mutable",
        "namespace",
        "new",
        "noexcept",
        "not",
        "nullptr",
        "operator",
        "override",
        "private",
        "protected",
        "public",
        "register",
        "return",
        "short",
        "signed",
        "sizeof",
        "static",
        "struct",
        "switch",
        "template",
        "this",
        "throw",
        "true",
        "try",
        "typedef",
        "typeid",
        "typename",
        "union",
        "unsigned",
        "using",
        "virtual",
        "void",
        "volatile",
        "while",
    ],
    types: &[
        "int8_t",
        "int16_t",
        "int32_t",
        "int64_t",
        "uint8_t",
        "uint16_t",
        "uint32_t",
        "uint64_t",
        "size_t",
        "string",
        "vector",
        "map",
        "unordered_map",
        "set",
    ],
    line_comments: &["//"],
    strings: &['"', '\''],
    case_sensitive: true,
    fn_call_highlight: true,
};

static JAVA: Grammar = Grammar {
    keywords: &[
        "abstract",
        "assert",
        "boolean",
        "break",
        "byte",
        "case",
        "catch",
        "char",
        "class",
        "const",
        "continue",
        "default",
        "do",
        "double",
        "else",
        "enum",
        "extends",
        "final",
        "finally",
        "float",
        "for",
        "goto",
        "if",
        "implements",
        "import",
        "instanceof",
        "int",
        "interface",
        "long",
        "native",
        "new",
        "null",
        "package",
        "private",
        "protected",
        "public",
        "return",
        "short",
        "static",
        "strictfp",
        "super",
        "switch",
        "synchronized",
        "this",
        "throw",
        "throws",
        "transient",
        "true",
        "false",
        "try",
        "void",
        "volatile",
        "while",
        "var",
    ],
    types: &[
        "String", "Integer", "Long", "Double", "Float", "Boolean", "List", "Map", "Set", "Object",
    ],
    line_comments: &["//"],
    strings: &['"', '\''],
    case_sensitive: true,
    fn_call_highlight: true,
};

/// Peek the next character at byte index `i`, or None at end-of-string.
fn char_at(line: &str, i: usize) -> Option<char> {
    line[i..].chars().next()
}

fn scan(line: &str, g: &'static Grammar, theme: Theme) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let base = theme.code_style();
    let mut i = 0usize;

    // Fast path for comment-only lines.
    for prefix in g.line_comments {
        let trimmed = line.trim_start();
        if trimmed.starts_with(prefix) {
            let indent_len = line.len() - trimmed.len();
            if indent_len > 0 {
                spans.push(Span::styled(line[..indent_len].to_string(), base));
            }
            spans.push(Span::styled(
                line[indent_len..].to_string(),
                theme.comment_style(),
            ));
            return spans;
        }
    }

    while i < line.len() {
        // Detect start of comment mid-line.
        let mut matched_comment = false;
        for prefix in g.line_comments {
            if line[i..].starts_with(prefix) {
                spans.push(Span::styled(line[i..].to_string(), theme.comment_style()));
                i = line.len();
                matched_comment = true;
                break;
            }
        }
        if matched_comment {
            break;
        }

        // `i` is always on a char boundary: we advance only by whole chars
        // (`ch.len_utf8()`) or by boundary-preserving offsets from sub-scanners.
        let Some(ch) = char_at(line, i) else { break };
        let ch_len = ch.len_utf8();

        // String literal.
        if g.strings.contains(&ch) {
            let (span, end) = read_string(line, i, ch, theme);
            spans.push(span);
            i = end;
            continue;
        }

        // Number literal.
        if ch.is_ascii_digit()
            || (ch == '.' && char_at(line, i + 1).is_some_and(|c| c.is_ascii_digit()))
        {
            let (span, end) = read_number(line, i, theme);
            spans.push(span);
            i = end;
            continue;
        }

        // Identifier / keyword / type / function-call.
        if is_ident_start(ch) {
            let start = i;
            let mut j = i;
            while let Some(c) = char_at(line, j) {
                if !is_ident_continue(c) {
                    break;
                }
                j += c.len_utf8();
            }
            let word = &line[start..j];
            let style = classify_word(word, g, theme);
            let final_style =
                if g.fn_call_highlight && style == base && char_at(line, j) == Some('(') {
                    theme.fn_style()
                } else {
                    style
                };
            spans.push(Span::styled(word.to_string(), final_style));
            i = j;
            continue;
        }

        // Otherwise, accumulate default-styled runs until the next interesting char.
        let start = i;
        let mut j = i;
        while let Some(c) = char_at(line, j) {
            if g.strings.contains(&c)
                || is_ident_start(c)
                || c.is_ascii_digit()
                || g.line_comments.iter().any(|p| line[j..].starts_with(*p))
            {
                break;
            }
            j += c.len_utf8();
        }
        if j == start {
            // No progress — force at least one char of progress to guarantee termination.
            j = start + ch_len;
        }
        spans.push(Span::styled(line[start..j].to_string(), base));
        i = j;
    }

    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base));
    }
    spans
}

fn read_string(line: &str, start: usize, delim: char, theme: Theme) -> (Span<'static>, usize) {
    let mut i = start + delim.len_utf8();
    let mut escape = false;
    while let Some(c) = char_at(line, i) {
        let cl = c.len_utf8();
        if escape {
            escape = false;
            i += cl;
            continue;
        }
        if c == '\\' {
            escape = true;
            i += cl;
            continue;
        }
        if c == delim {
            i += cl;
            return (
                Span::styled(line[start..i].to_string(), theme.string_style()),
                i,
            );
        }
        i += cl;
    }
    // Unterminated on this line — style through end.
    (
        Span::styled(line[start..].to_string(), theme.string_style()),
        line.len(),
    )
}

fn read_number(line: &str, start: usize, theme: Theme) -> (Span<'static>, usize) {
    let mut i = start;
    let mut saw_dot = false;
    let mut saw_e = false;
    // Hex, oct, bin prefixes.
    if line[i..].starts_with("0x") || line[i..].starts_with("0X") {
        i += 2;
        while let Some(c) = char_at(line, i) {
            if c.is_ascii_hexdigit() || c == '_' {
                i += c.len_utf8();
            } else {
                break;
            }
        }
    } else if line[i..].starts_with("0b") || line[i..].starts_with("0B") {
        i += 2;
        while let Some(c) = char_at(line, i) {
            if matches!(c, '0' | '1' | '_') {
                i += c.len_utf8();
            } else {
                break;
            }
        }
    } else {
        while let Some(c) = char_at(line, i) {
            if c.is_ascii_digit() || c == '_' {
                i += c.len_utf8();
            } else if c == '.' && !saw_dot && !saw_e {
                saw_dot = true;
                i += 1;
            } else if (c == 'e' || c == 'E') && !saw_e {
                saw_e = true;
                i += 1;
                if matches!(char_at(line, i), Some('+' | '-')) {
                    i += 1;
                }
            } else {
                break;
            }
        }
    }
    // Optional numeric suffix (e.g. 10u32, 1.0f64).
    while let Some(c) = char_at(line, i) {
        if is_ident_continue(c) {
            i += c.len_utf8();
        } else {
            break;
        }
    }
    (
        Span::styled(line[start..i].to_string(), theme.number_style()),
        i,
    )
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

fn classify_word(word: &str, g: &'static Grammar, theme: Theme) -> Style {
    let cmp: Box<dyn Fn(&&&str) -> bool> = if g.case_sensitive {
        Box::new(|k: &&&str| **k == word)
    } else {
        let lw = word.to_ascii_lowercase();
        Box::new(move |k: &&&str| k.eq_ignore_ascii_case(&lw))
    };
    if g.keywords.iter().any(|k| cmp(&k)) {
        return theme.keyword_style();
    }
    if g.types.iter().any(|k| cmp(&k)) {
        return theme.type_style();
    }
    theme.code_style()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{Theme, ThemeName};

    fn plain() -> Theme {
        Theme::resolve(ThemeName::Plain)
    }

    #[test]
    fn unknown_lang_passthrough() {
        let spans = highlight_line("hello world", "klingon", plain());
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content, "hello world");
    }

    #[test]
    fn rust_keyword_and_string() {
        let spans = highlight_line(r#"let x = "hi";"#, "rust", plain());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, r#"let x = "hi";"#);
        assert!(spans.iter().any(|s| s.content.as_ref() == "let"));
        assert!(spans.iter().any(|s| s.content.as_ref() == r#""hi""#));
    }

    #[test]
    fn python_comment() {
        let spans = highlight_line("x = 1  # comment", "python", plain());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "x = 1  # comment");
    }

    #[test]
    fn hex_number() {
        let spans = highlight_line("let n = 0xFF;", "rust", plain());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "let n = 0xFF;");
        assert!(spans.iter().any(|s| s.content.as_ref() == "0xFF"));
    }

    #[test]
    fn unterminated_string_does_not_panic() {
        let spans = highlight_line(r#"let s = "oops"#, "rust", plain());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, r#"let s = "oops"#);
    }

    #[test]
    fn fn_call_highlighted() {
        let spans = highlight_line("println!(foo())", "rust", plain());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, "println!(foo())");
    }

    #[test]
    fn handles_multibyte_chars_without_panicking() {
        // Ellipsis and em-dash inside code should not crash the scanner.
        let line = "if let Err(e) = auth::validate_token(token, &state.conf\u{2026}";
        let spans = highlight_line(line, "rust", plain());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, line);
    }

    #[test]
    fn handles_cjk_and_emoji_in_comments() {
        let line = "let x = 1; // 日本語 🎉 comment";
        let spans = highlight_line(line, "rust", plain());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, line);
    }

    #[test]
    fn handles_multibyte_in_string_literal() {
        let line = r#"let s = "héllo — world";"#;
        let spans = highlight_line(line, "rust", plain());
        let joined: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(joined, line);
    }
}
