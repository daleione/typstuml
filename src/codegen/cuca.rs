//! Class diagram codegen.
//!
//! Pipeline:
//!
//! 1. Estimate per-class bounding boxes (`geom`). Note / lollipop have
//!    their own shapes.
//! 2. Inflate every box by `SIBLING_PAD_PT / 2` on each side so Sugiyama
//!    leaves visible gaps between siblings; deflate the resulting
//!    top-lefts after layout.
//! 3. Build oriented edges + association-class couple virtual edges,
//!    then drive compound layout (`layout::compound_layout`).
//! 4. Post-layout fixes: leaf-only recenter when all predecessors share
//!    a rank; couple-edge A/B column alignment and C-clear-of-chord.
//! 5. Pick anchor sides + smart-align coord per edge (`route`), route
//!    through line-of-sight → Manhattan → pathplan → straight cubic
//!    fallback.
//! 6. Emit one `#cuca-layout(...)` call (`emit`).
//!
//! Heuristics this file owns:
//!
//! - `SIBLING_PAD_PT` (halo around each class bbox before Sugiyama).
//! - `ROUTE_PADDING_PT` (pathplan obstacle padding).
//! - `EDGE_FORCE_MAX_PT` (straight-fallback control-handle pull).
//! - `chord_pad` inside the couple-edge post-fix loop (visible dashed
//!   connector length).

mod emit;
mod geom;
mod layout;
pub(super) mod probe;
mod route;
mod text;
mod theme;

use crate::ir::{
    ArrowHead, CucaDiagram, Direction as IrDirection, Entity, HideOptions, LayoutDirection,
    LineStyle, Relation,
};
use crate::layout::geometry::Point;
use crate::layout::graph::Orientation;
use crate::layout::pathplan;
use crate::runtime::MeasurementSet;

use self::emit::{emit_class, emit_couple_edge, emit_edge, emit_packages, EmitGeom};
use self::geom::{
    Side, anchor_for_side, bot_anchor, box_center, class_geom_filtered, left_anchor,
    right_anchor, top_anchor, ClassGeom,
};
use self::layout::{compound_layout, LabelBand};
use self::route::{
    cubic_from_straight, diagonal_path, line_of_sight_clear, pick_edge_sides,
    side_tangent, smart_align_coord, straight_fallback, try_manhattan_route,
    SMART_ALIGN_HEADROOM_PT,
};
use self::text::typst_escape;
use self::theme::emit_skinparam_preamble;

/// Bezier control-handle pull for the straight-fallback path (same
/// scheme as `record_graph.rs`).
const EDGE_FORCE_MAX_PT: f64 = 30.0;
/// Obstacle padding the pathplan router uses when routing detours.
const ROUTE_PADDING_PT: f64 = 1.0;
/// Per-entity geometry: prefer measurement from pass-1 when available,
/// otherwise fall back to the Rust-side heuristic. We keep the heuristic
/// path in tree so `--no-measure` and the unit tests still work — and
/// so the codegen has something usable for entities whose probe failed
/// (e.g. measure compile error on a malformed Creole label).
fn resolve_geom(
    entity: &Entity,
    hide: &HideOptions,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) -> ClassGeom {
    if let Some(set) = measurements {
        let id = probe::class_id(diagram_idx, entity);
        if let Some(m) = set.get(&id) {
            return ClassGeom {
                size: Point::new(m.width_pt, m.height_pt),
                mid_x: m.width_pt / 2.0,
            };
        }
        // Falling back is correct behavior — a probe might be missing
        // if the pass-1 source failed to emit it (codegen bug) or the
        // entity was added between pass-1 and pass-2 (impossible
        // today, but a guardrail). Log once per missing ID at warn
        // level once the diagnostic system supports it.
    }
    class_geom_filtered(entity, hide)
}

/// Per-container label band measurements from pass-1. Returns one
/// entry per container in declaration order; `None` for `together`
/// (anonymous, no band) or when the measurement is missing.
fn resolve_label_bands(
    diag: &CucaDiagram,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) -> Vec<Option<LabelBand>> {
    let Some(set) = measurements else {
        return vec![None; diag.containers.len()];
    };
    (0..diag.containers.len())
        .map(|ci| {
            if !probe::has_label_band(&diag.containers[ci]) {
                return None;
            }
            let id = probe::package_id(diagram_idx, ci);
            set.get(&id).map(|m| LabelBand {
                w_pt: m.width_pt,
                h_pt: m.height_pt,
            })
        })
        .collect()
}

/// Halo added around each class bbox before Sugiyama placement so
/// siblings end up with a real gap (the underlying simple.rs / BK
/// pack nodes shoulder-to-shoulder). Half on each side, both axes —
/// gives ~SIBLING_PAD_PT inter-rank y-gap and ~SIBLING_PAD_PT
/// inter-sibling x-gap. Tuned so PlantUML-style fan-outs (heads,
/// basic) have visible breathing room without ballooning lollipop
/// and package layouts.
const SIBLING_PAD_PT: f64 = 24.0;

pub fn emit(
    out: &mut String,
    diag: &CucaDiagram,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) {
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
        .map(|e| resolve_geom(e, &diag.hide, measurements, diagram_idx))
        .collect();
    // Sugiyama's `simple` and BK passes pack siblings shoulder-to-
    // shoulder (gap ≈ EPSILON). For class diagrams we want visible
    // breathing room — fan-outs need each child clear of the next so
    // arrowheads don't overlap. Inflate every box by SIBLING_PAD_PT,
    // run layout against the halos, then deflate the resulting
    // top-lefts so the actual class draws centered inside its halo.
    let inflated_geoms: Vec<ClassGeom> = geoms
        .iter()
        .map(|g| ClassGeom {
            size: Point::new(g.size.x + SIBLING_PAD_PT, g.size.y + SIBLING_PAD_PT),
            mid_x: g.mid_x + SIBLING_PAD_PT / 2.0,
        })
        .collect();

    let orientation = match diag.direction {
        LayoutDirection::TopToBottom => Orientation::TopToBottom,
        LayoutDirection::LeftToRight => Orientation::LeftToRight,
    };
    let is_lr = diag.direction == LayoutDirection::LeftToRight;

    // Collect oriented edges and association-class couple edges. Both
    // contribute to layout (the couple edges add A→C and B→C virtual
    // dependencies so Sugiyama puts C below the pair).
    let mut oriented: Vec<OrientedEdge> = Vec::with_capacity(diag.relations.len());
    let mut couple_edges: Vec<CoupleEdge> = Vec::new();
    for rel in &diag.relations {
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
            couple_edges.push(CoupleEdge {
                a_idx: ai,
                b_idx: bi,
                c_idx: ci,
                relation: rel.clone(),
            });
            continue;
        }
        let normalized = normalize_use_case_relation(rel.clone());
        let Some(oe) = orient_relation(&normalized, &diag.entities) else {
            continue;
        };
        oriented.push(oe);
    }

    // Layout edges feeding Sugiyama: real oriented edges + two virtual
    // edges per couple-link, A→C and C→B, so C lands at the rank
    // between A and B. PlantUML draws an "apoint" anchor on the A-B
    // chord at C's row and connects it horizontally to C; placing C
    // mid-rank lets us reproduce that with a horizontal dashed
    // connector + a small dot on the chord.
    let mut layout_edges: Vec<(usize, usize)> = Vec::with_capacity(
        oriented.len() + 2 * couple_edges.len(),
    );
    for oe in &oriented {
        layout_edges.push((oe.src_idx, oe.dst_idx));
    }
    for ce in &couple_edges {
        layout_edges.push((ce.a_idx, ce.c_idx));
        layout_edges.push((ce.c_idx, ce.b_idx));
    }

    // Per-container label band measurements (None for `together`,
    // None when --no-measure or pass-1 missed the probe). Layout uses
    // these to size each container's outer rectangle: tall multi-line
    // labels get a taller header band, wide labels enforce a minimum
    // outer width so the title doesn't overflow narrow contents.
    let label_bands = resolve_label_bands(diag, measurements, diagram_idx);

    // Compound layout: one sub-Sugiyama per cluster (recursive into
    // nested containers), then a super-Sugiyama treating every
    // top-level cluster as one box. This guarantees non-overlapping
    // cluster rectangles even when one cluster's widest member is
    // wider than another cluster's narrowest. With no containers the
    // whole thing falls back to a flat single-pass layout.
    let layout = compound_layout(
        diag,
        &inflated_geoms,
        orientation,
        &layout_edges,
        &label_bands,
    );
    // Shift each top-left by half the halo so the class itself sits
    // centered inside its inflated bbox. Container bboxes returned by
    // the layout already include the halos around their members; we
    // tighten them below for clean container rendering.
    let half_pad = Point::new(SIBLING_PAD_PT / 2.0, SIBLING_PAD_PT / 2.0);
    let mut top_lefts: Vec<Point> = layout.top_lefts.iter().map(|p| p.add(half_pad)).collect();
    let container_bboxes = layout.container_bboxes;

    // Post-layout centering: when an entity's predecessors all sit
    // on the same rank, center it under their midpoint. The Sugiyama
    // BK pass tends to align such children with whichever corner sweep
    // won — a single child below two parents (e.g. Animal under
    // Dog/Cat in basic.puml) ends up flush with one parent instead
    // of centered. We do this *before* the chord-overlap fix below
    // because re-centering can resolve some overlaps too.
    {
        let mut preds: Vec<Vec<usize>> = vec![Vec::new(); diag.entities.len()];
        for &(s, d) in &layout_edges {
            preds[d].push(s);
        }
        let entity_count = diag.entities.len();
        for ei in 0..entity_count {
            let p = &preds[ei];
            if p.len() < 2 {
                continue;
            }
            let pred_y0 = top_lefts[p[0]].y;
            if !p.iter().all(|&pi| (top_lefts[pi].y - pred_y0).abs() < 1.0) {
                continue;
            }
            // Don't re-center if this entity has its own successors
            // that would themselves prefer different alignment — keep
            // the leaf-only rule simple.
            let mid_x_avg: f64 = p
                .iter()
                .map(|&pi| top_lefts[pi].x + geoms[pi].size.x / 2.0)
                .sum::<f64>()
                / p.len() as f64;
            let my_y = top_lefts[ei].y;
            let my_w = geoms[ei].size.x;
            let new_x = mid_x_avg - my_w / 2.0;
            let new_box = (
                Point::new(new_x, my_y),
                Point::new(new_x + my_w, my_y + geoms[ei].size.y),
            );
            // Reject if the move would crash into another entity at
            // ei's rank.
            let conflict = (0..entity_count).any(|j| {
                if j == ei || (top_lefts[j].y - my_y).abs() > 1.0 {
                    return false;
                }
                let other = (top_lefts[j], top_lefts[j].add(geoms[j].size));
                new_box.0.x < other.1.x
                    && other.0.x < new_box.1.x
                    && new_box.0.y < other.1.y
                    && other.0.y < new_box.1.y
            });
            if !conflict {
                top_lefts[ei] = Point::new(new_x, my_y);
            }
        }
    }

    // Couple-edge post-fixes:
    //
    // 1. Force A and B to share an x column so the chord is straight
    //    vertical (PlantUML's default look). Sugiyama's BK aligns long
    //    edges through their virtual midpoints, but adding A→C and
    //    C→B virtual edges puts C at the same rank as the A-B
    //    midpoint, and BK can favour C's column over A/B's. After
    //    shifting, A and B share the chord's mid-x.
    //
    // 2. If C straddles the chord column, push it past the chord on
    //    one side so the dashed connector reads as a clean horizontal
    //    line into C's near face.
    if !couple_edges.is_empty() {
        // Visible dashed-connector length between C and the chord
        // apoint. Tuned so a few dashes render at default stroke
        // thickness (PlantUML uses ~30pt of dashed line).
        let chord_pad: f64 = 32.0;
        let entity_count = diag.entities.len();
        let conflict_at_y = |new_box: (Point, Point), self_idx: usize, top_lefts: &[Point]| -> bool {
            (0..entity_count).any(|j| {
                if j == self_idx
                    || (top_lefts[j].y - new_box.0.y).abs() > 1.0
                {
                    return false;
                }
                let other = (top_lefts[j], top_lefts[j].add(geoms[j].size));
                new_box.0.x < other.1.x
                    && other.0.x < new_box.1.x
                    && new_box.0.y < other.1.y
                    && other.0.y < new_box.1.y
            })
        };
        for ce in &couple_edges {
            // Shift A and B (independently) to share an x column, so
            // the chord renders straight. Use the average of their
            // current mids so neither moves much.
            let a_mid = top_lefts[ce.a_idx].x + geoms[ce.a_idx].size.x / 2.0;
            let b_mid = top_lefts[ce.b_idx].x + geoms[ce.b_idx].size.x / 2.0;
            let chord_x = (a_mid + b_mid) / 2.0;
            for &idx in &[ce.a_idx, ce.b_idx] {
                let cur_mid = top_lefts[idx].x + geoms[idx].size.x / 2.0;
                if (cur_mid - chord_x).abs() < 1.0 {
                    continue;
                }
                let new_x = chord_x - geoms[idx].size.x / 2.0;
                let cur_y = top_lefts[idx].y;
                let new_box = (
                    Point::new(new_x, cur_y),
                    Point::new(new_x + geoms[idx].size.x, cur_y + geoms[idx].size.y),
                );
                if !conflict_at_y(new_box, idx, &top_lefts) {
                    top_lefts[idx] = Point::new(new_x, cur_y);
                }
            }

            // Now ensure C clears the chord by at least `chord_pad`
            // — both for the obvious "C straddles chord" case and the
            // "C just barely misses chord" case where the dashed
            // connector would render as a 1pt nub.
            let a_tl = top_lefts[ce.a_idx];
            let a_br = a_tl.add(geoms[ce.a_idx].size);
            let b_tl = top_lefts[ce.b_idx];
            let b_br = b_tl.add(geoms[ce.b_idx].size);
            let lo = a_tl.x.max(b_tl.x);
            let hi = a_br.x.min(b_br.x);
            let chord_x_eff = if hi - lo > SMART_ALIGN_HEADROOM_PT {
                (lo + hi) / 2.0
            } else {
                ((a_tl.x + a_br.x) / 2.0 + (b_tl.x + b_br.x) / 2.0) / 2.0
            };
            let c_tl = top_lefts[ce.c_idx];
            let c_w = geoms[ce.c_idx].size.x;
            let c_br = c_tl.add(geoms[ce.c_idx].size);
            let c_mid = c_tl.x + c_w / 2.0;
            // Decide which side C should sit on; default to whichever
            // side it's closer to (or right if it currently overlaps
            // the chord).
            let push_right = if chord_x_eff > c_tl.x && chord_x_eff < c_br.x {
                // Straddles — push to whichever side has more room.
                c_mid >= chord_x_eff
            } else {
                c_mid > chord_x_eff
            };
            let new_x = if push_right {
                let needed_left = chord_x_eff + chord_pad;
                if c_tl.x >= needed_left { c_tl.x } else { needed_left }
            } else {
                let needed_right = chord_x_eff - chord_pad;
                if c_br.x <= needed_right { c_tl.x } else { needed_right - c_w }
            };
            if (new_x - c_tl.x).abs() > 0.1 {
                top_lefts[ce.c_idx] = Point::new(new_x, c_tl.y);
            }
        }
    }

    out.push_str("#cuca-layout(\n");
    if is_lr {
        out.push_str("  direction: \"lr\",\n");
    }
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
        let g = EmitGeom {
            width_pt: geoms[i].size.x,
            height_pt: geoms[i].size.y,
        };
        emit_class(out, top_lefts[i], &g, entity, &diag.hide);
    }
    out.push_str("  ),\n");

    let class_bboxes: Vec<(Point, Point)> = (0..diag.entities.len())
        .map(|i| (top_lefts[i], top_lefts[i].add(geoms[i].size)))
        .collect();

    if !diag.containers.is_empty() {
        emit_packages(out, &diag.containers, &container_bboxes);
    }

    out.push_str("  edges: (\n");
    for oe in &oriented {
        let from = oe.src_idx;
        let to = oe.dst_idx;
        // Pick anchors based on the dominant displacement axis. Pure
        // top/bot ↔ top/bot anchoring (or right/left for LR) breaks down
        // when sibling clusters sit at the same rank: an edge's source
        // bot-mid is BELOW the target top-mid, forcing a U-turn. Whichever
        // axis the displacement is larger on, both endpoints anchor to
        // that side of their box.
        let (from_side, to_side) = pick_edge_sides(
            box_center(&geoms[from], top_lefts[from]),
            box_center(&geoms[to], top_lefts[to]),
            (top_lefts[from], top_lefts[from].add(geoms[from].size)),
            (top_lefts[to], top_lefts[to].add(geoms[to].size)),
            is_lr,
        );
        let mainly_vertical = matches!(from_side, Side::Top | Side::Bot);

        // Smart alignment: when both anchors are on the same axis and
        // the boxes' perpendicular extents overlap, place both anchors
        // at the same coordinate inside the overlap. This collapses
        // the Manhattan Z (3 segments, 2 turns) into a single straight
        // segment, which is the typical look for sibling-cluster
        // connections in PlantUML / dot.
        let aligned_coord = smart_align_coord(
            &geoms[from],
            top_lefts[from],
            &geoms[to],
            top_lefts[to],
            from_side,
            to_side,
        );
        let default_start = anchor_for_side(&geoms[from], top_lefts[from], from_side);
        let default_end = anchor_for_side(&geoms[to], top_lefts[to], to_side);
        let (start, end) = if let Some(coord) = aligned_coord {
            if mainly_vertical {
                (
                    Point::new(coord, default_start.y),
                    Point::new(coord, default_end.y),
                )
            } else {
                (
                    Point::new(default_start.x, coord),
                    Point::new(default_end.x, coord),
                )
            }
        } else {
            (default_start, default_end)
        };

        // Entity obstacles: every entity bbox except this edge's two
        // endpoints. M3 ranks clusters via Sugiyama and tighten pulls
        // them apart, so cross-cluster edges no longer need explicit
        // cluster-bbox obstacles to detour — the rank ordering keeps
        // the natural path from clipping through a sibling cluster.
        // try_manhattan_route's detour-bend remains the safety net for
        // residual obstacle-clipping cases.
        let obstacles: Vec<pathplan::Box> = (0..diag.entities.len())
            .filter(|i| *i != from && *i != to)
            .map(|i| pathplan::Box::new(class_bboxes[i].0, class_bboxes[i].1))
            .collect();
        // Tangents follow the actual anchor sides — bezier launch
        // direction matches the snap axis the painter will enforce,
        // otherwise the head tangent collapses to zero on edges where
        // we anchor on the perpendicular axis (e.g. a free-floating
        // note pointing horizontally to a class). The pathplan convention
        // is "direction of travel": at src we leave outward through the
        // face; at dst we approach inward through the face (opposite
        // sign of the outward normal).
        let route_opts = pathplan::RouteOpts {
            obstacle_padding: ROUTE_PADDING_PT,
            src_tangent: side_tangent(from_side),
            dst_tangent: side_tangent(to_side).neg(),
        };
        // Routing priority:
        //   1. Straight line of sight — single cubic with axis-aligned
        //      tangents from start to end. PlantUML-style fan-out from a
        //      hub: the line just goes diagonally across the gap, no
        //      Manhattan turn.
        //   2. Manhattan Z — for blocked diagonals, fall back to a
        //      down-across-down (or right-along-right) right-angle route.
        //   3. Pathplan bezier — for routes that need to detour around
        //      multiple obstacles.
        //   4. Forced straight cubic — last resort.
        let segments = if line_of_sight_clear(start, end, &obstacles) {
            diagonal_path(start, end, side_tangent(from_side))
        } else if let Some(segs) = try_manhattan_route(start, end, &obstacles, mainly_vertical) {
            segs
        } else {
            match pathplan::route_edge(start, end, &obstacles, route_opts) {
                Ok(cubics) => cubics
                    .into_iter()
                    .map(|c| c.into_painter_segment())
                    .collect(),
                Err(_) => straight_fallback(start, end, EDGE_FORCE_MAX_PT),
            }
        };

        emit_edge(out, oe, &segments, Some((from_side, to_side)), aligned_coord);
    }
    // Association-class edges. The layout pass placed C at the rank
    // between A and B (via virtual A→C, C→B edges), so the dashed
    // connector lands cleanly at C's row, perpendicular to the A-B
    // chord. We anchor on the chord at C's y-level and meet C on its
    // near side — same look as PlantUML's "apoint" connector. A small
    // dot is drawn at the chord anchor to mark the association point.
    for ce in &couple_edges {
        let a_tl = top_lefts[ce.a_idx];
        let a_br = a_tl.add(geoms[ce.a_idx].size);
        let b_tl = top_lefts[ce.b_idx];
        let b_br = b_tl.add(geoms[ce.b_idx].size);
        let c_tl = top_lefts[ce.c_idx];
        let c_br = c_tl.add(geoms[ce.c_idx].size);
        let a_center = box_center(&geoms[ce.a_idx], a_tl);
        let b_center = box_center(&geoms[ce.b_idx], b_tl);
        let c_center = box_center(&geoms[ce.c_idx], c_tl);

        // The A-B chord runs along whichever axis separates A and B.
        // For TB (the common case) that's vertical; the anchor x is
        // the smart-aligned column shared by A.bot and B.top, the
        // anchor y is C's row.
        let tb = a_br.y <= b_tl.y || b_br.y <= a_tl.y;
        let (anchor_x, anchor_y, end, from_side, to_side) = if tb {
            // Smart-align column for the A-B chord, mirroring
            // smart_align_coord above so the chord and apoint share x.
            let lo = a_tl.x.max(b_tl.x);
            let hi = a_br.x.min(b_br.x);
            let x = if hi - lo > SMART_ALIGN_HEADROOM_PT {
                (lo + hi) / 2.0
            } else {
                (a_center.x + b_center.x) / 2.0
            };
            // C is at a different rank from A and B (in between);
            // the connector runs horizontally at C's mid-y.
            let y = c_center.y;
            let (end, c_side) = if c_center.x >= x {
                (left_anchor(&geoms[ce.c_idx], c_tl), Side::Left)
            } else {
                (right_anchor(&geoms[ce.c_idx], c_tl), Side::Right)
            };
            // The apoint sits on the chord, so it "leaves" toward C
            // along the perpendicular axis — opposite of c_side.
            let from_side = match c_side {
                Side::Left => Side::Right,
                Side::Right => Side::Left,
                _ => unreachable!(),
            };
            (x, y, end, from_side, c_side)
        } else {
            // LR-style: A and B separated horizontally; the chord is
            // horizontal and the apoint is at C's column.
            let lo = a_tl.y.max(b_tl.y);
            let hi = a_br.y.min(b_br.y);
            let y = if hi - lo > SMART_ALIGN_HEADROOM_PT {
                (lo + hi) / 2.0
            } else {
                (a_center.y + b_center.y) / 2.0
            };
            let x = c_center.x;
            let (end, c_side) = if c_center.y >= y {
                (top_anchor(&geoms[ce.c_idx], c_tl), Side::Top)
            } else {
                (bot_anchor(&geoms[ce.c_idx], c_tl), Side::Bot)
            };
            let from_side = match c_side {
                Side::Top => Side::Bot,
                Side::Bot => Side::Top,
                _ => unreachable!(),
            };
            (x, y, end, from_side, c_side)
        };
        let _ = c_br;
        let start = Point::new(anchor_x, anchor_y);
        let segments = vec![cubic_from_straight(start, end)];
        emit_couple_edge(out, ce, &segments, start, from_side, to_side);
    }
    out.push_str("  ),\n");

    out.push_str(")\n");
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

/// Force use-case stereotype semantics on a relation. PlantUML draws
/// `<<include>>` and `<<extend>>` / `<<extends>>` as a dashed open
/// arrow regardless of which arrow token the user wrote — `A --> B :
/// <<include>>` is visually identical to `A ..> B : <<include>>`.
/// `rel.stereotype` is populated by the parser when the label
/// contains one of those tokens. No-op for any other (or absent)
/// stereotype.
fn normalize_use_case_relation(mut rel: Relation) -> Relation {
    let Some(st) = rel.stereotype.as_deref() else {
        return rel;
    };
    if !matches!(st, "include" | "extend" | "extends") {
        return rel;
    }
    rel.line_style = LineStyle::Dashed;
    if rel.head_from == ArrowHead::None && rel.head_to == ArrowHead::None {
        rel.head_to = ArrowHead::ArrowOpen;
    }
    rel
}

/// Pick an orientation for the rendered edge.
///
/// Default rule: keep the user-written direction (source → target →
/// top → bottom in TB), matching PlantUML. Earlier this code swapped
/// `B --|> A` so the parent (A) became the source/top — that gave
/// "semantically correct" inheritance trees but diverged from
/// PlantUML's text-order convention. The arrow head is rendered at
/// whichever end it was specified, so the parent visual is preserved
/// either way.
///
/// `direction: Up` / `Left` flips the edge — those keywords explicitly
/// mean "draw the target above/before the source".
fn orient_relation(rel: &Relation, entities: &[Entity]) -> Option<OrientedEdge> {
    let from_idx = entities.iter().position(|e| e.id == rel.from)?;
    let to_idx = entities.iter().position(|e| e.id == rel.to)?;

    let swapped = matches!(
        rel.direction,
        Some(IrDirection::Up) | Some(IrDirection::Left)
    );

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ClassFamilyKind, EntityKindData, LineStyle, Member, USymbol, Visibility};

    fn entity(id: &str, kind: ClassFamilyKind) -> Entity {
        Entity {
            usymbol: USymbol::None,
            id: id.into(),
            display: id.into(),
            stereotype: None,
            stereotype_marker: None,
            fill: None,
            line: 0,
            kind_data: EntityKindData::Compartment {
                kind,
                generic: None,
                fields: Vec::new(),
                methods: Vec::new(),
            },
        }
    }

    fn render(diag: CucaDiagram) -> String {
        let mut s = String::new();
        emit(&mut s, &diag, None, 0);
        s
    }

    #[test]
    fn empty_diagram_produces_placeholder() {
        let s = render(CucaDiagram::default());
        assert!(s.contains("(empty class diagram)"));
    }

    #[test]
    fn extends_keeps_user_text_order() {
        // user wrote: `class Dog`, `class Animal`, `Dog --|> Animal`.
        // PlantUML places the source (Dog) on top and the target
        // (Animal) below; the triangle is rendered at the target end
        // so it still visually points at the parent. We match that —
        // text order wins, no swap.
        let mut diag = CucaDiagram::default();
        diag.entities.push(entity("Dog", ClassFamilyKind::Class));
        diag.entities.push(entity("Animal", ClassFamilyKind::Class));
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
        assert!(s.contains("from: 0, to: 1"));
        assert!(s.contains("head-to: \"triangle-open\""));
        assert!(s.contains("head-from: \"none\""));
    }

    #[test]
    fn association_keeps_user_order() {
        let mut diag = CucaDiagram::default();
        diag.entities.push(entity("A", ClassFamilyKind::Class));
        diag.entities.push(entity("B", ClassFamilyKind::Class));
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
        let mut e = entity("Foo", ClassFamilyKind::Class);
        if let EntityKindData::Compartment { fields, methods, .. } = &mut e.kind_data {
            fields.push(Member {
                visibility: Visibility::Public,
                is_static: false,
                is_abstract: false,
                body: "name: String".into(),
                line: 0,
            });
            methods.push(Member {
                visibility: Visibility::Private,
                is_static: false,
                is_abstract: true,
                body: "render()".into(),
                line: 0,
            });
        }
        let mut diag = CucaDiagram::default();
        diag.entities.push(e);
        let s = render(diag);
        assert!(s.contains("(vis: \"+\", body: [name: String]),"));
        assert!(s.contains("(vis: \"-\", body: [render()], abstract: true),"));
    }

    #[test]
    fn entity_emits_kind_and_stereotype() {
        let mut e = entity("Repo", ClassFamilyKind::Interface);
        e.stereotype = Some("Service".into());
        if let EntityKindData::Compartment { generic, .. } = &mut e.kind_data {
            *generic = Some("T".into());
        }
        let mut diag = CucaDiagram::default();
        diag.entities.push(e);
        let s = render(diag);
        assert!(s.contains("kind: \"interface\""));
        assert!(s.contains("stereotype: [Service]"));
        assert!(s.contains("generic: [T]"));
    }

    #[test]
    fn direction_up_relabels_mult_and_role_to_rendered_ends() {
        // `Sub -up-> Sup` — the explicit `up` direction flips the edge,
        // so the rendered source is Sup (IR's `to`) and the rendered
        // target is Sub (IR's `from`). Multiplicity / role labels track
        // the rendered ends.
        let mut diag = CucaDiagram::default();
        diag.entities.push(entity("Sub", ClassFamilyKind::Class));
        diag.entities.push(entity("Sup", ClassFamilyKind::Class));
        diag.relations.push(Relation {
            from: "Sub".into(),
            to: "Sup".into(),
            from_couple: None,
            from_port: None,
            to_port: None,
            head_from: ArrowHead::None,
            head_to: ArrowHead::TriangleOpen,
            line_style: LineStyle::Solid,
            direction: Some(IrDirection::Up),
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
        let mut diag = CucaDiagram::default();
        diag.entities.push(entity("A", ClassFamilyKind::Class));
        diag.entities.push(entity("B", ClassFamilyKind::Class));
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
        let mut diag = CucaDiagram::default();
        diag.entities.push(entity("A", ClassFamilyKind::Class));
        diag.entities.push(entity("B", ClassFamilyKind::Class));
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
