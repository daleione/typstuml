//! Pure scanning / string helpers for the state-diagram parser — no
//! parser state. Tokenizing arrows, splitting labels, stripping color /
//! stereotype / history suffixes, unquoting.

use crate::ir::{BorderStyle, Direction, LineStyle, StateKind};

pub(super) use crate::parser::common::is_comment;

/// Strip a leading multi-word phrase (e.g. `"left of"`) when it is followed
/// by whitespace or end-of-string. Returns the trimmed remainder.
pub(super) fn strip_phrase<'s>(s: &'s str, phrase: &str) -> Option<&'s str> {
    let s = s.trim_start();
    let rest = s.strip_prefix(phrase)?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest.trim_start())
    } else {
        None
    }
}

/// Strip a leading keyword followed by whitespace (or the whole string).
pub(super) fn strip_kw<'s>(s: &'s str, kw: &str) -> Option<&'s str> {
    if !s.starts_with(kw) {
        return None;
    }
    if s.len() == kw.len() {
        return Some("");
    }
    let next = s.as_bytes()[kw.len()];
    if next.is_ascii_whitespace() {
        Some(s[kw.len() + 1..].trim_start())
    } else {
        None
    }
}

/// A concurrent-region divider line: `--`, `----`, `||`, etc. (nothing
/// else on the line).
pub(super) fn is_divider(s: &str) -> bool {
    (s.len() >= 2 && s.chars().all(|c| c == '-')) || (s.len() >= 2 && s.chars().all(|c| c == '|'))
}

/// Find the transition arrow inside `s`, returning its byte span.
///
/// Recognizes `->`, `-->`, `<-`, `<--`, direction hints (`-up->`,
/// `-l->`), bracketed styles (`-[#blue,dashed]->`), and an optional
/// leading `x` cross-start. An arrow must contain at least one `-` and
/// at least one of `<` / `>`.
pub(super) fn find_arrow(s: &str) -> Option<(usize, usize)> {
    let b = s.as_bytes();
    let n = b.len();
    let mut i = 0;
    while i < n {
        let c = b[i];
        let could_start = c == b'-'
            || c == b'<'
            || (c == b'x' && i + 1 < n && (b[i + 1] == b'-' || b[i + 1] == b'<'));
        if could_start {
            let mut j = i;
            let mut saw_lt = false;
            if b[j] == b'x' {
                j += 1;
            }
            if j < n && b[j] == b'<' {
                saw_lt = true;
                j += 1;
            }
            let mut saw_dash = false;
            while j < n && b[j] == b'-' {
                saw_dash = true;
                j += 1;
            }
            // optional [style]
            if j < n && b[j] == b'[' {
                while j < n && b[j] != b']' {
                    j += 1;
                }
                if j < n {
                    j += 1;
                }
            }
            // optional direction word
            while j < n && b[j].is_ascii_alphabetic() {
                j += 1;
            }
            // optional [style]
            if j < n && b[j] == b'[' {
                while j < n && b[j] != b']' {
                    j += 1;
                }
                if j < n {
                    j += 1;
                }
            }
            while j < n && b[j] == b'-' {
                saw_dash = true;
                j += 1;
            }
            let mut saw_gt = false;
            if j < n && b[j] == b'>' {
                saw_gt = true;
                j += 1;
            }
            if saw_dash && (saw_gt || saw_lt) {
                return Some((i, j));
            }
        }
        i += 1;
    }
    None
}

/// Parse `[#color,dashed]` style spec embedded in an arrow.
pub(super) fn parse_arrow_style(arrow: &str) -> (LineStyle, Option<String>) {
    let mut style = LineStyle::Solid;
    let mut color = None;
    if let Some(start) = arrow.find('[') {
        if let Some(end) = arrow[start..].find(']') {
            let inner = &arrow[start + 1..start + end];
            for part in inner.split(',') {
                let p = part.trim();
                if let Some(c) = p.strip_prefix('#') {
                    color = Some(format!("#{c}"));
                } else if p.starts_with('#') {
                    color = Some(p.to_string());
                } else {
                    match p.to_ascii_lowercase().as_str() {
                        "dashed" => style = LineStyle::Dashed,
                        "dotted" => style = LineStyle::Dotted,
                        "bold" | "plain" => {}
                        c if !c.is_empty() => color = Some(p.to_string()),
                        _ => {}
                    }
                }
            }
        }
    }
    (style, color)
}

/// Count the dashes that make up an arrow's shaft, ignoring any `-`
/// that sits inside a `[...]` style group. `->` is 1, `-->` / `-up->` /
/// `-[#blue]->` are 2.
pub(super) fn count_arrow_dashes(arrow: &str) -> usize {
    let mut count = 0;
    let mut in_bracket = false;
    for c in arrow.chars() {
        match c {
            '[' => in_bracket = true,
            ']' => in_bracket = false,
            '-' if !in_bracket => count += 1,
            _ => {}
        }
    }
    count
}

/// Parse a direction hint (`up` / `down` / `left` / `right` and the
/// one-letter forms) from an arrow's letters.
pub(super) fn parse_arrow_direction(arrow: &str) -> Option<Direction> {
    // Letters that sit between the dashes, ignoring any `[...]` style.
    let mut cleaned = String::new();
    let mut in_bracket = false;
    for c in arrow.chars() {
        match c {
            '[' => in_bracket = true,
            ']' => in_bracket = false,
            c if !in_bracket && c.is_ascii_alphabetic() => cleaned.push(c.to_ascii_lowercase()),
            _ => {}
        }
    }
    match cleaned.as_str() {
        "up" | "u" => Some(Direction::Up),
        "down" | "d" | "do" => Some(Direction::Down),
        "left" | "l" | "le" => Some(Direction::Left),
        "right" | "r" | "ri" => Some(Direction::Right),
        _ => None,
    }
}

/// Split a transition label into `(event, guard, action)`. The grammar is
/// `event [guard] / action` with all three parts optional.
pub(super) fn split_label(label: &str) -> (Option<String>, Option<String>, Option<String>) {
    let label = label.trim();
    if label.is_empty() {
        return (None, None, None);
    }
    // Action: everything after the first `/` at bracket depth 0.
    let mut depth = 0i32;
    let mut slash_at = None;
    for (i, c) in label.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            '/' if depth <= 0 => {
                slash_at = Some(i);
                break;
            }
            _ => {}
        }
    }
    let (head, action) = match slash_at {
        Some(i) => (label[..i].trim(), Some(label[i + 1..].trim().to_string())),
        None => (label, None),
    };
    // Guard: the first `[...]` group inside `head`.
    let mut event = head.to_string();
    let mut guard = None;
    if let Some(start) = head.find('[') {
        if let Some(end_rel) = head[start..].find(']') {
            guard = Some(head[start + 1..start + end_rel].trim().to_string());
            let before = head[..start].trim();
            let after = head[start + end_rel + 1..].trim();
            event = if after.is_empty() {
                before.to_string()
            } else if before.is_empty() {
                after.to_string()
            } else {
                format!("{before} {after}")
            };
        }
    }
    let norm = |s: String| {
        let t = s.trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    };
    (norm(event), guard.and_then(norm), action.and_then(norm))
}

/// Find the first top-level `:` (not inside quotes or `[]`) and split the
/// string there. Returns `(head, tail)` with the `:` removed.
pub(super) fn split_top_colon(s: &str) -> Option<(&str, &str)> {
    let b = s.as_bytes();
    let mut in_quote = false;
    let mut depth = 0i32;
    for i in 0..b.len() {
        match b[i] {
            b'"' => in_quote = !in_quote,
            b'[' if !in_quote => depth += 1,
            b']' if !in_quote => depth -= 1,
            b':' if !in_quote && depth <= 0 => {
                return Some((&s[..i], &s[i + 1..]));
            }
            _ => {}
        }
    }
    None
}

/// Split on the first run of two or more `.` (a `..` floating-note
/// connector, also `...` / `....`). Returns the trimmed `(left, right)`
/// when both sides are non-empty.
pub(super) fn split_dotted(s: &str) -> Option<(&str, &str)> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'.' {
            let start = i;
            while i < b.len() && b[i] == b'.' {
                i += 1;
            }
            if i - start >= 2 {
                let left = s[..start].trim();
                let right = s[i..].trim();
                if !left.is_empty() && !right.is_empty() {
                    return Some((left, right));
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Parse the name portion of a state declaration:
/// `Foo`, `"Display"`, `Foo as "Display"`, `"Display" as Foo`.
/// Returns `(id, display)`.
///
/// NOTE: intentionally distinct from the `parse_alias` functions in the
/// sequence and cuca parsers — state always succeeds (empty input → empty
/// strings, never `None`) and uses a single value for both id and display
/// when there's no `as`. See `sequence::participant::parse_alias` for the
/// per-diagram rationale; don't merge these.
pub(super) fn parse_name_part(s: &str) -> (String, String) {
    let s = s.trim();
    if s.is_empty() {
        return (String::new(), String::new());
    }
    // `... as ...` — split on a top-level ` as ` (outside quotes).
    if let Some((lhs, rhs)) = split_as(s) {
        let lhs = lhs.trim();
        let rhs = rhs.trim();
        let lhs_quoted = lhs.starts_with('"');
        let rhs_quoted = rhs.starts_with('"');
        if lhs_quoted && !rhs_quoted {
            // "Display" as Code
            return (unquote(rhs), unquote(lhs));
        }
        // Code as "Display"  (also the both-bare / both-quoted fallback)
        return (unquote(lhs), unquote(rhs));
    }
    let id = unquote(s);
    (id.clone(), id)
}

/// Split on a top-level ` as ` token (case-sensitive, outside quotes).
pub(super) fn split_as(s: &str) -> Option<(&str, &str)> {
    let b = s.as_bytes();
    let mut in_quote = false;
    let mut i = 0;
    while i + 4 <= b.len() {
        if b[i] == b'"' {
            in_quote = !in_quote;
        }
        if !in_quote
            && b[i].is_ascii_whitespace()
            && &s[i + 1..(i + 3).min(s.len())] == "as"
            && i + 3 < b.len()
            && b[i + 3].is_ascii_whitespace()
        {
            return Some((&s[..i], &s[i + 4..]));
        }
        i += 1;
    }
    None
}

/// Strip surrounding double quotes if present. Unlike
/// [`crate::parser::common::unquote`], the state parser trims surrounding
/// whitespace first.
pub(super) fn unquote(s: &str) -> String {
    crate::parser::common::unquote(s.trim())
}

/// Detect a trailing `#color` / `##[style]color` on a declaration line.
/// Returns `(remainder, fill, border_style, border_color)`.
///
/// NOTE: richer than `crate::parser::common::pop_trailing_color` on purpose —
/// state diagrams support the `##[dashed|dotted|bold]#color` border syntax,
/// which the simple shared scanner doesn't model. Kept separate deliberately;
/// don't collapse into `pop_trailing_color`.
pub(super) fn strip_trailing_color(
    s: &str,
) -> (&str, Option<String>, Option<BorderStyle>, Option<String>) {
    let s = s.trim_end();
    // `##[style]color` — border spec.
    if let Some(hashes) = s.rfind("##") {
        // Make sure this `##` starts a token (preceded by space or start).
        let ok = hashes == 0 || s.as_bytes()[hashes - 1].is_ascii_whitespace();
        if ok {
            let spec = &s[hashes + 2..];
            let mut border_style = None;
            let mut rest = spec;
            if let Some(close) = spec.find(']') {
                if spec.starts_with('[') {
                    let style = &spec[1..close];
                    border_style = match style.to_ascii_lowercase().as_str() {
                        "dashed" => Some(BorderStyle::Dashed),
                        "dotted" => Some(BorderStyle::Dotted),
                        "bold" => Some(BorderStyle::Bold),
                        _ => None,
                    };
                    rest = &spec[close + 1..];
                }
            }
            // The remainder after the optional `[style]` is the border
            // color — either a `#hex` token or a bare color name.
            let rest = rest.trim();
            let border_color = if rest.is_empty() {
                None
            } else if rest.starts_with('#') {
                Some(rest.to_string())
            } else {
                Some(format!("#{rest}"))
            };
            return (s[..hashes].trim_end(), None, border_style, border_color);
        }
    }
    // `#color` — fill.
    if let Some(hash) = s.rfind('#') {
        let ok = hash == 0 || s.as_bytes()[hash - 1].is_ascii_whitespace();
        let token = &s[hash..];
        let looks_color = token.len() > 1
            && token[1..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_');
        if ok && looks_color {
            return (s[..hash].trim_end(), Some(token.to_string()), None, None);
        }
    }
    (s, None, None, None)
}

/// Build a scoped synthetic id for a pseudostate: `base` alone at the
/// diagram level, `base + scope` when nested inside composite `scope`.
pub(super) fn scoped_pseudo_id(base: &str, scope: Option<&str>) -> String {
    match scope {
        Some(s) => format!("{base}{s}"),
        None => base.to_string(),
    }
}

/// `[H]` / `[H*]` / `Composite[H]` / `Composite[H*]` → `(kind, id, scope)`.
/// A bare `[H]` is scoped to `parent`; the `Composite[H]` form uses its
/// explicit prefix as the scope. `scope` is the composite the history
/// pseudostate belongs to (so it lays out *inside* that frame even when the
/// transition line sits outside the composite's block).
pub(super) fn strip_history_suffix(
    tok: &str,
    parent: Option<&str>,
) -> Option<(StateKind, String, Option<String>)> {
    let (prefix, kind, base) = if let Some(p) = tok.strip_suffix("[H*]") {
        (p, StateKind::DeepHistory, "__deephistory__")
    } else if let Some(p) = tok.strip_suffix("[H]") {
        (p, StateKind::History, "__history__")
    } else {
        return None;
    };
    let scope = if prefix.is_empty() {
        parent
    } else {
        Some(prefix)
    };
    Some((
        kind,
        scoped_pseudo_id(base, scope),
        scope.map(str::to_string),
    ))
}

/// `==Name==` → `Name`.
pub(super) fn strip_synchro(tok: &str) -> Option<String> {
    let t = tok.trim();
    if t.starts_with("==") && t.ends_with("==") && t.len() > 4 {
        let inner = t.trim_matches('=').trim();
        if !inner.is_empty() {
            return Some(inner.to_string());
        }
    }
    None
}
