//! Entity declarations: `class`, `interface`, `abstract`, `enum`, etc.
//! Also lollipop interfaces (`() Foo`) and the trailing decorations
//! (color, stereotype + custom marker, generic) shared by container
//! declarations.

use crate::ir::{Entity, EntityKind};

use super::util::{parse_alias, strip_prefix_keyword};

pub(super) const ENTITY_KEYWORDS: &[&str] = &[
    "abstract class",
    "static class",
    "abstract",
    "interface",
    "annotation",
    "protocol",
    "exception",
    "metaclass",
    "stereotype",
    "dataclass",
    "record",
    "class",
    "enum",
    "struct",
    "entity",
    "circle",
    "diamond",
];

pub(super) struct EntityAction {
    pub(super) entity: Entity,
    pub(super) has_block: bool,
}

/// Parse `() Foo` or `() "Display" as Foo` as a lollipop interface
/// declaration. Lollipops render as a small circle, not a full class
/// card. The leading `()` is the syntactic marker.
pub(super) fn parse_lollipop_decl(raw: &str, line_no: usize) -> Option<EntityAction> {
    let rest = raw.strip_prefix("()")?;
    if !rest.starts_with(char::is_whitespace) {
        return None;
    }
    let working = rest.trim();
    if working.is_empty() {
        return None;
    }
    let (id, display) = parse_alias(working)?;
    Some(EntityAction {
        entity: Entity {
            kind: EntityKind::Circle,
            id,
            display,
            generic: None,
            stereotype: None,
            stereotype_marker: None,
            fields: Vec::new(),
            methods: Vec::new(),
            body: None,
            fill: None,
            line: line_no,
        },
        has_block: false,
    })
}

/// Try to parse `raw` as an entity declaration. Returns `None` if the
/// line doesn't start with an entity keyword.
pub(super) fn parse_entity_decl(raw: &str, line_no: usize) -> Option<EntityAction> {
    let (kw, rest) = ENTITY_KEYWORDS
        .iter()
        .find_map(|kw| strip_prefix_keyword(raw, kw).map(|r| (*kw, r.trim())))?;
    let kind = match kw {
        "abstract class" | "abstract" => EntityKind::Abstract,
        // Aliases that fall back to plain Class — M0 doesn't distinguish.
        "static class" | "metaclass" | "stereotype" | "dataclass" | "record" => EntityKind::Class,
        other => EntityKind::from_keyword(other).unwrap_or(EntityKind::Class),
    };

    let mut rest = rest.to_string();
    let has_block = if rest.ends_with('{') {
        rest.truncate(rest.len() - 1);
        true
    } else {
        false
    };
    let rest_trim = rest.trim();
    let mut working = rest_trim.to_string();

    let fill = pop_trailing_color(&mut working);
    let (stereotype, stereotype_marker) =
        match pop_trailing_stereotype_with_marker(&mut working) {
            Some((text, marker)) => (Some(text).filter(|t| !t.is_empty()), marker),
            None => (None, None),
        };
    let generic = pop_trailing_generic(&mut working);
    let (id, display) = parse_alias(working.trim())?;

    Some(EntityAction {
        entity: Entity {
            kind,
            id,
            display,
            generic,
            stereotype,
            stereotype_marker,
            fields: Vec::new(),
            methods: Vec::new(),
            body: None,
            fill,
            line: line_no,
        },
        has_block,
    })
}

/// Strip a trailing ` #color` token; returns it (with the `#`) if present.
pub(super) fn pop_trailing_color(rest: &mut String) -> Option<String> {
    let trimmed = rest.trim_end();
    let bytes = trimmed.as_bytes();
    let mut i = bytes.len();
    while i > 0 && !bytes[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    if i == 0 {
        return None;
    }
    let token = trimmed[i..].to_string();
    if !token.starts_with('#') {
        return None;
    }
    let kept = trimmed[..i].trim_end().to_string();
    *rest = kept;
    Some(token)
}

/// Strip a trailing `<<stereotype>>` block. Also returns a custom
/// marker `(letter, color)` parsed from the `(L, color) text` prefix
/// PlantUML uses to override the kind-default chip; the returned text
/// has the prefix stripped.
pub(super) fn pop_trailing_stereotype_with_marker(
    rest: &mut String,
) -> Option<(String, Option<(String, Option<String>)>)> {
    let trimmed = rest.trim_end();
    let body = trimmed.strip_suffix(">>")?;
    let lt_idx = body.rfind("<<")?;
    let inner = body[lt_idx + 2..].trim().to_string();
    *rest = body[..lt_idx].trim_end().to_string();
    Some(parse_stereotype_inner(inner))
}

/// Backwards-compat helper used by call sites that don't care about
/// the marker (e.g. `package` decl).
pub(super) fn pop_trailing_stereotype(rest: &mut String) -> Option<String> {
    pop_trailing_stereotype_with_marker(rest).map(|(text, _)| text)
}

/// Split `(L, color) text` into (text, Some((L, color))) or treat the
/// whole thing as plain text. The color is optional: `(L) text` is
/// also valid and produces marker `(L, None)`.
fn parse_stereotype_inner(s: String) -> (String, Option<(String, Option<String>)>) {
    let s_trim = s.trim();
    if !s_trim.starts_with('(') {
        return (s, None);
    }
    let close = match s_trim.find(')') {
        Some(c) => c,
        None => return (s, None),
    };
    let inner = s_trim[1..close].trim();
    if inner.is_empty() {
        return (s, None);
    }
    let mut parts = inner.splitn(2, ',').map(str::trim);
    let letter = parts.next().unwrap_or("").to_string();
    let color = parts.next().map(|c| c.to_string()).filter(|c| !c.is_empty());
    if letter.is_empty() {
        return (s, None);
    }
    let after = s_trim[close + 1..].trim().to_string();
    (after, Some((letter, color)))
}

/// Strip a trailing `<T, U>` generic parameter list, balancing nested `<>`
/// so `Map<K, List<V>>` doesn't get mis-cut.
pub(super) fn pop_trailing_generic(rest: &mut String) -> Option<String> {
    let trimmed = rest.trim_end();
    if !trimmed.ends_with('>') {
        return None;
    }
    let bytes = trimmed.as_bytes();
    let mut depth = 0;
    let mut start = None;
    for i in (0..bytes.len()).rev() {
        match bytes[i] {
            b'>' => depth += 1,
            b'<' => {
                depth -= 1;
                if depth == 0 {
                    start = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let start = start?;
    if start == 0 {
        return None;
    }
    // Bare `<…>` at start of name is not a generic — only treat as generic
    // when it follows a name character without whitespace.
    let prev = bytes[start - 1] as char;
    if prev.is_whitespace() {
        return None;
    }
    let inner = trimmed[start + 1..trimmed.len() - 1].to_string();
    *rest = trimmed[..start].to_string();
    Some(inner)
}
