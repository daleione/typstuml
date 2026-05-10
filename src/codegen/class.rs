//! Class diagram codegen.
//!
//! Pipeline mirrors `record_graph.rs`:
//!
//! 1. Estimate per-class bounding box and middle-x (Sugiyama lays nodes
//!    out using the box, painter uses the middle-x to anchor edges).
//! 2. Build a `VisualGraph` in `TopToBottom` orientation. Edges flow from
//!    the rendered "source" (parent / owner / etc.) to the "target". For
//!    inheritance / aggregation / composition the codegen swaps the
//!    user-written endpoints so the head end ("parent" or "owner")
//!    always lands at the higher rank.
//! 3. Run pathplan with vertical entry/exit tangents. Fall back to a
//!    straight cubic when the constraint polygon rejects the input.
//! 4. Emit one `#class-layout(...)` call.
//!
//! M0 limitations (intentional, see `docs/class-diagram-design.md`):
//!   - No cluster (`package`/`namespace`) layout.
//!   - No orthogonal routing — all edges are cubic Beziers.
//!   - Edge label placement is naive (chord midpoint), no avoidance.
//!   - `containers` field on the IR is ignored.

use std::fmt::Write as _;

use crate::ir::{
    ArrowHead, ClassDiagram, Direction as IrDirection, Entity, EntityKind, LineStyle, Member,
    Relation, Skinparam, Visibility,
};
use crate::layout::geometry::Point;
use crate::layout::graph::{Edge, Element, Orientation, VisualGraph};
use crate::layout::pathplan;

const FONT_PT: f64 = 10.0;
/// Bold name glyphs run wider; the markup we emit also includes a
/// stereotype circle (`marker-w` ~ 14pt). 0.62em is conservative for
/// 10pt sans-serif.
const NAME_EM: f64 = 0.62;
/// Member rows are regular-weight; visibility glyph adds a small constant.
const BODY_EM: f64 = 0.55;
const PAD_X_PT: f64 = 0.6 * FONT_PT;
const PAD_Y_PT: f64 = 0.3 * FONT_PT;
const LINE_HEIGHT_PT: f64 = 1.2 * FONT_PT;
/// Stereotype circle box width allowance (matches painter's `marker-w =
/// 1.4em` when the entity has a circle marker).
const MARKER_W_PT: f64 = 1.4 * FONT_PT;
/// Bezier control-handle pull (same scheme as record_graph.rs).
const EDGE_FORCE_MAX_PT: f64 = 30.0;
const ROUTE_PADDING_PT: f64 = 1.0;

pub fn emit(out: &mut String, diag: &ClassDiagram) {
    emit_skinparam_preamble(out, &diag.skinparams);

    if let Some(title) = &diag.title {
        out.push_str("#align(center)[*");
        out.push_str(&typst_escape(title));
        out.push_str("*]\n\n");
    }

    if diag.entities.is_empty() {
        out.push_str("// (empty class diagram)\n");
        return;
    }

    let geoms: Vec<ClassGeom> = diag.entities.iter().map(class_geom).collect();

    // Build the visual graph with one box per entity and one edge per
    // relation, swapping endpoints so the "owner / parent" end is at
    // the source (lower rank in TB layout).
    let mut vg = VisualGraph::new(Orientation::TopToBottom);
    let handles: Vec<_> = geoms
        .iter()
        .map(|g| vg.add_node(Element::new_box(g.size, Orientation::TopToBottom)))
        .collect();

    let mut oriented: Vec<OrientedEdge> = Vec::with_capacity(diag.relations.len());
    for rel in &diag.relations {
        let Some(oe) = orient_relation(rel, &diag.entities) else {
            continue;
        };
        let (src, dst) = (oe.src_idx, oe.dst_idx);
        oriented.push(oe);
        vg.add_edge(Edge::default(), handles[src], handles[dst]);
    }

    vg.layout();

    let top_lefts: Vec<Point> = handles.iter().map(|h| vg.pos(*h).bbox(false).0).collect();

    out.push_str("#class-layout(\n");
    out.push_str("  classes: (\n");
    for (i, entity) in diag.entities.iter().enumerate() {
        emit_class(out, top_lefts[i], entity);
    }
    out.push_str("  ),\n");

    let class_bboxes: Vec<(Point, Point)> = (0..diag.entities.len())
        .map(|i| (top_lefts[i], top_lefts[i].add(geoms[i].size)))
        .collect();

    out.push_str("  edges: (\n");
    let route_opts = pathplan::RouteOpts {
        obstacle_padding: ROUTE_PADDING_PT,
        // Vertical tangents so the cubic launches downward from a class's
        // bottom edge and arrives downward into the next class's top edge.
        src_tangent: Point::new(0.0, 1.0),
        dst_tangent: Point::new(0.0, 1.0),
    };
    for oe in &oriented {
        let from = oe.src_idx;
        let to = oe.dst_idx;
        let start = bot_anchor(&geoms[from], top_lefts[from]);
        let end = top_anchor(&geoms[to], top_lefts[to]);

        let obstacles: Vec<pathplan::Box> = (0..diag.entities.len())
            .filter(|i| *i != from && *i != to)
            .map(|i| pathplan::Box::new(class_bboxes[i].0, class_bboxes[i].1))
            .collect();
        let segments = match pathplan::route_edge(start, end, &obstacles, route_opts) {
            Ok(cubics) => cubics
                .into_iter()
                .map(|c| c.into_painter_segment())
                .collect(),
            Err(_) => straight_fallback(start, end, EDGE_FORCE_MAX_PT),
        };

        emit_edge(out, oe, &segments);
    }
    out.push_str("  ),\n");

    out.push_str(")\n");
}

struct OrientedEdge {
    src_idx: usize,
    dst_idx: usize,
    head_src: ArrowHead,
    head_dst: ArrowHead,
    /// `true` iff the rendered edge runs in the opposite direction from
    /// the user-written `(rel.from, rel.to)` order. Used to map IR-side
    /// `mult_from`/`mult_to` (and roles) onto the rendered ends.
    swapped: bool,
    relation: Relation,
}

/// Pick an orientation for the rendered edge such that
/// generalization / composition / aggregation flows from "parent" to
/// "child" (head end → no-head end). Plain associations / dependencies
/// keep the user-written direction unless an explicit `Up` / `Left`
/// hint flips them — for top-to-bottom Sugiyama, both `Up` and `Left`
/// mean "the target should be visually before the source", so they
/// flip equivalently.
fn orient_relation(rel: &Relation, entities: &[Entity]) -> Option<OrientedEdge> {
    let from_idx = entities.iter().position(|e| e.id == rel.from)?;
    let to_idx = entities.iter().position(|e| e.id == rel.to)?;

    let owner_like = |h: ArrowHead| {
        matches!(
            h,
            ArrowHead::TriangleOpen | ArrowHead::DiamondFilled | ArrowHead::DiamondOpen
        )
    };

    let mut swapped = if owner_like(rel.head_to) && !owner_like(rel.head_from) {
        // `B --|> A` — head at `to` is the parent. Swap so parent is source.
        true
    } else {
        false
    };

    if matches!(rel.direction, Some(IrDirection::Up) | Some(IrDirection::Left)) {
        swapped = !swapped;
    }

    let (src_idx, dst_idx, head_src, head_dst) = if swapped {
        (to_idx, from_idx, rel.head_to, rel.head_from)
    } else {
        (from_idx, to_idx, rel.head_from, rel.head_to)
    };

    Some(OrientedEdge {
        src_idx,
        dst_idx,
        head_src,
        head_dst,
        swapped,
        relation: rel.clone(),
    })
}

fn straight_fallback(start: Point, end: Point, force_max: f64) -> Vec<(Point, Point, Point)> {
    let dist = start.distance_to(end);
    let force = (dist * 0.4).min(force_max);
    let c1 = start.add(Point::new(0.0, force));
    let c2 = end.sub(Point::new(0.0, force));
    vec![(c1, c2, end)]
}

struct ClassGeom {
    size: Point,
    /// Mid-x within the local frame. Used to anchor edge endpoints.
    mid_x: f64,
}

fn class_geom(entity: &Entity) -> ClassGeom {
    if entity.kind == EntityKind::Note {
        return note_geom(entity);
    }
    let name_w = name_width_pt(entity);
    let field_w = entity
        .fields
        .iter()
        .map(member_width_pt)
        .fold(0., f64::max);
    let method_w = entity
        .methods
        .iter()
        .map(member_width_pt)
        .fold(0., f64::max);
    let content_w = name_w.max(field_w).max(method_w);
    let total_w = content_w + 2. * PAD_X_PT;

    let row_h = LINE_HEIGHT_PT + 2. * PAD_Y_PT;
    let name_lines = if entity.stereotype.is_some() { 2.0 } else { 1.0 };
    let name_h = name_lines * row_h;
    let fields_h = entity.fields.len() as f64 * row_h;
    let methods_h = entity.methods.len() as f64 * row_h;
    let total_h = name_h + fields_h + methods_h;

    ClassGeom {
        size: Point::new(total_w, total_h),
        mid_x: total_w / 2.0,
    }
}

/// Width allowance for the dog-ear fold drawn at the top-right of a
/// note. Codegen has to reserve this in the bbox so the painter's fold
/// triangle doesn't push edge endpoints into the body text.
const NOTE_DOG_EAR_PT: f64 = 8.0;

fn note_geom(entity: &Entity) -> ClassGeom {
    let body = entity.body.as_deref().unwrap_or("");
    let line_widths: Vec<f64> = if body.is_empty() {
        vec![0.0]
    } else {
        body.lines().map(|l| text_width_pt(l, BODY_EM)).collect()
    };
    let max_w = line_widths.iter().cloned().fold(0.0, f64::max);
    let total_w = max_w + 2.0 * PAD_X_PT + NOTE_DOG_EAR_PT;
    let total_h = (line_widths.len() as f64) * LINE_HEIGHT_PT + 2.0 * PAD_Y_PT;
    ClassGeom {
        size: Point::new(total_w, total_h),
        mid_x: total_w / 2.0,
    }
}

fn name_width_pt(entity: &Entity) -> f64 {
    let mut name = String::new();
    name.push_str(&entity.display);
    if let Some(g) = &entity.generic {
        name.push_str(" <");
        name.push_str(g);
        name.push('>');
    }
    let stereo_w = entity
        .stereotype
        .as_deref()
        .map(|s| text_width_pt(&format!("«{s}»"), BODY_EM))
        .unwrap_or(0.);
    let title_w = text_width_pt(&name, NAME_EM);
    let marker = if entity.kind.marker_letter().is_some() {
        MARKER_W_PT
    } else {
        0.
    };
    title_w.max(stereo_w) + marker
}

fn member_width_pt(member: &Member) -> f64 {
    let mut s = String::new();
    if member.visibility != Visibility::None {
        s.push_str(member.visibility.glyph());
        s.push(' ');
    }
    s.push_str(&member.body);
    text_width_pt(&s, BODY_EM)
}

fn glyph_width_pt(c: char, em: f64) -> f64 {
    if c.is_ascii() {
        FONT_PT * em
    } else {
        FONT_PT
    }
}

fn text_width_pt(s: &str, em: f64) -> f64 {
    s.chars().map(|c| glyph_width_pt(c, em)).sum()
}

fn bot_anchor(g: &ClassGeom, top_left: Point) -> Point {
    Point::new(top_left.x + g.mid_x, top_left.y + g.size.y)
}

fn top_anchor(g: &ClassGeom, top_left: Point) -> Point {
    Point::new(top_left.x + g.mid_x, top_left.y)
}

fn emit_class(out: &mut String, top_left: Point, entity: &Entity) {
    if entity.kind == EntityKind::Note {
        return emit_note(out, top_left, entity);
    }
    out.push_str(&format!(
        "    (x: {:.2}pt, y: {:.2}pt, kind: \"{}\", name: [",
        top_left.x,
        top_left.y,
        entity.kind.keyword(),
    ));
    out.push_str(&typst_markup_escape(&entity.display));
    out.push(']');

    if let Some(g) = &entity.generic {
        out.push_str(", generic: [");
        out.push_str(&typst_markup_escape(g));
        out.push(']');
    }
    if let Some(s) = &entity.stereotype {
        out.push_str(", stereotype: [");
        out.push_str(&typst_markup_escape(s));
        out.push(']');
    }
    if let Some(c) = &entity.fill {
        if let Some(typst_color) = puml_color_to_typst(c) {
            out.push_str(", fill: ");
            out.push_str(&typst_color);
        }
    }

    out.push_str(", fields: (");
    for (i, m) in entity.fields.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        emit_member(out, m);
    }
    if entity.fields.len() == 1 {
        out.push(',');
    }
    out.push(')');

    out.push_str(", methods: (");
    for (i, m) in entity.methods.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        emit_member(out, m);
    }
    if entity.methods.len() == 1 {
        out.push(',');
    }
    out.push(')');

    out.push_str("),\n");
}

fn emit_note(out: &mut String, top_left: Point, entity: &Entity) {
    let body = entity.body.as_deref().unwrap_or("");
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
            out.push_str(&typst_markup_escape(line));
        }
    }
    out.push_str("]),\n");
}

fn emit_member(out: &mut String, m: &Member) {
    let _ = write!(
        out,
        "(vis: \"{}\", body: [{}]",
        m.visibility.glyph(),
        typst_markup_escape(&m.body),
    );
    if m.is_static {
        out.push_str(", static: true");
    }
    if m.is_abstract {
        out.push_str(", abstract: true");
    }
    out.push(')');
}

fn emit_edge(out: &mut String, oe: &OrientedEdge, segments: &[(Point, Point, Point)]) {
    let _ = write!(
        out,
        "    (from: {}, to: {}, head-from: \"{}\", head-to: \"{}\", style: \"{}\"",
        oe.src_idx,
        oe.dst_idx,
        head_keyword(oe.head_src),
        head_keyword(oe.head_dst),
        line_style_keyword(oe.relation.line_style),
    );
    if let Some(label) = &oe.relation.label {
        out.push_str(", label: [");
        out.push_str(&typst_markup_escape(label));
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
        out.push_str(&typst_markup_escape(s));
        out.push(']');
    }
    if let Some(s) = mult_dst {
        out.push_str(", mult-to: [");
        out.push_str(&typst_markup_escape(s));
        out.push(']');
    }
    if let Some(s) = role_src {
        out.push_str(", role-from: [");
        out.push_str(&typst_markup_escape(s));
        out.push(']');
    }
    if let Some(s) = role_dst {
        out.push_str(", role-to: [");
        out.push_str(&typst_markup_escape(s));
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
    }
}

fn line_style_keyword(s: LineStyle) -> &'static str {
    match s {
        LineStyle::Solid => "solid",
        LineStyle::Dashed => "dashed",
        LineStyle::Dotted => "dotted",
    }
}

fn emit_skinparam_preamble(out: &mut String, params: &[Skinparam]) {
    let mut text_args: Vec<String> = Vec::new();
    let mut page_fill: Option<String> = None;
    for p in params {
        match p.key.as_str() {
            "backgroundColor" | "BackgroundColor" => {
                if let Some(color) = puml_color_to_typst(&p.value) {
                    page_fill = Some(color);
                }
            }
            "defaultFontName" | "DefaultFontName" | "defaultFontFamily" => {
                let trimmed = p.value.trim_matches('"');
                if !trimmed.is_empty() {
                    text_args.push(format!("font: \"{}\"", typst_str_escape(trimmed)));
                }
            }
            "defaultFontSize" | "DefaultFontSize" => {
                if let Ok(pt) = p.value.trim().parse::<u32>() {
                    text_args.push(format!("size: {pt}pt"));
                }
            }
            _ => {}
        }
    }
    let had_page_fill = page_fill.is_some();
    if let Some(color) = page_fill {
        out.push_str(&format!("#set page(fill: {color})\n"));
    }
    if !text_args.is_empty() {
        out.push_str(&format!("#set text({})\n", text_args.join(", ")));
    }
    if had_page_fill || !text_args.is_empty() {
        out.push('\n');
    }
}

/// Best-effort PUML color → Typst color expression. Mirrors
/// `sequence.rs::puml_color_to_typst`; once class is in, we should
/// extract this helper to a shared module.
fn puml_color_to_typst(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let hex = s.strip_prefix('#').unwrap_or(s);
    let lower = hex.to_ascii_lowercase();
    let named = match lower.as_str() {
        "red" => Some("#FF0000"),
        "blue" => Some("#0000FF"),
        "green" => Some("#008000"),
        "yellow" => Some("#FFFF00"),
        "orange" => Some("#FFA500"),
        "purple" => Some("#800080"),
        "pink" => Some("#FFC0CB"),
        "black" => Some("#000000"),
        "white" => Some("#FFFFFF"),
        "gray" | "grey" => Some("#808080"),
        "lightblue" => Some("#ADD8E6"),
        "lightgreen" => Some("#90EE90"),
        "lightyellow" => Some("#FFFFE0"),
        "lightgray" | "lightgrey" => Some("#D3D3D3"),
        "darkblue" => Some("#00008B"),
        "darkgreen" => Some("#006400"),
        "darkred" => Some("#8B0000"),
        "gold" => Some("#FFD700"),
        "cyan" | "aqua" => Some("#00FFFF"),
        "magenta" => Some("#FF00FF"),
        _ => None,
    };
    let final_hex = match named {
        Some(h) => h.trim_start_matches('#').to_string(),
        None => {
            if hex.chars().all(|c| c.is_ascii_hexdigit()) && (hex.len() == 3 || hex.len() == 6) {
                hex.to_string()
            } else {
                return None;
            }
        }
    };
    Some(format!("rgb(\"#{}\")", final_hex))
}

fn typst_markup_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '*' | '_' | '#' | '$' | '`' | '~' | '@' | '<' | '>' | '[' | ']' | '{' | '}' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

fn typst_str_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out
}

fn typst_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('#', "\\#")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::EntityKind;

    fn entity(id: &str, kind: EntityKind) -> Entity {
        Entity {
            kind,
            id: id.into(),
            display: id.into(),
            generic: None,
            stereotype: None,
            fields: Vec::new(),
            methods: Vec::new(),
            body: None,
            fill: None,
            line: 0,
        }
    }

    fn render(diag: ClassDiagram) -> String {
        let mut s = String::new();
        emit(&mut s, &diag);
        s
    }

    #[test]
    fn empty_diagram_produces_placeholder() {
        let s = render(ClassDiagram::default());
        assert!(s.contains("(empty class diagram)"));
    }

    #[test]
    fn extends_swaps_so_parent_is_source() {
        // user wrote: `class Dog`, `class Animal`, `Dog --|> Animal`.
        // Animal is parent → should appear as the source of the rendered edge.
        let mut diag = ClassDiagram::default();
        diag.entities.push(entity("Dog", EntityKind::Class));
        diag.entities.push(entity("Animal", EntityKind::Class));
        diag.relations.push(Relation {
            from: "Dog".into(),
            to: "Animal".into(),
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
            line: 0,
        });
        let s = render(diag);
        // After orient_relation swap: src_idx = 1 (Animal), dst_idx = 0 (Dog).
        assert!(s.contains("from: 1, to: 0"));
        assert!(s.contains("head-from: \"triangle-open\""));
        assert!(s.contains("head-to: \"none\""));
    }

    #[test]
    fn association_keeps_user_order() {
        let mut diag = ClassDiagram::default();
        diag.entities.push(entity("A", EntityKind::Class));
        diag.entities.push(entity("B", EntityKind::Class));
        diag.relations.push(Relation {
            from: "A".into(),
            to: "B".into(),
            head_from: ArrowHead::None,
            head_to: ArrowHead::ArrowOpen,
            line_style: LineStyle::Solid,
            direction: None,
            label: None,
            mult_from: None,
            mult_to: None,
            role_from: None,
            role_to: None,
            stereotype: None,
            color: None,
            line: 0,
        });
        let s = render(diag);
        assert!(s.contains("from: 0, to: 1"));
        assert!(s.contains("head-to: \"arrow-open\""));
    }

    #[test]
    fn members_emit_with_visibility_glyphs() {
        let mut e = entity("Foo", EntityKind::Class);
        e.fields.push(Member {
            visibility: Visibility::Public,
            is_static: false,
            is_abstract: false,
            body: "name: String".into(),
            line: 0,
        });
        e.methods.push(Member {
            visibility: Visibility::Private,
            is_static: false,
            is_abstract: true,
            body: "render()".into(),
            line: 0,
        });
        let mut diag = ClassDiagram::default();
        diag.entities.push(e);
        let s = render(diag);
        assert!(s.contains("(vis: \"+\", body: [name: String]),"));
        assert!(s.contains("(vis: \"-\", body: [render()], abstract: true),"));
    }

    #[test]
    fn entity_emits_kind_and_stereotype() {
        let mut e = entity("Repo", EntityKind::Interface);
        e.stereotype = Some("Service".into());
        e.generic = Some("T".into());
        let mut diag = ClassDiagram::default();
        diag.entities.push(e);
        let s = render(diag);
        assert!(s.contains("kind: \"interface\""));
        assert!(s.contains("stereotype: [Service]"));
        assert!(s.contains("generic: [T]"));
    }

    #[test]
    fn swap_relabels_multiplicity_and_role_to_rendered_ends() {
        // Pre-fix: `swapped` was always false (the helper was a no-op),
        // so a `--|>` arrow with multiplicities emitted them on the IR's
        // original ends — which were now the *wrong* ends after the
        // owner-on-top swap. With the OrientedEdge.swapped flag, the
        // labels should follow the rendered ends.
        let mut diag = ClassDiagram::default();
        diag.entities.push(entity("Sub", EntityKind::Class));
        diag.entities.push(entity("Sup", EntityKind::Class));
        diag.relations.push(Relation {
            from: "Sub".into(),
            to: "Sup".into(),
            head_from: ArrowHead::None,
            head_to: ArrowHead::TriangleOpen,
            line_style: LineStyle::Solid,
            direction: None,
            label: None,
            mult_from: Some("S".into()),
            mult_to: Some("T".into()),
            role_from: Some("rs".into()),
            role_to: Some("rt".into()),
            stereotype: None,
            color: None,
            line: 0,
        });
        let s = render(diag);
        // After swap, rendered source = Sup (IR's `to`) and rendered
        // target = Sub (IR's `from`). `mult-from`/`role-from` track the
        // rendered source.
        assert!(s.contains("from: 1, to: 0"));
        assert!(
            s.contains("mult-from: [T]") && s.contains("mult-to: [S]"),
            "mult labels follow rendered ends after swap; got: {s}"
        );
        assert!(
            s.contains("role-from: [rt]") && s.contains("role-to: [rs]"),
            "role labels follow rendered ends after swap; got: {s}"
        );
    }

    #[test]
    fn direction_up_flips_rendered_edge() {
        // `A -up-> B` — user wants B above A in TB layout, so the
        // rendered edge should run from B (source/top) to A (target/bot).
        let mut diag = ClassDiagram::default();
        diag.entities.push(entity("A", EntityKind::Class));
        diag.entities.push(entity("B", EntityKind::Class));
        diag.relations.push(Relation {
            from: "A".into(),
            to: "B".into(),
            head_from: ArrowHead::None,
            head_to: ArrowHead::ArrowOpen,
            line_style: LineStyle::Solid,
            direction: Some(IrDirection::Up),
            label: None,
            mult_from: None,
            mult_to: None,
            role_from: None,
            role_to: None,
            stereotype: None,
            color: None,
            line: 0,
        });
        let s = render(diag);
        assert!(
            s.contains("from: 1, to: 0"),
            "Up flips edge: B → A; got: {s}"
        );
        // Head was on `to` originally; after flip it's on the new source.
        assert!(s.contains("head-from: \"arrow-open\""));
    }

    #[test]
    fn direction_left_flips_like_up() {
        // For TB orientation `Left` is equivalent to `Up`: the target
        // should appear before (above) the source.
        let mut diag = ClassDiagram::default();
        diag.entities.push(entity("A", EntityKind::Class));
        diag.entities.push(entity("B", EntityKind::Class));
        diag.relations.push(Relation {
            from: "A".into(),
            to: "B".into(),
            head_from: ArrowHead::None,
            head_to: ArrowHead::ArrowOpen,
            line_style: LineStyle::Solid,
            direction: Some(IrDirection::Left),
            label: None,
            mult_from: None,
            mult_to: None,
            role_from: None,
            role_to: None,
            stereotype: None,
            color: None,
            line: 0,
        });
        let s = render(diag);
        assert!(s.contains("from: 1, to: 0"));
    }
}
