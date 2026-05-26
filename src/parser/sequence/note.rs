//! Note lines: `note over A, B : text`, `note left of A`, `rnote`, `hnote`,
//! plus the multi-line `note … end note` opening form.
//!
//! NOTE: the state and cuca parsers have their own note parsers
//! (`state::note`, `cuca::note`) that are intentionally not shared with this
//! one. The grammars overlap but the *action models* differ fundamentally:
//! this one returns a neutral `NoteParse` enum; `state::note` mutates parser
//! state in place (consumes lines, anchors notes to nodes); `cuca::note`
//! returns decl structs for post-processing. Sequence also uniquely accepts
//! `note left:` with the colon directly after the side keyword. Don't unify.

use crate::ir::{NotePosition, Step};

use crate::parser::common::strip_prefix_keyword;

pub(super) enum NoteParse {
    Single(Step),
    MultilineStart {
        position: NotePosition,
        participants: Vec<String>,
    },
}

pub(super) fn parse_note_line(line: &str, line_no: usize) -> Option<NoteParse> {
    // Accept `note`, `rnote`, `hnote` (alternate styles) — keyword only.
    let after_kw = strip_prefix_keyword(line, "note")
        .or_else(|| strip_prefix_keyword(line, "rnote"))
        .or_else(|| strip_prefix_keyword(line, "hnote"))?
        .trim_start();

    if let Some(over_rest) = strip_prefix_keyword(after_kw, "over") {
        return Some(parse_note_over(over_rest.trim(), line_no));
    }
    if let Some(rest) = strip_note_side_keyword(after_kw, "left") {
        return Some(parse_note_side(NotePosition::LeftOf, rest, line_no));
    }
    if let Some(rest) = strip_note_side_keyword(after_kw, "right") {
        return Some(parse_note_side(NotePosition::RightOf, rest, line_no));
    }
    None
}

/// Same as `strip_prefix_keyword` but also accepts `:` immediately after the
/// keyword — needed for `note left: text` where the colon directly follows
/// the side keyword with no whitespace.
fn strip_note_side_keyword<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(keyword)?;
    match rest.chars().next() {
        None => Some(rest),
        Some(c) if c.is_whitespace() => Some(rest.trim_start()),
        Some(':') => Some(rest),
        _ => None,
    }
}

fn parse_note_over(rest: &str, line_no: usize) -> NoteParse {
    let (targets, label) = match rest.split_once(':') {
        Some((t, l)) => (t.trim(), Some(l.trim().to_string())),
        None => (rest.trim(), None),
    };
    let participants: Vec<String> = if targets.is_empty() {
        Vec::new()
    } else {
        targets.split(',').map(|t| t.trim().to_string()).collect()
    };
    match label {
        Some(text) => NoteParse::Single(Step::Note {
            position: NotePosition::Over,
            participants,
            text,
            line: line_no,
        }),
        None => NoteParse::MultilineStart {
            position: NotePosition::Over,
            participants,
        },
    }
}

fn parse_note_side(position: NotePosition, rest: &str, line_no: usize) -> NoteParse {
    // `note left of X : t`, `note right : t`, etc. We keep target list empty
    // when none was given — codegen serializes back to `note left/right` and
    // lets seq-puml resolve `__last__`.
    let rest = rest.strip_prefix("of").map(str::trim_start).unwrap_or(rest);
    let (targets, label) = match rest.split_once(':') {
        Some((t, l)) => (t.trim(), Some(l.trim().to_string())),
        None => (rest.trim(), None),
    };
    let participants: Vec<String> = if targets.is_empty() {
        Vec::new()
    } else {
        targets.split(',').map(|t| t.trim().to_string()).collect()
    };
    match label {
        Some(text) => NoteParse::Single(Step::Note {
            position,
            participants,
            text,
            line: line_no,
        }),
        None => NoteParse::MultilineStart {
            position,
            participants,
        },
    }
}
