//! Pure scanning / string helpers for the activity-diagram parser — no
//! parser state. Terminator matching, block-head parsing, paren args,
//! arrow labels, swimlane / note tokenizing.

use crate::ir::{ActivityStmt, NoteAttach, NotePosition, PartitionKind};

use super::Terminator;

pub(super) fn is_comment(s: &str) -> bool {
    s.starts_with('\'') || s.starts_with("/'")
}

pub(super) fn strip_kw<'s>(s: &'s str, kw: &str) -> Option<&'s str> {
    let bytes = s.as_bytes();
    if !s.starts_with(kw) {
        return None;
    }
    // Either the keyword is the whole string, or it's followed by
    // whitespace.
    if s.len() == kw.len() {
        return Some("");
    }
    let next = bytes[kw.len()];
    if next.is_ascii_whitespace() {
        Some(&s[kw.len() + 1..])
    } else {
        None
    }
}

pub(super) fn match_terminator(raw: &str) -> Option<Terminator> {
    if raw == "endif" {
        return Some(Terminator::EndIf);
    }
    if raw == "else" || raw.starts_with("else ") || raw.starts_with("else(") {
        return Some(Terminator::Else);
    }
    if raw.starts_with("elseif (")
        || raw.starts_with("elseif(")
        || raw.starts_with("else if (")
        || raw.starts_with("else if(")
    {
        return Some(Terminator::ElseIf);
    }
    if raw == "endwhile"
        || raw.starts_with("endwhile ")
        || raw.starts_with("endwhile(")
        || raw.starts_with("endwhile(")
    {
        return Some(Terminator::EndWhile);
    }
    if raw.starts_with("repeat while") {
        return Some(Terminator::RepeatWhile);
    }
    if raw == "end fork" || raw == "endfork" || raw.starts_with("end fork ") || raw == "end merge" {
        return Some(Terminator::EndFork);
    }
    if raw == "fork again" {
        return Some(Terminator::ForkAgain);
    }
    if raw == "end split" || raw == "endsplit" {
        return Some(Terminator::EndSplit);
    }
    if raw == "split again" {
        return Some(Terminator::SplitAgain);
    }
    if raw == "endswitch" || raw == "end switch" {
        return Some(Terminator::EndSwitch);
    }
    if raw.starts_with("case (") || raw.starts_with("case(") {
        return Some(Terminator::Case);
    }
    if raw == "}" {
        return Some(Terminator::BraceClose);
    }
    if raw == "end note" || raw == "endnote" {
        return Some(Terminator::EndNote);
    }
    None
}

pub(super) fn terminator_keyword(t: Terminator) -> &'static str {
    match t {
        Terminator::Eof => "<eof>",
        Terminator::EndIf => "endif",
        Terminator::Else => "else",
        Terminator::ElseIf => "elseif",
        Terminator::EndWhile => "endwhile",
        Terminator::RepeatWhile => "repeat while",
        Terminator::EndFork => "end fork",
        Terminator::ForkAgain => "fork again",
        Terminator::EndSplit => "end split",
        Terminator::SplitAgain => "split again",
        Terminator::EndSwitch => "endswitch",
        Terminator::Case => "case",
        Terminator::BraceClose => "}",
        Terminator::EndNote => "end note",
    }
}

/// Which terminator keywords are accepted while we're scanning under
/// `parent`. `Eof` accepts only `Eof` (everything else is unexpected at
/// top level). For nested blocks, the inner block accepts its own
/// matching closer plus the few "soft" siblings (`else`, `elseif`,
/// `case`, `fork again`, `split again`) so the caller can keep iterating.
pub(super) fn terminator_is_compatible(parent: Terminator, candidate: Terminator) -> bool {
    if candidate == Terminator::Eof {
        return parent == Terminator::Eof;
    }
    match parent {
        Terminator::Eof => false,
        Terminator::EndIf => matches!(
            candidate,
            Terminator::EndIf | Terminator::Else | Terminator::ElseIf
        ),
        Terminator::EndWhile => candidate == Terminator::EndWhile,
        Terminator::RepeatWhile => candidate == Terminator::RepeatWhile,
        Terminator::EndFork => matches!(candidate, Terminator::EndFork | Terminator::ForkAgain),
        Terminator::EndSplit => matches!(candidate, Terminator::EndSplit | Terminator::SplitAgain),
        Terminator::EndSwitch | Terminator::Case => {
            matches!(candidate, Terminator::EndSwitch | Terminator::Case)
        }
        Terminator::BraceClose => candidate == Terminator::BraceClose,
        Terminator::EndNote => candidate == Terminator::EndNote,
        // ForkAgain / SplitAgain / Else / ElseIf themselves can't be
        // "parents" — they're soft transitions, not block openers.
        Terminator::ForkAgain | Terminator::SplitAgain | Terminator::Else | Terminator::ElseIf => {
            false
        }
    }
}

pub(super) fn partition_kind(raw: &str) -> Option<PartitionKind> {
    for (kw, kind) in [
        ("partition ", PartitionKind::Partition),
        ("package ", PartitionKind::Package),
        ("rectangle ", PartitionKind::Rectangle),
        ("card ", PartitionKind::Card),
        ("group ", PartitionKind::Group),
    ] {
        if raw.starts_with(kw) {
            return Some(kind);
        }
    }
    None
}

pub(super) fn parse_partition_head(raw: &str, kind: PartitionKind) -> (String, Option<String>) {
    let kw = match kind {
        PartitionKind::Partition => "partition",
        PartitionKind::Package => "package",
        PartitionKind::Rectangle => "rectangle",
        PartitionKind::Card => "card",
        PartitionKind::Group => "group",
    };
    let rest = raw.strip_prefix(kw).unwrap_or(raw).trim_start();
    // Strip trailing `{`.
    let rest = rest.trim_end_matches('{').trim_end();
    // Optional leading `#color` between keyword and label.
    let (color, rest) = if let Some(after) = rest.strip_prefix('#') {
        let mut it = after.splitn(2, char::is_whitespace);
        let c = it.next().unwrap_or("");
        let r = it.next().unwrap_or("").trim_start();
        (Some(format!("#{c}")), r)
    } else {
        (None, rest)
    };
    // Quoted label `"foo bar"`.
    let label = if let Some(after) = rest.strip_prefix('"') {
        if let Some(end) = after.find('"') {
            after[..end].to_string()
        } else {
            after.to_string()
        }
    } else {
        rest.split_whitespace().next().unwrap_or("").to_string()
    };
    (label, color)
}

/// Parse the head of an `if (...)` opener and return `(cond, then_label)`.
pub(super) fn parse_if_head(raw: &str) -> (String, Option<String>) {
    let after = raw.strip_prefix("if").unwrap_or(raw).trim_start();
    let (cond, rest) = take_paren(after);
    // Optional `then (label)` clause.
    let rest = rest.trim_start();
    let then_label = parse_then_clause(rest);
    (cond, then_label)
}

pub(super) fn parse_elseif_head(raw: &str) -> (String, Option<String>) {
    let after = raw
        .strip_prefix("elseif")
        .or_else(|| raw.strip_prefix("else if"))
        .unwrap_or(raw)
        .trim_start();
    let (cond, rest) = take_paren(after);
    let then_label = parse_then_clause(rest.trim_start());
    (cond, then_label)
}

pub(super) fn parse_then_clause(s: &str) -> Option<String> {
    // Common shapes: `then (foo)`, `(foo) then`, or omitted. Also accept
    // an optional `is (…)` clause (used by `if` / `while`) which carries
    // the affirmative label.
    let s = s.trim();
    if let Some(after) = s.strip_prefix("then") {
        let after = after.trim_start();
        if let Some(after_open) = after.strip_prefix('(') {
            if let Some(end) = after_open.find(')') {
                return Some(after_open[..end].trim().to_string());
            }
        }
        return None;
    }
    if let Some(after) = s.strip_prefix("is") {
        let after = after.trim_start();
        if let Some(after_open) = after.strip_prefix('(') {
            if let Some(end) = after_open.find(')') {
                return Some(after_open[..end].trim().to_string());
            }
        }
    }
    None
}

pub(super) fn parse_else_label(raw: &str) -> Option<String> {
    let after = raw.strip_prefix("else").unwrap_or(raw).trim_start();
    let after = after.strip_prefix('(')?;
    let end = after.find(')')?;
    Some(after[..end].trim().to_string())
}

pub(super) fn parse_while_head(raw: &str) -> (String, Option<String>) {
    let after = raw.strip_prefix("while").unwrap_or(raw).trim_start();
    let (cond, rest) = take_paren(after);
    let is_label = parse_then_clause(rest.trim_start());
    (cond, is_label)
}

pub(super) fn parse_endwhile_label(raw: &str) -> Option<String> {
    let after = raw.strip_prefix("endwhile").unwrap_or(raw).trim_start();
    let after = after.strip_prefix('(')?;
    let end = after.find(')')?;
    Some(after[..end].trim().to_string())
}

pub(super) fn parse_repeat_while_head(
    raw: &str,
) -> (Option<String>, Option<String>, Option<String>) {
    let after = raw.strip_prefix("repeat while").unwrap_or(raw).trim_start();
    if after.is_empty() {
        return (None, None, None);
    }
    let (cond, mut rest) = take_paren(after);
    let mut is_label: Option<String> = None;
    let mut not_label: Option<String> = None;
    rest = rest.trim_start();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("is") {
            let after = after.trim_start();
            if let Some(after_open) = after.strip_prefix('(') {
                if let Some(end) = after_open.find(')') {
                    is_label = Some(after_open[..end].trim().to_string());
                    rest = after_open[end + 1..].trim_start();
                    continue;
                }
            }
            break;
        }
        if let Some(after) = rest.strip_prefix("not") {
            let after = after.trim_start();
            if let Some(after_open) = after.strip_prefix('(') {
                if let Some(end) = after_open.find(')') {
                    not_label = Some(after_open[..end].trim().to_string());
                    rest = after_open[end + 1..].trim_start();
                    continue;
                }
            }
            break;
        }
        break;
    }
    (Some(cond), is_label, not_label)
}

pub(super) fn parse_fork_end_merge(raw: &str) -> bool {
    // `end fork` / `end fork merge` → merge=true.
    // `end fork no merge` / `end fork nomerge` → merge=false.
    let t = raw.trim();
    if t.contains("no merge") || t.contains("nomerge") {
        return false;
    }
    true
}

pub(super) fn parse_paren_arg(raw: &str, kw: &str) -> Option<String> {
    let after = raw.strip_prefix(kw).unwrap_or(raw).trim_start();
    let after = after.strip_prefix('(')?;
    let end = after.rfind(')')?;
    Some(after[..end].trim().to_string())
}

pub(super) fn take_paren(s: &str) -> (String, &str) {
    if let Some(after) = s.strip_prefix('(') {
        if let Some(end) = after.find(')') {
            return (after[..end].trim().to_string(), &after[end + 1..]);
        }
    }
    (String::new(), s)
}

pub(super) fn parse_arrow_label(raw: &str) -> Option<String> {
    // `-> foo;` or `-[#color,dashed]-> foo;`.
    let s = raw.trim_end_matches(';').trim();
    let after = if let Some(after) = s.strip_prefix("->") {
        after
    } else if let Some(open) = s.find("[") {
        // Skip the `-[…]->` decoration.
        let after_open = &s[open + 1..];
        let close = after_open.find(']')?;
        let rest = &after_open[close + 1..];
        let after = rest.strip_prefix("->")?;
        after
    } else {
        return None;
    };
    let label = after.trim();
    if label.is_empty() {
        None
    } else {
        Some(label.to_string())
    }
}

pub(super) fn parse_swimlane(raw: &str, line_no: usize) -> Option<ActivityStmt> {
    if !raw.starts_with('|') {
        return None;
    }
    // `|Lane|` or `|#color| Lane |`.
    let rest = raw.trim_start_matches('|');
    let (color, label_part) = if let Some(after) = rest.strip_prefix('#') {
        let mut it = after.splitn(2, '|');
        let c = it.next().unwrap_or("").to_string();
        let label_part = it.next().unwrap_or("");
        (Some(format!("#{c}")), label_part)
    } else {
        (None, rest)
    };
    let end = label_part.find('|')?;
    let label = label_part[..end].trim().to_string();
    Some(ActivityStmt::SwimlaneSwitch {
        label,
        color,
        line: line_no,
    })
}

pub(super) fn split_note_side(s: &str) -> (Option<NotePosition>, &str) {
    if let Some(rest) = s.strip_prefix("left") {
        return (Some(NotePosition::LeftOf), rest.trim_start());
    }
    if let Some(rest) = s.strip_prefix("right") {
        return (Some(NotePosition::RightOf), rest.trim_start());
    }
    (None, s)
}

/// Walk back over the most recently pushed statements and return the
/// notes-vector of the trailing `Action`, if any. Skips intervening
/// `SwimlaneSwitch` / `GotoLabel` / `Goto` / `Break` (which can't carry
/// notes themselves).
pub(super) fn last_action_mut(body: &mut [ActivityStmt]) -> Option<&mut Vec<NoteAttach>> {
    for stmt in body.iter_mut().rev() {
        match stmt {
            ActivityStmt::Action { notes, .. } => return Some(notes),
            ActivityStmt::SwimlaneSwitch { .. }
            | ActivityStmt::GotoLabel { .. }
            | ActivityStmt::Goto { .. }
            | ActivityStmt::Break { .. } => continue,
            _ => return None,
        }
    }
    None
}

/// Extract any `backward …` statements from `body` and return them as a
/// new list. PlantUML's `backward :foo;` lines mark statements that ride
/// the back-edge of a `repeat`; the parser stages them as ordinary
/// actions for simplicity, then this pass pulls them out by sniffing
/// label prefixes.
///
/// M0 implementation: we don't lex `backward` specially, so this returns
/// `None` for now. The IR field is preserved so future parser work can
/// fill it without an IR migration.
pub(super) fn take_backward(_body: &mut Vec<ActivityStmt>) -> Option<Vec<ActivityStmt>> {
    None
}
