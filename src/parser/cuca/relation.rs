//! Arrow / relation parsing.
//!
//! Centerpiece is `find_arrow_span` — locating the body of an arrow
//! inside an arbitrary line. Class-diagram arrow grammar is rich
//! (eight head shapes, three line styles, direction keywords, bracket
//! annotations, multiplicity / role / port / lollipop / couple
//! endpoints) so the search is layered:
//!
//! 1. Walk the line skipping quoted segments.
//! 2. On the first body char (`-` / `.` / `=`), expand outward through
//!    head decorations, bracket annotations, and inline direction kws.
//! 3. Validate that the run is bordered by whitespace / quote / colon
//!    so we don't fire on `def-foo`.

use crate::ir::{ArrowHead, Direction, LineStyle};

use super::util::{Flavor, unquote};

/// Per-endpoint shape hint used by the caller when auto-creating an
/// entity that the user referenced but didn't declare. Distinct from
/// the bare lollipop `bool` of earlier revisions because in use-case
/// flavor `(Foo)` means a usecase ellipse, not a small interface
/// circle, and `:Foo:` means an actor.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum EndpointHint {
    None,
    Lollipop,
    Actor,
    UseCase,
}

/// Locate the arrow token in `line`. An arrow has shape
/// `[head]<body>[direction-kw][body][head]` where:
///   - `head` is one of `<|`, `<`, `<<`, `|>`, `>`, `>>`, `*`, `o`, `x`,
///     `+`, `#`, `(0`, `0)`, `(0)` — all optional;
///   - `body` is one or more of `-`, `.`, `=` (mixing allowed);
///   - the inner direction kw is one of `up`/`down`/`left`/`right` or
///     their two-letter abbreviations;
///   - bracketed annotations like `[#red]` or `[dashed]` may appear
///     anywhere inside the body.
///
/// Returns `Some((start, end))` byte indices on success. The detection is
/// heuristic: we find a run of body chars (length ≥ 1) bordered by
/// whitespace + non-body characters on each side, then expand outward to
/// include the optional heads.
pub(super) fn find_arrow_span(line: &str) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        // Skip a quoted segment so heads / bodies inside string literals
        // (e.g., multiplicities `"1..*"`) don't trip the search.
        if bytes[i] == b'"' {
            i += 1;
            while i < n && bytes[i] != b'"' {
                i += 1;
            }
            if i < n {
                i += 1;
            }
            continue;
        }
        if matches!(bytes[i], b'-' | b'.' | b'=') {
            // Body char found. Scan back for an optional head, forward
            // through the rest of the arrow, and validate.
            if let Some((start, end)) = expand_arrow_around(bytes, i) {
                if validate_arrow_borders(bytes, start, end) {
                    return Some((start, end));
                }
            }
        }
        i += 1;
    }
    None
}

/// Expand the arrow span around a body character at `pivot`. Returns
/// `(start, end)` byte indices.
fn expand_arrow_around(bytes: &[u8], pivot: usize) -> Option<(usize, usize)> {
    // Walk forward through body / inner-head / `[...]` / direction kws
    // until the shape no longer matches.
    let n = bytes.len();
    let mut end = pivot;
    while end < n {
        let c = bytes[end];
        if matches!(c, b'-' | b'.' | b'=' | b'<' | b'>' | b'|' | b'*' | b'o' | b'x' | b'+' | b'#')
        {
            end += 1;
            continue;
        }
        if c == b'[' {
            let mut j = end + 1;
            while j < n && bytes[j] != b']' {
                j += 1;
            }
            if j >= n {
                return None;
            }
            end = j + 1;
            continue;
        }
        if matches!(c, b'(' | b')') {
            // Lollipop-style heads `(0` / `0)` / `(0)`.
            end += 1;
            continue;
        }
        // Inline direction keyword like `-up->`.
        if let Some(kw_len) = match_direction_keyword(&bytes[end..]) {
            // The keyword must be flanked by body chars on at least one
            // side (the previous byte is a body char by construction at
            // the first iteration).
            let after = end + kw_len;
            if end > 0 && matches!(bytes[end - 1], b'-' | b'.' | b'=')
                && (after == n || matches!(bytes[after], b'-' | b'.' | b'='))
            {
                end = after;
                continue;
            }
        }
        break;
    }

    // Walk backward through head characters only. Body chars (`-`/`.`/`=`)
    // would have been consumed in a previous iteration of `find_arrow_span`
    // (we always pivot on the first body char of a contiguous run), so
    // including them here would let us absorb a trailing dot from the
    // `from` token like `foo.. -- bar`.
    let mut start = pivot;
    while start > 0 {
        let prev = bytes[start - 1];
        if matches!(prev, b'<' | b'>' | b'|' | b'*' | b'o' | b'x' | b'+' | b'#' | b'(' | b')') {
            start -= 1;
            continue;
        }
        break;
    }

    if end <= start {
        return None;
    }
    Some((start, end))
}

/// `up` / `down` / `left` / `right` (or `le` / `ri` / `do`) starting at
/// the first byte of `s`. Returns the length consumed, or `None`.
///
/// Byte-compares — `str::eq_ignore_ascii_case` on a UTF-8 slice would
/// panic when `kw.len()` indexes inside a multi-byte char (e.g. CJK
/// label on the right-hand side of an arrow body).
fn match_direction_keyword(s: &[u8]) -> Option<usize> {
    for kw in ["right", "left", "down", "up", "ri", "le", "do"] {
        let kb = kw.as_bytes();
        if s.len() >= kb.len() && s[..kb.len()].eq_ignore_ascii_case(kb) {
            return Some(kb.len());
        }
    }
    None
}

/// The arrow span must be flanked by whitespace, end-of-line, or a quote
/// character on each side, and must contain at least one body char. This
/// rejects false positives like the `-` in `def-foo`.
fn validate_arrow_borders(bytes: &[u8], start: usize, end: usize) -> bool {
    let span = &bytes[start..end];
    if !span.iter().any(|b| matches!(b, b'-' | b'.' | b'=')) {
        return false;
    }
    let left_ok = start == 0
        || bytes[start - 1].is_ascii_whitespace()
        || bytes[start - 1] == b'"';
    let right_ok = end >= bytes.len()
        || bytes[end].is_ascii_whitespace()
        || bytes[end] == b'"'
        || bytes[end] == b':';
    left_ok && right_ok
}

pub(super) fn parse_relation(raw: &str, line_no: usize, flavor: Flavor) -> Option<RelationParse> {
    let (start, end) = find_arrow_span(raw)?;
    // Arrow string itself.
    let arrow = &raw[start..end];
    // Left side: from-id (and optional multiplicity / role / qualifier).
    let left = raw[..start].trim_end();
    let right = raw[end..].trim_start();

    // Find the `:` that introduces the label. PlantUML requires the
    // label colon to be preceded by whitespace (e.g. `B : owns`), so
    // we skip any leading `:` belonging to a `:Actor:` shorthand
    // endpoint or a member port `B::value` (the latter via the `::`
    // run skip).
    let label_colon = {
        let bytes = right.as_bytes();
        let mut i = 0;
        let mut found = None;
        while i < bytes.len() {
            if bytes[i] == b':' {
                if i + 1 < bytes.len() && bytes[i + 1] == b':' {
                    i += 2; // skip `::`
                    continue;
                }
                if i == 0 || !bytes[i - 1].is_ascii_whitespace() {
                    i += 1;
                    continue;
                }
                found = Some(i);
                break;
            }
            i += 1;
        }
        found
    };
    let (label, right) = match label_colon {
        Some(i) => (Some(right[i + 1..].trim().to_string()), right[..i].trim_end()),
        None => (None, right),
    };

    let (from_id, mult_from, role_from, from_port, from_hint, from_couple_l) =
        parse_endpoint_left(left, flavor)?;
    let (to_id, mult_to, role_to, to_port, to_hint, to_couple) =
        parse_endpoint_right(right, flavor)?;
    // If the user wrote `C -- (A, B)`, the couple is on the right; we
    // normalize to from_couple by swapping. After normalization, `to`
    // is the lone class and `from_couple` is the (A, B) pair.
    let (final_from, final_to, from_couple) = match (from_couple_l, to_couple) {
        (Some(c), _) => (String::new(), to_id, Some(c)),
        (None, Some(c)) => (String::new(), from_id, Some(c)),
        (None, None) => (from_id, to_id, None),
    };
    let from_id = final_from;
    let to_id = final_to;

    let (head_from, head_to, line_style, direction, color) = decode_arrow(arrow);
    let stereotype = label.as_deref().and_then(extract_stereotype);

    Some(RelationParse {
        rel: crate::ir::Relation {
            from: from_id,
            to: to_id,
            from_couple,
            from_port,
            to_port,
            head_from,
            head_to,
            line_style,
            direction,
            label,
            mult_from,
            mult_to,
            note: None,
            role_from,
            role_to,
            stereotype,
            color,
            line: line_no,
        },
        from_hint,
        to_hint,
    })
}

/// Parsed relation plus per-endpoint shape hints. The caller uses
/// the hints when auto-creating endpoints (so `(Foo) --> Bar` makes
/// Foo a Circle / Usecase / Actor instead of a default Class).
pub(super) struct RelationParse {
    pub rel: crate::ir::Relation,
    pub from_hint: EndpointHint,
    pub to_hint: EndpointHint,
}

/// Endpoint parse result: (id, mult, role, port, hint, couple).
/// `couple` is `Some((A, B))` when the user wrote `(A, B)` —
/// PlantUML's association-class syntax: the edge anchors at the
/// midpoint of the existing A-B edge instead of a single entity.
type EndpointTuple = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    EndpointHint,
    Option<(String, String)>,
);

fn parse_endpoint_left(s: &str, flavor: Flavor) -> Option<EndpointTuple> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Couple form has top priority — the comma inside `()` is what
    // distinguishes it from a lollipop reference.
    if let Some(couple) = parse_couple_parens(s) {
        return Some((String::new(), None, None, None, EndpointHint::None, Some(couple)));
    }
    let (id_part, mult) = pop_trailing_quoted(s);
    let id_part = id_part.trim();
    let (id, role) = pop_trailing_role(id_part);
    if id.is_empty() {
        return None;
    }
    let (id, hint) = classify_endpoint_id(unquote(id), flavor);
    let (id, port) = split_member_port(id);
    Some((id, mult, role, port, hint, None))
}

fn parse_endpoint_right(s: &str, flavor: Flavor) -> Option<EndpointTuple> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(couple) = parse_couple_parens(s) {
        return Some((String::new(), None, None, None, EndpointHint::None, Some(couple)));
    }
    let (rest, mult) = pop_leading_quoted(s);
    let rest = rest.trim();
    let (id, role) = pop_trailing_role(rest);
    if id.is_empty() {
        return None;
    }
    let (id, hint) = classify_endpoint_id(unquote(id), flavor);
    let (id, port) = split_member_port(id);
    Some((id, mult, role, port, hint, None))
}

/// Strip shorthand brackets off an endpoint id and report the
/// implied shape. In `Class` flavor `(Foo)` is a lollipop interface
/// (PlantUML's default for class context) and `:Foo:` is left
/// untouched. In `UseCase` flavor `(Foo)` is a usecase ellipse and
/// `:Foo:` is an actor.
fn classify_endpoint_id(id: String, flavor: Flavor) -> (String, EndpointHint) {
    match flavor {
        Flavor::Class => {
            let (id, is_lollipop) = strip_lollipop_parens(id);
            (id, if is_lollipop { EndpointHint::Lollipop } else { EndpointHint::None })
        }
        Flavor::UseCase => {
            let (id, is_actor) = strip_actor_colons(id);
            if is_actor {
                return (id, EndpointHint::Actor);
            }
            let (id, is_usecase) = strip_lollipop_parens(id);
            (id, if is_usecase { EndpointHint::UseCase } else { EndpointHint::None })
        }
    }
}

/// `:Name:` → ("Name", true); plain id → (id, false). The inner part
/// must be non-empty and free of further colons (which would mean
/// `A::field` or a member-port form, not an actor reference).
fn strip_actor_colons(id: String) -> (String, bool) {
    let t = id.trim();
    if t.starts_with(':') && t.ends_with(':') && t.len() >= 2 {
        let inner = t[1..t.len() - 1].trim();
        if !inner.is_empty() && !inner.contains(':') {
            return (inner.to_string(), true);
        }
    }
    (id, false)
}

/// Scan a relation label for `<<include>>` / `<<extend>>` /
/// `<<extends>>` and return the normalized lowercase token (or
/// `None` if no recognized stereotype is present). Does not modify
/// the label — the visible `<<…>>` text stays for the painter.
pub(super) fn extract_stereotype(label: &str) -> Option<String> {
    let mut idx = 0usize;
    loop {
        let start = label[idx..].find("<<")?;
        let s = idx + start;
        let after = s + 2;
        let end_rel = match label[after..].find(">>") {
            Some(e) => e,
            None => return None,
        };
        let token = label[after..after + end_rel].trim().to_ascii_lowercase();
        if matches!(token.as_str(), "include" | "extend" | "extends") {
            return Some(token);
        }
        idx = after + end_rel + 2;
    }
}

/// Detect `(A, B)` couple form. Returns `(A, B)` on success.
fn parse_couple_parens(s: &str) -> Option<(String, String)> {
    let t = s.trim();
    if !(t.starts_with('(') && t.ends_with(')') && t.len() >= 2) {
        return None;
    }
    let inner = &t[1..t.len() - 1];
    let comma = inner.find(',')?;
    let a = inner[..comma].trim().to_string();
    let b = inner[comma + 1..].trim().to_string();
    if a.is_empty() || b.is_empty() {
        return None;
    }
    if a.contains('(') || b.contains('(') {
        return None;
    }
    Some((a, b))
}

/// `(Name)` → (`"Name"`, true); plain id → (id, false). Whitespace
/// inside the parens is trimmed.
fn strip_lollipop_parens(id: String) -> (String, bool) {
    let t = id.trim();
    if t.starts_with('(') && t.ends_with(')') && t.len() >= 2 {
        let inner = t[1..t.len() - 1].trim().to_string();
        if !inner.is_empty() && !inner.contains('(') && !inner.contains(')') {
            return (inner, true);
        }
    }
    (id, false)
}

/// `Class::member` → (`"Class"`, `Some("member")`); plain id → (id, None).
/// Only splits at the *last* `::` so qualified names like `outer::inner`
/// still work (the inner-most segment is treated as the port).
fn split_member_port(id: String) -> (String, Option<String>) {
    if let Some(i) = id.rfind("::") {
        let head = id[..i].to_string();
        let port = id[i + 2..].to_string();
        if !head.is_empty() && !port.is_empty() {
            return (head, Some(port));
        }
    }
    (id, None)
}

fn pop_trailing_quoted(s: &str) -> (&str, Option<String>) {
    let trimmed = s.trim_end();
    if !trimmed.ends_with('"') {
        return (trimmed, None);
    }
    let body = &trimmed[..trimmed.len() - 1];
    let open = body.rfind('"');
    match open {
        Some(o) => {
            let mult = body[o + 1..].to_string();
            (body[..o].trim_end(), Some(mult))
        }
        None => (trimmed, None),
    }
}

fn pop_leading_quoted(s: &str) -> (&str, Option<String>) {
    let trimmed = s.trim_start();
    if !trimmed.starts_with('"') {
        return (trimmed, None);
    }
    let after = &trimmed[1..];
    let close = match after.find('"') {
        Some(c) => c,
        None => return (trimmed, None),
    };
    let mult = after[..close].to_string();
    (after[close + 1..].trim_start(), Some(mult))
}

fn pop_trailing_role(s: &str) -> (&str, Option<String>) {
    let trimmed = s.trim_end();
    let slash = match trimmed.rfind('/') {
        Some(i) => i,
        None => return (trimmed, None),
    };
    // Slash must follow whitespace to disambiguate from path-like ids.
    if slash == 0 || !trimmed.as_bytes()[slash - 1].is_ascii_whitespace() {
        return (trimmed, None);
    }
    let role = trimmed[slash + 1..].trim().to_string();
    if role.is_empty() {
        return (trimmed, None);
    }
    (trimmed[..slash].trim_end(), Some(role))
}

/// Decompose an arrow token (e.g. `-up->`, `<|..`, `*--`, `<|--`) into
/// its head decorations, line style, and direction hint.
fn decode_arrow(
    arrow: &str,
) -> (ArrowHead, ArrowHead, LineStyle, Option<Direction>, Option<String>) {
    // Strip `[…]` color/style annotations; capture the first `#…`
    // color found inside any such annotation. PlantUML accepts forms
    // like `[#red]`, `[#abcdef]`, `[#red,bold]`, `[bold,#red]` —
    // we split on `,` and take whichever token starts with `#`.
    let mut s = String::new();
    let mut color: Option<String> = None;
    let bytes = arrow.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            let start = i + 1;
            while i < bytes.len() && bytes[i] != b']' {
                i += 1;
            }
            let inner = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
            if color.is_none() {
                for tok in inner.split(',').map(str::trim) {
                    if let Some(rest) = tok.strip_prefix('#') {
                        if !rest.is_empty() {
                            color = Some(format!("#{rest}"));
                            break;
                        }
                    }
                }
            }
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }
        s.push(bytes[i] as char);
        i += 1;
    }

    // Pull a direction keyword if present.
    let lower = s.to_ascii_lowercase();
    let mut direction = None;
    for (kw, dir) in [
        ("up", Direction::Up),
        ("down", Direction::Down),
        ("left", Direction::Left),
        ("right", Direction::Right),
        ("do", Direction::Down),
        ("le", Direction::Left),
        ("ri", Direction::Right),
    ] {
        if let Some(idx) = lower.find(kw) {
            let after = idx + kw.len();
            // Must be flanked by `-`/`.`/`=` (the body chars) on at
            // least one side, to avoid matching shape characters.
            let before_ok = idx == 0
                || matches!(s.as_bytes()[idx - 1], b'-' | b'.' | b'=');
            let after_ok = after == s.len()
                || matches!(s.as_bytes()[after], b'-' | b'.' | b'=');
            if before_ok && after_ok {
                direction = Some(dir);
                s.replace_range(idx..idx + kw.len(), "");
                break;
            }
        }
    }

    let dotted = s.contains('.');
    let line_style = if dotted { LineStyle::Dashed } else { LineStyle::Solid };

    // Body chars are `-` / `.` / `=`. Anything before the first body
    // char is the left head; anything after the last body char is the
    // right head.
    let bytes = s.as_bytes();
    let body_start = bytes
        .iter()
        .position(|b| matches!(b, b'-' | b'.' | b'='))
        .unwrap_or(bytes.len());
    let body_end = bytes
        .iter()
        .rposition(|b| matches!(b, b'-' | b'.' | b'='))
        .map(|i| i + 1)
        .unwrap_or(0);
    let head_left = &s[..body_start];
    let head_right = &s[body_end..];

    (
        decode_head(head_left, true),
        decode_head(head_right, false),
        line_style,
        direction,
        color,
    )
}

fn decode_head(s: &str, is_left: bool) -> ArrowHead {
    let s = s.trim();
    if s.is_empty() {
        return ArrowHead::None;
    }
    match s {
        "<|" if is_left => ArrowHead::TriangleOpen,
        "|>" if !is_left => ArrowHead::TriangleOpen,
        "<" if is_left => ArrowHead::ArrowOpen,
        ">" if !is_left => ArrowHead::ArrowOpen,
        "<<" if is_left => ArrowHead::ArrowOpen,
        ">>" if !is_left => ArrowHead::ArrowOpen,
        "*" => ArrowHead::DiamondFilled,
        "o" => ArrowHead::DiamondOpen,
        "x" => ArrowHead::Cross,
        "+" => ArrowHead::Plus,
        "#" => ArrowHead::None, // square (M2)
        // Component-interface socket heads (PlantUML LinkDecor.PARENTHESIS).
        // `Foo -( Bar` puts a right-facing socket on Bar; `Foo )- Bar`
        // puts a left-facing socket on Foo.
        "(" if !is_left => ArrowHead::SocketOpen,
        ")" if is_left => ArrowHead::SocketClosed,
        // Symmetric forms — sometimes seen but PlantUML's renderer
        // treats them identically.
        "(" if is_left => ArrowHead::SocketOpen,
        ")" if !is_left => ArrowHead::SocketClosed,
        _ => ArrowHead::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span_of(line: &str) -> Option<&str> {
        find_arrow_span(line).map(|(s, e)| &line[s..e])
    }

    #[test]
    fn finds_basic_arrows() {
        assert_eq!(span_of("A -- B"), Some("--"));
        assert_eq!(span_of("A --> B"), Some("-->"));
        assert_eq!(span_of("A <|-- B"), Some("<|--"));
        assert_eq!(span_of("A ..|> B"), Some("..|>"));
        assert_eq!(span_of("A *-- B"), Some("*--"));
        assert_eq!(span_of("A o-- B"), Some("o--"));
    }

    #[test]
    fn finds_arrow_with_inline_direction() {
        assert_eq!(span_of("A -up-> B"), Some("-up->"));
        assert_eq!(span_of("A -left-> B"), Some("-left->"));
        assert_eq!(span_of("A -ri-> B"), Some("-ri->"));
    }

    #[test]
    fn finds_arrow_with_color_annotation() {
        assert_eq!(span_of("A -[#red]-> B"), Some("-[#red]->"));
        assert_eq!(span_of("A -[#abcdef,bold]-> B"), Some("-[#abcdef,bold]->"));
    }

    #[test]
    fn rejects_arrow_without_whitespace_border() {
        // `def-foo` — the `-` has no whitespace before/after the run,
        // so it shouldn't be picked as an arrow.
        assert!(find_arrow_span("def-foo").is_none());
    }

    #[test]
    fn skips_arrow_chars_inside_quoted_multiplicity() {
        // `"1..*"` contains a `..` sequence that LOOKS like a dashed
        // arrow body, but the scanner skips quoted regions entirely so
        // it picks the real arrow after the closing quote.
        let line = r#"A "1..*" -- "1" B"#;
        assert_eq!(span_of(line), Some("--"));
    }

    #[test]
    fn finds_arrow_followed_by_label_colon() {
        // The right-side validator allows `:` as the right border so a
        // label-introducing colon doesn't push the span end inside the
        // label.
        let line = "A --> B : owns";
        assert_eq!(span_of(line), Some("-->"));
    }

    #[test]
    fn relation_with_member_port_keeps_id_intact() {
        // `A::name --> B::value` — the `::` runs are part of the
        // endpoints, not arrows. The colon in `:` (label intro) is
        // skipped iff followed by another colon.
        let rp = parse_relation("A::name --> B::value", 1, Flavor::Class).unwrap();
        assert_eq!(rp.rel.from, "A");
        assert_eq!(rp.rel.from_port.as_deref(), Some("name"));
        assert_eq!(rp.rel.to, "B");
        assert_eq!(rp.rel.to_port.as_deref(), Some("value"));
        assert!(rp.rel.label.is_none());
    }

    #[test]
    fn relation_with_member_port_and_label() {
        let rp = parse_relation("A::field --> B : carries", 2, Flavor::Class).unwrap();
        assert_eq!(rp.rel.from, "A");
        assert_eq!(rp.rel.from_port.as_deref(), Some("field"));
        assert_eq!(rp.rel.label.as_deref(), Some("carries"));
    }

    #[test]
    fn relation_actor_colons_in_use_case_flavor() {
        // `:Bob: --> :Alice:` — both endpoints carry Actor hints under
        // UseCase flavor. Under Class flavor the colons stay literal.
        let rp = parse_relation(":Bob: --> :Alice:", 1, Flavor::UseCase).unwrap();
        assert_eq!(rp.rel.from, "Bob");
        assert_eq!(rp.rel.to, "Alice");
        assert_eq!(rp.from_hint, EndpointHint::Actor);
        assert_eq!(rp.to_hint, EndpointHint::Actor);
        let rp = parse_relation(":Bob: --> :Alice:", 1, Flavor::Class).unwrap();
        assert_eq!(rp.rel.from, ":Bob:");
        assert_eq!(rp.from_hint, EndpointHint::None);
    }

    #[test]
    fn relation_usecase_parens_in_use_case_flavor() {
        let rp = parse_relation("Bob --> (Login)", 1, Flavor::UseCase).unwrap();
        assert_eq!(rp.rel.to, "Login");
        assert_eq!(rp.to_hint, EndpointHint::UseCase);
        let rp = parse_relation("Bob --> (Iface)", 1, Flavor::Class).unwrap();
        assert_eq!(rp.rel.to, "Iface");
        assert_eq!(rp.to_hint, EndpointHint::Lollipop);
    }

    #[test]
    fn stereotype_extracted_from_label() {
        // `<<include>>` / `<<extend>>` / `<<extends>>` populate
        // rel.stereotype; the visible label keeps the original text
        // so the painter renders `<<include>>` next to the edge.
        let rp = parse_relation("A ..> B : <<include>>", 1, Flavor::Class).unwrap();
        assert_eq!(rp.rel.stereotype.as_deref(), Some("include"));
        assert_eq!(rp.rel.label.as_deref(), Some("<<include>>"));
        let rp = parse_relation("A ..> B : <<extends>>", 1, Flavor::UseCase).unwrap();
        assert_eq!(rp.rel.stereotype.as_deref(), Some("extends"));
        let rp = parse_relation("A ..> B : <<unrelated>>", 1, Flavor::Class).unwrap();
        assert!(rp.rel.stereotype.is_none());
        let rp = parse_relation("A --> B : owns", 1, Flavor::Class).unwrap();
        assert!(rp.rel.stereotype.is_none());
    }

    #[test]
    fn decode_arrow_extracts_color_in_either_order() {
        let (_, _, _, _, color) = decode_arrow("-[#abc,bold]->");
        assert_eq!(color.as_deref(), Some("#abc"));
        let (_, _, _, _, color) = decode_arrow("-[bold,#def]->");
        assert_eq!(color.as_deref(), Some("#def"));
    }

    #[test]
    fn decode_arrow_picks_up_direction_keyword() {
        let (_, _, _, dir, _) = decode_arrow("-up->");
        assert_eq!(dir, Some(Direction::Up));
        let (_, _, _, dir, _) = decode_arrow("-left->");
        assert_eq!(dir, Some(Direction::Left));
        let (_, _, _, dir, _) = decode_arrow("--");
        assert!(dir.is_none());
    }

    #[test]
    fn decode_arrow_dashed_body_yields_dashed_style() {
        let (_, _, style, _, _) = decode_arrow("..|>");
        assert_eq!(style, LineStyle::Dashed);
        let (_, _, style, _, _) = decode_arrow("--|>");
        assert_eq!(style, LineStyle::Solid);
    }

    #[test]
    fn cjk_target_after_arrow_does_not_panic() {
        // Multi-byte char immediately after the arrow body used to
        // panic in `match_direction_keyword` when it sliced
        // `text[..kw.len()]` and landed inside the UTF-8 boundary
        // of the leading CJK char.
        let r = parse_relation("电商领域模型 ..> 基础设施层 : 依赖", 1, Flavor::Class)
            .expect("parses without panic");
        assert_eq!(r.rel.from, "电商领域模型");
        assert_eq!(r.rel.to, "基础设施层");
    }
}
