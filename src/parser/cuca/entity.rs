//! Entity declarations: `class`, `interface`, `abstract`, `enum`, etc.
//! Also lollipop interfaces (`() Foo`) and the trailing decorations
//! (color, stereotype + custom marker, generic) shared by container
//! declarations.

use crate::ir::{ClassFamilyKind, Entity, EntityKindData, StereotypeMarker, USymbol};

use super::util::{parse_alias, strip_prefix_keyword};

/// Entity-leaf keywords. Order matters: multi-word forms (`abstract
/// class`) must precede their single-word prefixes (`abstract`) so the
/// linear-scan dispatcher in `parse_entity_decl` doesn't pick the
/// shorter alternative first. Within a length tier, ordering is
/// arbitrary.
///
/// Class family (USymbol::None + Compartment) is grouped at the top.
/// Desc family (USymbol::* + Plain) comes after. See
/// `docs/cuca-diagram-design.md` §3.1 for the visual mapping.
pub(super) const ENTITY_KEYWORDS: &[&str] = &[
    // Class family
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
    // Specials
    "circle",
    "diamond",
    "note",
    // Desc family (M5+: each gets its own painter; v1 falls back to
    // the compartment painter when `usymbol.keyword()` is unknown).
    "component",
    "actor",
    "usecase",
    "node",
    "database",
    "cloud",
    "queue",
    "stack",
    "storage",
    "artifact",
    "agent",
    "person",
    "collections",
    "rectangle",
    "card",
    "folder",
    "frame",
    "file",
    "hexagon",
    "action",
    "process",
    "label",
    "boundary",
    "control",
    "port",
    "portin",
    "portout",
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
            usymbol: USymbol::Interface,
            id,
            display,
            stereotype: None,
            stereotype_marker: None,
            fill: None,
            line: line_no,
            kind_data: EntityKindData::Plain { members: Vec::new() },
        },
        has_block: false,
    })
}

/// Inline shorthand for desc-family entity declarations, mirroring
/// PlantUML's `[…]` / `(…)` / `:…:` forms:
///
/// | Syntax          | Equivalent      |
/// |-----------------|-----------------|
/// | `[Foo]`         | `component Foo` |
/// | `(Foo)`         | `usecase Foo`   |
/// | `:Foo:`         | `actor Foo`     |
///
/// Trailing decorations (`as Alias`, `<<stereotype>>`, `#color`) follow
/// the closing delimiter exactly like the longhand form. Returns
/// `None` if `raw` doesn't open with one of the three delimiter chars
/// or the brackets aren't balanced.
pub(super) fn parse_inline_shorthand(raw: &str, line_no: usize) -> Option<EntityAction> {
    let first = raw.chars().next()?;
    let (close, usymbol) = match first {
        '[' => (']', USymbol::Component),
        '(' => {
            // `()` is the lollipop syntax, handled separately. Defer.
            if raw.starts_with("()") {
                return None;
            }
            (')', USymbol::UseCase)
        }
        ':' => (':', USymbol::Actor),
        _ => return None,
    };
    let bytes = raw.as_bytes();
    // Find the matching close character. For balanced delimiters
    // `[` and `(` we have to scan past nested brackets; for `:` the
    // SECOND `:` ends the name (PlantUML semantics).
    let close_idx = match first {
        '[' => find_balanced(bytes, b'[', b']')?,
        '(' => find_balanced(bytes, b'(', b')')?,
        ':' => bytes[1..].iter().position(|&b| b == b':').map(|i| i + 1)?,
        _ => unreachable!(),
    };
    let inner = raw[1..close_idx].trim();
    if inner.is_empty() {
        return None;
    }
    // Couple-link form `(A, B) .. C` looks like usecase shorthand at
    // first glance — reject when the parens contain a comma since
    // PlantUML uses commas only as couple separators (entity names
    // themselves never contain commas).
    if first == '(' && inner.contains(',') {
        return None;
    }
    let trailing = raw[close_idx + 1..].trim();
    // Trailing portion must be EMPTY or begin with `as ` / `<<` / `#`
    // (alias / stereotype / color). Anything else — arrow operators
    // (`..`, `--`, `->`), more text — means this isn't a pure
    // declaration and we should fall through to the relation parser.
    let trailing_ok = trailing.is_empty()
        || trailing.starts_with("as ")
        || trailing.starts_with("<<")
        || trailing.starts_with('#');
    if !trailing_ok {
        return None;
    }
    let mut working = if trailing.is_empty() {
        inner.to_string()
    } else {
        format!("{inner} {trailing}")
    };
    let fill = pop_trailing_color(&mut working);
    let (stereotype, stereotype_marker) =
        match pop_trailing_stereotype_with_marker(&mut working) {
            Some((text, marker)) => (Some(text).filter(|t| !t.is_empty()), marker),
            None => (None, None),
        };
    let _ = close; // close char captured for symmetry; not needed past parsing
    let (id, display) = parse_alias(working.trim())?;
    Some(EntityAction {
        entity: Entity {
            usymbol,
            id,
            display,
            stereotype,
            stereotype_marker,
            fill,
            line: line_no,
            kind_data: EntityKindData::Plain { members: Vec::new() },
        },
        has_block: false,
    })
}

/// Scan `bytes` (starting at index 0 which holds `open`) for the
/// matching close character at depth 0. Returns the byte index of the
/// close, or `None` if unbalanced. Quoted strings are treated as
/// opaque so `[Foo "with ]" content]` doesn't split early.
fn find_balanced(bytes: &[u8], open: u8, close: u8) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut i = 0;
    let mut in_quote = false;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            in_quote = !in_quote;
        } else if !in_quote {
            if b == open {
                depth += 1;
            } else if b == close {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
        i += 1;
    }
    None
}

/// Try to parse `raw` as an entity declaration. Returns `None` if the
/// line doesn't start with an entity keyword.
pub(super) fn parse_entity_decl(raw: &str, line_no: usize) -> Option<EntityAction> {
    let (kw, rest) = ENTITY_KEYWORDS
        .iter()
        .find_map(|kw| strip_prefix_keyword(raw, kw).map(|r| (*kw, r.trim())))?;
    let (usymbol, kind_for_compartment) = match kw {
        // Class family.
        "abstract class" | "abstract" => (USymbol::None, Some(ClassFamilyKind::Abstract)),
        "static class" | "metaclass" | "stereotype" | "dataclass" | "record" => {
            (USymbol::None, Some(ClassFamilyKind::Class))
        }
        "class" => (USymbol::None, Some(ClassFamilyKind::Class)),
        "interface" => (USymbol::None, Some(ClassFamilyKind::Interface)),
        "annotation" => (USymbol::None, Some(ClassFamilyKind::Annotation)),
        "protocol" => (USymbol::None, Some(ClassFamilyKind::Protocol)),
        "exception" => (USymbol::None, Some(ClassFamilyKind::Exception)),
        "enum" => (USymbol::None, Some(ClassFamilyKind::Enum)),
        "struct" => (USymbol::None, Some(ClassFamilyKind::Struct)),
        "entity" => (USymbol::None, Some(ClassFamilyKind::EntityShape)),
        // Specials.
        "circle" => (USymbol::Interface, None),
        "diamond" => (USymbol::Diamond, None),
        "note" => return None, // handled by note-specific parsers
        // Desc family (Plain — no compartment).
        "component" => (USymbol::Component, None),
        "actor" => (USymbol::Actor, None),
        "usecase" => (USymbol::UseCase, None),
        "node" => (USymbol::Node, None),
        "database" => (USymbol::Database, None),
        "cloud" => (USymbol::Cloud, None),
        "queue" => (USymbol::Queue, None),
        "stack" => (USymbol::Stack, None),
        "storage" => (USymbol::Storage, None),
        "artifact" => (USymbol::Artifact, None),
        "agent" => (USymbol::Agent, None),
        "person" => (USymbol::Person, None),
        "collections" => (USymbol::Collections, None),
        "rectangle" => (USymbol::Rectangle, None),
        "card" => (USymbol::Card, None),
        "folder" => (USymbol::Folder, None),
        "frame" => (USymbol::Frame, None),
        "file" => (USymbol::File, None),
        "hexagon" => (USymbol::Hexagon, None),
        "action" => (USymbol::Action, None),
        "process" => (USymbol::Process, None),
        "label" => (USymbol::Label, None),
        "boundary" => (USymbol::Boundary, None),
        "control" => (USymbol::Control, None),
        "port" => (USymbol::Port, None),
        "portin" => (USymbol::PortIn, None),
        "portout" => (USymbol::PortOut, None),
        _ => (USymbol::None, Some(ClassFamilyKind::Class)),
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

    let kind_data = match kind_for_compartment {
        Some(kind) => EntityKindData::Compartment {
            kind,
            generic,
            fields: Vec::new(),
            methods: Vec::new(),
        },
        None => EntityKindData::Plain { members: Vec::new() },
    };

    Some(EntityAction {
        entity: Entity {
            usymbol,
            id,
            display,
            stereotype,
            stereotype_marker,
            fill,
            line: line_no,
            kind_data,
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
) -> Option<(String, Option<StereotypeMarker>)> {
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

/// Split `(L, color) text` into (text, Some(StereotypeMarker)) or treat
/// the whole thing as plain text. The color is optional: `(L) text` is
/// also valid and produces marker `(L, None)`.
fn parse_stereotype_inner(s: String) -> (String, Option<StereotypeMarker>) {
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
    (after, Some(StereotypeMarker { letter, color }))
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
