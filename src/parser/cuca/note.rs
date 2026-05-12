//! Note-directive parsers covering all five PlantUML forms:
//! anchored (`note left of …`), spanning (`note over …`),
//! on-link (`note on link`), quoted standalone (`note "body" as N`),
//! and freestanding multi-line (`note as N … end note`).

use crate::ir::Direction;

use super::util::{strip_leading_quoted, strip_prefix_keyword};

/// Parse `[side] on link [: body]` from a `note` directive. Outer
/// `Option` indicates whether the line matched the form; inner
/// `Option<String>` is the inline body (None means follow-on lines
/// up to `end note`).
pub(super) fn parse_note_on_link_decl(rest: &str) -> Option<Option<String>> {
    let mut s = rest.trim();
    for side in ["left", "right", "top", "bottom"] {
        if let Some(after) = strip_prefix_keyword(s, side) {
            s = after.trim_start();
            break;
        }
    }
    let after_on = strip_prefix_keyword(s, "on")?.trim_start();
    let after = strip_prefix_keyword(after_on, "link")?.trim();
    if let Some(idx) = after.find(':') {
        return Some(Some(after[idx + 1..].trim().to_string()));
    }
    if !after.is_empty() {
        return None;
    }
    Some(None)
}

/// Parse `over A[, B[, C…]] [: body]` from a `note` directive. Returns
/// `(targets, inline_body)`. Empty inline body means the body is on
/// subsequent lines (terminated by `end note`).
pub(super) fn parse_note_over_decl(rest: &str) -> Option<(Vec<String>, Option<String>)> {
    let after_over = strip_prefix_keyword(rest, "over")?.trim();
    if after_over.is_empty() {
        return None;
    }
    let (targets_part, body) = match after_over.find(':') {
        Some(idx) => (
            after_over[..idx].trim(),
            Some(after_over[idx + 1..].trim().to_string()),
        ),
        None => (after_over.trim(), None),
    };
    let targets: Vec<String> = targets_part
        .split(',')
        .map(|s| {
            let s = s.trim();
            if let Some((quoted, _)) = strip_leading_quoted(s) {
                quoted
            } else {
                s.split_whitespace().next().unwrap_or("").to_string()
            }
        })
        .filter(|s| !s.is_empty())
        .collect();
    if targets.is_empty() {
        return None;
    }
    Some((targets, body))
}

/// Parse `<side> of <target> [: body]` from the body of a `note`
/// directive. Returns `(side_keyword, target_id, optional_inline_body)`.
pub(super) fn parse_anchored_note_decl(
    rest: &str,
) -> Option<(&'static str, String, Option<String>)> {
    const SIDES: &[&str] = &["left", "right", "top", "bottom", "above", "below"];
    for side in SIDES {
        let after_side = match strip_prefix_keyword(rest, side) {
            Some(s) => s.trim_start(),
            None => continue,
        };
        let after_of = match strip_prefix_keyword(after_side, "of") {
            Some(s) => s.trim_start(),
            None => continue,
        };
        let (target_part, body) = match after_of.find(':') {
            Some(idx) => (
                after_of[..idx].trim(),
                Some(after_of[idx + 1..].trim().to_string()),
            ),
            None => (after_of.trim(), None),
        };
        if target_part.is_empty() {
            return None;
        }
        // Target may be quoted ("Foo Bar") or a bare identifier.
        let target = if let Some((quoted, _)) = strip_leading_quoted(target_part) {
            quoted
        } else {
            target_part
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string()
        };
        if target.is_empty() {
            return None;
        }
        return Some((side, target, body));
    }
    None
}

/// Parse `"body" [as id]` — a standalone single-line note.
pub(super) fn parse_quoted_note_decl(rest: &str) -> Option<(String, Option<String>)> {
    let (body, after) = strip_leading_quoted(rest.trim())?;
    let after = after.trim_start();
    if after.is_empty() {
        return Some((body, None));
    }
    let after_as = strip_prefix_keyword(after, "as")?.trim_start();
    let id = after_as.split_whitespace().next()?.to_string();
    if id.is_empty() {
        return None;
    }
    Some((body, Some(id)))
}

/// Parse `as id` or a bare `id` — a freestanding note whose body
/// follows on subsequent lines until `end note`.
pub(super) fn parse_freestanding_note_decl(rest: &str) -> Option<String> {
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    if let Some(after_as) = strip_prefix_keyword(rest, "as") {
        let id = after_as.trim_start().split_whitespace().next()?.to_string();
        if id.is_empty() {
            return None;
        }
        return Some(id);
    }
    let id = rest.split_whitespace().next()?.to_string();
    if id.is_empty() {
        return None;
    }
    Some(id)
}

pub(super) fn side_to_direction(side: &str) -> Direction {
    match side {
        "left" => Direction::Left,
        "right" => Direction::Right,
        "top" | "above" => Direction::Up,
        "bottom" | "below" => Direction::Down,
        _ => Direction::Right,
    }
}
