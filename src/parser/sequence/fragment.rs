//! Fragment openers: `alt`, `opt`, `loop`, `par`, … with an optional
//! `#color` prefix and a guard/label.

use crate::ir::FragmentKind;

use super::scan::strip_color_prefixes;

pub(super) fn parse_fragment_start(line: &str) -> Option<(FragmentKind, Option<String>)> {
    let (head, rest) = match line.split_once(char::is_whitespace) {
        Some((h, r)) => (h, r.trim()),
        None => (line, ""),
    };
    let kind = FragmentKind::from_keyword(head)?;
    let stripped = strip_color_prefixes(rest);
    Some((
        kind,
        if stripped.is_empty() {
            None
        } else {
            Some(stripped.to_string())
        },
    ))
}
