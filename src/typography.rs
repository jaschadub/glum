//! Typographic substitutions: straight quotes → curly, -- → em dash, ... → ellipsis.
//!
//! Performed on plain text *before* wrapping so that display widths reflect the
//! final glyphs. We deliberately avoid touching text inside code spans or code
//! blocks — those are passed through verbatim by the renderer.

/// Apply smart-quote, em-dash, and ellipsis substitutions to a paragraph.
pub fn smarten(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let prev = if i == 0 { None } else { Some(chars[i - 1]) };

        // --- → em dash; -- → en dash (keep simple: treat both double+triple as em).
        if c == '-' && i + 1 < chars.len() && chars[i + 1] == '-' {
            let triple = i + 2 < chars.len() && chars[i + 2] == '-';
            out.push('\u{2014}'); // em dash
            i += if triple { 3 } else { 2 };
            continue;
        }

        // ... → ellipsis (only an exact triple, to avoid swallowing "....").
        if c == '.' && i + 2 < chars.len() && chars[i + 1] == '.' && chars[i + 2] == '.' {
            let next_is_dot = i + 3 < chars.len() && chars[i + 3] == '.';
            let prev_is_dot = matches!(prev, Some('.'));
            if !next_is_dot && !prev_is_dot {
                out.push('\u{2026}');
                i += 3;
                continue;
            }
        }

        // Smart quotes: open if preceded by whitespace/start/opening punctuation.
        if c == '"' {
            let opens = match prev {
                None => true,
                Some(p) => p.is_whitespace() || matches!(p, '(' | '[' | '{' | '\u{2014}' | '\u{2013}'),
            };
            out.push(if opens { '\u{201C}' } else { '\u{201D}' });
            i += 1;
            continue;
        }
        if c == '\'' {
            // Apostrophe if between letters (don't, it's) or after letter (boys').
            let between_letters = matches!(
                (prev, chars.get(i + 1)),
                (Some(a), Some(b)) if a.is_alphanumeric() && b.is_alphanumeric()
            );
            let after_letter = matches!(prev, Some(a) if a.is_alphanumeric());
            if between_letters || after_letter {
                out.push('\u{2019}');
            } else {
                let opens = match prev {
                    None => true,
                    Some(p) => p.is_whitespace() || matches!(p, '(' | '[' | '{' | '"' | '\u{201C}'),
                };
                out.push(if opens { '\u{2018}' } else { '\u{2019}' });
            }
            i += 1;
            continue;
        }

        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::smarten;

    #[test]
    fn em_dash() {
        assert_eq!(smarten("a--b"), "a\u{2014}b");
        assert_eq!(smarten("a---b"), "a\u{2014}b");
    }

    #[test]
    fn ellipsis() {
        assert_eq!(smarten("wait..."), "wait\u{2026}");
        assert_eq!(smarten("x....y"), "x....y"); // four dots unchanged
    }

    #[test]
    fn quotes_open_close() {
        assert_eq!(smarten("he said \"hi\""), "he said \u{201C}hi\u{201D}");
    }

    #[test]
    fn apostrophe() {
        assert_eq!(smarten("don't"), "don\u{2019}t");
        assert_eq!(smarten("boys'"), "boys\u{2019}");
    }

    #[test]
    fn single_open_quote() {
        assert_eq!(smarten("he said 'hi'"), "he said \u{2018}hi\u{2019}");
    }

    #[test]
    fn preserves_plain_text() {
        assert_eq!(smarten("hello world"), "hello world");
    }
}
