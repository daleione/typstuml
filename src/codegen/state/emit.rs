//! The `#state-layout(...)` emitter: runs the layout, resolves back-edge
//! bows / port fans / detours, places notes, and serializes everything to
//! the Typst painter call.

use std::collections::HashMap;
use std::fmt::Write as _;

use crate::ir::{
    BorderStyle, Direction, LayoutDirection, LineStyle, NoteAnchor, NotePosition, RegionOrient,
    StateDiagram, StateKind,
};
use crate::layout::geometry::Point;
use crate::layout::graph::Orientation;
use crate::runtime::MeasurementSet;

use super::geom::{resolve_edge_label_size, resolve_node_geom, resolve_note_size, NodeGeom};
use super::layout::layout_nodes;
use super::route::{
    detour_around, node_shape, perimeter_point, route_transitions, seg_crosses_box,
    smooth_polyline, straight_cubic, RoutedEdge,
};
use super::view::{NodeBoxes, NodeTopology};
use super::{emit_opt_str, typst_str_escape};
use crate::codegen::common::puml_color_to_typst;

/// A placed note sticky: its box `(x, y, w, h)`, body text, the side it sits
/// on relative to its anchor, and the anchor rectangle the dashed connector
/// points at (`a*`) — the anchored state's bbox for `note … of`, or a
/// degenerate point at the link midpoint for `note on link`.
struct NoteBox {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    body: String,
    side: &'static str,
    ax: f64,
    ay: f64,
    aw: f64,
    ah: f64,
}

/// Margin reserved around the diagram content, in pt. Generous enough to
/// clear self-loop arcs and edge labels that extend past the node bboxes.
const MARGIN_PT: f64 = 30.0;
/// Right-side canvas space reserved for a self-loop arc + its label.
const SELF_LOOP_RESERVE_PT: f64 = 64.0;
/// Right-side canvas space reserved for a back-edge side-bow + its label.
const BACK_BOW_RESERVE_PT: f64 = 96.0;
/// Gap between an anchored note and the state it points at.
const NOTE_GAP_PT: f64 = 26.0;

pub fn emit(
    out: &mut String,
    diag: &StateDiagram,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) {
    let _ = &diag.skinparams; // S4: skinparam state* → preamble

    if diag.nodes.is_empty() {
        out.push_str("#state-layout()\n");
        return;
    }

    let geoms: Vec<NodeGeom> = diag
        .nodes
        .iter()
        .map(|n| resolve_node_geom(n, measurements, diagram_idx))
        .collect();

    // Per-transition label sizes (painter-measured when pass-1 ran), indexed
    // by transition. Threaded into the layout so label virtual nodes and bow
    // reserves use the true rendered size instead of a char-count estimate.
    let label_sizes: Vec<Option<(f64, f64)>> = diag
        .transitions
        .iter()
        .enumerate()
        .map(|(ti, tr)| resolve_edge_label_size(tr, ti, measurements, diagram_idx))
        .collect();

    let orientation = match diag.direction {
        LayoutDirection::TopToBottom => Orientation::TopToBottom,
        LayoutDirection::LeftToRight => Orientation::LeftToRight,
    };

    let topo = NodeTopology::new(diag);
    let id_to_idx = |id: &str| topo.index(id);

    // Recursive cluster layout (dot's model): each composite's interior is
    // laid out as its own sub-graph (network-simplex rank + x, minlen from
    // the dash count, edge labels as virtual nodes), the resulting frame
    // size becomes a single box node in the parent level, and a composite's
    // outside successors rank below that whole box — so the frame placement
    // and exit spacing fall out of the layout instead of post-hoc patches.
    // Concurrent regions are sibling sub-layouts inside their composite.
    let layout = layout_nodes(diag, &geoms, &label_sizes, orientation);
    let mut top_lefts = layout.top_lefts;
    let eff_geom = layout.eff_geom;
    let mut back = layout.back;
    let dividers = layout.dividers;
    let mut waypoints = layout.waypoints;
    let mut label_pos = layout.label_pos;
    let is_lr = matches!(orientation, Orientation::LeftToRight);

    reclassify_back_edges(diag, &topo, &top_lefts, &eff_geom, &mut back);

    // Normalize so the content starts at (MARGIN, MARGIN).
    let min_x = top_lefts.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let min_y = top_lefts.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    for p in &mut top_lefts {
        p.x = p.x - min_x + MARGIN_PT;
        p.y = p.y - min_y + MARGIN_PT;
    }
    for chain in waypoints.values_mut() {
        for p in chain.iter_mut() {
            p.x = p.x - min_x + MARGIN_PT;
            p.y = p.y - min_y + MARGIN_PT;
        }
    }
    for p in label_pos.values_mut() {
        p.x = p.x - min_x + MARGIN_PT;
        p.y = p.y - min_y + MARGIN_PT;
    }

    // Per-back-edge bow side. PlantUML routes a back-edge around the
    // *outside* of the graph; bowing toward whichever perpendicular
    // extreme the edge's endpoints sit nearer to keeps it from crossing
    // the interior. `"min"` = the low side of the perpendicular axis
    // (left in TB, top in LR); `"max"` = the high side.
    let perp_center = |i: usize| -> f64 {
        if is_lr {
            top_lefts[i].y + eff_geom[i].y / 2.0
        } else {
            top_lefts[i].x + eff_geom[i].x / 2.0
        }
    };
    let perp_lo = (0..diag.nodes.len())
        .map(|i| {
            if is_lr {
                top_lefts[i].y
            } else {
                top_lefts[i].x
            }
        })
        .fold(f64::INFINITY, f64::min);
    let perp_hi = (0..diag.nodes.len())
        .map(|i| {
            if is_lr {
                top_lefts[i].y + eff_geom[i].y
            } else {
                top_lefts[i].x + eff_geom[i].x
            }
        })
        .fold(0.0_f64, f64::max);
    let bow_side: Vec<&'static str> = diag
        .transitions
        .iter()
        .enumerate()
        .map(|(ti, tr)| {
            if !back[ti] {
                return "max";
            }
            let (Some(s), Some(d)) = (id_to_idx(&tr.from), id_to_idx(&tr.to)) else {
                return "max";
            };
            // An interior back-edge always bows toward the high side —
            // that's the side `layout_nodes` reserved room for inside the
            // enclosing composite frame.
            if diag.nodes[s].parent.is_some() {
                return "max";
            }
            let pos = (perp_center(s) + perp_center(d)) / 2.0;
            // Strict `<` so an exactly-centered edge (e.g. a single-column
            // diagram) ties to the high side, matching PlantUML's default.
            if pos - perp_lo < perp_hi - pos {
                "min"
            } else {
                "max"
            }
        })
        .collect();

    // Reserve space for self-loop arcs and back-edge bows — drawn by the
    // painter outside the node bboxes, on the perpendicular axis. Self
    // loops always bow toward the high side; back-edges bow per `bow_side`.
    // Only *top-level* back-edges need a canvas reserve — an interior
    // back-edge's bow is already contained by its composite frame (see
    // `COMPOSITE_BACK_BOW_PT` in `layout_nodes`).
    let has_self_loop = diag
        .transitions
        .iter()
        .any(|tr| id_to_idx(&tr.from).is_some() && tr.from == tr.to);
    let is_top_level = |ti: usize| -> bool {
        diag.transitions
            .get(ti)
            .and_then(|tr| id_to_idx(&tr.from))
            .map(|s| diag.nodes[s].parent.is_none())
            .unwrap_or(true)
    };
    let back_lo = back
        .iter()
        .enumerate()
        .zip(&bow_side)
        .any(|((ti, &b), &side)| b && side == "min" && is_top_level(ti));
    let back_hi = back
        .iter()
        .enumerate()
        .zip(&bow_side)
        .any(|((ti, &b), &side)| b && side == "max" && is_top_level(ti));
    let reserve_lo = if back_lo { BACK_BOW_RESERVE_PT } else { 0.0 };
    let reserve_hi = {
        let mut r: f64 = 0.0;
        if has_self_loop {
            r = r.max(SELF_LOOP_RESERVE_PT);
        }
        if back_hi {
            r = r.max(BACK_BOW_RESERVE_PT);
        }
        r
    };

    // Shift the content over by the low-side reserve so a `"min"` bow has
    // room to curl outside the canvas's content area.
    for p in &mut top_lefts {
        if is_lr {
            p.y += reserve_lo;
        } else {
            p.x += reserve_lo;
        }
    }
    for chain in waypoints.values_mut() {
        for p in chain.iter_mut() {
            if is_lr {
                p.y += reserve_lo;
            } else {
                p.x += reserve_lo;
            }
        }
    }
    for p in label_pos.values_mut() {
        if is_lr {
            p.y += reserve_lo;
        } else {
            p.x += reserve_lo;
        }
    }

    let mut note_boxes = place_notes(
        diag,
        &topo,
        &top_lefts,
        &eff_geom,
        &back,
        &bow_side,
        is_lr,
        measurements,
        diagram_idx,
    );

    let mut label_boxes = estimate_label_boxes(
        diag,
        &topo,
        &top_lefts,
        &eff_geom,
        &back,
        &label_sizes,
        &label_pos,
    );

    // A left-of note (or a `"min"` bow on a left-edge node, or a left-side
    // edge label) may have pushed content past x = 0 / y = 0. Re-normalize.
    let content_min_x = top_lefts
        .iter()
        .map(|p| p.x)
        .chain(note_boxes.iter().flat_map(|n| [n.x, n.ax]))
        .chain(label_boxes.iter().map(|l| l.0))
        .fold(f64::INFINITY, f64::min);
    let content_min_y = top_lefts
        .iter()
        .map(|p| p.y)
        .chain(note_boxes.iter().flat_map(|n| [n.y, n.ay]))
        .chain(label_boxes.iter().map(|l| l.1))
        .fold(f64::INFINITY, f64::min);
    let shift_x = (MARGIN_PT - content_min_x).max(0.0);
    let shift_y = (MARGIN_PT - content_min_y).max(0.0);
    for p in &mut top_lefts {
        p.x += shift_x;
        p.y += shift_y;
    }
    for chain in waypoints.values_mut() {
        for p in chain.iter_mut() {
            p.x += shift_x;
            p.y += shift_y;
        }
    }
    for p in label_pos.values_mut() {
        p.x += shift_x;
        p.y += shift_y;
    }
    for nb in &mut note_boxes {
        nb.x += shift_x;
        nb.y += shift_y;
        nb.ax += shift_x;
        nb.ay += shift_y;
    }
    for lb in &mut label_boxes {
        lb.0 += shift_x;
        lb.1 += shift_y;
    }

    let mut routed = route_edges(diag, &topo, &top_lefts, &eff_geom, &back, &waypoints, is_lr);

    assign_ports(
        diag,
        &topo,
        &top_lefts,
        &eff_geom,
        &back,
        is_lr,
        &mut routed,
    );

    let max_x = top_lefts
        .iter()
        .zip(&eff_geom)
        .map(|(p, g)| p.x + g.x)
        .chain(note_boxes.iter().flat_map(|n| [n.x + n.w, n.ax + n.aw]))
        .chain(label_boxes.iter().map(|l| l.0 + l.2))
        .fold(0.0_f64, f64::max);
    let max_y = top_lefts
        .iter()
        .zip(&eff_geom)
        .map(|(p, g)| p.y + g.y)
        .chain(note_boxes.iter().flat_map(|n| [n.y + n.h, n.ay + n.ah]))
        .chain(label_boxes.iter().map(|l| l.1 + l.3))
        .fold(0.0_f64, f64::max);
    let (page_w, page_h) = if is_lr {
        (max_x + MARGIN_PT, max_y + reserve_hi + MARGIN_PT)
    } else {
        (max_x + reserve_hi + MARGIN_PT, max_y + MARGIN_PT)
    };

    write_state_layout(
        out,
        diag,
        &topo,
        &top_lefts,
        &eff_geom,
        &back,
        &bow_side,
        &routed,
        &label_pos,
        &note_boxes,
        &dividers,
        page_w,
        page_h,
        is_lr,
    );
}

/// A back-edge is a rank artifact: it points from a lower rank back to a
/// higher one (e.g. a top-level cycle `A→B` / `B→A` between two composites).
/// The wide outer bow is only warranted when a direct line would otherwise
/// run *through* the interior; dot reverses a back-edge and routes it as an
/// ordinary spline whenever it can. Drop the `back` flag — so the painter
/// draws straight and `route_transitions` treats it as a normal edge — when
/// either:
///
///   * the target is a composite's history pseudostate (the resume / wake
///     transition; the history node already sits at the composite's exit
///     side, so a straight line enters cleanly and a bow would swing around
///     the exit edge and terminal), or
///   * the straight perimeter line already has clear line-of-sight (no
///     unrelated box in the way, same obstacle rule as `route_transitions`)
///     AND no reverse edge `d→s` exists that a straight draw would coincide
///     with. This is the `X→Z` / `Z→Y` nested-composite case: PlantUML draws
///     both as straight diagonals, not one bowed around the outside.
fn reclassify_back_edges(
    diag: &StateDiagram,
    topo: &NodeTopology,
    top_lefts: &[Point],
    eff_geom: &[Point],
    back: &mut [bool],
) {
    let boxes = NodeBoxes::new(diag, top_lefts, eff_geom);
    // True when the straight perimeter line `s→d` crosses no box that is
    // unrelated to either endpoint. A node is "involved" (never an obstacle)
    // when it contains or is contained by either endpoint; inner boxes whose
    // parent also blocks are skipped since the frame already covers them.
    let los_clear = |s: usize, d: usize| -> bool {
        let involved = |x: usize| {
            topo.anc_or_self(x, s)
                || topo.anc_or_self(x, d)
                || topo.anc_or_self(s, x)
                || topo.anc_or_self(d, x)
        };
        let start = boxes.perimeter_toward(s, boxes.center(d));
        let end = boxes.perimeter_toward(d, boxes.center(s));
        for x in 0..diag.nodes.len() {
            if involved(x) {
                continue;
            }
            if let Some(p) = topo.parent(x) {
                if !involved(p) {
                    continue; // outer frame already covers this child
                }
            }
            let (lo, hi) = boxes.bbox(x);
            if seg_crosses_box(start, end, lo, hi) {
                return false;
            }
        }
        true
    };
    for ti in 0..diag.transitions.len() {
        if !back[ti] {
            continue;
        }
        let tr = &diag.transitions[ti];
        let (Some(s), Some(d)) = (topo.index(&tr.from), topo.index(&tr.to)) else {
            continue;
        };
        let history = matches!(
            diag.nodes[d].kind,
            StateKind::History | StateKind::DeepHistory
        );
        let has_reverse = diag
            .transitions
            .iter()
            .any(|t2| t2.from == tr.to && t2.to == tr.from);
        if history || (!has_reverse && los_clear(s, d)) {
            back[ti] = false;
        }
    }
}

/// Place every note sticky. A `note … of` sticky sits left / right of its
/// anchor state; a `note on link` sticky sits next to the transition
/// midpoint; an unconnected floating note stacks in a left column. Each
/// natural position is pushed further out until it clears every obstacle
/// (other node bboxes, plus back-edge bows / self-loop arcs in TB).
#[allow(clippy::too_many_arguments)]
fn place_notes(
    diag: &StateDiagram,
    topo: &NodeTopology,
    top_lefts: &[Point],
    eff_geom: &[Point],
    back: &[bool],
    bow_side: &[&str],
    is_lr: bool,
    measurements: Option<&MeasurementSet>,
    diagram_idx: usize,
) -> Vec<NoteBox> {
    let boxes = NodeBoxes::new(diag, top_lefts, eff_geom);
    // Obstacles a note must clear: every node bbox plus — in TB, where
    // notes and bows share the x axis — the bands occupied by back-edge
    // bows and self-loop arcs. (In LR those bows curl on the y axis, clear
    // of an x-moving note, so only node bboxes matter.)
    let mut obstacles: Vec<(f64, f64, f64, f64)> = (0..diag.nodes.len())
        .map(|i| (top_lefts[i].x, top_lefts[i].y, eff_geom[i].x, eff_geom[i].y))
        .collect();
    if !is_lr {
        for (ti, tr) in diag.transitions.iter().enumerate() {
            let (Some(s), Some(d)) = (topo.index(&tr.from), topo.index(&tr.to)) else {
                continue;
            };
            let (sp, sg, dp, dg) = (top_lefts[s], eff_geom[s], top_lefts[d], eff_geom[d]);
            if tr.from == tr.to {
                // Self-loop arc bulges right of the node.
                obstacles.push((sp.x + sg.x, sp.y, 32.0, sg.y));
            } else if back[ti] {
                let y0 = sp.y.min(dp.y);
                let y1 = (sp.y + sg.y).max(dp.y + dg.y);
                if bow_side[ti] == "min" {
                    let x = sp.x.min(dp.x);
                    obstacles.push((x - 36.0, y0, 36.0, y1 - y0));
                } else {
                    let x = (sp.x + sg.x).max(dp.x + dg.x);
                    obstacles.push((x, y0, 36.0, y1 - y0));
                }
            }
        }
    }
    // Slide a note box along x (away from its anchor) until it overlaps no
    // obstacle. `side` is the direction it may travel.
    let clear_note_x = |mut nx: f64, ny: f64, w: f64, h: f64, side: &str| -> f64 {
        let (y0, y1) = (ny, ny + h);
        for _ in 0..=obstacles.len() {
            let mut moved = false;
            for &(ox, oy, ow, oh) in &obstacles {
                if oy + oh <= y0 || oy >= y1 {
                    continue; // no vertical overlap
                }
                if ox < nx + w && ox + ow > nx {
                    if side == "right" {
                        nx = ox + ow + NOTE_GAP_PT;
                    } else {
                        nx = ox - NOTE_GAP_PT - w;
                    }
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }
        nx
    };

    let mut note_boxes: Vec<NoteBox> = Vec::new();
    // Stacking cursor for unconnected floating notes (placed in a left
    // column; content re-normalization shifts the column to the margin).
    let mut float_cursor_y = MARGIN_PT;
    for (note_idx, note) in diag.notes.iter().enumerate() {
        match &note.anchor {
            NoteAnchor::OfNode { node_id, side } => {
                let Some(ai) = topo.index(node_id) else {
                    continue;
                };
                let sz = resolve_note_size(note_idx, &note.body, measurements, diagram_idx);
                let ap = top_lefts[ai];
                let ag = eff_geom[ai];
                let cy = ap.y + ag.y / 2.0 - sz.y / 2.0;
                let (natural_nx, side_kw) = match side {
                    NotePosition::RightOf => (ap.x + ag.x + NOTE_GAP_PT, "right"),
                    // `left of` and the unused `over` both fall to the left.
                    _ => (ap.x - NOTE_GAP_PT - sz.x, "left"),
                };
                let nx = clear_note_x(natural_nx, cy, sz.x, sz.y, side_kw);
                note_boxes.push(NoteBox {
                    x: nx,
                    y: cy,
                    w: sz.x,
                    h: sz.y,
                    body: note.body.clone(),
                    side: side_kw,
                    ax: ap.x,
                    ay: ap.y,
                    aw: ag.x,
                    ah: ag.y,
                });
            }
            NoteAnchor::OnLink { transition_idx } => {
                let Some(tr) = diag.transitions.get(*transition_idx) else {
                    continue;
                };
                let (Some(si), Some(di)) = (topo.index(&tr.from), topo.index(&tr.to)) else {
                    continue;
                };
                let (sc, dc) = (boxes.center(si), boxes.center(di));
                let mx = (sc.x + dc.x) / 2.0;
                let my = (sc.y + dc.y) / 2.0;
                let sz = resolve_note_size(note_idx, &note.body, measurements, diagram_idx);
                let cy = my - sz.y / 2.0;
                // Sticky sits to the right of the link midpoint; its dashed
                // connector exits the left edge back toward the midpoint.
                let nx = clear_note_x(mx + NOTE_GAP_PT, cy, sz.x, sz.y, "right");
                note_boxes.push(NoteBox {
                    x: nx,
                    y: cy,
                    w: sz.x,
                    h: sz.y,
                    body: note.body.clone(),
                    side: "right",
                    ax: mx,
                    ay: my,
                    aw: 0.0,
                    ah: 0.0,
                });
            }
            NoteAnchor::Floating { links, .. } => {
                let sz = resolve_note_size(note_idx, &note.body, measurements, diagram_idx);
                match links.iter().find_map(|id| topo.index(id)) {
                    // Connected: place like a right-of note next to the
                    // first linked state, with a dashed connector.
                    Some(ai) => {
                        let ap = top_lefts[ai];
                        let ag = eff_geom[ai];
                        let cy = ap.y + ag.y / 2.0 - sz.y / 2.0;
                        let nx = clear_note_x(ap.x + ag.x + NOTE_GAP_PT, cy, sz.x, sz.y, "right");
                        note_boxes.push(NoteBox {
                            x: nx,
                            y: cy,
                            w: sz.x,
                            h: sz.y,
                            body: note.body.clone(),
                            side: "right",
                            ax: ap.x,
                            ay: ap.y,
                            aw: ag.x,
                            ah: ag.y,
                        });
                    }
                    // Unconnected: stack in a left column, no connector.
                    None => {
                        let y = float_cursor_y;
                        float_cursor_y += sz.y + 10.0;
                        note_boxes.push(NoteBox {
                            x: -sz.x - NOTE_GAP_PT,
                            y,
                            w: sz.x,
                            h: sz.y,
                            body: note.body.clone(),
                            side: "none",
                            ax: 0.0,
                            ay: 0.0,
                            aw: 0.0,
                            ah: 0.0,
                        });
                    }
                }
            }
        }
    }
    note_boxes
}

/// Estimate straight-edge label boxes (the painter offsets each label
/// perpendicular to its edge) so a label on a left-column edge isn't clipped
/// off the canvas. Self-loop / back-edge labels live inside the reserved bow
/// bands and are already covered by the reserve shift.
fn estimate_label_boxes(
    diag: &StateDiagram,
    topo: &NodeTopology,
    top_lefts: &[Point],
    eff_geom: &[Point],
    back: &[bool],
    label_sizes: &[Option<(f64, f64)>],
    label_pos: &HashMap<usize, Point>,
) -> Vec<(f64, f64, f64, f64)> {
    let boxes = NodeBoxes::new(diag, top_lefts, eff_geom);
    let mut label_boxes: Vec<(f64, f64, f64, f64)> = Vec::new();
    for (ti, tr) in diag.transitions.iter().enumerate() {
        if tr.from == tr.to || back[ti] {
            continue;
        }
        let (Some(s), Some(d)) = (topo.index(&tr.from), topo.index(&tr.to)) else {
            continue;
        };
        let Some((w, h)) = label_sizes.get(ti).copied().flatten() else {
            continue;
        };
        // A label laid out as a dot-style label node has a reserved
        // position; the painter draws it just right of that point. Box it
        // there. Otherwise fall back to the perpendicular-midpoint estimate.
        if let Some(p) = label_pos.get(&ti) {
            label_boxes.push((p.x, p.y - h / 2.0, w, h));
            continue;
        }
        let (sc, dc) = (boxes.center(s), boxes.center(d));
        let (mx, my) = ((sc.x + dc.x) / 2.0, (sc.y + dc.y) / 2.0);
        let (dx, dy) = (dc.x - sc.x, dc.y - sc.y);
        let len = (dx * dx + dy * dy).sqrt();
        let (nx, ny) = if len > 1e-6 {
            (-dy / len, dx / len)
        } else {
            (0.0, -1.0)
        };
        let off = nx.abs() * w / 2.0 + ny.abs() * h / 2.0 + 4.0;
        let (lcx, lcy) = (mx + nx * off, my + ny * off);
        label_boxes.push((lcx - w / 2.0, lcy - h / 2.0, w, h));
    }
    label_boxes
}

/// Per-transition routed path. When a layout supplies connector waypoints
/// (dot's "edge follows its virtual-node chain") we clip the real endpoints
/// to the node faces and stitch the chain into cubic segments. The recursive
/// layout supplies none today, so edges fall back to the obstacle detour
/// router (`route_transitions`).
fn route_edges(
    diag: &StateDiagram,
    topo: &NodeTopology,
    top_lefts: &[Point],
    eff_geom: &[Point],
    back: &[bool],
    waypoints: &HashMap<usize, Vec<Point>>,
    is_lr: bool,
) -> Vec<Option<RoutedEdge>> {
    let boxes = NodeBoxes::new(diag, top_lefts, eff_geom);
    let mut routed: Vec<Option<RoutedEdge>> = if waypoints.is_empty() {
        route_transitions(diag, &top_lefts, &eff_geom, &back, is_lr)
    } else {
        (0..diag.transitions.len()).map(|_| None).collect()
    };
    if !waypoints.is_empty() {
        // Composite frames are routing obstacles: an edge between two
        // states that both live *outside* a composite must skirt its
        // frame, not cut diagonally through it. The connector waypoints
        // alone don't guarantee this — compaction bunches the side-lane
        // dummies near the top, leaving a long straight tail that pierces
        // the box — so detect a crossing and replace the path with a
        // clean ortho detour down a side lane (dot's `splines=ortho`
        // look). Parallel detours around the same side get spread into
        // distinct lanes.
        const ROUTE_LANE_GAP: f64 = 14.0;
        let comp_boxes: Vec<(usize, Point, Point)> = diag
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.kind == StateKind::Composite)
            .map(|(i, _)| {
                (
                    i,
                    top_lefts[i],
                    Point::new(
                        top_lefts[i].x + eff_geom[i].x,
                        top_lefts[i].y + eff_geom[i].y,
                    ),
                )
            })
            .collect();
        let is_descendant = |mut node: usize, comp: usize| -> bool {
            loop {
                if node == comp {
                    return true;
                }
                match diag.nodes[node]
                    .parent
                    .as_deref()
                    .and_then(|p| topo.index(p))
                {
                    Some(p) => node = p,
                    None => return false,
                }
            }
        };
        let mut lane_count: HashMap<(usize, bool), usize> = HashMap::new();
        for (ti, tr) in diag.transitions.iter().enumerate() {
            let (Some(s), Some(d)) = (topo.index(&tr.from), topo.index(&tr.to)) else {
                continue;
            };
            if s == d {
                continue;
            }
            let mids = waypoints.get(&ti);
            let toward_s = mids
                .and_then(|m| m.first().copied())
                .unwrap_or_else(|| boxes.center(d));
            let toward_d = mids
                .and_then(|m| m.last().copied())
                .unwrap_or_else(|| boxes.center(s));
            let start = perimeter_point(
                boxes.center(s),
                eff_geom[s].x / 2.0,
                eff_geom[s].y / 2.0,
                node_shape(diag.nodes[s].kind),
                toward_s,
            );
            let end = perimeter_point(
                boxes.center(d),
                eff_geom[d].x / 2.0,
                eff_geom[d].y / 2.0,
                node_shape(diag.nodes[d].kind),
                toward_d,
            );
            let obstacle = comp_boxes.iter().find(|(ci, lo, hi)| {
                !is_descendant(s, *ci)
                    && !is_descendant(d, *ci)
                    && seg_crosses_box(
                        start,
                        end,
                        Point::new(lo.x - 1.0, lo.y - 1.0),
                        Point::new(hi.x + 1.0, hi.y + 1.0),
                    )
            });
            if let Some((ci, lo, hi)) = obstacle {
                let side_hi = if is_lr {
                    (start.y + end.y) / 2.0 >= (lo.y + hi.y) / 2.0
                } else {
                    (start.x + end.x) / 2.0 >= (lo.x + hi.x) / 2.0
                };
                let k = {
                    let e = lane_count.entry((*ci, side_hi)).or_insert(0);
                    let v = *e;
                    *e += 1;
                    v
                };
                let off = ROUTE_LANE_GAP * (1.0 + k as f64);
                let side_coord = match (is_lr, side_hi) {
                    (false, true) => hi.x + off,
                    (false, false) => lo.x - off,
                    (true, true) => hi.y + off,
                    (true, false) => lo.y - off,
                };
                let segments = detour_around(start, end, side_coord, is_lr);
                routed[ti] = Some(RoutedEdge { start, segments });
            } else if let Some(mids) = mids {
                if mids.is_empty() {
                    continue;
                }
                let mut pts = Vec::with_capacity(mids.len() + 2);
                pts.push(start);
                pts.extend_from_slice(mids);
                pts.push(end);
                let segments = smooth_polyline(&pts);
                routed[ti] = Some(RoutedEdge { start, segments });
            }
        }
    }
    routed
}

/// Port assignment. dot routes every edge through its own virtual-node
/// lane, so several edges incident on one node face leave / enter at
/// *distinct points* spread along that face (ordered toward their far ends)
/// rather than one shared perimeter point. Group each non-detoured edge
/// endpoint by (node, face); for any face carrying >= 2 edges, fan the ports
/// around the natural exit direction and route a straight line from each.
fn assign_ports(
    diag: &StateDiagram,
    topo: &NodeTopology,
    top_lefts: &[Point],
    eff_geom: &[Point],
    back: &[bool],
    is_lr: bool,
    routed: &mut [Option<RoutedEdge>],
) {
    let boxes = NodeBoxes::new(diag, top_lefts, eff_geom);
    struct Ep {
        ti: usize,
        node: usize,
        far: usize,
        is_src: bool,
        other: Point,
    }
    // Which face an edge leaves/enters, dot-style: a *rank* edge (the two
    // boxes don't overlap on the rank axis — they sit in different ranks)
    // leaves the rank-end face and enters the rank-start face, so it flows
    // with the layout; only a *flat* edge (boxes overlapping on the rank
    // axis, i.e. same rank) uses a perpendicular side face. A raw angle
    // test instead sent a forward edge out the side whenever the target
    // sat diagonally past a wide box's corner — which is how State3's two
    // exits to the terminal ended up on the right face, crossing the
    // self-loop, rather than fanning along the bottom.
    // Faces: 0 top, 1 bottom, 2 left, 3 right.
    let face_of = |node: usize, far: usize| -> u8 {
        let (np, ng) = (top_lefts[node], eff_geom[node]);
        let (fp, fg) = (top_lefts[far], eff_geom[far]);
        if is_lr {
            // Rank axis = x. Overlap on x ⇒ flat edge ⇒ top/bottom face.
            let flat = np.x < fp.x + fg.x && fp.x < np.x + ng.x;
            if flat {
                if fp.y + fg.y / 2.0 >= np.y + ng.y / 2.0 {
                    1
                } else {
                    0
                }
            } else if fp.x >= np.x + ng.x {
                3
            } else {
                2
            }
        } else {
            // Rank axis = y. Overlap on y ⇒ flat edge ⇒ left/right face.
            let flat = np.y < fp.y + fg.y && fp.y < np.y + ng.y;
            if flat {
                if fp.x + fg.x / 2.0 >= np.x + ng.x / 2.0 {
                    3
                } else {
                    2
                }
            } else if fp.y >= np.y + ng.y {
                1
            } else {
                0
            }
        }
    };
    let mut eps: Vec<Ep> = Vec::new();
    for (ti, tr) in diag.transitions.iter().enumerate() {
        let (Some(s), Some(d)) = (topo.index(&tr.from), topo.index(&tr.to)) else {
            continue;
        };
        if s == d || back[ti] || routed[ti].is_some() {
            continue;
        }
        eps.push(Ep {
            ti,
            node: s,
            far: d,
            is_src: true,
            other: boxes.center(d),
        });
        eps.push(Ep {
            ti,
            node: d,
            far: s,
            is_src: false,
            other: boxes.center(s),
        });
    }
    let mut groups: HashMap<(usize, u8), Vec<usize>> = HashMap::new();
    for (i, ep) in eps.iter().enumerate() {
        groups
            .entry((ep.node, face_of(ep.node, ep.far)))
            .or_default()
            .push(i);
    }
    let mut port: HashMap<(usize, bool), Point> = HashMap::new();
    for ((node, face), mut idxs) in groups {
        if idxs.len() < 2 {
            continue; // single-edge faces keep the painter's perimeter point
        }
        let c = boxes.center(node);
        let (hw, hh) = (eff_geom[node].x / 2.0, eff_geom[node].y / 2.0);
        let shape = node_shape(diag.nodes[node].kind);
        let horiz = face <= 1; // top/bottom spread along x; left/right along y
        let along = |p: Point| if horiz { p.x } else { p.y };
        idxs.sort_by(|&a, &b| {
            along(eps[a].other)
                .partial_cmp(&along(eps[b].other))
                .unwrap()
                .then(eps[a].ti.cmp(&eps[b].ti))
        });
        let count = idxs.len();
        let centroid = idxs
            .iter()
            .map(|&i| along(perimeter_point(c, hw, hh, shape, eps[i].other)))
            .sum::<f64>()
            / count as f64;
        let (lo, hi) = if horiz {
            (c.x - hw + 4.0, c.x + hw - 4.0)
        } else {
            (c.y - hh + 4.0, c.y + hh - 4.0)
        };
        // Evenly distribute ports across the usable face (dot spreads
        // multi-edge ports by the available width, not a fixed gap), so
        // parallel edges off a wide box (e.g. a composite's two exits to
        // the terminal) leave from the middle and the corner rather than
        // bunched at one spot. Floored so narrow faces stay legible.
        let spacing = ((hi - lo) / count as f64).max(6.0);
        // Re-centre the fan so its whole width fits inside the face. The
        // natural exit (`centroid`) points at the shared target, which —
        // when the target sits off to one side of a wide box — lands at
        // the face edge; without this, every port `clamp`s onto that
        // edge and the parallel edges collapse into one line. Pull the
        // centre in by the half-span so the ports stay distinct.
        let half_span = ((count as f64 - 1.0) / 2.0 * spacing).min((hi - lo) / 2.0);
        let centroid = centroid.clamp(lo + half_span, (hi - half_span).max(lo + half_span));
        for (k, &i) in idxs.iter().enumerate() {
            let pos = (centroid + (k as f64 - (count as f64 - 1.0) / 2.0) * spacing).clamp(lo, hi);
            let pt = match face {
                0 => Point::new(pos, c.y - hh),
                1 => Point::new(pos, c.y + hh),
                2 => Point::new(c.x - hw, pos),
                _ => Point::new(c.x + hw, pos),
            };
            port.insert((eps[i].ti, eps[i].is_src), pt);
        }
    }
    for (ti, tr) in diag.transitions.iter().enumerate() {
        let (Some(s), Some(d)) = (topo.index(&tr.from), topo.index(&tr.to)) else {
            continue;
        };
        if s == d || back[ti] || routed[ti].is_some() {
            continue;
        }
        let (ps, pd) = (
            port.get(&(ti, true)).copied(),
            port.get(&(ti, false)).copied(),
        );
        if ps.is_none() && pd.is_none() {
            continue; // no multi-edge face on either end
        }
        let (cs, cd) = (boxes.center(s), boxes.center(d));
        let start = ps.unwrap_or_else(|| {
            perimeter_point(
                cs,
                eff_geom[s].x / 2.0,
                eff_geom[s].y / 2.0,
                node_shape(diag.nodes[s].kind),
                cd,
            )
        });
        let end = pd.unwrap_or_else(|| {
            perimeter_point(
                cd,
                eff_geom[d].x / 2.0,
                eff_geom[d].y / 2.0,
                node_shape(diag.nodes[d].kind),
                cs,
            )
        });
        // The distinct port already separates this edge from its
        // siblings, so draw a straight line — a curved spline would
        // bow an edge that should run straight (fork bar → worker,
        // worker → join bar, etc.).
        routed[ti] = Some(RoutedEdge {
            start,
            segments: vec![straight_cubic(start, end)],
        });
    }
}

/// Serialize the resolved layout into the `#state-layout(...)` painter call.
#[allow(clippy::too_many_arguments)]
fn write_state_layout(
    out: &mut String,
    diag: &StateDiagram,
    topo: &NodeTopology,
    top_lefts: &[Point],
    eff_geom: &[Point],
    back: &[bool],
    bow_side: &[&str],
    routed: &[Option<RoutedEdge>],
    label_pos: &HashMap<usize, Point>,
    note_boxes: &[NoteBox],
    dividers: &[Vec<(Point, Point)>],
    page_w: f64,
    page_h: f64,
    is_lr: bool,
) {
    // ----- emit -----
    out.push_str("#state-layout(\n");

    out.push_str("  nodes: (\n");
    for (i, n) in diag.nodes.iter().enumerate() {
        let p = top_lefts[i];
        let g = eff_geom[i];
        out.push_str("    (");
        write!(out, "id: \"{}\", ", typst_str_escape(&n.id)).unwrap();
        write!(out, "kind: \"{}\", ", n.kind.keyword()).unwrap();
        write!(
            out,
            "x: {:.2}pt, y: {:.2}pt, w: {:.2}pt, h: {:.2}pt, ",
            p.x, p.y, g.x, g.y
        )
        .unwrap();
        write!(out, "display: \"{}\", ", typst_str_escape(&n.display)).unwrap();
        out.push_str("body: (");
        for (bi, row) in n.body.iter().enumerate() {
            if bi > 0 {
                out.push_str(", ");
            }
            write!(out, "\"{}\"", typst_str_escape(row)).unwrap();
        }
        if n.body.len() == 1 {
            out.push(',');
        }
        out.push_str("), ");
        match n.fill.as_deref().and_then(puml_color_to_typst) {
            Some(c) => write!(out, "fill: {c}, ").unwrap(),
            None => out.push_str("fill: none, "),
        }
        write!(
            out,
            "border-style: \"{}\", ",
            border_style_kw(n.border_style)
        )
        .unwrap();
        match n.border_color.as_deref().and_then(puml_color_to_typst) {
            Some(c) => write!(out, "border-color: {c}").unwrap(),
            None => out.push_str("border-color: none"),
        }
        out.push_str("),\n");
    }
    out.push_str("  ),\n");

    out.push_str("  transitions: (\n");
    for (ti, tr) in diag.transitions.iter().enumerate() {
        if topo.index(&tr.from).is_none() || topo.index(&tr.to).is_none() {
            continue;
        }
        out.push_str("    (");
        write!(out, "from: \"{}\", ", typst_str_escape(&tr.from)).unwrap();
        write!(out, "to: \"{}\", ", typst_str_escape(&tr.to)).unwrap();
        emit_opt_str(out, "event", tr.event.as_deref());
        emit_opt_str(out, "guard", tr.guard.as_deref());
        emit_opt_str(out, "action", tr.action.as_deref());
        write!(out, "style: \"{}\", ", line_style_kw(tr.line_style)).unwrap();
        match tr.color.as_deref().and_then(puml_color_to_typst) {
            Some(c) => write!(out, "color: {c}, ").unwrap(),
            None => out.push_str("color: none, "),
        }
        let _ = direction_kw(tr.direction); // S2+: direction-biased routing
        write!(out, "self-loop: {}, ", tr.from == tr.to).unwrap();
        write!(out, "back: {}, ", back[ti]).unwrap();
        write!(out, "bow-side: \"{}\"", bow_side[ti]).unwrap();
        // Obstacle-routed detour: explicit start anchor + cubic path. The
        // painter draws this instead of a straight center-to-center line.
        if let Some(re) = &routed[ti] {
            write!(out, ", start: ({:.2}pt, {:.2}pt)", re.start.x, re.start.y).unwrap();
            out.push_str(", path: (");
            for seg in &re.segments {
                write!(
                    out,
                    "(c1: ({:.2}pt, {:.2}pt), c2: ({:.2}pt, {:.2}pt), end: ({:.2}pt, {:.2}pt)), ",
                    seg.0.x, seg.0.y, seg.1.x, seg.1.y, seg.2.x, seg.2.y
                )
                .unwrap();
            }
            out.push(')');
        }
        // Reserved label position from the dot-style label node: the
        // painter draws the label just right of this point instead of
        // computing its own midpoint.
        if let Some(p) = label_pos.get(&ti) {
            write!(out, ", label-pos: ({:.2}pt, {:.2}pt)", p.x, p.y).unwrap();
        }
        out.push_str("),\n");
    }
    out.push_str("  ),\n");

    out.push_str("  notes: (\n");
    for nb in note_boxes {
        out.push_str("    (");
        write!(
            out,
            "x: {:.2}pt, y: {:.2}pt, w: {:.2}pt, h: {:.2}pt, ",
            nb.x, nb.y, nb.w, nb.h
        )
        .unwrap();
        write!(out, "body: \"{}\", ", typst_str_escape(&nb.body)).unwrap();
        write!(out, "side: \"{}\", ", nb.side).unwrap();
        write!(
            out,
            "anchor: (x: {:.2}pt, y: {:.2}pt, w: {:.2}pt, h: {:.2}pt)",
            nb.ax, nb.ay, nb.aw, nb.ah
        )
        .unwrap();
        out.push_str("),\n");
    }
    out.push_str("  ),\n");

    // Concurrent-region dividers — emitted only when a composite actually
    // has `--` / `||` regions, so plain diagrams keep their `regions: ()`
    // default and their golden output unchanged.
    if dividers.iter().any(|d| !d.is_empty()) {
        out.push_str("  regions: (\n");
        for (ci, segs) in dividers.iter().enumerate() {
            if segs.is_empty() {
                continue;
            }
            let base = top_lefts[ci];
            let orient = diag
                .regions
                .iter()
                .find(|rg| rg.composite_id == diag.nodes[ci].id)
                .map(|rg| match rg.orientation {
                    RegionOrient::Vertical => "vertical",
                    RegionOrient::Horizontal => "horizontal",
                })
                .unwrap_or("horizontal");
            out.push_str("    (");
            write!(
                out,
                "parent: \"{}\", ",
                typst_str_escape(&diag.nodes[ci].id)
            )
            .unwrap();
            write!(out, "orientation: \"{orient}\", ").unwrap();
            out.push_str("dividers: (");
            for (a, b) in segs {
                write!(
                    out,
                    "(x0: {:.2}pt, y0: {:.2}pt, x1: {:.2}pt, y1: {:.2}pt), ",
                    base.x + a.x,
                    base.y + a.y,
                    base.x + b.x,
                    base.y + b.y,
                )
                .unwrap();
            }
            out.push_str(")),\n");
        }
        out.push_str("  ),\n");
    }

    write!(out, "  page: ({page_w:.2}pt, {page_h:.2}pt),\n").unwrap();
    match &diag.title {
        Some(t) => write!(out, "  title: \"{}\",\n", typst_str_escape(t)).unwrap(),
        None => out.push_str("  title: none,\n"),
    }
    write!(
        out,
        "  hide-empty-description: {},\n",
        diag.hide_empty_description
    )
    .unwrap();
    write!(
        out,
        "  direction: \"{}\",\n",
        if is_lr { "lr" } else { "tb" }
    )
    .unwrap();
    out.push_str(")\n");
}

fn line_style_kw(s: LineStyle) -> &'static str {
    match s {
        LineStyle::Solid => "solid",
        LineStyle::Dashed => "dashed",
        LineStyle::Dotted => "dotted",
    }
}

fn border_style_kw(s: Option<BorderStyle>) -> &'static str {
    match s {
        Some(BorderStyle::Solid) | None => "solid",
        Some(BorderStyle::Dashed) => "dashed",
        Some(BorderStyle::Dotted) => "dotted",
        Some(BorderStyle::Bold) => "bold",
    }
}

fn direction_kw(d: Option<Direction>) -> &'static str {
    match d {
        Some(Direction::Up) => "up",
        Some(Direction::Down) => "down",
        Some(Direction::Left) => "left",
        Some(Direction::Right) => "right",
        None => "none",
    }
}
