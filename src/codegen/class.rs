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
    ArrowHead, ClassDiagram, Container, ContainerKind, Direction as IrDirection, Entity,
    EntityKind, HideOptions, LineStyle, Member, Relation, Skinparam, Visibility,
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
    let overrides = emit_skinparam_preamble(out, &diag.skinparams);

    if let Some(title) = &diag.title {
        out.push_str("#align(center)[*");
        out.push_str(&typst_escape(title));
        out.push_str("*]\n\n");
    }

    if diag.entities.is_empty() {
        out.push_str("// (empty class diagram)\n");
        return;
    }

    let geoms: Vec<ClassGeom> = diag
        .entities
        .iter()
        .map(|e| class_geom_filtered(e, &diag.hide))
        .collect();

    // Build the visual graph with one box per entity and one edge per
    // relation, swapping endpoints so the "owner / parent" end is at
    // the source (lower rank in TB layout).
    let mut vg = VisualGraph::new(Orientation::TopToBottom);
    let handles: Vec<_> = geoms
        .iter()
        .map(|g| vg.add_node(Element::new_box(g.size, Orientation::TopToBottom)))
        .collect();

    let mut oriented: Vec<OrientedEdge> = Vec::with_capacity(diag.relations.len());
    let mut couple_edges: Vec<CoupleEdge> = Vec::new();
    for rel in &diag.relations {
        // Couple-link relations (`(A, B) -- C`) bypass orient_relation:
        // they don't have a single "from" entity. We feed both A and B
        // as edges into Sugiyama so it accounts for the C-attached
        // anchor, but the actual rendered path is computed later.
        if let Some((a, b)) = &rel.from_couple {
            let Some(ai) = diag.entities.iter().position(|e| &e.id == a) else {
                continue;
            };
            let Some(bi) = diag.entities.iter().position(|e| &e.id == b) else {
                continue;
            };
            let Some(ci) = diag.entities.iter().position(|e| e.id == rel.to) else {
                continue;
            };
            // Add two virtual edges so Sugiyama places C below A and B.
            vg.add_edge(Edge::default(), handles[ai], handles[ci]);
            vg.add_edge(Edge::default(), handles[bi], handles[ci]);
            couple_edges.push(CoupleEdge {
                a_idx: ai,
                b_idx: bi,
                c_idx: ci,
                relation: rel.clone(),
            });
            continue;
        }
        let Some(oe) = orient_relation(rel, &diag.entities) else {
            continue;
        };
        let (src, dst) = (oe.src_idx, oe.dst_idx);
        oriented.push(oe);
        vg.add_edge(Edge::default(), handles[src], handles[dst]);
    }

    vg.layout();

    let mut top_lefts: Vec<Point> = handles.iter().map(|h| vg.pos(*h).bbox(false).0).collect();

    // Cluster-grouping pass: Sugiyama's mincross doesn't know about
    // packages, so members of different clusters can land intermingled
    // within a rank. Re-order each rank by cluster chain (root → leaf)
    // and re-pack x with a slightly larger gap between cluster
    // boundaries. This fixes the within-rank ordering; it does NOT
    // prevent the case where Cluster A's widest member at rank 0
    // extends past Cluster B's narrower member at rank 1 — the cluster
    // bboxes can still touch or overlap. A proper fix needs compound
    // layout (per-cluster sub-Sugiyama + super-Sugiyama composition),
    // tracked as part of the M1 refinement.
    if !diag.containers.is_empty() {
        regroup_ranks_by_cluster(&mut top_lefts, &geoms, &diag.containers, &diag.entities);
    }

    out.push_str("#class-layout(\n");
    if let Some(c) = &overrides.class_fill {
        out.push_str(&format!("  default-fill: {c},\n"));
    }
    if let Some(c) = &overrides.class_stroke_color {
        out.push_str(&format!("  stroke: 1pt + {c},\n"));
    }
    if let Some(c) = &overrides.edge_color {
        out.push_str(&format!("  edge-color: {c},\n"));
    }
    if let Some(c) = &overrides.package_fill {
        out.push_str(&format!("  package-fill: {c},\n"));
    }
    if let Some(c) = &overrides.package_stroke_color {
        out.push_str(&format!("  package-stroke: 0.6pt + {c},\n"));
    }
    out.push_str("  classes: (\n");
    for (i, entity) in diag.entities.iter().enumerate() {
        emit_class(out, top_lefts[i], entity, &diag.hide);
    }
    out.push_str("  ),\n");

    let class_bboxes: Vec<(Point, Point)> = (0..diag.entities.len())
        .map(|i| (top_lefts[i], top_lefts[i].add(geoms[i].size)))
        .collect();

    if !diag.containers.is_empty() {
        emit_packages(out, &diag.containers, &diag.entities, &class_bboxes);
    }

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
        // Class diagrams traditionally use orthogonal (Manhattan) edges,
        // not curves. Try a 3-segment Z route first (down-across-down for
        // TB layout); fall back to obstacle-aware bezier routing only if
        // the Manhattan route would intersect a class bbox.
        let segments = match try_manhattan_route(start, end, &obstacles) {
            Some(segs) => segs,
            None => match pathplan::route_edge(start, end, &obstacles, route_opts) {
                Ok(cubics) => cubics
                    .into_iter()
                    .map(|c| c.into_painter_segment())
                    .collect(),
                Err(_) => straight_fallback(start, end, EDGE_FORCE_MAX_PT),
            },
        };

        emit_edge(out, oe, &segments);
    }
    // Association-class edges: a dashed connector from the midpoint of
    // (A, B)'s box centers to C. We don't try to anchor on the actual
    // routed A-B path — the box-centers approximation is good enough
    // for short hops.
    for ce in &couple_edges {
        let a_center = box_center(&geoms[ce.a_idx], top_lefts[ce.a_idx]);
        let b_center = box_center(&geoms[ce.b_idx], top_lefts[ce.b_idx]);
        let start = Point::new(
            (a_center.x + b_center.x) / 2.0,
            (a_center.y + b_center.y) / 2.0,
        );
        let end = top_anchor(&geoms[ce.c_idx], top_lefts[ce.c_idx]);
        let segments = vec![cubic_from_straight(start, end)];
        emit_couple_edge(out, ce, &segments, start);
    }
    out.push_str("  ),\n");

    out.push_str(")\n");
}

fn box_center(g: &ClassGeom, top_left: Point) -> Point {
    Point::new(top_left.x + g.size.x / 2.0, top_left.y + g.size.y / 2.0)
}

fn emit_couple_edge(
    out: &mut String,
    ce: &CoupleEdge,
    segments: &[(Point, Point, Point)],
    start: Point,
) {
    let _ = write!(
        out,
        "    (from-couple: ({}, {}), to: {}, head-from: \"none\", head-to: \"none\", style: \"dashed\", start: ({:.2}pt, {:.2}pt)",
        ce.a_idx, ce.b_idx, ce.c_idx, start.x, start.y,
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

struct CoupleEdge {
    /// Index of A in `diag.entities`.
    a_idx: usize,
    /// Index of B in `diag.entities`.
    b_idx: usize,
    /// Index of the association class (C).
    c_idx: usize,
    relation: Relation,
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

/// Try to route `start → end` as 3 axis-aligned segments
/// (down → across → down for TB layout). Returns `None` if any segment
/// would clip a class bbox in `obstacles`, in which case the caller
/// falls back to bezier routing.
fn try_manhattan_route(
    start: Point,
    end: Point,
    obstacles: &[pathplan::Box],
) -> Option<Vec<(Point, Point, Point)>> {
    const X_TOL: f64 = 1.0;
    if (start.x - end.x).abs() < X_TOL {
        // Source and target share an x — emit a single straight cubic.
        let dy = end.y - start.y;
        let c1 = Point::new(start.x, start.y + dy / 3.0);
        let c2 = Point::new(start.x, start.y + 2.0 * dy / 3.0);
        // Even a straight line should miss any in-between obstacle for
        // this trivial route to be safe.
        if obstacles
            .iter()
            .any(|ob| seg_intersects_box(start, end, ob))
        {
            return None;
        }
        return Some(vec![(c1, c2, end)]);
    }

    // Z-route: vertical down to mid-y, horizontal across, vertical down.
    let mid_y = (start.y + end.y) / 2.0;
    let p1 = start;
    let p2 = Point::new(start.x, mid_y);
    let p3 = Point::new(end.x, mid_y);
    let p4 = end;

    for ob in obstacles {
        if seg_intersects_box(p1, p2, ob)
            || seg_intersects_box(p2, p3, ob)
            || seg_intersects_box(p3, p4, ob)
        {
            return None;
        }
    }

    Some(vec![
        cubic_from_straight(p1, p2),
        cubic_from_straight(p2, p3),
        cubic_from_straight(p3, p4),
    ])
}

/// Express a straight line a→b as a (c1, c2, end) cubic Bezier whose
/// path is exactly the line. Control handles sit at 1/3 and 2/3 along.
fn cubic_from_straight(a: Point, b: Point) -> (Point, Point, Point) {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    (
        Point::new(a.x + dx / 3.0, a.y + dy / 3.0),
        Point::new(a.x + 2.0 * dx / 3.0, a.y + 2.0 * dy / 3.0),
        b,
    )
}

/// True iff the axis-aligned segment a→b touches the rectangle `ob`.
/// We only call this with axis-aligned (vertical or horizontal)
/// segments — the diagonal branch returns `false` defensively.
fn seg_intersects_box(a: Point, b: Point, ob: &pathplan::Box) -> bool {
    let lo = ob.min;
    let hi = ob.max;
    if (a.x - b.x).abs() < 1e-6 {
        // Vertical segment at x.
        let x = a.x;
        if x <= lo.x || x >= hi.x {
            return false;
        }
        let y_lo = a.y.min(b.y);
        let y_hi = a.y.max(b.y);
        return !(y_hi <= lo.y || y_lo >= hi.y);
    }
    if (a.y - b.y).abs() < 1e-6 {
        // Horizontal segment at y.
        let y = a.y;
        if y <= lo.y || y >= hi.y {
            return false;
        }
        let x_lo = a.x.min(b.x);
        let x_hi = a.x.max(b.x);
        return !(x_hi <= lo.x || x_lo >= hi.x);
    }
    false
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

fn class_geom_filtered(entity: &Entity, hide: &HideOptions) -> ClassGeom {
    if entity.kind == EntityKind::Note {
        return note_geom(entity);
    }
    if entity.kind == EntityKind::Circle {
        return lollipop_geom(entity);
    }
    let show_fields = !(hide.fields || hide.members);
    let show_methods = !(hide.methods || hide.members);
    let show_marker = !hide.circle;
    let show_stereo = !hide.stereotype;
    let name_w = name_width_pt_filtered(entity, show_marker, show_stereo);
    let field_w = if show_fields {
        entity.fields.iter().map(member_width_pt).fold(0., f64::max)
    } else {
        0.0
    };
    let method_w = if show_methods {
        entity.methods.iter().map(member_width_pt).fold(0., f64::max)
    } else {
        0.0
    };
    let content_w = name_w.max(field_w).max(method_w);
    let total_w = content_w + 2. * PAD_X_PT;

    let row_h = LINE_HEIGHT_PT + 2. * PAD_Y_PT;
    let name_lines = if show_stereo && entity.stereotype.is_some() {
        2.0
    } else {
        1.0
    };
    let name_h = name_lines * row_h;
    let fields_h = if show_fields {
        entity.fields.len() as f64 * row_h
    } else {
        0.0
    };
    let methods_h = if show_methods {
        entity.methods.len() as f64 * row_h
    } else {
        0.0
    };
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

/// Lollipop / interface circle: a small filled disc with the label
/// rendered below. Width = max(disc, label width). Height = disc + gap
/// + label.
const LOLLIPOP_DIAMETER_PT: f64 = 14.0;
const LOLLIPOP_LABEL_GAP_PT: f64 = 2.0;

fn lollipop_geom(entity: &Entity) -> ClassGeom {
    let label_w = text_width_pt(&entity.display, BODY_EM);
    let total_w = label_w.max(LOLLIPOP_DIAMETER_PT);
    let total_h = LOLLIPOP_DIAMETER_PT + LOLLIPOP_LABEL_GAP_PT + LINE_HEIGHT_PT;
    ClassGeom {
        size: Point::new(total_w, total_h),
        mid_x: total_w / 2.0,
    }
}

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

fn name_width_pt_filtered(entity: &Entity, show_marker: bool, show_stereo: bool) -> f64 {
    // Generic parameters render as a small dashed box at the top-right
    // corner of the class — they don't widen the name line.
    let name = entity.display.clone();
    let stereo_w = if show_stereo {
        entity
            .stereotype
            .as_deref()
            .map(|s| text_width_pt(&format!("«{s}»"), BODY_EM))
            .unwrap_or(0.)
    } else {
        0.0
    };
    let title_w = text_width_pt(&name, NAME_EM);
    let marker = if show_marker && entity.kind.marker_letter().is_some() {
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

fn emit_class(out: &mut String, top_left: Point, entity: &Entity, hide: &HideOptions) {
    if entity.kind == EntityKind::Note {
        return emit_note(out, top_left, entity);
    }
    if entity.kind == EntityKind::Circle {
        return emit_lollipop(out, top_left, entity);
    }
    let show_fields = !(hide.fields || hide.members);
    let show_methods = !(hide.methods || hide.members);
    out.push_str(&format!(
        "    (x: {:.2}pt, y: {:.2}pt, kind: \"{}\", name: [",
        top_left.x,
        top_left.y,
        entity.kind.keyword(),
    ));
    out.push_str(&creole_to_typst(&entity.display));
    out.push(']');

    if let Some(g) = &entity.generic {
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
    } else if let Some((letter, color)) = &entity.stereotype_marker {
        out.push_str(&format!(
            ", marker-letter: \"{}\"",
            typst_str_escape(letter)
        ));
        if let Some(c) = color {
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

    let fields_to_emit: &[Member] = if show_fields { &entity.fields } else { &[] };
    let methods_to_emit: &[Member] = if show_methods { &entity.methods } else { &[] };

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

    out.push_str("),\n");
}

fn emit_lollipop(out: &mut String, top_left: Point, entity: &Entity) {
    out.push_str(&format!(
        "    (x: {:.2}pt, y: {:.2}pt, kind: \"lollipop\", name: [",
        top_left.x, top_left.y,
    ));
    out.push_str(&creole_to_typst(&entity.display));
    out.push_str("]),\n");
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
        out.push_str(&creole_to_typst(label));
        out.push(']');
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

/// Within-rank gap between two siblings of the same cluster.
const INTRA_CLUSTER_GAP_PT: f64 = 6.0;
/// Within-rank gap between members of different clusters (or between
/// a clustered and non-clustered node), so the package borders won't
/// touch the next entity over.
const INTER_CLUSTER_GAP_PT: f64 = 18.0;
/// Tolerance for grouping by y-rank: positions whose y differs by less
/// than this are considered to share a rank. The Sugiyama placer uses
/// integer ranks, so any non-zero tolerance handles f64 round-off.
const RANK_Y_TOL_PT: f64 = 0.5;

/// Build the cluster chain (outer → inner) for each entity. Entities
/// outside any container get an empty chain.
fn cluster_chains(containers: &[Container], entities: &[Entity]) -> Vec<Vec<usize>> {
    // parent[c] = index of the container whose `children_containers`
    // list holds c, or `None` if c is a top-level container.
    let mut parent: Vec<Option<usize>> = vec![None; containers.len()];
    for (pi, c) in containers.iter().enumerate() {
        for &ci in &c.children_containers {
            if ci < parent.len() {
                parent[ci] = Some(pi);
            }
        }
    }
    entities
        .iter()
        .map(|e| {
            // Pick the innermost container that holds the entity. If
            // multiple do (legal in PUML for `together` overlaps), the
            // last one wins — gives the user a stable rule.
            let direct = containers
                .iter()
                .enumerate()
                .rev()
                .find(|(_, c)| c.children_entities.iter().any(|cid| cid == &e.id))
                .map(|(i, _)| i);
            let mut chain = Vec::new();
            let mut cur = direct;
            while let Some(c) = cur {
                chain.push(c);
                cur = parent[c];
            }
            chain.reverse();
            chain
        })
        .collect()
}

/// Re-arrange entities within each rank so cluster siblings sit
/// contiguously, then re-pack the x axis. Preserves intra-cluster
/// relative order from Sugiyama.
fn regroup_ranks_by_cluster(
    top_lefts: &mut [Point],
    geoms: &[ClassGeom],
    containers: &[Container],
    entities: &[Entity],
) {
    let chains = cluster_chains(containers, entities);

    // Bucket entities by approximate y-rank (Sugiyama emits one y per rank).
    let mut ranks: Vec<Vec<usize>> = Vec::new();
    let mut bucket_y: Vec<f64> = Vec::new();
    for (i, p) in top_lefts.iter().enumerate() {
        let mut placed = false;
        for (bi, &y) in bucket_y.iter().enumerate() {
            if (p.y - y).abs() < RANK_Y_TOL_PT {
                ranks[bi].push(i);
                placed = true;
                break;
            }
        }
        if !placed {
            ranks.push(vec![i]);
            bucket_y.push(p.y);
        }
    }

    for row in ranks.iter_mut() {
        if row.len() < 2 {
            continue;
        }
        // Anchor the re-pack at the leftmost original x in the rank, so
        // subsequent ranks (already laid out elsewhere) don't drift.
        let start_x = row
            .iter()
            .map(|&i| top_lefts[i].x)
            .fold(f64::INFINITY, f64::min);
        // Sort by cluster chain (lex), tie-break by original x.
        row.sort_by(|&a, &b| {
            chains[a].cmp(&chains[b]).then(
                top_lefts[a]
                    .x
                    .partial_cmp(&top_lefts[b].x)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
        });
        let mut x = start_x;
        for i in 0..row.len() {
            let ei = row[i];
            top_lefts[ei] = Point::new(x, top_lefts[ei].y);
            if i + 1 < row.len() {
                let nxt = row[i + 1];
                let gap = if chains[ei] == chains[nxt] {
                    INTRA_CLUSTER_GAP_PT
                } else {
                    INTER_CLUSTER_GAP_PT
                };
                x += geoms[ei].size.x + gap;
            }
        }
    }
}

/// Padding between a container's bbox and the AABB of its members.
/// Outer containers reserve more so a label can sit on top without
/// touching member geometry; inner-nesting containers shrink so they
/// nest cleanly inside the parent's pad.
const CONTAINER_PAD_PT: f64 = 14.0;
/// Reserved space above the AABB top for a labeled container header.
/// Together (anonymous) gets 0; everything else gets this band.
const CONTAINER_LABEL_PT: f64 = 14.0;

fn emit_packages(
    out: &mut String,
    containers: &[Container],
    entities: &[Entity],
    class_bboxes: &[(Point, Point)],
) {
    // Build entity-id → index map once.
    let mut id_to_idx: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::with_capacity(entities.len());
    for (i, e) in entities.iter().enumerate() {
        id_to_idx.insert(e.id.as_str(), i);
    }

    // Recursively compute each container's AABB. Nested containers sit
    // inside their parent's pad band; their own children contribute via
    // the same recursion, so the result is naturally hierarchical.
    let mut bboxes: Vec<Option<(Point, Point)>> = vec![None; containers.len()];
    for i in 0..containers.len() {
        compute_container_bbox(
            i,
            containers,
            &id_to_idx,
            class_bboxes,
            &mut bboxes,
        );
    }

    out.push_str("  packages: (\n");
    for (i, c) in containers.iter().enumerate() {
        let Some((min, max)) = bboxes[i] else {
            continue;
        };
        // `together` is anonymous, no header band.
        let label_band = if matches!(c.kind, ContainerKind::Together) {
            0.0
        } else {
            CONTAINER_LABEL_PT
        };
        let pad = CONTAINER_PAD_PT;
        let x = min.x - pad;
        let y = min.y - pad - label_band;
        let w = (max.x - min.x) + 2.0 * pad;
        let h = (max.y - min.y) + 2.0 * pad + label_band;
        let _ = write!(
            out,
            "    (x: {:.2}pt, y: {:.2}pt, w: {:.2}pt, h: {:.2}pt, kind: \"{}\", label: [",
            x,
            y,
            w,
            h,
            container_kind_keyword(c.kind),
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

fn compute_container_bbox(
    idx: usize,
    containers: &[Container],
    id_to_idx: &std::collections::HashMap<&str, usize>,
    class_bboxes: &[(Point, Point)],
    out: &mut [Option<(Point, Point)>],
) {
    if out[idx].is_some() {
        return;
    }
    let c = &containers[idx];
    let mut acc: Option<(Point, Point)> = None;
    for child_id in &c.children_entities {
        if let Some(&ei) = id_to_idx.get(child_id.as_str()) {
            let (mn, mx) = class_bboxes[ei];
            acc = Some(merge_bbox(acc, (mn, mx)));
        }
    }
    for &child_idx in &c.children_containers {
        // Recurse first so nested bbox is available.
        compute_container_bbox(child_idx, containers, id_to_idx, class_bboxes, out);
        if let Some(child_bb) = out[child_idx] {
            // Inflate the child by its own pad+label band so the parent
            // bbox accounts for the nested container's outer rectangle,
            // not just its inner contents.
            let inner_pad = CONTAINER_PAD_PT;
            let inner_band = if matches!(containers[child_idx].kind, ContainerKind::Together) {
                0.0
            } else {
                CONTAINER_LABEL_PT
            };
            let inflated = (
                Point::new(child_bb.0.x - inner_pad, child_bb.0.y - inner_pad - inner_band),
                Point::new(child_bb.1.x + inner_pad, child_bb.1.y + inner_pad),
            );
            acc = Some(merge_bbox(acc, inflated));
        }
    }
    out[idx] = acc;
}

fn merge_bbox(acc: Option<(Point, Point)>, new: (Point, Point)) -> (Point, Point) {
    match acc {
        None => new,
        Some((amin, amax)) => (
            Point::new(amin.x.min(new.0.x), amin.y.min(new.0.y)),
            Point::new(amax.x.max(new.1.x), amax.y.max(new.1.y)),
        ),
    }
}

fn container_kind_keyword(k: ContainerKind) -> &'static str {
    match k {
        ContainerKind::Package => "package",
        ContainerKind::Namespace => "namespace",
        ContainerKind::Folder => "folder",
        ContainerKind::Frame => "frame",
        ContainerKind::Cloud => "cloud",
        ContainerKind::Node => "node",
        ContainerKind::Together => "together",
    }
}

/// Per-class-layout overrides resolved from `skinparam` and `!theme`
/// directives. Values left as `None` fall through to the painter's
/// built-in defaults.
#[derive(Default, Clone)]
struct PaintOverrides {
    class_fill: Option<String>,
    class_stroke_color: Option<String>,
    edge_color: Option<String>,
    package_fill: Option<String>,
    package_stroke_color: Option<String>,
}

fn emit_skinparam_preamble(out: &mut String, params: &[Skinparam]) -> PaintOverrides {
    let mut text_args: Vec<String> = Vec::new();
    let mut page_fill: Option<String> = None;
    let mut overrides = PaintOverrides::default();
    // Optionally expand a `!theme NAME` value into a synthetic skinparam
    // sequence (handled here so all theme names funnel through the same
    // override resolution).
    let expanded = expand_theme(params);
    for p in expanded.iter() {
        // Both PascalCase and camelCase variants appear in real-world
        // PlantUML; normalize to lowercase for lookup.
        let key = p.key.to_ascii_lowercase();
        match key.as_str() {
            "backgroundcolor" => {
                if let Some(color) = puml_color_to_typst(&p.value) {
                    page_fill = Some(color);
                }
            }
            "defaultfontname" | "defaultfontfamily" => {
                let trimmed = p.value.trim_matches('"');
                if !trimmed.is_empty() {
                    text_args.push(format!("font: \"{}\"", typst_str_escape(trimmed)));
                }
            }
            "defaultfontsize" => {
                if let Ok(pt) = p.value.trim().parse::<u32>() {
                    text_args.push(format!("size: {pt}pt"));
                }
            }
            "classbackgroundcolor" => {
                overrides.class_fill = puml_color_to_typst(&p.value);
            }
            "classbordercolor" | "classborder" => {
                overrides.class_stroke_color = puml_color_to_typst(&p.value);
            }
            "arrowcolor" => {
                overrides.edge_color = puml_color_to_typst(&p.value);
            }
            "packagebackgroundcolor" | "packagebackground" => {
                overrides.package_fill = puml_color_to_typst(&p.value);
            }
            "packagebordercolor" => {
                overrides.package_stroke_color = puml_color_to_typst(&p.value);
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
    overrides
}

/// Expand `!theme <name>` into a flat list of synthetic skinparams plus
/// the original list. PlantUML has dozens of themes; we ship a tiny
/// subset (vibrant, plain, amiga, cerulean) — unknown theme names are
/// passed through with no expansion, so `!theme some-other` silently
/// keeps the default styling rather than failing.
fn expand_theme(params: &[Skinparam]) -> Vec<Skinparam> {
    let mut out: Vec<Skinparam> = Vec::with_capacity(params.len());
    for p in params {
        let key = p.key.to_ascii_lowercase();
        if key == "theme" || key == "!theme" {
            let theme = p.value.trim().to_ascii_lowercase();
            for (k, v) in builtin_theme(&theme) {
                out.push(Skinparam {
                    key: k.to_string(),
                    value: v.to_string(),
                    line: p.line,
                });
            }
            continue;
        }
        out.push(p.clone());
    }
    out
}

fn builtin_theme(name: &str) -> &'static [(&'static str, &'static str)] {
    match name {
        "plain" | "default" => &[],
        "vibrant" => &[
            ("backgroundColor", "#FFFEF7"),
            ("classBackgroundColor", "#FFFB96"),
            ("classBorderColor", "#5C5400"),
            ("packageBackgroundColor", "#FFFCEA"),
            ("packageBorderColor", "#9C8800"),
            ("arrowColor", "#5C5400"),
        ],
        "amiga" => &[
            ("backgroundColor", "#0044AA"),
            ("classBackgroundColor", "#FFFFFF"),
            ("classBorderColor", "#000000"),
            ("arrowColor", "#FFFFFF"),
        ],
        "cerulean" => &[
            ("backgroundColor", "#FFFFFF"),
            ("classBackgroundColor", "#E5F0FA"),
            ("classBorderColor", "#2780E3"),
            ("arrowColor", "#2780E3"),
            ("packageBackgroundColor", "#F4F8FC"),
        ],
        _ => &[],
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

/// Convert PlantUML Creole-lite markup to Typst markup. Handles
/// `**bold**`, `//italic//`, literal `\n` (line break), and
/// `<color:NAME>…</color>`. All other characters are escaped via
/// `typst_markup_escape`. Nested formatting works (e.g. `**//foo//**`)
/// because the body of each construct is recursed into.
fn creole_to_typst(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"**") {
            if let Some(end) = find_marker(bytes, i + 2, b"**") {
                let body = &s[i + 2..end];
                out.push_str("#strong[");
                out.push_str(&creole_to_typst(body));
                out.push(']');
                i = end + 2;
                continue;
            }
        }
        if bytes[i..].starts_with(b"//") {
            if let Some(end) = find_marker(bytes, i + 2, b"//") {
                let body = &s[i + 2..end];
                out.push_str("#emph[");
                out.push_str(&creole_to_typst(body));
                out.push(']');
                i = end + 2;
                continue;
            }
        }
        if bytes[i..].starts_with(b"\\n") {
            out.push_str(" \\ ");
            i += 2;
            continue;
        }
        if bytes[i..].starts_with(b"<color:") {
            let after_open = i + b"<color:".len();
            if let Some(rel) = bytes[after_open..].iter().position(|&b| b == b'>') {
                let color_end = after_open + rel;
                let color = &s[after_open..color_end];
                let body_start = color_end + 1;
                if let Some(rel_close) = s[body_start..].find("</color>") {
                    let body = &s[body_start..body_start + rel_close];
                    let typst_color = puml_color_to_typst(color)
                        .unwrap_or_else(|| "black".to_string());
                    let _ = write!(out, "#text(fill: {})[", typst_color);
                    out.push_str(&creole_to_typst(body));
                    out.push(']');
                    i = body_start + rel_close + b"</color>".len();
                    continue;
                }
            }
        }
        // Default: escape one char and advance by its UTF-8 length.
        let ch = s[i..].chars().next().unwrap();
        out.push_str(&escape_one(ch));
        i += ch.len_utf8();
    }
    out
}

fn find_marker(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from >= bytes.len() {
        return None;
    }
    let n = needle.len();
    let mut i = from;
    while i + n <= bytes.len() {
        if &bytes[i..i + n] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn escape_one(c: char) -> String {
    match c {
        '\\' => "\\\\".into(),
        '*' | '_' | '#' | '$' | '`' | '~' | '@' | '<' | '>' | '[' | ']' | '{' | '}' => {
            format!("\\{c}")
        }
        _ => c.to_string(),
    }
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
            stereotype_marker: None,
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
            from_couple: None,
            from_port: None,
            to_port: None,
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
            note: None,
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
            from_couple: None,
            from_port: None,
            to_port: None,
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
            note: None,
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
            from_couple: None,
            from_port: None,
            to_port: None,
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
            note: None,
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
            from_couple: None,
            from_port: None,
            to_port: None,
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
            note: None,
            line: 0,
        });
        let s = render(diag);
        assert!(s.contains("from: 1, to: 0"));
    }
}
