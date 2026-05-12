//! Native class diagram parser.
//!
//! Hand-written line scanner covering a broad subset of PlantUML's
//! class diagram syntax:
//!
//! - Entity declarations: `class Foo`, `interface I`, `abstract A`,
//!   `enum E`, `struct S`, `entity X`, `protocol P`, `annotation A`,
//!   `exception X`, lollipop `() Foo`, etc., with optional generic
//!   `<T>`, `<<stereotype>>` (including custom marker
//!   `<<(L, #color) text>>`), `#color`, alias `as`, and trailing
//!   `{ … }` member block.
//! - Member additions: `Foo : + bar()` and the inline form inside
//!   `class Foo { … }`. Supports `{static}` / `{abstract}` /
//!   `{classifier}` modifiers and the `+` / `-` / `#` / `~`
//!   visibility glyphs.
//! - Relations: PlantUML's full arrow grammar with two heads, body
//!   style (solid `--` / dashed `..`), explicit direction
//!   (`-up->` / `-left->`), label, multiplicity, role, member port
//!   (`A::field`), couple endpoints (`(A, B) .. C`), and inline
//!   color (`-[#red]->`).
//! - Notes: `note left of Foo : body`, `note over A, B`, `note on link`,
//!   `note "body" as N`, freestanding `note as N … end note`. Anchored
//!   notes auto-create a dashed dependency relation between the note
//!   and its target.
//! - Containers: `package`, `namespace`, `together`, `folder`, `frame`,
//!   `node`, `cloud` — including nesting.
//! - Global visibility filters: `hide` / `show` of `circle`,
//!   `stereotype`, `members`, `methods`, `fields` / `attributes`.
//! - `!theme <name>`, `skinparam`, `title`, `left to right direction`,
//!   `top to bottom direction`.

mod container;
mod entity;
mod member;
mod note;
mod relation;
mod util;

use crate::diagnostics::{CompatMode, Diagnostic, Error, Level, Result};
use crate::ir::{
    ArrowHead, ClassFamilyKind, Container, CucaDiagram, Diagram, Entity, EntityKindData,
    LayoutDirection, LineStyle, Member, Relation, Skinparam, USymbol,
};
use crate::parser::lexer::{BodyLine, UmlBlock};

use self::container::{ContainerOpen, parse_container_open};
use self::entity::{EntityAction, parse_entity_decl, parse_inline_shorthand, parse_lollipop_decl};
use self::member::{is_method_signature, parse_member, split_member_line};
use self::note::{
    parse_anchored_note_decl, parse_freestanding_note_decl, parse_note_on_link_decl,
    parse_note_over_decl, parse_quoted_note_decl, side_to_direction,
};
use self::relation::parse_relation;
use self::util::{is_comment, is_skip_directive, parse_annotation, strip_prefix_keyword};

pub fn parse(block: &UmlBlock, compat: CompatMode) -> Result<(Diagram, Vec<Diagnostic>)> {
    let mut parser = Parser::new(&block.body, compat);
    parser.run()?;
    let mut diag = parser.diag;
    diag.name = block.name.clone();
    Ok((Diagram::Cuca(diag), parser.diagnostics))
}

struct Parser<'a> {
    lines: &'a [BodyLine],
    pos: usize,
    compat: CompatMode,
    diag: CucaDiagram,
    /// Frame stack for nested `{ … }` blocks. Both `class A { … }`
    /// (entity members) and `package "X" { … }` (cluster children) push
    /// here; the variant tells `handle_block_member` how to dispatch.
    block_stack: Vec<(BlockFrame, usize)>,
    diagnostics: Vec<Diagnostic>,
    /// Java-style `@Entity` / `@Table(name="x")` annotations seen since
    /// the last declaration. Attached to the next entity (as additional
    /// stereotype lines) or member (prepended to its body) and then
    /// drained. Multiple annotations stack in source order.
    pending_annotations: Vec<String>,
}

#[derive(Copy, Clone, Debug)]
enum BlockFrame {
    /// Inside `class A { … }` — member lines go to `entities[idx]`.
    Entity(usize),
    /// Inside `package "X" { … }` / `namespace foo { … }` — declared
    /// entities and nested containers register as children of
    /// `containers[idx]`.
    Container(usize),
}

impl<'a> Parser<'a> {
    fn new(lines: &'a [BodyLine], compat: CompatMode) -> Self {
        Self {
            lines,
            pos: 0,
            compat,
            diag: CucaDiagram::default(),
            block_stack: Vec::new(),
            diagnostics: Vec::new(),
            pending_annotations: Vec::new(),
        }
    }

    /// Drain pending annotations to a single newline-joined string. Empty
    /// list returns `None`. PlantUML semantics — annotations collected
    /// here render via the stereotype slot.
    fn take_annotations(&mut self) -> Option<String> {
        if self.pending_annotations.is_empty() {
            return None;
        }
        let joined = self.pending_annotations.join("\\n");
        self.pending_annotations.clear();
        Some(joined)
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
            // Java-style annotation lines like `@Entity` or
            // `@Table(name="orders")`. Accumulate; commit_entity /
            // add_member drain on the next concrete declaration.
            // Stays out of the way of `@startuml` / `@enduml` (handled
            // by the lexer) and `@unlinked` (handled by hide/show).
            if let Some(name) = parse_annotation(raw) {
                self.pending_annotations.push(name);
                continue;
            }
            // Layout direction overrides.
            if raw.starts_with("left to right direction")
                || raw == "left to right"
            {
                self.diag.direction = LayoutDirection::LeftToRight;
                continue;
            }
            if raw.starts_with("top to bottom direction") || raw == "top to bottom" {
                self.diag.direction = LayoutDirection::TopToBottom;
                continue;
            }
            // `!theme <name>` — captured as a synthetic skinparam so
            // codegen can expand it into the theme's preset values.
            if let Some(rest) = raw.strip_prefix("!theme") {
                let name = rest.trim().split_whitespace().next().unwrap_or("");
                if !name.is_empty() {
                    self.diag.skinparams.push(Skinparam {
                        key: "theme".to_string(),
                        value: name.to_string(),
                        line: line_no,
                    });
                }
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
            if self.try_parse_hide_show(raw) {
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

            // Lollipop: `() Foo` or `() "Display Foo" as Foo`.
            if let Some(action) = parse_lollipop_decl(raw, line_no) {
                self.commit_entity(action);
                continue;
            }

            // Inline shorthand (M6): `[Foo]` / `(Foo)` / `:Foo:`.
            // Must come BEFORE relation parsing so `[Foo] --> [Bar]`
            // doesn't get torn apart by the arrow scanner. Lollipop
            // parsing above already handles `() Foo` so the `(…)` case
            // here is unambiguous.
            if let Some(action) = parse_inline_shorthand(raw, line_no) {
                self.commit_entity(action);
                continue;
            }

            // `package "Foo" {` / `namespace foo {` / `together { … }` /
            // `database "Cluster" {` / `node "Box" {` etc. Container-
            // open must beat entity-decl because keywords like
            // `database`, `node`, `cloud`, `component` are both
            // container-capable AND valid leaf keywords — the trailing
            // `{` disambiguates. `class Foo { … }` (entity-with-members
            // block) doesn't trigger this branch because `class` is
            // not in the container whitelist.
            if let Some(open) = parse_container_open(raw) {
                self.commit_container(open, line_no);
                continue;
            }

            // Entity declaration: `class A`, `interface I`, `abstract X`,
            // `component Foo`, `actor Bob`, `database X`, etc.
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
            if let Some(rp) = parse_relation(raw, line_no) {
                self.commit_relation_with_hints(rp.rel, rp.from_lollipop, rp.to_lollipop);
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
        let EntityAction { mut entity, has_block } = action;
        // Java-style annotations accumulated since the last declaration
        // attach to this entity. Render path is the stereotype slot,
        // so multi-annotation classes (`@Entity\n@Table(...)`) show
        // stacked above the name. If the source also has an explicit
        // `<<stereotype>>`, prepend the annotations.
        if let Some(annotations) = self.take_annotations() {
            entity.stereotype = match entity.stereotype.take() {
                Some(existing) => Some(format!("{annotations}\\n{existing}")),
                None => Some(annotations),
            };
        }
        let line_no = entity.line;
        let idx = self.upsert_entity(entity);
        // If this entity is being declared inside a `package` / `namespace`,
        // wire it into that container's child list. Multiple declarations
        // of the same id inside the same container collapse to one entry.
        if let Some(&(BlockFrame::Container(c_idx), _)) = self.block_stack.last() {
            let id = self.diag.entities[idx].id.clone();
            let cont = &mut self.diag.containers[c_idx];
            if !cont.children_entities.contains(&id) {
                cont.children_entities.push(id);
            }
        }
        if has_block {
            self.block_stack.push((BlockFrame::Entity(idx), line_no));
        }
    }

    fn commit_container(&mut self, open: ContainerOpen, line_no: usize) {
        let new_idx = self.diag.containers.len();
        self.diag.containers.push(Container {
            usymbol: open.usymbol,
            together: open.together,
            label: open.label,
            stereotype: open.stereotype,
            children_entities: Vec::new(),
            children_containers: Vec::new(),
            line: line_no,
        });
        if let Some(&(BlockFrame::Container(parent_idx), _)) = self.block_stack.last() {
            self.diag.containers[parent_idx]
                .children_containers
                .push(new_idx);
        }
        self.block_stack.push((BlockFrame::Container(new_idx), line_no));
    }

    fn upsert_entity(&mut self, entity: Entity) -> usize {
        if let Some(i) = self.diag.entities.iter().position(|e| e.id == entity.id) {
            // Merge: prefer the new declaration's class-family kind /
            // generic / stereotype if it has them, and append nothing
            // else (members are added line-by-line so they don't double
            // up). Only meaningful when both old and new declarations
            // are compartment-shaped — otherwise we just keep the
            // first.
            let existing = &mut self.diag.entities[i];
            if let (
                EntityKindData::Compartment {
                    kind: ex_kind,
                    generic: ex_generic,
                    ..
                },
                EntityKindData::Compartment {
                    kind: new_kind,
                    generic: new_generic,
                    ..
                },
            ) = (&mut existing.kind_data, &entity.kind_data)
            {
                if *ex_kind == ClassFamilyKind::Class && *new_kind != ClassFamilyKind::Class {
                    *ex_kind = *new_kind;
                }
                if ex_generic.is_none() && new_generic.is_some() {
                    *ex_generic = new_generic.clone();
                }
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
                    usymbol: USymbol::None,
                    id: id.to_string(),
                    display: id.to_string(),
                    stereotype: None,
                    stereotype_marker: None,
                    fill: None,
                    line: line_no,
                    kind_data: EntityKindData::Compartment {
                        kind: ClassFamilyKind::Class,
                        generic: None,
                        fields: Vec::new(),
                        methods: Vec::new(),
                    },
                };
                self.diag.entities.push(entity);
                self.diag.entities.len() - 1
            }
        };
        let mut member = parse_member(body, line_no);
        if let Some(ann) = self.take_annotations() {
            // Prepend annotations to the rendered body. The split is on
            // visibility (already extracted by parse_member), so adding
            // text in front of `member.body` just stacks above the
            // identifier when the painter renders it.
            member.body = format!("{ann} {}", member.body);
        }
        push_member(&mut self.diag.entities[idx], member);
    }

    fn handle_block_member(&mut self, raw: &str, line_no: usize) -> Result<()> {
        let &(frame, _) = self.block_stack.last().expect("stack non-empty");
        match frame {
            BlockFrame::Container(_) => {
                // Inside `package`/`namespace` — re-dispatch as if at top
                // level. Entity declarations and nested containers are
                // automatically wired to the current container by their
                // commit_* functions. Container-open beats entity-decl
                // for the same reason as in `run`: keywords like
                // `database` / `node` / `component` can be either.
                if let Some(open) = parse_container_open(raw) {
                    self.commit_container(open, line_no);
                    return Ok(());
                }
                if let Some(action) = parse_entity_decl(raw, line_no) {
                    self.commit_entity(action);
                    return Ok(());
                }
                if self.try_parse_note(raw, line_no)? {
                    return Ok(());
                }
                if let Some((id, body)) = split_member_line(raw) {
                    self.add_member(&id, body, line_no);
                    return Ok(());
                }
                if let Some(rp) = parse_relation(raw, line_no) {
                    self.commit_relation_with_hints(rp.rel, rp.from_lollipop, rp.to_lollipop);
                    return Ok(());
                }
                self.unsupported(raw, line_no)
            }
            BlockFrame::Entity(idx) => {
                // Inline member inside `class A { + foo() }`.
                let mut member = parse_member(raw, line_no);
                if let Some(ann) = self.take_annotations() {
                    member.body = format!("{ann} {}", member.body);
                }
                push_member(&mut self.diag.entities[idx], member);
                Ok(())
            }
        }
    }

    fn commit_relation(&mut self, rel: Relation) {
        self.commit_relation_with_hints(rel, false, false);
    }

    /// Like `commit_relation` but uses `from_lollipop` / `to_lollipop`
    /// to pick `Circle` over `Class` when auto-creating an endpoint
    /// that hasn't been declared yet.
    fn commit_relation_with_hints(
        &mut self,
        rel: Relation,
        from_lollipop: bool,
        to_lollipop: bool,
    ) {
        for (id, lollipop) in [
            (&rel.from, from_lollipop),
            (&rel.to, to_lollipop),
        ] {
            // Couple form (`(A, B) .. C`) leaves the `from` id empty —
            // the real endpoints A and B are tracked separately in
            // `rel.from_couple`. Don't auto-create a phantom empty
            // entity here.
            if id.is_empty() {
                continue;
            }
            if !self.diag.entities.iter().any(|e| e.id == *id) {
                let (usymbol, kind_data) = if lollipop {
                    (
                        USymbol::Interface,
                        EntityKindData::Plain { members: Vec::new() },
                    )
                } else {
                    (
                        USymbol::None,
                        EntityKindData::Compartment {
                            kind: ClassFamilyKind::Class,
                            generic: None,
                            fields: Vec::new(),
                            methods: Vec::new(),
                        },
                    )
                };
                self.diag.entities.push(Entity {
                    usymbol,
                    id: id.clone(),
                    display: id.clone(),
                    stereotype: None,
                    stereotype_marker: None,
                    fill: None,
                    line: rel.line,
                    kind_data,
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

        // `note [side] on link [: body]` — attaches the note to the most
        // recently parsed relation. The PUML form has an optional
        // direction ("left", "right", "top", "bottom") in front of
        // "on link"; we ignore it for now (the painter places the note
        // at the chord midpoint).
        if let Some(body) = parse_note_on_link_decl(rest) {
            let body = match body {
                Some(b) => b,
                None => self.collect_note_body(),
            };
            if let Some(last) = self.diag.relations.last_mut() {
                last.note = Some(body);
            } else {
                self.unsupported(raw, line_no)?;
            }
            return Ok(true);
        }

        // `note over A, B [: body]` — note spans multiple entities; auto-
        // create a dashed dependency edge from the note to each target.
        if let Some((targets, inline_body)) = parse_note_over_decl(rest) {
            let id = format!("__note_{line_no}");
            let body = match inline_body {
                Some(b) => b,
                None => self.collect_note_body(),
            };
            self.push_note(id.clone(), body, line_no);
            for target in targets {
                self.commit_relation(Relation {
                    from: id.clone(),
                    to: target,
                    from_couple: None,
                    from_port: None,
                    to_port: None,
                    head_from: ArrowHead::None,
                    head_to: ArrowHead::None,
                    line_style: LineStyle::Dashed,
                    direction: None,
                    label: None,
                    mult_from: None,
                    mult_to: None,
                    role_from: None,
                    role_to: None,
                    stereotype: None,
                    color: None,
                    note: None,
                    line: line_no,
                });
            }
            return Ok(true);
        }

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
                from_couple: None,
                from_port: None,
                to_port: None,
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
                note: None,
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
            usymbol: USymbol::Note,
            id: id.clone(),
            display: id,
            stereotype: None,
            stereotype_marker: None,
            fill: None,
            line: line_no,
            kind_data: EntityKindData::Note { body },
        });
    }

    /// Parse `hide …` / `show …` global filter directives. Returns
    /// `true` iff the line was a hide/show that we recognized
    /// (consumed); unknown variants are still consumed but flagged by
    /// the caller via the trailing `unsupported` path.
    fn try_parse_hide_show(&mut self, raw: &str) -> bool {
        let (set_to, rest) = if let Some(rest) = strip_prefix_keyword(raw, "hide") {
            (true, rest.trim())
        } else if let Some(rest) = strip_prefix_keyword(raw, "show") {
            (false, rest.trim())
        } else {
            return false;
        };
        let lower = rest.to_ascii_lowercase();
        let lower = lower.as_str();
        // `hide @unlinked …` and stereotype-scoped variants are not
        // supported; we still consume the line so they don't trigger
        // the "unrecognized syntax" diagnostic.
        let lower = lower
            .trim_start_matches("@unlinked")
            .trim_start_matches("empty")
            .trim();
        match lower {
            "circle" => self.diag.hide.circle = set_to,
            "stereotype" | "stereotypes" => self.diag.hide.stereotype = set_to,
            "members" => self.diag.hide.members = set_to,
            "methods" | "method" => self.diag.hide.methods = set_to,
            "fields" | "field" | "attributes" | "attribute" => self.diag.hide.fields = set_to,
            // `hide empty members` etc. are no-ops for us — empty
            // compartments are already collapsed by the painter.
            "" => {}
            _ => {} // silently ignore stereotype-scoped or class-scoped variants
        }
        true
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

/// Append `member` to the appropriate compartment of `entity`. For
/// compartment-shaped entities, `is_method_signature` decides which
/// list; for plain (desc-family) entities, members live in
/// `EntityKindData::Plain.members`; for Note / Object the call is a
/// no-op (a note's body is fixed at declaration time).
fn push_member(entity: &mut Entity, member: Member) {
    match &mut entity.kind_data {
        EntityKindData::Compartment { fields, methods, .. } => {
            if is_method_signature(&member.body) {
                methods.push(member);
            } else {
                fields.push(member);
            }
        }
        EntityKindData::Plain { members } => {
            members.push(member);
        }
        EntityKindData::Note { .. } | EntityKindData::Object { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Direction, LineStyle, Member, Visibility};

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

    fn parse_ok(body: &[&str]) -> CucaDiagram {
        let (diagram, _) = parse(&block(body), CompatMode::Warn).expect("parse ok");
        match diagram {
            Diagram::Cuca(c) => c,
            _ => panic!("expected cuca diagram"),
        }
    }

    /// Helper: pattern-match the entity's compartment data and panic
    /// loudly if it isn't a class-family entity.
    fn compartment(e: &Entity) -> (ClassFamilyKind, &Option<String>, &[Member], &[Member]) {
        match &e.kind_data {
            EntityKindData::Compartment { kind, generic, fields, methods } => {
                (*kind, generic, fields.as_slice(), methods.as_slice())
            }
            other => panic!("expected compartment entity, got {other:?}"),
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
        let (kind, _, fields, methods) = compartment(foo);
        assert_eq!(kind, ClassFamilyKind::Class);
        assert_eq!(fields.len(), 2);
        assert_eq!(methods.len(), 1);
        assert_eq!(fields[0].visibility, Visibility::Public);
        assert_eq!(fields[1].visibility, Visibility::Private);
        assert_eq!(methods[0].body, "getName(): String");
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
        let (_, _, _, methods) = compartment(&c.entities[0]);
        assert_eq!(methods.len(), 1);
        assert_eq!(methods[0].body, "foo()");
    }

    #[test]
    fn parses_static_and_abstract_modifiers() {
        let c = parse_ok(&[
            "class A {",
            "  {static} count: int",
            "  {abstract} render(): void",
            "}",
        ]);
        let (_, _, fields, methods) = compartment(&c.entities[0]);
        assert_eq!(fields.len(), 1);
        assert!(fields[0].is_static);
        assert_eq!(methods.len(), 1);
        assert!(methods[0].is_abstract);
    }

    #[test]
    fn parses_generic_and_stereotype() {
        let c = parse_ok(&[r#"class Repo<T> <<Service>> #LightBlue"#]);
        let e = &c.entities[0];
        assert_eq!(e.id, "Repo");
        let (_, generic, _, _) = compartment(e);
        assert_eq!(generic.as_deref(), Some("T"));
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
        let (_, _, _, methods) = compartment(&c.entities[0]);
        assert_eq!(methods[0].visibility, Visibility::Package);
        assert_eq!(methods[0].body, "helper(): void");
    }

    #[test]
    fn classifier_modifier_maps_to_static() {
        let c = parse_ok(&[
            "class A {",
            "  {classifier} factory(): A",
            "}",
        ]);
        let (_, _, _, methods) = compartment(&c.entities[0]);
        assert!(methods[0].is_static, "{{classifier}} should set is_static");
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
    fn parses_note_on_link_inline() {
        let c = parse_ok(&[
            "class A",
            "class B",
            "A --> B",
            "note on link : reads from",
        ]);
        let r = &c.relations[0];
        assert_eq!(r.note.as_deref(), Some("reads from"));
    }

    #[test]
    fn parses_note_on_link_multiline() {
        let c = parse_ok(&[
            "class A",
            "class B",
            "A --> B",
            "note left on link",
            "  body line 1",
            "end note",
        ]);
        let r = &c.relations[0];
        assert_eq!(r.note.as_deref().unwrap().trim(), "body line 1");
    }

    #[test]
    fn parses_note_over_two_targets() {
        let c = parse_ok(&[
            "class A",
            "class B",
            "note over A, B : shared invariant",
        ]);
        let note = c.entities.iter().find(|e| e.usymbol == USymbol::Note).unwrap();
        assert_eq!(note.kind_data.note_body(), Some("shared invariant"));
        // Two auto-relations, one for each target.
        assert_eq!(c.relations.len(), 2);
        assert!(c.relations.iter().any(|r| r.to == "A"));
        assert!(c.relations.iter().any(|r| r.to == "B"));
        for r in &c.relations {
            assert_eq!(r.line_style, LineStyle::Dashed);
            assert_eq!(r.from, note.id);
        }
    }

    #[test]
    fn parses_anchored_note_inline() {
        let c = parse_ok(&["class Foo", "note left of Foo : just a hint"]);
        // Entities: Foo + the auto-generated note.
        assert_eq!(c.entities.len(), 2);
        let note = c.entities.iter().find(|e| e.usymbol == USymbol::Note).unwrap();
        assert_eq!(note.kind_data.note_body(), Some("just a hint"));
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
        let note = c.entities.iter().find(|e| e.usymbol == USymbol::Note).unwrap();
        let body = note.kind_data.note_body().unwrap();
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
        assert_eq!(note.usymbol, USymbol::Note);
        assert_eq!(note.kind_data.note_body(), Some("hello world"));
        // User-written N1 .. Foo should produce one relation; no auto-rel.
        assert_eq!(c.relations.len(), 1);
        let r = &c.relations[0];
        assert_eq!(r.from, "N1");
        assert_eq!(r.to, "Foo");
        assert_eq!(r.line_style, LineStyle::Dashed);
    }

    #[test]
    fn parses_package_with_nested_class() {
        let c = parse_ok(&[
            "package \"Domain\" {",
            "  class Order",
            "  class LineItem",
            "}",
            "class External",
        ]);
        assert_eq!(c.entities.len(), 3);
        assert_eq!(c.containers.len(), 1);
        let pkg = &c.containers[0];
        assert_eq!(pkg.usymbol, USymbol::Package);
        assert!(!pkg.together);
        assert_eq!(pkg.label, "Domain");
        assert_eq!(pkg.children_entities, vec!["Order", "LineItem"]);
    }

    #[test]
    fn parses_nested_namespaces() {
        let c = parse_ok(&[
            "namespace outer {",
            "  namespace inner {",
            "    class Inner",
            "  }",
            "  class Mid",
            "}",
        ]);
        assert_eq!(c.containers.len(), 2);
        let outer = c.containers.iter().find(|c| c.label == "outer").unwrap();
        let inner = c.containers.iter().find(|c| c.label == "inner").unwrap();
        // outer holds Mid + a child container ref.
        assert!(outer.children_entities.contains(&"Mid".to_string()));
        assert_eq!(outer.children_containers.len(), 1);
        // inner holds Inner.
        assert_eq!(inner.children_entities, vec!["Inner"]);
    }

    #[test]
    fn parses_together_block() {
        let c = parse_ok(&[
            "together {",
            "  class A",
            "  class B",
            "}",
        ]);
        assert_eq!(c.containers.len(), 1);
        let t = &c.containers[0];
        assert!(t.together);
        assert!(t.label.is_empty());
        assert_eq!(t.children_entities, vec!["A", "B"]);
    }

    #[test]
    fn parses_association_class_left_couple() {
        let c = parse_ok(&[
            "class A",
            "class B",
            "class C",
            "A -- B",
            "(A, B) .. C",
        ]);
        // Two relations: the regular A--B and the couple .. C.
        assert_eq!(c.relations.len(), 2);
        let assoc = &c.relations[1];
        assert_eq!(assoc.from_couple, Some(("A".into(), "B".into())));
        assert_eq!(assoc.to, "C");
        assert_eq!(assoc.line_style, LineStyle::Dashed);
    }

    #[test]
    fn parses_association_class_right_couple_normalizes() {
        // `C -- (A, B)` — the couple is on the right; parser swaps so
        // the IR consistently has from_couple + to.
        let c = parse_ok(&[
            "class A",
            "class B",
            "class C",
            "C -- (A, B)",
        ]);
        let assoc = &c.relations[0];
        assert_eq!(assoc.from_couple, Some(("A".into(), "B".into())));
        assert_eq!(assoc.to, "C");
    }

    #[test]
    fn parses_lollipop_decl() {
        let c = parse_ok(&["() Foo"]);
        assert_eq!(c.entities.len(), 1);
        let e = &c.entities[0];
        assert_eq!(e.usymbol, USymbol::Interface);
        assert_eq!(e.id, "Foo");
    }

    #[test]
    fn lollipop_in_relation_auto_creates_circle() {
        // `(Iface)` references a lollipop — auto-create as Interface
        // (lollipop). `class A` declared explicitly stays Class.
        let c = parse_ok(&["class A", "A --> (Iface)"]);
        let iface = c.entities.iter().find(|e| e.id == "Iface").unwrap();
        assert_eq!(iface.usymbol, USymbol::Interface);
    }

    #[test]
    fn parses_custom_stereotype_marker_with_color() {
        let c = parse_ok(&["class Robot <<(R, #FF8800) Service>>"]);
        let e = &c.entities[0];
        assert_eq!(e.stereotype.as_deref(), Some("Service"));
        let marker = e.stereotype_marker.as_ref().unwrap();
        assert_eq!(marker.letter, "R");
        assert_eq!(marker.color.as_deref(), Some("#FF8800"));
    }

    #[test]
    fn parses_custom_marker_without_color() {
        let c = parse_ok(&["class Foo <<(X) something>>"]);
        let e = &c.entities[0];
        assert_eq!(e.stereotype.as_deref(), Some("something"));
        let marker = e.stereotype_marker.as_ref().unwrap();
        assert_eq!(marker.letter, "X");
        assert!(marker.color.is_none());
    }

    #[test]
    fn parses_member_port() {
        let c = parse_ok(&[
            "class A {",
            "  + name: String",
            "}",
            "class B",
            "A::name --> B",
        ]);
        // Two classes, no phantom `A::name` entity.
        assert_eq!(c.entities.len(), 2);
        let r = &c.relations[0];
        assert_eq!(r.from, "A");
        assert_eq!(r.from_port.as_deref(), Some("name"));
        assert_eq!(r.to, "B");
        assert!(r.to_port.is_none());
    }

    #[test]
    fn member_port_on_target_side() {
        let c = parse_ok(&[
            "class A",
            "class B {",
            "  + value: int",
            "}",
            "A --> B::value",
        ]);
        let r = &c.relations[0];
        assert!(r.from_port.is_none());
        assert_eq!(r.to_port.as_deref(), Some("value"));
    }

    #[test]
    fn parses_edge_inline_color() {
        let c = parse_ok(&[
            "class A",
            "class B",
            "A -[#red]-> B",
        ]);
        let r = &c.relations[0];
        assert_eq!(r.color.as_deref(), Some("#red"));
    }

    #[test]
    fn parses_edge_color_with_extra_modifier() {
        let c = parse_ok(&[
            "class A",
            "class B",
            "A -[#abcdef,bold]-> B",
        ]);
        let r = &c.relations[0];
        assert_eq!(r.color.as_deref(), Some("#abcdef"));
    }

    #[test]
    fn parses_hide_directives() {
        let c = parse_ok(&[
            "hide circle",
            "hide methods",
            "hide stereotype",
            "class A",
        ]);
        assert!(c.hide.circle);
        assert!(c.hide.methods);
        assert!(c.hide.stereotype);
        assert!(!c.hide.fields);
    }

    #[test]
    fn show_reverses_hide() {
        let c = parse_ok(&["hide circle", "show circle", "class A"]);
        assert!(!c.hide.circle);
    }

    #[test]
    fn parses_freestanding_note_block() {
        let c = parse_ok(&[
            "note as N1",
            "  body line",
            "end note",
        ]);
        let note = &c.entities[0];
        assert_eq!(note.usymbol, USymbol::Note);
        assert_eq!(note.id, "N1");
        assert_eq!(note.kind_data.note_body().unwrap().trim(), "body line");
        // No auto relation for freestanding form.
        assert!(c.relations.is_empty());
    }

    // --- M5-partial / M6: desc family + inline shorthand ---

    #[test]
    fn parses_component_keyword() {
        let c = parse_ok(&["component Foo"]);
        assert_eq!(c.entities.len(), 1);
        assert_eq!(c.entities[0].usymbol, USymbol::Component);
        assert_eq!(c.entities[0].id, "Foo");
        assert!(matches!(c.entities[0].kind_data, EntityKindData::Plain { .. }));
    }

    #[test]
    fn parses_actor_keyword() {
        let c = parse_ok(&["actor Bob"]);
        assert_eq!(c.entities[0].usymbol, USymbol::Actor);
    }

    #[test]
    fn parses_usecase_keyword() {
        let c = parse_ok(&["usecase Login"]);
        assert_eq!(c.entities[0].usymbol, USymbol::UseCase);
    }

    #[test]
    fn parses_database_as_leaf() {
        let c = parse_ok(&[r#"database "User DB" as UDB"#]);
        assert_eq!(c.entities[0].usymbol, USymbol::Database);
        assert_eq!(c.entities[0].id, "UDB");
        assert_eq!(c.entities[0].display, "User DB");
    }

    #[test]
    fn parses_database_as_container() {
        // `database "X" {` opens a cluster, not a leaf.
        let c = parse_ok(&[r#"database "Cluster" {"#, "class Inner", "}"]);
        assert_eq!(c.containers.len(), 1);
        assert_eq!(c.containers[0].usymbol, USymbol::Database);
        assert_eq!(c.containers[0].label, "Cluster");
        assert_eq!(c.containers[0].children_entities, vec!["Inner"]);
    }

    #[test]
    fn parses_component_shorthand() {
        let c = parse_ok(&["[WebApp]"]);
        assert_eq!(c.entities[0].usymbol, USymbol::Component);
        assert_eq!(c.entities[0].id, "WebApp");
    }

    #[test]
    fn parses_usecase_shorthand() {
        let c = parse_ok(&["(Login)"]);
        assert_eq!(c.entities[0].usymbol, USymbol::UseCase);
        assert_eq!(c.entities[0].id, "Login");
    }

    #[test]
    fn parses_actor_shorthand() {
        let c = parse_ok(&[":Bob:"]);
        assert_eq!(c.entities[0].usymbol, USymbol::Actor);
        assert_eq!(c.entities[0].id, "Bob");
    }

    #[test]
    fn parses_socket_open_head() {
        // `Foo -( Bar` — right-end socket (PlantUML LinkDecor.PARENTHESIS).
        let c = parse_ok(&["class Foo", "class Bar", "Foo -( Bar"]);
        assert_eq!(c.relations.len(), 1);
        let r = &c.relations[0];
        assert_eq!(r.from, "Foo");
        assert_eq!(r.to, "Bar");
        assert_eq!(r.head_to, ArrowHead::SocketOpen);
        assert_eq!(r.head_from, ArrowHead::None);
    }

    #[test]
    fn parses_socket_closed_head() {
        // `Foo )- Bar` — left-end socket.
        let c = parse_ok(&["class Foo", "class Bar", "Foo )- Bar"]);
        let r = &c.relations[0];
        assert_eq!(r.head_from, ArrowHead::SocketClosed);
        assert_eq!(r.head_to, ArrowHead::None);
    }

    #[test]
    fn shorthand_does_not_swallow_relation_line() {
        // `[A] --> [B]` is a relation line with two component shorthand
        // endpoints. parse_inline_shorthand must reject it (trailing
        // `--> [B]` isn't `as`/`<<`/`#`) so parse_relation gets to run.
        // The endpoint-cleanup (strip `[…]` brackets from auto-created
        // ids and tag them as USymbol::Component) is a follow-up — for
        // now we just confirm the inline-shorthand parser bows out so
        // the relation parser can take the line.
        let c = parse_ok(&["[A] --> [B]"]);
        assert_eq!(c.relations.len(), 1, "relation must still be parsed");
    }

    #[test]
    fn couple_form_not_misread_as_shorthand() {
        // `(A, B) .. C` is the association-class couple form. The comma
        // inside parens disambiguates it from `(Foo)` usecase shorthand.
        let c = parse_ok(&["class A", "class B", "class C", "(A, B) .. C"]);
        assert_eq!(c.entities.len(), 3);
        assert_eq!(c.relations.len(), 1);
        assert_eq!(c.relations[0].from_couple, Some(("A".into(), "B".into())));
        assert_eq!(c.relations[0].to, "C");
    }
}
