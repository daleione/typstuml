//! Native class diagram parser.
//!
//! Hand-written line scanner covering the M0 subset of PlantUML's class
//! diagram syntax:
//!
//! - Entity declarations: `class Foo`, `interface I`, `abstract A`,
//!   `enum E`, `struct S`, `entity X`, etc., with optional generic
//!   `<T>`, `<<stereotype>>`, `#color`, alias `as`, and trailing `{ … }`
//!   member block.
//! - Member additions: `Foo : + bar()` and the inline form inside
//!   `class Foo { … }`.
//! - Relations: PlantUML's full arrow grammar with two heads, body
//!   style (solid `--` / dashed `..`), explicit direction
//!   (`-up->` / `-left->`), label, multiplicity, role, and stereotype.
//!
//! - Notes: `note left of Foo : body`, `note as Foo … end note`,
//!   `note "body" as Foo`, multi-line bodies. Anchored notes auto-create
//!   a dashed dependency relation between the note and its target.
//!
//! Out-of-M0 (warned and skipped):
//!   `package`, `namespace`, `together`, lollipop interfaces,
//!   association classes, `hide` / `show` filters, sprites, URL, link
//!   color, and the bulk of `skinparam` keys other than the small subset
//!   `codegen/class.rs::emit_skinparam_preamble` recognises.

use crate::diagnostics::{CompatMode, Diagnostic, Error, Level, Result};
use crate::ir::{
    ArrowHead, ClassDiagram, Diagram, Direction, Entity, EntityKind, LineStyle, Member, Relation,
    Skinparam, Visibility,
};
use crate::parser::lexer::{BodyLine, UmlBlock};

pub fn parse(block: &UmlBlock, compat: CompatMode) -> Result<(Diagram, Vec<Diagnostic>)> {
    let mut parser = Parser::new(&block.body, compat);
    parser.run()?;
    let mut diag = parser.diag;
    diag.name = block.name.clone();
    Ok((Diagram::Class(diag), parser.diagnostics))
}

struct Parser<'a> {
    lines: &'a [BodyLine],
    pos: usize,
    compat: CompatMode,
    diag: ClassDiagram,
    /// Stack of (entity_index, line_no) for `class A {` … `}` blocks.
    block_stack: Vec<(usize, usize)>,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Parser<'a> {
    fn new(lines: &'a [BodyLine], compat: CompatMode) -> Self {
        Self {
            lines,
            pos: 0,
            compat,
            diag: ClassDiagram::default(),
            block_stack: Vec::new(),
            diagnostics: Vec::new(),
        }
    }

    fn run(&mut self) -> Result<()> {
        while self.pos < self.lines.len() {
            let body_line = &self.lines[self.pos];
            self.pos += 1;
            let line_no = body_line.line;
            let raw = body_line.text.trim();

            if raw.is_empty() || is_comment(raw) {
                continue;
            }
            if is_skip_directive(raw) {
                continue;
            }
            if let Some(rest) = strip_prefix_keyword(raw, "skinparam") {
                self.handle_skinparam(rest, line_no);
                continue;
            }
            if let Some(rest) = strip_prefix_keyword(raw, "title") {
                let t = rest.trim();
                if !t.is_empty() {
                    self.diag.title = Some(t.to_string());
                }
                continue;
            }

            // Inside a `class A { … }` block?
            if !self.block_stack.is_empty() {
                if raw == "}" {
                    self.block_stack.pop();
                    continue;
                }
                self.handle_block_member(raw, line_no)?;
                continue;
            }

            // `note left of Foo : …`, `note "body" as N`, `note as N` …
            // `end note`. Handled before entity-decl so a literal `note`
            // can't be misread as a class keyword.
            if self.try_parse_note(raw, line_no)? {
                continue;
            }

            // Entity declaration: `class A`, `interface I`, `abstract X`, etc.
            if let Some(action) = parse_entity_decl(raw, line_no) {
                self.commit_entity(action);
                continue;
            }

            // `<entity> : <member>` — add a member to a previously declared
            // entity (or auto-create it).
            if let Some((id, member_text)) = split_member_line(raw) {
                self.add_member(&id, member_text, line_no);
                continue;
            }

            // Relation: `A --|> B`, `A *-- "*" B : owns`, etc.
            if let Some(rel) = parse_relation(raw, line_no) {
                self.commit_relation(rel);
                continue;
            }

            // `package "Foo" {` / `namespace foo {` — M0 ignores the cluster
            // structure (containers stays empty) but still consumes the
            // matching `}` so members inside aren't misparsed.
            if is_container_open(raw) {
                self.block_stack.push((usize::MAX, line_no));
                continue;
            }

            self.unsupported(raw, line_no)?;
        }

        // Best-effort: any unterminated block is flagged but doesn't drop
        // the work we already collected.
        while let Some((_, line)) = self.block_stack.pop() {
            self.warn_or_err(
                Level::Warning,
                Some(line),
                "unterminated `{` block (missing `}`)".to_string(),
            )?;
        }
        Ok(())
    }

    fn commit_entity(&mut self, action: EntityAction) {
        let EntityAction { entity, has_block } = action;
        let line_no = entity.line;
        let idx = self.upsert_entity(entity);
        if has_block {
            self.block_stack.push((idx, line_no));
        }
    }

    fn upsert_entity(&mut self, entity: Entity) -> usize {
        if let Some(i) = self.diag.entities.iter().position(|e| e.id == entity.id) {
            // Merge: prefer the new declaration's kind / generic /
            // stereotype if it has them, and append nothing else (members
            // are added line-by-line so they don't double up).
            let existing = &mut self.diag.entities[i];
            if existing.kind == EntityKind::Class && entity.kind != EntityKind::Class {
                existing.kind = entity.kind;
            }
            if existing.generic.is_none() && entity.generic.is_some() {
                existing.generic = entity.generic;
            }
            if existing.stereotype.is_none() && entity.stereotype.is_some() {
                existing.stereotype = entity.stereotype;
            }
            if existing.fill.is_none() && entity.fill.is_some() {
                existing.fill = entity.fill;
            }
            i
        } else {
            self.diag.entities.push(entity);
            self.diag.entities.len() - 1
        }
    }

    fn add_member(&mut self, id: &str, body: &str, line_no: usize) {
        let idx = match self.diag.entities.iter().position(|e| e.id == id) {
            Some(i) => i,
            None => {
                let entity = Entity {
                    kind: EntityKind::Class,
                    id: id.to_string(),
                    display: id.to_string(),
                    generic: None,
                    stereotype: None,
                    fields: Vec::new(),
                    methods: Vec::new(),
                    body: None,
                    fill: None,
                    line: line_no,
                };
                self.diag.entities.push(entity);
                self.diag.entities.len() - 1
            }
        };
        let member = parse_member(body, line_no);
        if is_method_signature(&member.body) {
            self.diag.entities[idx].methods.push(member);
        } else {
            self.diag.entities[idx].fields.push(member);
        }
    }

    fn handle_block_member(&mut self, raw: &str, line_no: usize) -> Result<()> {
        let &(idx, _) = self.block_stack.last().expect("stack non-empty");
        if idx == usize::MAX {
            // Inside a container (package/namespace) — for M0 we treat
            // the body as if it were at the top level (re-dispatch).
            // Simplest: push the line back onto the queue. Instead of
            // touching `pos`, we recurse on the supported forms inline.
            if let Some(action) = parse_entity_decl(raw, line_no) {
                self.commit_entity(action);
                return Ok(());
            }
            if let Some((id, body)) = split_member_line(raw) {
                self.add_member(&id, body, line_no);
                return Ok(());
            }
            if let Some(rel) = parse_relation(raw, line_no) {
                self.commit_relation(rel);
                return Ok(());
            }
            if is_container_open(raw) {
                self.block_stack.push((usize::MAX, line_no));
                return Ok(());
            }
            return self.unsupported(raw, line_no);
        }
        // Inline member inside `class A { + foo() }`.
        let member = parse_member(raw, line_no);
        let entity = &mut self.diag.entities[idx];
        if is_method_signature(&member.body) {
            entity.methods.push(member);
        } else {
            entity.fields.push(member);
        }
        Ok(())
    }

    fn commit_relation(&mut self, rel: Relation) {
        // Auto-create endpoints if not yet declared (PlantUML behavior).
        for id in [&rel.from, &rel.to] {
            if !self.diag.entities.iter().any(|e| e.id == *id) {
                self.diag.entities.push(Entity {
                    kind: EntityKind::Class,
                    id: id.clone(),
                    display: id.clone(),
                    generic: None,
                    stereotype: None,
                    fields: Vec::new(),
                    methods: Vec::new(),
                    body: None,
                    fill: None,
                    line: rel.line,
                });
            }
        }
        self.diag.relations.push(rel);
    }

    /// Try to parse `raw` as a note declaration. Returns `Ok(true)` iff
    /// the line was consumed (possibly along with subsequent body lines
    /// up to `end note`). Returns `Ok(false)` if `raw` is not a `note`
    /// directive — the caller falls through to other parsers.
    fn try_parse_note(&mut self, raw: &str, line_no: usize) -> Result<bool> {
        let Some(rest) = strip_prefix_keyword(raw, "note") else {
            return Ok(false);
        };
        let rest = rest.trim();

        // Anchored: `note <side> of <target> [: body]`.
        if let Some((side, target, inline_body)) = parse_anchored_note_decl(rest) {
            let id = format!("__note_{line_no}");
            let body = match inline_body {
                Some(b) => b,
                None => self.collect_note_body(),
            };
            self.push_note(id.clone(), body, line_no);
            self.commit_relation(Relation {
                from: id,
                to: target,
                head_from: ArrowHead::None,
                head_to: ArrowHead::None,
                line_style: LineStyle::Dashed,
                direction: Some(side_to_direction(side)),
                label: None,
                mult_from: None,
                mult_to: None,
                role_from: None,
                role_to: None,
                stereotype: None,
                color: None,
                line: line_no,
            });
            return Ok(true);
        }

        // Quoted standalone: `note "body" [as id]`.
        if let Some((body, id)) = parse_quoted_note_decl(rest) {
            let id = id.unwrap_or_else(|| format!("__note_{line_no}"));
            self.push_note(id, body, line_no);
            return Ok(true);
        }

        // Freestanding multi-line: `note as id` ... `end note`, or a bare
        // alias (`note id`).
        if let Some(id) = parse_freestanding_note_decl(rest) {
            let body = self.collect_note_body();
            self.push_note(id, body, line_no);
            return Ok(true);
        }

        // `note` keyword recognized but the rest didn't match any known
        // form. Warn and consume so we don't mis-parse the remainder.
        self.unsupported(raw, line_no)?;
        Ok(true)
    }

    fn collect_note_body(&mut self) -> String {
        let mut lines = Vec::new();
        while self.pos < self.lines.len() {
            let line = &self.lines[self.pos];
            self.pos += 1;
            let trimmed = line.text.trim();
            if trimmed.eq_ignore_ascii_case("end note")
                || trimmed.eq_ignore_ascii_case("endnote")
            {
                return lines.join("\n");
            }
            // Source-side indentation is structural, not rendered. Trim it
            // so users can indent the body for readability without that
            // indentation showing up in the painted note.
            lines.push(trimmed.to_string());
        }
        lines.join("\n")
    }

    fn push_note(&mut self, id: String, body: String, line_no: usize) {
        self.diag.entities.push(Entity {
            kind: EntityKind::Note,
            id: id.clone(),
            display: id,
            generic: None,
            stereotype: None,
            fields: Vec::new(),
            methods: Vec::new(),
            body: Some(body),
            fill: None,
            line: line_no,
        });
    }

    fn handle_skinparam(&mut self, rest: &str, line_no: usize) {
        let rest = rest.trim();
        if rest.is_empty() {
            return;
        }
        let (key, value) = match rest.split_once(char::is_whitespace) {
            Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
            None => (rest.to_string(), String::new()),
        };
        self.diag.skinparams.push(Skinparam {
            key,
            value,
            line: line_no,
        });
    }

    fn unsupported(&mut self, raw: &str, line_no: usize) -> Result<()> {
        let head: String = raw
            .split_whitespace()
            .next()
            .unwrap_or("")
            .chars()
            .take(40)
            .collect();
        self.warn_or_err(
            Level::Warning,
            Some(line_no),
            format!("unrecognized class syntax (starts with {head:?})"),
        )
    }

    fn warn_or_err(&mut self, level: Level, line: Option<usize>, message: String) -> Result<()> {
        if self.compat == CompatMode::Strict && level == Level::Warning {
            return Err(Error::Parse {
                line: line.unwrap_or(0),
                message,
            });
        }
        if self.compat == CompatMode::Loose {
            return Ok(());
        }
        self.diagnostics.push(Diagnostic {
            level,
            line,
            message,
        });
        Ok(())
    }
}

// ---- Per-line parsers -----------------------------------------------------

fn is_comment(line: &str) -> bool {
    line.starts_with('\'') || line.starts_with("/'")
}

fn is_skip_directive(line: &str) -> bool {
    const HEADS: &[&str] = &[
        "@startuml",
        "@enduml",
        "hide ",
        "show ",
        "header ",
        "footer ",
        "!theme",
        "!pragma",
        "!define",
        "!include",
        "scale ",
        "left to right",
        "top to bottom",
        "set namespaceSeparator",
        "set separator",
    ];
    HEADS
        .iter()
        .any(|h| line == h.trim() || line.starts_with(h))
}

fn strip_prefix_keyword<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(keyword)?;
    match rest.chars().next() {
        None => Some(rest),
        Some(c) if c.is_whitespace() => Some(rest),
        _ => None,
    }
}

const ENTITY_KEYWORDS: &[&str] = &[
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

struct EntityAction {
    entity: Entity,
    has_block: bool,
}

/// Try to parse `raw` as an entity declaration. Returns `None` if the
/// line doesn't start with an entity keyword.
fn parse_entity_decl(raw: &str, line_no: usize) -> Option<EntityAction> {
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
    let stereotype = pop_trailing_stereotype(&mut working);
    let generic = pop_trailing_generic(&mut working);
    let (id, display) = parse_alias(working.trim())?;

    Some(EntityAction {
        entity: Entity {
            kind,
            id,
            display,
            generic,
            stereotype,
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
fn pop_trailing_color(rest: &mut String) -> Option<String> {
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

/// Strip a trailing `<<stereotype>>` block.
fn pop_trailing_stereotype(rest: &mut String) -> Option<String> {
    let trimmed = rest.trim_end();
    let body = trimmed.strip_suffix(">>")?;
    let lt_idx = body.rfind("<<")?;
    let inner = body[lt_idx + 2..].trim().to_string();
    *rest = body[..lt_idx].trim_end().to_string();
    Some(inner)
}

/// Strip a trailing `<T, U>` generic parameter list, balancing nested `<>`
/// so `Map<K, List<V>>` doesn't get mis-cut.
fn pop_trailing_generic(rest: &mut String) -> Option<String> {
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

/// Parse `<side> of <target> [: body]` from the body of a `note`
/// directive. Returns `(side_keyword, target_id, optional_inline_body)`.
fn parse_anchored_note_decl(rest: &str) -> Option<(&'static str, String, Option<String>)> {
    const SIDES: &[&str] = &["left", "right", "top", "bottom", "above", "below"];
    for side in SIDES {
        let after_side = match strip_prefix_keyword(rest, side) {
            Some(s) => s.trim_start(),
            None => continue,
        };
        let after_of = match strip_prefix_keyword(after_side, "of") {
            Some(s) => s.trim_start(),
            None => continue,
        };
        let (target_part, body) = match after_of.find(':') {
            Some(idx) => (
                after_of[..idx].trim(),
                Some(after_of[idx + 1..].trim().to_string()),
            ),
            None => (after_of.trim(), None),
        };
        if target_part.is_empty() {
            return None;
        }
        // Target may be quoted ("Foo Bar") or a bare identifier.
        let target = if let Some((quoted, _)) = strip_leading_quoted(target_part) {
            quoted
        } else {
            target_part
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string()
        };
        if target.is_empty() {
            return None;
        }
        return Some((side, target, body));
    }
    None
}

/// Parse `"body" [as id]` — a standalone single-line note.
fn parse_quoted_note_decl(rest: &str) -> Option<(String, Option<String>)> {
    let (body, after) = strip_leading_quoted(rest.trim())?;
    let after = after.trim_start();
    if after.is_empty() {
        return Some((body, None));
    }
    let after_as = strip_prefix_keyword(after, "as")?.trim_start();
    let id = after_as.split_whitespace().next()?.to_string();
    if id.is_empty() {
        return None;
    }
    Some((body, Some(id)))
}

/// Parse `as id` or a bare `id` — a freestanding note whose body
/// follows on subsequent lines until `end note`.
fn parse_freestanding_note_decl(rest: &str) -> Option<String> {
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }
    if let Some(after_as) = strip_prefix_keyword(rest, "as") {
        let id = after_as.trim_start().split_whitespace().next()?.to_string();
        if id.is_empty() {
            return None;
        }
        return Some(id);
    }
    let id = rest.split_whitespace().next()?.to_string();
    if id.is_empty() {
        return None;
    }
    Some(id)
}

fn side_to_direction(side: &str) -> Direction {
    match side {
        "left" => Direction::Left,
        "right" => Direction::Right,
        "top" | "above" => Direction::Up,
        "bottom" | "below" => Direction::Down,
        _ => Direction::Right,
    }
}

/// Parse `Name as alias`, `"Display Name" as alias`, or a bare name.
fn parse_alias(rest: &str) -> Option<(String, String)> {
    if rest.is_empty() {
        return None;
    }
    if let Some((quoted, after)) = strip_leading_quoted(rest) {
        let after = after.trim();
        if let Some(alias) = strip_prefix_keyword(after, "as").map(str::trim) {
            if !alias.is_empty() {
                return Some((alias.to_string(), quoted));
            }
        }
        return Some((quoted.clone(), quoted));
    }
    let mut parts = rest.splitn(2, char::is_whitespace);
    let first = parts.next()?;
    let tail = parts.next().unwrap_or("").trim_start();
    if let Some(after_as) = strip_prefix_keyword(tail, "as").map(str::trim) {
        // `Foo as Bar` — id is the alias, display is the original name.
        // `Foo as "Long Foo"` — id is `Foo`, display is the quoted form.
        if let Some((quoted, _)) = strip_leading_quoted(after_as) {
            return Some((first.to_string(), quoted));
        }
        let alias = after_as.split_whitespace().next()?.to_string();
        return Some((alias, first.to_string()));
    }
    Some((first.to_string(), first.to_string()))
}

fn strip_leading_quoted(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    if !s.starts_with('"') {
        return None;
    }
    let after = &s[1..];
    let close = after.find('"')?;
    let inner = after[..close].to_string();
    Some((inner, &after[close + 1..]))
}

/// `Foo : + bar()` → `Some(("Foo", "+ bar()"))`. Filters lines that look
/// like relations (those have an arrow) so we don't mis-classify
/// `A -- B : associate` as a member.
fn split_member_line(raw: &str) -> Option<(String, &str)> {
    if find_arrow_span(raw).is_some() {
        return None;
    }
    let colon = raw.find(':')?;
    let name = raw[..colon].trim();
    if name.is_empty() {
        return None;
    }
    // The id must be a single token (possibly quoted).
    let id = if name.starts_with('"') {
        let close = name[1..].find('"')? + 1;
        if close + 1 != name.len() {
            return None;
        }
        name[1..close].to_string()
    } else if name.contains(char::is_whitespace) {
        return None;
    } else {
        name.to_string()
    };
    Some((id, raw[colon + 1..].trim()))
}

fn parse_member(raw: &str, line_no: usize) -> Member {
    let mut s = raw.trim().to_string();
    let mut is_static = false;
    let mut is_abstract = false;
    let mut visibility = Visibility::None;
    // Both `+ {static} foo()` and `{static} + foo()` are valid PlantUML;
    // loop until neither prefix matches so visibility / modifiers can
    // appear in either order.
    loop {
        if let Some((modifier, rest)) = strip_brace_modifier(&s) {
            // `{classifier}` is PUML's spelling for "owned by the class
            // (not the instance)" — same semantics as `{static}`. Treat
            // them identically rather than dropping the modifier.
            if modifier == "static" || modifier == "classifier" {
                is_static = true;
            } else if modifier == "abstract" {
                is_abstract = true;
            }
            s = rest.trim().to_string();
            continue;
        }
        if visibility == Visibility::None {
            if let Some(c) = s.chars().next() {
                if let Some(v) = Visibility::from_char(c) {
                    visibility = v;
                    s = s[c.len_utf8()..].trim_start().to_string();
                    continue;
                }
            }
        }
        break;
    }
    Member {
        visibility,
        is_static,
        is_abstract,
        body: s,
        line: line_no,
    }
}

/// `{static} foo()` → `Some(("static", " foo()"))`. Returns the modifier
/// keyword and the remainder.
fn strip_brace_modifier(s: &str) -> Option<(String, String)> {
    let trimmed = s.trim_start();
    let inner = trimmed.strip_prefix('{')?;
    let close = inner.find('}')?;
    let modifier = inner[..close].trim().to_ascii_lowercase();
    if modifier != "static" && modifier != "abstract" && modifier != "classifier" {
        return None;
    }
    Some((modifier, inner[close + 1..].to_string()))
}

/// Heuristic: a member is a method if it contains a balanced pair of
/// parentheses, a field otherwise.
fn is_method_signature(body: &str) -> bool {
    body.contains('(') && body.contains(')')
}

/// True if `raw` opens a `package` / `namespace` / `together` block.
/// M0 swallows the body without recording the container — see
/// `docs/class-diagram-design.md` for the M1 plan that actually
/// populates `ClassDiagram.containers`.
fn is_container_open(raw: &str) -> bool {
    for kw in ["package", "namespace", "together", "folder", "frame", "node", "cloud"] {
        if let Some(rest) = strip_prefix_keyword(raw, kw) {
            if rest.trim_end().ends_with('{') {
                return true;
            }
        }
    }
    false
}

// ---- Arrow / relation parsing ---------------------------------------------

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
fn find_arrow_span(line: &str) -> Option<(usize, usize)> {
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
fn match_direction_keyword(s: &[u8]) -> Option<usize> {
    let text = std::str::from_utf8(s).ok()?;
    for kw in ["right", "left", "down", "up", "ri", "le", "do"] {
        if text.len() >= kw.len() && text[..kw.len()].eq_ignore_ascii_case(kw) {
            return Some(kw.len());
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

fn parse_relation(raw: &str, line_no: usize) -> Option<Relation> {
    let (start, end) = find_arrow_span(raw)?;
    // Arrow string itself.
    let arrow = &raw[start..end];
    // Left side: from-id (and optional multiplicity / role / qualifier).
    let left = raw[..start].trim_end();
    let right = raw[end..].trim_start();

    let (label, right) = match right.find(':') {
        Some(i) => (Some(right[i + 1..].trim().to_string()), right[..i].trim_end()),
        None => (None, right),
    };

    let (from_id, mult_from, role_from) = parse_endpoint_left(left)?;
    let (to_id, mult_to, role_to) = parse_endpoint_right(right)?;

    let (head_from, head_to, line_style, direction) = decode_arrow(arrow);

    Some(Relation {
        from: from_id,
        to: to_id,
        head_from,
        head_to,
        line_style,
        direction,
        label,
        mult_from,
        mult_to,
        role_from,
        role_to,
        stereotype: None,
        color: None,
        line: line_no,
    })
}

fn parse_endpoint_left(s: &str) -> Option<(String, Option<String>, Option<String>)> {
    // Right-most pieces of `s` are: optional `"mult"`, optional `/role`,
    // and the id sits before them.
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Pop trailing `"mult"`.
    let (id_part, mult) = pop_trailing_quoted(s);
    let id_part = id_part.trim();
    // No further role parsing on the left for M0 (PlantUML's left-side
    // role uses a different position than right-side; treat as label).
    let (id, role) = pop_trailing_role(id_part);
    if id.is_empty() {
        return None;
    }
    Some((unquote(id), mult, role))
}

fn parse_endpoint_right(s: &str) -> Option<(String, Option<String>, Option<String>)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // Leading `"mult"`.
    let (rest, mult) = pop_leading_quoted(s);
    let rest = rest.trim();
    // Trailing `/role` (rare; PlantUML's `/role` syntax for the right side).
    let (id, role) = pop_trailing_role(rest);
    if id.is_empty() {
        return None;
    }
    Some((unquote(id), mult, role))
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

fn unquote(s: &str) -> String {
    if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Decompose an arrow token (e.g. `-up->`, `<|..`, `*--`, `<|--`) into
/// its head decorations, line style, and direction hint.
fn decode_arrow(arrow: &str) -> (ArrowHead, ArrowHead, LineStyle, Option<Direction>) {
    // Strip `[…]` color/style annotations.
    let mut s = String::new();
    let bytes = arrow.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'[' {
            while i < bytes.len() && bytes[i] != b']' {
                i += 1;
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
        _ => ArrowHead::None,
    }
}

// ---- Tests ----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn block(body: &[&str]) -> UmlBlock {
        UmlBlock {
            start_line: 1,
            kind_tag: "uml".into(),
            name: None,
            body: body
                .iter()
                .enumerate()
                .map(|(i, t)| BodyLine {
                    line: i + 1,
                    text: (*t).to_string(),
                })
                .collect(),
        }
    }

    fn parse_ok(body: &[&str]) -> ClassDiagram {
        let (diagram, _) = parse(&block(body), CompatMode::Warn).expect("parse ok");
        match diagram {
            Diagram::Class(c) => c,
            _ => panic!("expected class diagram"),
        }
    }

    #[test]
    fn parses_class_with_inline_members() {
        let c = parse_ok(&[
            "class Foo {",
            "  + name: String",
            "  - count: int",
            "  + getName(): String",
            "}",
        ]);
        assert_eq!(c.entities.len(), 1);
        let foo = &c.entities[0];
        assert_eq!(foo.id, "Foo");
        assert_eq!(foo.kind, EntityKind::Class);
        assert_eq!(foo.fields.len(), 2);
        assert_eq!(foo.methods.len(), 1);
        assert_eq!(foo.fields[0].visibility, Visibility::Public);
        assert_eq!(foo.fields[1].visibility, Visibility::Private);
        assert_eq!(foo.methods[0].body, "getName(): String");
    }

    #[test]
    fn parses_inheritance() {
        let c = parse_ok(&["class A", "class B", "B --|> A"]);
        assert_eq!(c.relations.len(), 1);
        let r = &c.relations[0];
        assert_eq!(r.from, "B");
        assert_eq!(r.to, "A");
        assert_eq!(r.head_from, ArrowHead::None);
        assert_eq!(r.head_to, ArrowHead::TriangleOpen);
        assert_eq!(r.line_style, LineStyle::Solid);
    }

    #[test]
    fn parses_realization_dashed() {
        let c = parse_ok(&["class A", "interface I", "A ..|> I"]);
        let r = &c.relations[0];
        assert_eq!(r.head_to, ArrowHead::TriangleOpen);
        assert_eq!(r.line_style, LineStyle::Dashed);
    }

    #[test]
    fn parses_composition_with_mult_and_label() {
        let c = parse_ok(&[r#"A "1" *-- "*" B : owns"#]);
        let r = &c.relations[0];
        assert_eq!(r.from, "A");
        assert_eq!(r.to, "B");
        assert_eq!(r.head_from, ArrowHead::DiamondFilled);
        assert_eq!(r.mult_from.as_deref(), Some("1"));
        assert_eq!(r.mult_to.as_deref(), Some("*"));
        assert_eq!(r.label.as_deref(), Some("owns"));
    }

    #[test]
    fn parses_member_add_line() {
        let c = parse_ok(&["class A", "A : + foo()"]);
        assert_eq!(c.entities[0].methods.len(), 1);
        assert_eq!(c.entities[0].methods[0].body, "foo()");
    }

    #[test]
    fn parses_static_and_abstract_modifiers() {
        let c = parse_ok(&[
            "class A {",
            "  {static} count: int",
            "  {abstract} render(): void",
            "}",
        ]);
        let a = &c.entities[0];
        assert_eq!(a.fields.len(), 1);
        assert!(a.fields[0].is_static);
        assert_eq!(a.methods.len(), 1);
        assert!(a.methods[0].is_abstract);
    }

    #[test]
    fn parses_generic_and_stereotype() {
        let c = parse_ok(&[r#"class Repo<T> <<Service>> #LightBlue"#]);
        let e = &c.entities[0];
        assert_eq!(e.id, "Repo");
        assert_eq!(e.generic.as_deref(), Some("T"));
        assert_eq!(e.stereotype.as_deref(), Some("Service"));
        assert_eq!(e.fill.as_deref(), Some("#LightBlue"));
    }

    #[test]
    fn parses_alias() {
        let c = parse_ok(&[r#"class "Long Name" as Foo"#]);
        let e = &c.entities[0];
        assert_eq!(e.id, "Foo");
        assert_eq!(e.display, "Long Name");
    }

    #[test]
    fn parses_alias_unquoted() {
        // `class Foo as Bar` — id is the alias, display keeps the original
        // name. Pre-fix, both id and display became `Bar`.
        let c = parse_ok(&["class Foo as Bar"]);
        let e = &c.entities[0];
        assert_eq!(e.id, "Bar");
        assert_eq!(e.display, "Foo");
    }

    #[test]
    fn parses_alias_with_quoted_display() {
        // `class Foo as "Long Foo"` — id stays `Foo`, display is the quoted form.
        let c = parse_ok(&[r#"class Foo as "Long Foo""#]);
        let e = &c.entities[0];
        assert_eq!(e.id, "Foo");
        assert_eq!(e.display, "Long Foo");
    }

    #[test]
    fn parses_package_visibility() {
        let c = parse_ok(&[
            "class A {",
            "  ~ helper(): void",
            "}",
        ]);
        let m = &c.entities[0].methods[0];
        assert_eq!(m.visibility, Visibility::Package);
        assert_eq!(m.body, "helper(): void");
    }

    #[test]
    fn classifier_modifier_maps_to_static() {
        let c = parse_ok(&[
            "class A {",
            "  {classifier} factory(): A",
            "}",
        ]);
        let m = &c.entities[0].methods[0];
        assert!(m.is_static, "{{classifier}} should set is_static");
    }

    #[test]
    fn auto_creates_unknown_endpoint() {
        let c = parse_ok(&["class A", "A --> B"]);
        assert_eq!(c.entities.len(), 2);
        assert!(c.entities.iter().any(|e| e.id == "B"));
    }

    #[test]
    fn unrecognized_warns() {
        let (_d, diags) = parse(&block(&["frobnicate"]), CompatMode::Warn).unwrap();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, Level::Warning);
    }

    #[test]
    fn parses_anchored_note_inline() {
        let c = parse_ok(&["class Foo", "note left of Foo : just a hint"]);
        // Entities: Foo + the auto-generated note.
        assert_eq!(c.entities.len(), 2);
        let note = c.entities.iter().find(|e| e.kind == EntityKind::Note).unwrap();
        assert_eq!(note.body.as_deref(), Some("just a hint"));
        // Auto-generated id starts with `__note_`.
        assert!(note.id.starts_with("__note_"));
        // Auto-relation: dashed, no heads, direction Left.
        assert_eq!(c.relations.len(), 1);
        let r = &c.relations[0];
        assert_eq!(r.from, note.id);
        assert_eq!(r.to, "Foo");
        assert_eq!(r.line_style, LineStyle::Dashed);
        assert_eq!(r.head_from, ArrowHead::None);
        assert_eq!(r.head_to, ArrowHead::None);
        assert_eq!(r.direction, Some(Direction::Left));
    }

    #[test]
    fn parses_anchored_note_multiline() {
        let c = parse_ok(&[
            "class Foo",
            "note right of Foo",
            "  first line",
            "  second line",
            "end note",
        ]);
        let note = c.entities.iter().find(|e| e.kind == EntityKind::Note).unwrap();
        let body = note.body.as_deref().unwrap();
        assert!(body.contains("first line"));
        assert!(body.contains("second line"));
        assert_eq!(c.relations[0].direction, Some(Direction::Right));
    }

    #[test]
    fn parses_quoted_note_with_alias() {
        let c = parse_ok(&[
            "class Foo",
            "note \"hello world\" as N1",
            "N1 .. Foo",
        ]);
        let note = c.entities.iter().find(|e| e.id == "N1").unwrap();
        assert_eq!(note.kind, EntityKind::Note);
        assert_eq!(note.body.as_deref(), Some("hello world"));
        // User-written N1 .. Foo should produce one relation; no auto-rel.
        assert_eq!(c.relations.len(), 1);
        let r = &c.relations[0];
        assert_eq!(r.from, "N1");
        assert_eq!(r.to, "Foo");
        assert_eq!(r.line_style, LineStyle::Dashed);
    }

    #[test]
    fn parses_freestanding_note_block() {
        let c = parse_ok(&[
            "note as N1",
            "  body line",
            "end note",
        ]);
        let note = &c.entities[0];
        assert_eq!(note.kind, EntityKind::Note);
        assert_eq!(note.id, "N1");
        assert_eq!(note.body.as_deref().unwrap().trim(), "body line");
        // No auto relation for freestanding form.
        assert!(c.relations.is_empty());
    }
}
