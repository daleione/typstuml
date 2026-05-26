//! Pure string helpers for the sequence parser — directive / comment
//! detection, dividers, and display-string normalization. No parser state.

pub(super) fn is_end_note(line: &str) -> bool {
    matches!(line, "end note" | "endnote" | "endrnote" | "endhnote")
}

pub(super) fn is_skip_directive(line: &str) -> bool {
    const HEADS: &[&str] = &[
        "@startuml",
        "@enduml",
        "hide ",
        "show ",
        "header ",
        "footer ",
        "mainframe ",
        "newpage",
        "!theme",
        "!pragma",
        "scale ",
        "left to right",
        "top to bottom",
        "box ",
        "end box",
    ];
    HEADS
        .iter()
        .any(|h| line == h.trim() || line.starts_with(h))
}

pub(super) fn strip_inline_comment(line: &str) -> &str {
    if let Some(idx) = line.find(" '") {
        line[..idx].trim_end()
    } else {
        line
    }
}

pub(super) fn parse_divider(line: &str) -> Option<String> {
    let inner = line.strip_prefix("==")?.strip_suffix("==")?;
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Interpret PlantUML escape sequences in a quoted display string. Only `\n`
/// is meaningful for our renderer; everything else passes through unchanged.
pub(super) fn unescape_display(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Strip leading `#color ` tokens used by PUML to tint fragment headers.
pub(super) fn strip_color_prefixes(s: &str) -> &str {
    let mut s = s.trim_start();
    while let Some(rest) = s.strip_prefix('#') {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let after = &rest[end..];
        if after.is_empty() {
            return rest[end..].trim_start();
        }
        s = after.trim_start();
    }
    s
}
