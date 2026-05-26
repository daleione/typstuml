//! Shared low-level scanning helpers used across the per-diagram parsers.
//!
//! Only verbatim-identical helpers live here. Functions that look similar
//! but differ in trimming / unescaping / token semantics are intentionally
//! kept local to their parser (e.g. `sequence`'s `parse_alias` unescapes
//! display strings, `state`'s `strip_kw` trims its remainder) to avoid
//! changing parse behavior.

use crate::parser::lexer::BodyLine;

/// A line comment: PlantUML uses `'…` (single) and `/'…` (block open).
pub(crate) fn is_comment(line: &str) -> bool {
    line.starts_with('\'') || line.starts_with("/'")
}

/// Strip a leading keyword when it's followed by whitespace or end-of-line.
/// Returns the remainder **untrimmed** (leading whitespace preserved), or
/// `None` if the keyword is only a prefix of a longer word (`classifier`
/// must not match `class`).
pub(crate) fn strip_prefix_keyword<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(keyword)?;
    match rest.chars().next() {
        None => Some(rest),
        Some(c) if c.is_whitespace() => Some(rest),
        _ => None,
    }
}

/// Like [`strip_prefix_keyword`] but returns the remainder trimmed on both
/// ends. Used by the tree parsers (mind-map / WBS) whose grammars expect a
/// trimmed label after the keyword.
pub(crate) fn strip_keyword_trimmed<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    strip_prefix_keyword(line, keyword).map(str::trim)
}

/// If `s` (after trimming leading whitespace) opens with a `"…"` token,
/// return its inner text and the remainder after the closing quote.
pub(crate) fn strip_leading_quoted(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    if !s.starts_with('"') {
        return None;
    }
    let after = &s[1..];
    let close = after.find('"')?;
    let inner = after[..close].to_string();
    Some((inner, &after[close + 1..]))
}

/// Strip surrounding double quotes if both are present. Does **not** trim;
/// callers that need leading/trailing whitespace ignored should trim first.
pub(crate) fn unquote(s: &str) -> String {
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Strip a trailing ` #color` token from `rest`; returns it (with the `#`)
/// if present, mutating `rest` to drop the token.
pub(crate) fn pop_trailing_color(rest: &mut String) -> Option<String> {
    let trimmed = rest.trim_end();
    let bytes = trimmed.as_bytes();
    let mut i = bytes.len();
    while i > 0 && !bytes[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    if i == 0 {
        return None;
    }
    let token = trimmed[i..].to_string();
    if !token.starts_with('#') {
        return None;
    }
    let kept = trimmed[..i].trim_end().to_string();
    *rest = kept;
    Some(token)
}

/// Pull a leading `title <text>` line off `body` and return it separately.
/// Comments / blanks before the title are skipped along with it; everything
/// after the (optional) title is returned as the remaining body.
pub(crate) fn split_off_title(body: &[BodyLine]) -> (Option<String>, Vec<BodyLine>) {
    let mut title = None;
    let mut idx = 0;
    while idx < body.len() {
        let trimmed = body[idx].text.trim();
        if trimmed.is_empty() || trimmed.starts_with('\'') || trimmed.starts_with("/'") {
            idx += 1;
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix("title")
            .filter(|r| r.is_empty() || r.starts_with(char::is_whitespace))
        {
            let t = rest.trim();
            if !t.is_empty() {
                title = Some(t.to_string());
            }
            idx += 1;
            continue;
        }
        break;
    }
    (title, body[idx..].to_vec())
}

pub(crate) fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

pub(crate) fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_prefix_keyword_requires_word_boundary() {
        assert_eq!(strip_prefix_keyword("class Foo", "class"), Some(" Foo"));
        assert!(strip_prefix_keyword("classifier", "class").is_none());
        assert_eq!(strip_prefix_keyword("class", "class"), Some(""));
    }

    #[test]
    fn strip_keyword_trimmed_trims_remainder() {
        assert_eq!(strip_keyword_trimmed("title  Hello ", "title"), Some("Hello"));
        assert_eq!(strip_keyword_trimmed("title", "title"), Some(""));
        assert!(strip_keyword_trimmed("titles", "title").is_none());
    }

    #[test]
    fn strip_leading_quoted_basics() {
        let (body, after) = strip_leading_quoted("\"hello\" world").unwrap();
        assert_eq!(body, "hello");
        assert_eq!(after, " world");
        assert!(strip_leading_quoted("\"unclosed").is_none());
    }

    #[test]
    fn unquote_passes_through_unquoted() {
        assert_eq!(unquote("\"hello\""), "hello");
        assert_eq!(unquote("hello"), "hello");
        assert_eq!(unquote("\""), "\"");
    }

    #[test]
    fn pop_trailing_color_extracts_hash_token() {
        let mut s = "Foo #aabbcc".to_string();
        assert_eq!(pop_trailing_color(&mut s), Some("#aabbcc".to_string()));
        assert_eq!(s, "Foo");
        let mut s = "Foo".to_string();
        assert_eq!(pop_trailing_color(&mut s), None);
        assert_eq!(s, "Foo");
    }
}
