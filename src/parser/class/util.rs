//! Shared low-level parsing utilities: directive detection, keyword
//! prefix stripping, quoted-string handling, alias parsing.

pub(super) fn is_comment(line: &str) -> bool {
    line.starts_with('\'') || line.starts_with("/'")
}

pub(super) fn is_skip_directive(line: &str) -> bool {
    // `hide …` / `show …` are intentionally NOT in this list — they're
    // dispatched via `try_parse_hide_show` and may flip flags on the
    // diagram.
    // `!theme`, `left to right direction`, and `top to bottom direction`
    // are intentionally NOT in this list — they're captured by
    // dedicated handlers in `run()` so codegen can act on them.
    const HEADS: &[&str] = &[
        "@startuml",
        "@enduml",
        "header ",
        "footer ",
        "!pragma",
        "!define",
        "!include",
        "scale ",
        "set namespaceSeparator",
        "set separator",
    ];
    HEADS
        .iter()
        .any(|h| line == h.trim() || line.starts_with(h))
}

pub(super) fn strip_prefix_keyword<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(keyword)?;
    match rest.chars().next() {
        None => Some(rest),
        Some(c) if c.is_whitespace() => Some(rest),
        _ => None,
    }
}

/// Parse `Name as alias`, `"Display Name" as alias`, or a bare name.
pub(super) fn parse_alias(rest: &str) -> Option<(String, String)> {
    if rest.is_empty() {
        return None;
    }
    if let Some((quoted, after)) = strip_leading_quoted(rest) {
        let after = after.trim();
        if let Some(alias) = strip_prefix_keyword(after, "as").map(str::trim) {
            if !alias.is_empty() {
                return Some((alias.to_string(), quoted));
            }
        }
        return Some((quoted.clone(), quoted));
    }
    let mut parts = rest.splitn(2, char::is_whitespace);
    let first = parts.next()?;
    let tail = parts.next().unwrap_or("").trim_start();
    if let Some(after_as) = strip_prefix_keyword(tail, "as").map(str::trim) {
        // `Foo as Bar` — id is the alias, display is the original name.
        // `Foo as "Long Foo"` — id is `Foo`, display is the quoted form.
        if let Some((quoted, _)) = strip_leading_quoted(after_as) {
            return Some((first.to_string(), quoted));
        }
        let alias = after_as.split_whitespace().next()?.to_string();
        return Some((alias, first.to_string()));
    }
    Some((first.to_string(), first.to_string()))
}

pub(super) fn strip_leading_quoted(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    if !s.starts_with('"') {
        return None;
    }
    let after = &s[1..];
    let close = after.find('"')?;
    let inner = after[..close].to_string();
    Some((inner, &after[close + 1..]))
}

pub(super) fn unquote(s: &str) -> String {
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_alias_bare_name() {
        assert_eq!(parse_alias("Foo"), Some(("Foo".into(), "Foo".into())));
    }

    #[test]
    fn parse_alias_quoted_display() {
        // `"Long Name"` — id and display both become the quoted text.
        assert_eq!(
            parse_alias("\"Long Name\""),
            Some(("Long Name".into(), "Long Name".into()))
        );
    }

    #[test]
    fn parse_alias_quoted_with_as_clause() {
        // `"Long Name" as Foo` — id is alias, display is the quoted form.
        assert_eq!(
            parse_alias("\"Long Name\" as Foo"),
            Some(("Foo".into(), "Long Name".into()))
        );
    }

    #[test]
    fn parse_alias_unquoted_with_as_clause() {
        // `Foo as Bar` — id is alias, display retains original.
        assert_eq!(
            parse_alias("Foo as Bar"),
            Some(("Bar".into(), "Foo".into()))
        );
    }

    #[test]
    fn parse_alias_unquoted_with_quoted_alias() {
        // `Foo as "Long Foo"` — id stays the bare identifier, display
        // becomes the quoted form.
        assert_eq!(
            parse_alias("Foo as \"Long Foo\""),
            Some(("Foo".into(), "Long Foo".into()))
        );
    }

    #[test]
    fn parse_alias_empty_returns_none() {
        assert!(parse_alias("").is_none());
    }

    #[test]
    fn strip_prefix_keyword_requires_whitespace_after() {
        // `class` is a keyword; `class Foo` matches, but `classifier`
        // must not (it would be treated as the keyword `class` followed
        // by the body `ifier`).
        assert_eq!(strip_prefix_keyword("class Foo", "class"), Some(" Foo"));
        assert!(strip_prefix_keyword("classifier", "class").is_none());
        // Empty-tail (`"class"` alone with nothing after) is allowed —
        // keyword consumes its own border.
        assert_eq!(strip_prefix_keyword("class", "class"), Some(""));
    }

    #[test]
    fn is_skip_directive_catches_uml_envelope() {
        assert!(is_skip_directive("@startuml"));
        assert!(is_skip_directive("@enduml"));
        assert!(is_skip_directive("scale 2"));
        // Not skipped — these have dedicated handlers.
        assert!(!is_skip_directive("!theme vibrant"));
        assert!(!is_skip_directive("hide circle"));
        assert!(!is_skip_directive("left to right direction"));
    }

    #[test]
    fn strip_leading_quoted_basics() {
        let (body, after) = strip_leading_quoted("\"hello\" world").unwrap();
        assert_eq!(body, "hello");
        assert_eq!(after, " world");
        // Unterminated quote returns None.
        assert!(strip_leading_quoted("\"unclosed").is_none());
    }

    #[test]
    fn unquote_passes_through_unquoted() {
        assert_eq!(unquote("\"hello\""), "hello");
        assert_eq!(unquote("hello"), "hello");
        assert_eq!(unquote("\""), "\""); // single quote — keep as-is
    }
}
