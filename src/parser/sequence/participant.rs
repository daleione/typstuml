//! Participant declarations: `participant`, `actor`, `boundary`, … with
//! optional `as` alias, trailing `#color`, and `order N`.

use crate::ir::{Participant, ParticipantKind};
use crate::parser::common::{pop_trailing_color, strip_leading_quoted, strip_prefix_keyword};

use super::scan::unescape_display;

pub(super) const PARTICIPANT_KEYWORDS: &[&str] = &[
    "participant",
    "actor",
    "boundary",
    "control",
    "entity",
    "database",
    "collections",
    "queue",
];

/// If `line` is a participant declaration whose trailing token is `[`
/// (opening a multi-line `[ … ]` rich-content block), return the line with
/// the trailing `[` stripped along with a `true` flag. Returns `None` when
/// the line does not start with a participant keyword, leaving the caller to
/// fall back to the regular parser. The `false` flag form is unused today
/// but reserved for a future single-line `[…]` variant.
pub(super) fn strip_participant_block_open(line: &str) -> Option<(String, bool)> {
    let starts_with_pkw = PARTICIPANT_KEYWORDS
        .iter()
        .any(|kw| strip_prefix_keyword(line, kw).is_some());
    if !starts_with_pkw {
        return None;
    }
    let trimmed = line.trim_end();
    let stripped = trimmed.strip_suffix('[')?;
    Some((stripped.trim_end().to_string(), true))
}

pub(super) fn parse_participant(line: &str, line_no: usize) -> Option<Participant> {
    let (kw, rest) = PARTICIPANT_KEYWORDS
        .iter()
        .find_map(|kw| strip_prefix_keyword(line, kw).map(|r| (*kw, r.trim())))?;
    let kind = ParticipantKind::from_keyword(kw)?;

    let mut rest = rest.to_string();
    let color = pop_trailing_color(&mut rest);
    pop_trailing_order(&mut rest);

    let (id, display) = parse_alias(rest.trim())?;
    Some(Participant {
        kind,
        id,
        display,
        display_block: None,
        color,
        line: line_no,
    })
}

fn pop_trailing_order(rest: &mut String) {
    // Match `\s+order\s+\d+\s*$`, case-insensitive.
    let trimmed = rest.trim_end();
    let lower = trimmed.to_ascii_lowercase();
    let Some(idx) = lower.rfind(" order ") else {
        return;
    };
    let after = trimmed[idx + " order ".len()..].trim();
    if after.is_empty() || !after.chars().all(|c| c.is_ascii_digit()) {
        return;
    }
    let kept = trimmed[..idx].trim_end().to_string();
    *rest = kept;
}

/// Parse `"Long Name" as alias`, `alias as "Long Name"`, or a bare name.
/// Returns `(canonical_id, display_label)`.
///
/// NOTE: intentionally **not** shared with `cuca::util::parse_alias` or
/// `state::scan::parse_name_part`. The naming semantics differ per diagram:
/// sequence unescapes the display (`\n` → newline, meaningful in message
/// labels) and keeps the full remaining text as display; cuca takes only the
/// first word as the stable id and drops the rest; state always succeeds and
/// uses one value for both id and display. Don't "unify" these.
fn parse_alias(rest: &str) -> Option<(String, String)> {
    if rest.is_empty() {
        return None;
    }
    if let Some((quoted, after)) = strip_leading_quoted(rest) {
        let display = unescape_display(&quoted);
        let after = after.trim();
        if let Some(alias) = strip_prefix_keyword(after, "as").map(str::trim) {
            if !alias.is_empty() {
                return Some((alias.to_string(), display));
            }
        }
        return Some((display.clone(), display));
    }
    let mut parts = rest.splitn(2, char::is_whitespace);
    let first = parts.next()?;
    let tail = parts.next().unwrap_or("").trim_start();
    if let Some(after_as) = strip_prefix_keyword(tail, "as").map(str::trim) {
        if let Some((quoted, _)) = strip_leading_quoted(after_as) {
            // `id as "Display"` — first token is the id, quoted is the display.
            return Some((first.to_string(), unescape_display(&quoted)));
        }
        // `Display as id` — token after `as` is the alias used in messages.
        return Some((after_as.to_string(), first.to_string()));
    }
    Some((first.to_string(), rest.to_string()))
}
