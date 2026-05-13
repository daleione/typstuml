//! Entity declarations: `class`, `interface`, `abstract`, `enum`, etc.
//! Also lollipop interfaces (`() Foo`) and the trailing decorations
//! (color, stereotype + custom marker, generic) shared by container
//! declarations.

use crate::ir::{ClassFamilyKind, Entity, EntityKindData, StereotypeMarker, USymbol};

use super::util::{parse_alias, strip_prefix_keyword};

/// What body-row grammar a declared entity uses. Class-family entities
/// take `+ method()` / `- field: T` rows; objects take `name = value`
/// rows; desc-family ("Plain") entities take inline `{ + foo }` rows.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum BodyKind {
    Compartment(ClassFamilyKind),
    Object,
    Plain,
}

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
    // Object diagrams (PlantUML's `objectdiagram`): same name+alias /
    // stereotype / color / `{ … }` block grammar as class-family, but
    // body rows are `name = value` instead of `+ method()`. The painter
    // draws a 2-compartment card with the name underlined on top.
    "object",
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

/// One entity declaration's parser output. `extends` / `implements` are
/// the parent / interface ids picked up from
/// `class Foo extends Bar implements Baz1, Baz2` — they become real
/// relations once the caller commits the entity.
pub(super) struct EntityAction {
    pub(super) entity: Entity,
    pub(super) has_block: bool,
    /// `class A extends B` → `extends = ["B"]`. A generalization edge
    /// B ◁── A gets created by the commit step.
    pub(super) extends: Vec<String>,
    /// `class A implements I1, I2` → `implements = ["I1", "I2"]`.
    /// Dashed generalization edge per interface.
    pub(super) implements: Vec<String>,
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
        extends: Vec::new(),
        implements: Vec::new(),
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
        extends: Vec::new(),
        implements: Vec::new(),
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
    let (usymbol, body_kind) = match kw {
        // Class family.
        "abstract class" | "abstract" => {
            (USymbol::None, BodyKind::Compartment(ClassFamilyKind::Abstract))
        }
        "static class" | "metaclass" | "stereotype" | "dataclass" | "record" => {
            (USymbol::None, BodyKind::Compartment(ClassFamilyKind::Class))
        }
        "class" => (USymbol::None, BodyKind::Compartment(ClassFamilyKind::Class)),
        "interface" => (USymbol::None, BodyKind::Compartment(ClassFamilyKind::Interface)),
        "annotation" => (USymbol::None, BodyKind::Compartment(ClassFamilyKind::Annotation)),
        "protocol" => (USymbol::None, BodyKind::Compartment(ClassFamilyKind::Protocol)),
        "exception" => (USymbol::None, BodyKind::Compartment(ClassFamilyKind::Exception)),
        "enum" => (USymbol::None, BodyKind::Compartment(ClassFamilyKind::Enum)),
        "struct" => (USymbol::None, BodyKind::Compartment(ClassFamilyKind::Struct)),
        "entity" => (USymbol::None, BodyKind::Compartment(ClassFamilyKind::EntityShape)),
        // Object (instance-level) — name=value rows, no method compartment.
        "object" => (USymbol::None, BodyKind::Object),
        // Specials.
        "circle" => (USymbol::Interface, BodyKind::Plain),
        "diamond" => (USymbol::Diamond, BodyKind::Plain),
        "note" => return None, // handled by note-specific parsers
        // Desc family (Plain — no compartment).
        "component" => (USymbol::Component, BodyKind::Plain),
        "actor" => (USymbol::Actor, BodyKind::Plain),
        "usecase" => (USymbol::UseCase, BodyKind::Plain),
        "node" => (USymbol::Node, BodyKind::Plain),
        "database" => (USymbol::Database, BodyKind::Plain),
        "cloud" => (USymbol::Cloud, BodyKind::Plain),
        "queue" => (USymbol::Queue, BodyKind::Plain),
        "stack" => (USymbol::Stack, BodyKind::Plain),
        "storage" => (USymbol::Storage, BodyKind::Plain),
        "artifact" => (USymbol::Artifact, BodyKind::Plain),
        "agent" => (USymbol::Agent, BodyKind::Plain),
        "person" => (USymbol::Person, BodyKind::Plain),
        "collections" => (USymbol::Collections, BodyKind::Plain),
        "rectangle" => (USymbol::Rectangle, BodyKind::Plain),
        "card" => (USymbol::Card, BodyKind::Plain),
        "folder" => (USymbol::Folder, BodyKind::Plain),
        "frame" => (USymbol::Frame, BodyKind::Plain),
        "file" => (USymbol::File, BodyKind::Plain),
        "hexagon" => (USymbol::Hexagon, BodyKind::Plain),
        "action" => (USymbol::Action, BodyKind::Plain),
        "process" => (USymbol::Process, BodyKind::Plain),
        "label" => (USymbol::Label, BodyKind::Plain),
        "boundary" => (USymbol::Boundary, BodyKind::Plain),
        "control" => (USymbol::Control, BodyKind::Plain),
        "port" => (USymbol::Port, BodyKind::Plain),
        "portin" => (USymbol::PortIn, BodyKind::Plain),
        "portout" => (USymbol::PortOut, BodyKind::Plain),
        _ => (USymbol::None, BodyKind::Compartment(ClassFamilyKind::Class)),
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
    // Strip Java-style `extends Base` / `implements I1, I2` clauses
    // before the alias parser runs — both produce generalisation
    // edges that `commit_entity` adds on commit. `implements` may
    // carry a comma-separated list; `extends` typically a single
    // target but a comma-list is also accepted.
    let implements = pop_trailing_implements(&mut working);
    let extends = pop_trailing_extends(&mut working);
    let generic = pop_trailing_generic(&mut working);
    let (id, display) = parse_alias(working.trim())?;

    let kind_data = match body_kind {
        BodyKind::Compartment(kind) => EntityKindData::Compartment {
            kind,
            generic,
            fields: Vec::new(),
            methods: Vec::new(),
        },
        BodyKind::Object => EntityKindData::Object { fields: Vec::new() },
        BodyKind::Plain => EntityKindData::Plain {
            members: Vec::new(),
        },
    };

    Some(EntityAction {
        extends,
        implements,
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

/// Strip a trailing ` implements I1, I2, …` clause and return the
/// interface ids. The match is whitespace-bounded on the left (so
/// `class FooImplements` isn't mis-cut). Empty list when absent.
pub(super) fn pop_trailing_implements(rest: &mut String) -> Vec<String> {
    pop_trailing_clause(rest, "implements")
}

/// Strip a trailing ` extends B[, C…]` clause and return the parent
/// ids. PlantUML almost always carries a single target here but a
/// comma-list is harmless to accept.
pub(super) fn pop_trailing_extends(rest: &mut String) -> Vec<String> {
    pop_trailing_clause(rest, "extends")
}

fn pop_trailing_clause(rest: &mut String, kw: &str) -> Vec<String> {
    let trimmed = rest.trim_end();
    let pattern = format!(" {kw} ");
    let Some(idx) = trimmed.rfind(&pattern) else {
        return Vec::new();
    };
    let tail = &trimmed[idx + pattern.len()..];
    let ids: Vec<String> = tail
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.contains(char::is_whitespace))
        .collect();
    if ids.is_empty() {
        return Vec::new();
    }
    *rest = trimmed[..idx].to_string();
    ids
}
