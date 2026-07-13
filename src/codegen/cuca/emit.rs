//! Typst emitters: turn IR + computed geometry/segments into the
//! `#cuca-layout(...)` argument tree the painter consumes.

use std::fmt::Write as _;

use crate::ir::{
    ArrowHead, Container, Entity, EntityKindData, HideOptions, LineStyle, Member, USymbol,
};
use crate::layout::geometry::Point;

use super::geom::Side;
use super::text::{creole_to_typst, typst_str_escape};
use super::theme::puml_color_to_typst;
use super::{CoupleEdge, OrientedEdge};

/// Geometry the layout pass has resolved for one entity. Used both for
/// the `width:` / `height:` arguments passed to the blockcell painter
/// and (via [`super::emit`]) by Sugiyama upstream. When the
/// measure protocol is enabled, these come from
/// [`crate::runtime::MeasurementSet`] — otherwise from the heuristic
/// `class_geom_filtered` / `note_geom` / `lollipop_geom` estimators.
pub(super) struct EmitGeom {
    pub width_pt: f64,
    pub height_pt: f64,
}

pub(super) fn emit_class(
    out: &mut String,
    top_left: Point,
    geom: &EmitGeom,
    entity: &Entity,
    hide: &HideOptions,
) {
    if matches!(entity.kind_data, EntityKindData::Note { .. }) {
        return emit_note(out, top_left, geom, entity);
    }
    if entity.usymbol == USymbol::Interface {
        return emit_lollipop(out, top_left, geom, entity);
    }
    // Force the painter to render the box with codegen's exact width
    // and height. Without this, Typst's `measure` of the rendered text
    // gives slightly different sizes — and the resulting mid-x is what
    // the painter uses to anchor edges, so edges land off-centre
    // relative to the codegen-computed routing. With the measure
    // protocol on, `geom` came from the painter itself so this lock
    // is a no-op; without it, it's the safety belt that keeps the
    // heuristic from drifting.
    out.push_str(&format!(
        "    (x: {:.2}pt, y: {:.2}pt, width: {:.2}pt, height: {:.2}pt, ",
        top_left.x, top_left.y, geom.width_pt, geom.height_pt,
    ));
    write_class_spec_body(out, entity, hide);
    out.push_str("),\n");
}

/// Emit the spec body (everything inside the Typst dict argument to the
/// `cuca-layout` `classes:` list, minus the `x:` / `y:` / `width:` /
/// `height:` positional fields). The same body is reused as the
/// argument to `#cuca-probe(spec: (...))` in the pass-1 measure
/// source — see `super::probe::collect`.
pub(super) fn write_class_spec_body(out: &mut String, entity: &Entity, hide: &HideOptions) {
    let kind_kw = entity_kind_keyword(entity);
    out.push_str("kind: \"");
    out.push_str(kind_kw);
    out.push_str("\"");
    if let EntityKindData::Note { body } = &entity.kind_data {
        out.push_str(", body: [");
        if !body.is_empty() {
            for (i, line) in body.lines().enumerate() {
                if i > 0 {
                    out.push_str(" \\ ");
                }
                out.push_str(&creole_to_typst(line));
            }
        }
        out.push(']');
        return;
    }
    out.push_str(", name: [");
    out.push_str(&creole_to_typst(&entity.display));
    out.push(']');
    if entity.usymbol == USymbol::Interface {
        return;
    }
    // Objects: emit `fields: ((name, value), …)` and bail out — they
    // don't have stereotype / generic / methods / hide options that the
    // class painter consumes.
    if let EntityKindData::Object { fields } = &entity.kind_data {
        if let Some(c) = &entity.fill {
            if let Some(typst_color) = puml_color_to_typst(c) {
                out.push_str(", fill: ");
                out.push_str(&typst_color);
            }
        }
        if !hide.stereotype {
            if let Some(s) = &entity.stereotype {
                out.push_str(", stereotype: [");
                out.push_str(&creole_to_typst(s));
                out.push(']');
            }
        }
        // Emit each row as pre-joined content `[name = value]` so the
        // painter doesn't need to concatenate (which interferes with
        // inline measurement).
        out.push_str(", fields: (");
        for (i, f) in fields.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push('[');
            out.push_str(&creole_to_typst(&f.name));
            if !f.value.is_empty() {
                out.push_str(" = ");
                out.push_str(&creole_to_typst(&f.value));
            }
            out.push(']');
        }
        if fields.len() == 1 {
            out.push(',');
        }
        out.push(')');
        return;
    }

    let show_fields = !(hide.fields || hide.members);
    let show_methods = !(hide.methods || hide.members);

    if let Some(g) = entity.kind_data.generic() {
        out.push_str(", generic: [");
        out.push_str(&creole_to_typst(g));
        out.push(']');
    }
    if !hide.stereotype {
        if let Some(s) = &entity.stereotype {
            out.push_str(", stereotype: [");
            out.push_str(&creole_to_typst(s));
            out.push(']');
        }
    }
    if hide.circle {
        out.push_str(", hide-marker: true");
    } else if let Some(marker) = &entity.stereotype_marker {
        out.push_str(&format!(
            ", marker-letter: \"{}\"",
            typst_str_escape(&marker.letter)
        ));
        if let Some(c) = &marker.color {
            if let Some(typst_color) = puml_color_to_typst(c) {
                out.push_str(", marker-color: ");
                out.push_str(&typst_color);
            }
        }
    }
    if let Some(c) = &entity.fill {
        if let Some(typst_color) = puml_color_to_typst(c) {
            out.push_str(", fill: ");
            out.push_str(&typst_color);
        }
    }

    let fields_to_emit: &[Member] = if show_fields { entity.kind_data.fields() } else { &[] };
    let methods_to_emit: &[Member] = if show_methods { entity.kind_data.methods() } else { &[] };

    out.push_str(", fields: (");
    for (i, m) in fields_to_emit.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        emit_member(out, m);
    }
    if fields_to_emit.len() == 1 {
        out.push(',');
    }
    out.push(')');

    out.push_str(", methods: (");
    for (i, m) in methods_to_emit.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        emit_member(out, m);
    }
    if methods_to_emit.len() == 1 {
        out.push(',');
    }
    out.push(')');
}

fn emit_lollipop(out: &mut String, top_left: Point, geom: &EmitGeom, entity: &Entity) {
    out.push_str(&format!(
        "    (x: {:.2}pt, y: {:.2}pt, kind: \"lollipop\", width: {:.2}pt, name: [",
        top_left.x, top_left.y, geom.width_pt,
    ));
    out.push_str(&creole_to_typst(&entity.display));
    out.push_str("]),\n");
}

fn emit_note(out: &mut String, top_left: Point, _geom: &EmitGeom, entity: &Entity) {
    let body = entity.kind_data.note_body().unwrap_or("");
    out.push_str(&format!(
        "    (x: {:.2}pt, y: {:.2}pt, kind: \"note\", body: [",
        top_left.x, top_left.y,
    ));
    if body.is_empty() {
        // Empty body — leave the content slot blank.
    } else {
        // Multi-line body. Use Typst hard-break ` \ ` between lines so a
        // single content slot renders the multi-line note.
        for (i, line) in body.lines().enumerate() {
            if i > 0 {
                out.push_str(" \\ ");
            }
            out.push_str(&creole_to_typst(line));
        }
    }
    out.push_str("]),\n");
}

fn emit_member(out: &mut String, m: &Member) {
    let _ = write!(
        out,
        "(vis: \"{}\", body: [{}]",
        m.visibility.glyph(),
        creole_to_typst(&m.body),
    );
    if m.is_static {
        out.push_str(", static: true");
    }
    if m.is_abstract {
        out.push_str(", abstract: true");
    }
    out.push(')');
}

pub(super) fn emit_edge(
    out: &mut String,
    oe: &OrientedEdge,
    segments: &[(Point, Point, Point)],
    sides: Option<(Side, Side)>,
    from_override: Option<f64>,
    to_override: Option<f64>,
    label_pos: Option<Point>,
) {
    let _ = write!(
        out,
        "    (from: {}, to: {}, head-from: \"{}\", head-to: \"{}\", style: \"{}\"",
        oe.src_idx,
        oe.dst_idx,
        head_keyword(oe.head_src),
        head_keyword(oe.head_dst),
        line_style_keyword(oe.relation.line_style),
    );
    if let Some((from_side, to_side)) = sides {
        out.push_str(&format!(
            ", from-side: \"{}\", to-side: \"{}\"",
            from_side.keyword(),
            to_side.keyword(),
        ));
        // The override applies to whichever axis is "free" given the
        // side: left/right side fixes x, frees y; top/bot fixes y,
        // frees x. From and to are emitted independently — they may
        // differ when sibling edges are distributed along a shared
        // face so their arrowheads don't pile up.
        let from_key = if matches!(from_side, Side::Left | Side::Right) {
            "y"
        } else {
            "x"
        };
        let to_key = if matches!(to_side, Side::Left | Side::Right) {
            "y"
        } else {
            "x"
        };
        if let Some(c) = from_override {
            out.push_str(&format!(", from-{from_key}: {c:.2}pt"));
        }
        if let Some(c) = to_override {
            out.push_str(&format!(", to-{to_key}: {c:.2}pt"));
        }
    }
    if let Some(label) = &oe.relation.label {
        out.push_str(", label: [");
        out.push_str(&creole_to_typst(label));
        out.push(']');
    }
    if let Some(p) = label_pos {
        let _ = write!(out, ", label-pos: ({:.2}pt, {:.2}pt)", p.x, p.y);
    }
    if let Some(c) = &oe.relation.color {
        if let Some(typst_color) = puml_color_to_typst(c) {
            out.push_str(", color: ");
            out.push_str(&typst_color);
        }
    }
    if let Some(note) = &oe.relation.note {
        out.push_str(", note: [");
        for (i, line) in note.lines().enumerate() {
            if i > 0 {
                out.push_str(" \\ ");
            }
            out.push_str(&creole_to_typst(line));
        }
        out.push(']');
    }
    // After orientation swap, `mult-from` / `role-from` corresponds to the
    // new source of the edge — which is the IR's `to` side iff we swapped.
    let (mult_src, mult_dst, role_src, role_dst) = if oe.swapped {
        (
            &oe.relation.mult_to,
            &oe.relation.mult_from,
            &oe.relation.role_to,
            &oe.relation.role_from,
        )
    } else {
        (
            &oe.relation.mult_from,
            &oe.relation.mult_to,
            &oe.relation.role_from,
            &oe.relation.role_to,
        )
    };
    if let Some(s) = mult_src {
        out.push_str(", mult-from: [");
        out.push_str(&creole_to_typst(s));
        out.push(']');
    }
    if let Some(s) = mult_dst {
        out.push_str(", mult-to: [");
        out.push_str(&creole_to_typst(s));
        out.push(']');
    }
    if let Some(s) = role_src {
        out.push_str(", role-from: [");
        out.push_str(&creole_to_typst(s));
        out.push(']');
    }
    if let Some(s) = role_dst {
        out.push_str(", role-to: [");
        out.push_str(&creole_to_typst(s));
        out.push(']');
    }

    out.push_str(", path: (");
    for (i, (c1, c2, end)) in segments.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let _ = write!(
            out,
            "(c1: ({:.2}pt, {:.2}pt), c2: ({:.2}pt, {:.2}pt), end: ({:.2}pt, {:.2}pt))",
            c1.x, c1.y, c2.x, c2.y, end.x, end.y,
        );
    }
    if segments.len() == 1 {
        out.push(',');
    }
    out.push_str(")),\n");
}

pub(super) fn emit_couple_edge(
    out: &mut String,
    ce: &CoupleEdge,
    segments: &[(Point, Point, Point)],
    start: Point,
    from_side: Side,
    to_side: Side,
) {
    let _ = write!(
        out,
        "    (from-couple: ({}, {}), to: {}, head-from: \"none\", head-to: \"none\", style: \"dashed\", from-side: \"{}\", to-side: \"{}\", start: ({:.2}pt, {:.2}pt)",
        ce.a_idx, ce.b_idx, ce.c_idx,
        from_side.keyword(), to_side.keyword(),
        start.x, start.y,
    );
    if let Some(label) = &ce.relation.label {
        out.push_str(", label: [");
        out.push_str(&creole_to_typst(label));
        out.push(']');
    }
    out.push_str(", path: (");
    for (i, (c1, c2, end)) in segments.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        let _ = write!(
            out,
            "(c1: ({:.2}pt, {:.2}pt), c2: ({:.2}pt, {:.2}pt), end: ({:.2}pt, {:.2}pt))",
            c1.x, c1.y, c2.x, c2.y, end.x, end.y,
        );
    }
    if segments.len() == 1 {
        out.push(',');
    }
    out.push_str(")),\n");
}

pub(super) fn emit_packages(
    out: &mut String,
    containers: &[Container],
    container_bboxes: &[Option<(Point, Point)>],
) {
    out.push_str("  packages: (\n");
    for (i, c) in containers.iter().enumerate() {
        let Some((top_left, bot_right)) = container_bboxes[i] else {
            continue;
        };
        let w = bot_right.x - top_left.x;
        let h = bot_right.y - top_left.y;
        let _ = write!(
            out,
            "    (x: {:.2}pt, y: {:.2}pt, w: {:.2}pt, h: {:.2}pt, kind: \"{}\", label: [",
            top_left.x,
            top_left.y,
            w,
            h,
            container_kind_keyword(c),
        );
        out.push_str(&creole_to_typst(&c.label));
        out.push(']');
        if let Some(s) = &c.stereotype {
            out.push_str(", stereotype: [");
            out.push_str(&creole_to_typst(s));
            out.push(']');
        }
        out.push_str("),\n");
    }
    out.push_str("  ),\n");
}

fn head_keyword(h: ArrowHead) -> &'static str {
    match h {
        ArrowHead::None => "none",
        ArrowHead::TriangleOpen => "triangle-open",
        ArrowHead::ArrowOpen => "arrow-open",
        ArrowHead::DiamondOpen => "diamond-open",
        ArrowHead::DiamondFilled => "diamond-filled",
        ArrowHead::Cross => "cross",
        ArrowHead::Plus => "plus",
        ArrowHead::CircleConnect => "circle",
        // SocketOpen / SocketClosed land in M7 (`Foo -( Bar`); v1
        // painter has no socket head, fall back to plain arrow.
        ArrowHead::SocketOpen => "socket-open",
        ArrowHead::SocketClosed => "socket-closed",
    }
}

fn line_style_keyword(s: LineStyle) -> &'static str {
    match s {
        LineStyle::Solid => "solid",
        LineStyle::Dashed => "dashed",
        LineStyle::Dotted => "dotted",
    }
}

/// Painter-side `kind:` keyword for an entity. Class-family
/// compartments emit their `class` / `interface` / … keyword;
/// notes and lollipops have dedicated keywords; recognized desc-family
/// shapes (M5 core: `actor` / `database` / `component` / `node`) emit
/// their USymbol keyword and the painter dispatches to a per-shape
/// renderer. Unrecognized USymbols fall back to the compartment painter
/// (`"class"` keyword) until M5+M8 add their painters.
fn entity_kind_keyword(entity: &Entity) -> &'static str {
    if matches!(entity.kind_data, EntityKindData::Note { .. }) {
        return "note";
    }
    if matches!(entity.kind_data, EntityKindData::Object { .. }) {
        return "object";
    }
    if entity.usymbol == USymbol::Interface {
        return "lollipop";
    }
    if let EntityKindData::Compartment { kind, .. } = &entity.kind_data {
        return kind.keyword();
    }
    // Desc-family Plain entities: route to the painters that exist.
    // Keep this list in sync with the `kind ==` dispatch chain in
    // `vendor/blockcell/src/cuca.typ::cuca-layout` and `cuca-probe`.
    match entity.usymbol {
        USymbol::Actor => "actor",
        USymbol::Database => "database",
        USymbol::Component => "component",
        USymbol::Node => "node",
        USymbol::UseCase => "usecase",
        USymbol::Cloud => "cloud",
        USymbol::Rectangle => "rectangle",
        USymbol::Folder => "folder",
        USymbol::Frame => "frame",
        USymbol::File => "file",
        USymbol::Queue => "queue",
        USymbol::Storage => "storage",
        USymbol::Hexagon => "hexagon",
        USymbol::Card => "card",
        USymbol::Artifact => "artifact",
        USymbol::Collections => "collections",
        USymbol::Action => "action",
        USymbol::Process => "process",
        USymbol::Label => "label",
        USymbol::Stack => "stack",
        USymbol::Agent => "agent",
        USymbol::Person => "person",
        USymbol::Boundary => "boundary",
        USymbol::Control => "control",
        USymbol::EntityDomain => "entity-domain",
        _ => "class",
    }
}

/// Painter-side `kind:` keyword for a container. `together` carries
/// no border / label, so we tag it specially.
fn container_kind_keyword(c: &Container) -> &'static str {
    if c.together {
        return "together";
    }
    match c.usymbol {
        USymbol::Package => "package",
        USymbol::Folder => "folder",
        USymbol::Frame => "frame",
        USymbol::Cloud => "cloud",
        USymbol::Node => "node",
        _ => "package",
    }
}
