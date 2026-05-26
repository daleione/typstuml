//! Message lines: `A -> B : label`. Arrow scanning handles both spaced and
//! unspaced forms plus inline `[#color]` shafts and `o`/`x` heads.

use crate::ir::Step;

pub(super) fn parse_message(line: &str, line_no: usize) -> Option<Step> {
    let split = split_arrow(line)?;
    let from = split.from.to_string();
    let arrow = split.arrow.to_string();

    // After the arrow we have: <to-token> [<suffix>] [: label]
    let (to_token, label) = match split.rest.split_once(':') {
        Some((t, l)) => (t.trim(), Some(l.trim().to_string())),
        None => (split.rest.trim(), None),
    };
    if to_token.is_empty() {
        return None;
    }
    // Strip a trailing suffix (`+`, `-`, `*`, `!`, or combos). We drop it on
    // the floor — seq-puml ignores it on most arrows, and the activate
    // semantics we'd need round-trip via explicit `activate`/`deactivate`.
    let to = strip_message_suffix(to_token).to_string();

    Some(Step::Message {
        from,
        to,
        arrow,
        label,
        line: line_no,
    })
}

fn strip_message_suffix(tok: &str) -> &str {
    tok.trim_end_matches(|c: char| matches!(c, '+' | '-' | '*' | '!'))
}

/// Split a trailing ` #color` token off a message target. Used by the driver
/// when handling `activate`/`deactivate`/`create` target lines.
pub(super) fn split_target_color(rest: &str) -> (&str, Option<String>) {
    if let Some((before, after)) = rest.rsplit_once(char::is_whitespace) {
        if after.starts_with('#') {
            return (before.trim(), Some(after.to_string()));
        }
    }
    (rest, None)
}

struct ArrowSplit<'a> {
    from: &'a str,
    arrow: &'a str,
    rest: &'a str,
}

/// Find the `<from> <arrow> <rest>` shape on a single line.
///
/// PlantUML accepts both spaced (`A -> B`) and unspaced (`A->B`) arrows, so
/// we scan for runs of arrow-shaft chars (`-<>/\\`, optionally with `[…]`
/// for inline colors) rather than splitting on whitespace. A run only
/// counts as an arrow if it actually contains `-`; runs that turn out to be
/// just `<` or `>` (e.g. inside an angle-bracketed identifier) get skipped
/// and scanning continues.
///
/// `o` / `x` arrow heads (`->o`, `->x`) need whitespace around the arrow to
/// be recognised, since both letters are otherwise valid identifier chars
/// (`Otto->Alice` is unambiguously `from=Otto, arrow=->`). With whitespace,
/// the leading/trailing alphanumeric boundary disambiguates and the head
/// gets folded into the arrow token below.
fn split_arrow(line: &str) -> Option<ArrowSplit<'_>> {
    let bytes = line.as_bytes();
    let mut search = 0;

    loop {
        // Skip ahead to the next arrow-shaft char.
        while search < bytes.len() && !is_arrow_shaft(bytes[search]) {
            search += 1;
        }
        if search >= bytes.len() {
            return None;
        }
        let start = search;

        // Extend through contiguous shaft chars + inline `[...]` color blocks.
        let mut end = start;
        while end < bytes.len() {
            let b = bytes[end];
            if is_arrow_shaft(b) {
                end += 1;
            } else if b == b'[' {
                let mut j = end + 1;
                while j < bytes.len() && bytes[j] != b']' {
                    j += 1;
                }
                if j >= bytes.len() {
                    break; // unterminated `[`; bail on this run
                }
                end = j + 1;
            } else {
                break;
            }
        }

        // Whitespace-bounded arrows can extend through `o` / `x` heads on
        // either side (e.g. `->o`, `x<-`). Without whitespace, the same
        // letters could be part of an identifier, so we only fold them in
        // when separated from the surrounding text by whitespace.
        let mut arrow_start = start;
        let mut arrow_end = end;
        if arrow_start > 0
            && matches!(bytes[arrow_start - 1], b'o' | b'x')
            && (arrow_start == 1 || bytes[arrow_start - 2].is_ascii_whitespace())
        {
            arrow_start -= 1;
        }
        if arrow_end < bytes.len()
            && matches!(bytes[arrow_end], b'o' | b'x')
            && (arrow_end + 1 == bytes.len() || bytes[arrow_end + 1].is_ascii_whitespace())
        {
            arrow_end += 1;
        }

        let candidate = &line[arrow_start..arrow_end];
        if !is_arrow_token(candidate) {
            // e.g. a lone `<` from `Class<Generic>` — skip past and try again.
            search = end.max(start + 1);
            continue;
        }

        let from = line[..arrow_start].trim();
        let rest = line[arrow_end..].trim_start();
        if from.is_empty() || rest.is_empty() {
            return None;
        }
        return Some(ArrowSplit {
            from,
            arrow: candidate,
            rest,
        });
    }
}

fn is_arrow_shaft(b: u8) -> bool {
    matches!(b, b'-' | b'<' | b'>' | b'/' | b'\\')
}

fn is_arrow_token(s: &str) -> bool {
    if !s.contains('-') {
        return false;
    }
    // Allow only arrow-shape chars plus an inline `[#color]`. Walk byte by byte.
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if matches!(b, b'-' | b'<' | b'>' | b'o' | b'x' | b'/' | b'\\') {
            i += 1;
            continue;
        }
        if b == b'[' {
            // Skip until matching `]`.
            i += 1;
            while i < bytes.len() && bytes[i] != b']' {
                i += 1;
            }
            if i >= bytes.len() {
                return false;
            }
            i += 1;
            continue;
        }
        return false;
    }
    true
}
