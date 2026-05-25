//! Builder methods: commit parsed entities / containers / relations / notes
//! into the `CucaDiagram`, with auto-create + merge semantics.

use crate::diagnostics::Result;
use crate::ir::{
    ArrowHead, ClassFamilyKind, Container, Entity, EntityKindData, LineStyle, Member, Relation,
    USymbol,
};

use super::container::{parse_container_open, ContainerOpen};
use super::entity::{parse_entity_decl, EntityAction};
use super::member::{is_method_signature, parse_member, split_member_line};
use super::note::{
    parse_anchored_note_decl, parse_freestanding_note_decl, parse_note_on_link_decl,
    parse_note_over_decl, parse_quoted_note_decl, side_to_direction,
};
use super::relation::{parse_relation, EndpointHint};
use super::util::strip_prefix_keyword;
use super::{BlockFrame, Parser};

impl<'a> Parser<'a> {
    pub(super) fn commit_entity(&mut self, action: EntityAction) {
        let EntityAction {
            mut entity,
            has_block,
            extends,
            implements,
        } = action;
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
        let child_id = entity.id.clone();
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
        // Generalisation edges from `extends` / `implements`. PlantUML
        // draws extends as solid `--|>` and implements as dashed
        // `..|>`; both put the triangle-open head on the parent side.
        // We emit `child --> parent` so Sugiyama places the parent
        // above the child in TB.
        for parent in extends {
            self.diag.relations.push(Relation {
                from: child_id.clone(),
                to: parent,
                from_couple: None,
                from_port: None,
                to_port: None,
                head_from: ArrowHead::None,
                head_to: ArrowHead::TriangleOpen,
                line_style: LineStyle::Solid,
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
        for iface in implements {
            self.diag.relations.push(Relation {
                from: child_id.clone(),
                to: iface,
                from_couple: None,
                from_port: None,
                to_port: None,
                head_from: ArrowHead::None,
                head_to: ArrowHead::TriangleOpen,
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
    }

    pub(super) fn commit_container(&mut self, open: ContainerOpen, line_no: usize) {
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
        self.block_stack
            .push((BlockFrame::Container(new_idx), line_no));
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

    pub(super) fn add_member(&mut self, id: &str, body: &str, line_no: usize) {
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

    pub(super) fn handle_block_member(&mut self, raw: &str, line_no: usize) -> Result<()> {
        let frame = self.block_stack.last().expect("stack non-empty").0.clone();
        match frame {
            BlockFrame::SkinparamBlock(ref prefix) => {
                let parts: Vec<&str> = raw.splitn(2, char::is_whitespace).collect();
                if parts.len() == 2 {
                    let key_suffix = parts[0].trim();
                    let value = parts[1].trim();
                    if !key_suffix.is_empty() {
                        // Combine `skinparam class { BackgroundColor … }`
                        // into the flat key `classBackgroundColor`
                        // (lower-case first char of target + capitalised
                        // suffix) to match the single-line form.
                        let mut key = String::with_capacity(prefix.len() + key_suffix.len());
                        key.push_str(prefix);
                        let mut chars = key_suffix.chars();
                        if let Some(c) = chars.next() {
                            key.extend(c.to_uppercase());
                            key.push_str(chars.as_str());
                        }
                        self.diag.skinparams.push(crate::ir::Skinparam {
                            key,
                            value: value.to_string(),
                            line: line_no,
                        });
                    }
                }
                Ok(())
            }
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
                if let Some(rp) = parse_relation(raw, line_no, self.flavor) {
                    self.commit_relation_with_hints(rp.rel, rp.from_hint, rp.to_hint);
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
        self.commit_relation_with_hints(rel, EndpointHint::None, EndpointHint::None);
    }

    /// Like `commit_relation` but consults per-endpoint hints to pick
    /// the right `USymbol` when auto-creating an endpoint that the
    /// user referenced but didn't declare.
    pub(super) fn commit_relation_with_hints(
        &mut self,
        rel: Relation,
        from_hint: EndpointHint,
        to_hint: EndpointHint,
    ) {
        for (id, hint) in [(&rel.from, from_hint), (&rel.to, to_hint)] {
            // Couple form (`(A, B) .. C`) leaves the `from` id empty —
            // the real endpoints A and B are tracked separately in
            // `rel.from_couple`. Don't auto-create a phantom empty
            // entity here.
            if id.is_empty() {
                continue;
            }
            if !self.diag.entities.iter().any(|e| e.id == *id) {
                let (usymbol, kind_data) = match hint {
                    EndpointHint::Lollipop => (
                        USymbol::Interface,
                        EntityKindData::Plain {
                            members: Vec::new(),
                        },
                    ),
                    EndpointHint::Actor => (
                        USymbol::Actor,
                        EntityKindData::Plain {
                            members: Vec::new(),
                        },
                    ),
                    EndpointHint::UseCase => (
                        USymbol::UseCase,
                        EntityKindData::Plain {
                            members: Vec::new(),
                        },
                    ),
                    EndpointHint::None => (
                        USymbol::None,
                        EntityKindData::Compartment {
                            kind: ClassFamilyKind::Class,
                            generic: None,
                            fields: Vec::new(),
                            methods: Vec::new(),
                        },
                    ),
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
    pub(super) fn try_parse_note(&mut self, raw: &str, line_no: usize) -> Result<bool> {
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
            if trimmed.eq_ignore_ascii_case("end note") || trimmed.eq_ignore_ascii_case("endnote") {
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
}

/// Append `member` to the appropriate compartment of `entity`. For
/// compartment-shaped entities, `is_method_signature` decides which
/// list; for plain (desc-family) entities, members live in
/// `EntityKindData::Plain.members`. Objects re-parse the body as
/// `name = value` and store the pair in `ObjectField`. Notes ignore.
fn push_member(entity: &mut Entity, member: Member) {
    match &mut entity.kind_data {
        EntityKindData::Compartment {
            fields, methods, ..
        } => {
            if is_method_signature(&member.body) {
                methods.push(member);
            } else {
                fields.push(member);
            }
        }
        EntityKindData::Plain { members } => {
            members.push(member);
        }
        EntityKindData::Object { fields } => {
            // The member's visibility / static / abstract modifiers don't
            // apply to object instance rows; we only need the body, which
            // we then split on the first `=` into (name, value). Lines
            // that don't contain `=` are stored with an empty value so
            // codegen renders them as bare names.
            let body = member.body.trim();
            let (name, value) = match body.find('=') {
                Some(i) => (
                    body[..i].trim().to_string(),
                    body[i + 1..].trim().to_string(),
                ),
                None => (body.to_string(), String::new()),
            };
            fields.push(crate::ir::ObjectField {
                name,
                value,
                line: member.line,
            });
        }
        EntityKindData::Note { .. } => {}
    }
}
