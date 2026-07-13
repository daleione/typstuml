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

mod build;
mod container;
mod entity;
mod flavor;
mod member;
mod note;
mod relation;
mod util;

#[cfg(test)]
mod tests;

use crate::diagnostics::{CompatMode, Diagnostic, Level, Result};
use crate::ir::{
    ClassFamilyKind, CucaDiagram, Diagram, EntityKindData, LayoutDirection, Skinparam, USymbol,
};
use crate::parser::lexer::{BodyLine, UmlBlock};

use self::container::parse_container_open;
use self::entity::{parse_entity_decl, parse_inline_shorthand, parse_lollipop_decl};
use self::flavor::sniff_flavor;
use self::member::split_member_line;
use self::relation::parse_relation;
use self::util::{is_comment, is_skip_directive, parse_annotation, strip_prefix_keyword, Flavor};

pub fn parse(block: &UmlBlock, compat: CompatMode) -> Result<(Diagram, Vec<Diagnostic>)> {
    let mut parser = Parser::new(&block.body, compat);
    parser.run()?;
    let mut diag = parser.diag;
    diag.name = block.name.clone();
    desugar_bare_interfaces_to_lollipops(&mut diag);
    Ok((Diagram::Cuca(diag), parser.diagnostics))
}

/// A bare `interface Foo` (no fields, no methods, no generic) reads as
/// a lollipop in an architecture diagram — PlantUML's own desc-family
/// rendering treats it that way, and `interface`'s only other parse
/// path (`() Foo`) already produces one. `entity.rs` always emits the
/// class-family compartment shape at parse time (before we know the
/// diagram's shape mix), so fix it up here once every entity is known:
/// desugar every such bare interface into the lollipop it would have
/// been had it been written `() Foo`, but only when the diagram
/// actually contains a desc/architecture shape — a plain class diagram
/// with an empty marker interface keeps the compartment box (matches
/// `codegen::cuca::is_desc_flavor`'s criteria; see
/// docs/cuca-architecture-layout-redesign.md §3.6).
fn desugar_bare_interfaces_to_lollipops(diag: &mut CucaDiagram) {
    if !diag.has_desc_shape() {
        return;
    }
    for e in &mut diag.entities {
        let is_bare_interface = matches!(
            &e.kind_data,
            EntityKindData::Compartment {
                kind: ClassFamilyKind::Interface,
                generic: None,
                fields,
                methods,
            } if fields.is_empty() && methods.is_empty()
        );
        if is_bare_interface {
            e.usymbol = USymbol::Interface;
            e.kind_data = EntityKindData::Plain { members: Vec::new() };
        }
    }
}

struct Parser<'a> {
    lines: &'a [BodyLine],
    pos: usize,
    compat: CompatMode,
    diag: CucaDiagram,
    /// Sniffed from the body before the main pass. Selects flavor-
    /// sensitive behavior in `parse_relation` (e.g. `:Foo:` is an
    /// actor reference only when `flavor == UseCase`).
    flavor: Flavor,
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

#[derive(Clone, Debug)]
enum BlockFrame {
    /// Inside `class A { … }` — member lines go to `entities[idx]`.
    Entity(usize),
    /// Inside `package "X" { … }` / `namespace foo { … }` — declared
    /// entities and nested containers register as children of
    /// `containers[idx]`.
    Container(usize),
    /// Inside a `skinparam <target> { … }` block. Lines are `Key Value`
    /// pairs that get combined with the target prefix to form normal
    /// skinparam entries.
    SkinparamBlock(String),
}

impl<'a> Parser<'a> {
    fn new(lines: &'a [BodyLine], compat: CompatMode) -> Self {
        let flavor = sniff_flavor(lines);
        Self {
            lines,
            pos: 0,
            compat,
            diag: CucaDiagram::default(),
            flavor,
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
            if raw.starts_with("left to right direction") || raw == "left to right" {
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
                let trimmed = rest.trim();
                // `skinparam class { … }` block form — collect the lines
                // between the braces as `<class><Key>` entries on the
                // skinparam list, matching PlantUML's prefix semantics.
                if let Some(head) = trimmed.strip_suffix('{') {
                    let prefix = head.trim();
                    if !prefix.is_empty() {
                        self.block_stack
                            .push((BlockFrame::SkinparamBlock(prefix.to_string()), line_no));
                        continue;
                    }
                }
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
            if let Some(rp) = parse_relation(raw, line_no, self.flavor) {
                self.commit_relation_with_hints(rp.rel, rp.from_hint, rp.to_hint);
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
        crate::parser::common::warn_or_err(&mut self.diagnostics, self.compat, level, line, message)
    }
}
