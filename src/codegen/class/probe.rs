//! Pass-1 probe emission for class diagrams.
//!
//! For each entity in a `ClassDiagram` we emit a `#class-probe(id:
//! "mc-<diagram_idx>-<entity_id>", spec: (...))` call into the pass-1
//! Typst source. The painter measures the natural size and emits
//! `metadata((id, w, h)) <typstuml_measure>`; `runtime::measure::run`
//! ingests it back into a `MeasurementSet`.
//!
//! IDs MUST be stable across pass-1 / pass-2: codegen calls `class_id`
//! in both phases and looks up the measurement by the same string. The
//! sanitization rule below treats entity IDs that are already valid
//! `[A-Za-z0-9_]+` as identity (the common case) and only escapes weird
//! IR identifiers.

use crate::ir::{ClassDiagram, Entity, HideOptions};

use super::emit::write_class_spec_body;

/// Build the stable probe ID for an entity. Format:
/// `mc-{diagram_idx}-{sanitized_entity_id}`.
pub fn class_id(diagram_idx: usize, entity: &Entity) -> String {
    format!("mc-{diagram_idx}-{}", sanitize(&entity.id))
}

/// Emit one `#class-probe(...)` call per entity into `out`. Pushes the
/// expected IDs into `expected_ids` so `runtime::measure::run` can
/// verify the protocol round-trip.
pub fn collect(
    diag: &ClassDiagram,
    diagram_idx: usize,
    out: &mut String,
    expected_ids: &mut Vec<String>,
) {
    for entity in &diag.entities {
        let id = class_id(diagram_idx, entity);
        out.push_str("#class-probe(id: \"");
        out.push_str(&id);
        out.push_str("\", spec: (");
        write_class_spec_body(out, entity, &diag.hide);
        out.push_str("))\n");
        expected_ids.push(id);
    }
}

/// True iff `diag` has at least one entity that needs measurement.
/// Today every entity does — including notes and lollipops, which have
/// their own kind-specific layout in the painter.
pub fn has_probes(diag: &ClassDiagram) -> bool {
    !diag.entities.is_empty()
}

/// Sanitize an IR entity ID into a string safe to embed in a `metadata`
/// dict value (`mc-...-{id}`). Allowed: ASCII letters, digits, `_`,
/// `.`. Everything else collapses to `_`. The result is non-empty
/// because the input is the IR's required `Entity::id` (parser
/// guarantees non-empty).
fn sanitize(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '.' {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// Whether the entity has a probe at all. Reserved for future scopes
/// (e.g. notes with empty body, lollipops without label) where we might
/// decide to skip emission.
#[allow(dead_code)]
pub(crate) fn probes_for(_entity: &Entity, _hide: &HideOptions) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::EntityKind;

    fn ent(id: &str) -> Entity {
        Entity {
            kind: EntityKind::Class,
            id: id.into(),
            display: id.into(),
            generic: None,
            stereotype: None,
            stereotype_marker: None,
            fields: Vec::new(),
            methods: Vec::new(),
            body: None,
            fill: None,
            line: 0,
        }
    }

    #[test]
    fn ascii_id_round_trips() {
        let e = ent("Animal");
        assert_eq!(class_id(0, &e), "mc-0-Animal");
    }

    #[test]
    fn nonascii_id_sanitized_to_underscore() {
        let e = ent("a/b c.d");
        assert_eq!(class_id(2, &e), "mc-2-a_b_c.d");
    }

    #[test]
    fn empty_diagram_has_no_probes() {
        let d = ClassDiagram::default();
        assert!(!has_probes(&d));
    }
}
