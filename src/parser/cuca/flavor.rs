//! Cuca-flavor sniffing: decide class vs. use-case before the main pass.

use crate::parser::lexer::BodyLine;

use super::util::{is_comment, strip_prefix_keyword, Flavor};

/// Decide cuca flavor from the body before parsing starts. Use case
/// signals (`actor` / `usecase` / `actorStyle` / `usecaseStyle`) carry
/// the most weight; explicit class-family keywords (`class` / `interface`
/// / `enum` / `abstract`) push the score the other way. Standalone
/// `:Foo:` / `(Foo)` shorthand on a line is a weak signal — it counts
/// for use case only when the line has nothing else on it (which
/// `parse_inline_shorthand` would have handled). Default is `Class`.
pub(super) fn sniff_flavor(lines: &[BodyLine]) -> Flavor {
    let mut score: i32 = 0;
    for bl in lines {
        let raw = bl.text.trim();
        if raw.is_empty() || is_comment(raw) {
            continue;
        }
        // Hard signals.
        if strip_prefix_keyword(raw, "actor").is_some()
            || strip_prefix_keyword(raw, "usecase").is_some()
        {
            score += 2;
            continue;
        }
        // Skinparam carries use case style hints too.
        if let Some(rest) = strip_prefix_keyword(raw, "skinparam") {
            let lower = rest.trim().to_ascii_lowercase();
            if lower.starts_with("actorstyle") || lower.starts_with("usecasestyle") {
                score += 2;
                continue;
            }
        }
        // `:Word:` anywhere on the line is a use case actor reference —
        // class diagrams never use this form (member ports use `::` not
        // single `:`, member-add lines have `Class : member` with the
        // colon followed by whitespace + body). Strong signal.
        if has_actor_colon_token(raw) {
            score += 2;
            continue;
        }
        // Hard class signals.
        if strip_prefix_keyword(raw, "class").is_some()
            || strip_prefix_keyword(raw, "interface").is_some()
            || strip_prefix_keyword(raw, "enum").is_some()
            || strip_prefix_keyword(raw, "abstract").is_some()
            || strip_prefix_keyword(raw, "struct").is_some()
            || strip_prefix_keyword(raw, "annotation").is_some()
            || strip_prefix_keyword(raw, "protocol").is_some()
            || strip_prefix_keyword(raw, "exception").is_some()
        {
            score -= 2;
        }
    }
    if score >= 1 {
        Flavor::UseCase
    } else {
        Flavor::Class
    }
}

/// Detect a `:Word:` token anywhere on `line` (with whitespace / start
/// / end of line on the outside). Skips `::` runs (member ports like
/// `B::value`) and `: body` (member-add line, where the colon is
/// followed by whitespace). Returns true on the first match.
fn has_actor_colon_token(line: &str) -> bool {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip `::` runs entirely.
        if bytes[i] == b':' && i + 1 < bytes.len() && bytes[i + 1] == b':' {
            i += 2;
            continue;
        }
        if bytes[i] == b':' && (i == 0 || bytes[i - 1].is_ascii_whitespace()) && i + 1 < bytes.len()
        {
            // Find a closing `:` that is also at a word boundary
            // (followed by whitespace or end of line). Stop at
            // whitespace / arrow-body chars in between — those
            // would indicate this isn't the `:Foo:` shape.
            let mut j = i + 1;
            while j < bytes.len() {
                let c = bytes[j];
                if c == b':' {
                    let right_ok = j + 1 == bytes.len() || bytes[j + 1].is_ascii_whitespace();
                    if right_ok && j > i + 1 {
                        return true;
                    }
                    break;
                }
                // Bail out on arrow-body chars or other punctuation
                // that would indicate this isn't a clean identifier.
                if matches!(c, b'\n' | b'-' | b'.' | b'=' | b'<' | b'>') {
                    break;
                }
                j += 1;
            }
        }
        i += 1;
    }
    false
}
