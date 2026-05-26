//! Cuca-family parsing utilities: flavor sniffing, annotation parsing,
//! directive detection. Generic line helpers (comment / keyword / quote /
//! ident scanning) are re-exported from [`crate::parser::common`] so cuca
//! call sites can keep importing them via `super::util::…`.

pub(super) use crate::parser::common::{
    is_comment, is_ident_continue, is_ident_start, strip_leading_quoted, strip_prefix_keyword,
    unquote,
};

/// Cuca-family flavor. Determines how ambiguous shorthand resolves:
/// `(Foo)` in a relation is a lollipop interface under `Class`, a
/// usecase ellipse under `UseCase`. `:Foo:` is treated as an actor
/// reference only under `UseCase`. Sniffed from the body before
/// the main parse pass.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Flavor {
    Class,
    UseCase,
}

/// Match a Java-style `@Name` or `@Name(args)` annotation line.
/// Returns the annotation text **without** the leading `@`, e.g.
/// `Entity` for `@Entity` or `Table(name="orders")` for
/// `@Table(name="orders")`. Returns `None` for anything else, including
/// trailing chars after the annotation (so `@Foo bar` doesn't consume).
///
/// We accept any non-empty identifier-ish run after `@`, but reject
/// `@startuml` / `@enduml` (handled by the lexer) and the special
/// `@unlinked` (used by `hide @unlinked …`).
pub(super) fn parse_annotation(line: &str) -> Option<String> {
    let rest = line.strip_prefix('@')?;
    let bytes = rest.as_bytes();
    if bytes.is_empty() || !is_ident_start(bytes[0]) {
        return None;
    }
    let mut i = 1;
    while i < bytes.len() && is_ident_continue(bytes[i]) {
        i += 1;
    }
    let name = &rest[..i];
    if matches!(name, "startuml" | "enduml" | "unlinked") {
        return None;
    }
    let after = rest[i..].trim_start();
    if after.is_empty() {
        return Some(rest[..i].to_string());
    }
    if !after.starts_with('(') {
        return None;
    }
    // Find the matching `)` honoring nested parens and quoted strings.
    let mut depth = 0usize;
    let mut in_quote = false;
    let mut j = 0usize;
    let ab = after.as_bytes();
    while j < ab.len() {
        let c = ab[j];
        if in_quote {
            if c == b'\\' && j + 1 < ab.len() {
                j += 2;
                continue;
            }
            if c == b'"' {
                in_quote = false;
            }
            j += 1;
            continue;
        }
        match c {
            b'"' => in_quote = true,
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    let args_end = j + 1;
                    let tail = after[args_end..].trim();
                    if !tail.is_empty() {
                        return None;
                    }
                    let mut out = name.to_string();
                    out.push_str(&after[..args_end]);
                    return Some(out);
                }
            }
            _ => {}
        }
        j += 1;
    }
    None
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
    fn parse_annotation_basics() {
        assert_eq!(parse_annotation("@Entity"), Some("Entity".into()));
        assert_eq!(parse_annotation("@Id"), Some("Id".into()));
        assert_eq!(
            parse_annotation("@Table(name=\"orders\")"),
            Some("Table(name=\"orders\")".into())
        );
        // Quoted ) inside the args list must not terminate.
        assert_eq!(
            parse_annotation("@A(s=\")\")"),
            Some("A(s=\")\")".into())
        );
        // Trailing junk after the annotation rejects it.
        assert!(parse_annotation("@Entity foo").is_none());
        assert!(parse_annotation("@Table(name=\"x\") extra").is_none());
        // Reserved tags handled elsewhere.
        assert!(parse_annotation("@startuml").is_none());
        assert!(parse_annotation("@enduml").is_none());
        assert!(parse_annotation("@unlinked").is_none());
        // Non-annotation inputs.
        assert!(parse_annotation("class Foo").is_none());
        assert!(parse_annotation("@").is_none());
        assert!(parse_annotation("@123").is_none());
    }
}
