//! Batch routing from geometric constraints; contains no diagram or language checks.
use super::geom::{anchor_for_side, box_center, NodeGeom, Side};
use super::route::{
    cubic_from_straight, line_of_sight_clear, pick_edge_sides, side_tangent, smart_align_coord,
    straight_fallback, try_manhattan_route,
};
use crate::layout::{geometry::Point, ortho, pathplan, spacing::Spacing};

const EDGE_FORCE_MAX_PT: f64 = 30.0;
const ROUTE_PADDING_PT: f64 = 1.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RoutingMode {
    Spline,
    Ortho,
    Polyline,
}
#[derive(Clone, Copy)]
pub(crate) enum LabelPlacement {
    Trunk,
    AvoidOverlaps,
}
#[derive(Clone, Copy, Default)]
pub(crate) struct AnchorConstraints {
    pub fixed: bool,
    pub left_inset: f64,
    pub prefers_spline: bool,
}
pub(crate) struct RouteEdge {
    pub src_idx: usize,
    pub dst_idx: usize,
    pub has_label: bool,
    pub label_size: Option<Point>,
}
pub(crate) struct RoutingOptions {
    pub mode: RoutingMode,
    pub spacing: Spacing,
    pub exterior_self_loops: bool,
    pub labels: LabelPlacement,
}
pub(crate) struct RoutedEdge {
    pub segments: Vec<(Point, Point, Point)>,
    pub sides: (Side, Side),
    pub from_override: Option<f64>,
    pub to_override: Option<f64>,
    pub label_pos: Option<Point>,
}
pub(crate) struct RoutingInput<'a> {
    pub geoms: &'a [NodeGeom],
    pub top_lefts: &'a [Point],
    pub edges: &'a [RouteEdge],
    pub constraints: &'a [AnchorConstraints],
    pub is_lr: bool,
    pub foreign_obstacles: &'a dyn Fn(usize, usize) -> Vec<pathplan::Box>,
}
pub(crate) fn route(input: RoutingInput<'_>, options: RoutingOptions) -> Vec<RoutedEdge> {
    let RoutingInput {
        geoms,
        top_lefts,
        edges,
        constraints,
        is_lr,
        foreign_obstacles,
    } = input;
    let fixed_anchors = |i: usize| constraints[i].fixed;
    let node_boxes: Vec<_> = top_lefts
        .iter()
        .zip(geoms)
        .map(|(&p, g)| (p, p.add(g.size)))
        .collect();
    let mut result = Vec::with_capacity(edges.len());
    // Pre-pass 1: pick from/to sides for every edge. Distribution needs
    // side info before we can group siblings by shared face.
    let edge_sides: Vec<(Side, Side)> = edges
        .iter()
        .map(|oe| {
            let from = oe.src_idx;
            let to = oe.dst_idx;
            if from == to && options.exterior_self_loops {
                return (Side::Right, Side::Top);
            }
            pick_edge_sides(
                box_center(&geoms[from], top_lefts[from]),
                box_center(&geoms[to], top_lefts[to]),
                (top_lefts[from], top_lefts[from].add(geoms[from].size)),
                (top_lefts[to], top_lefts[to].add(geoms[to].size)),
                is_lr,
            )
        })
        .collect();

    // Pre-pass 2: nudge arrowheads off each other when sibling edges
    // collide at the same anchor point on a destination face. We DO NOT
    // redistribute by default — `smart_align_coord` already places
    // anchors at geometrically meaningful coords (perpendicular-overlap
    // overlaps that yield straight sibling-rank lines), and moving them
    // turns previously-clean horizontals into S-bends.
    //
    // What we do: for each destination face with >=2 edges arriving,
    // collect their natural anchor coords (smart-aligned or midpoint).
    // If two or more land within COLLISION_EPS_PT of each other, keep
    // the smart-aligned anchors in place and shove the un-aligned ones
    // to fresh slots along the face. Source faces aren't redistributed
    // since arrowheads sit at the destination — tails fanning out from
    // a shared point don't pile visibly.
    const COLLISION_EPS_PT: f64 = 4.0;
    const MIN_SEPARATION_PT: f64 = 10.0;
    const FACE_INSET_FRAC: f64 = 0.15;
    use std::collections::BTreeMap;
    let mut dst_face_groups: BTreeMap<(usize, Side), Vec<usize>> = BTreeMap::new();
    for (i, oe) in edges.iter().enumerate() {
        let (_, ts) = edge_sides[i];
        dst_face_groups.entry((oe.dst_idx, ts)).or_default().push(i);
    }
    let mut to_overrides: Vec<Option<f64>> = vec![None; edges.len()];

    // Per-edge pre-computed default + smart-align coords for the
    // destination face. Needed to decide collisions before we commit to
    // any override.
    let mut to_natural: Vec<f64> = Vec::with_capacity(edges.len());
    let mut to_aligned_flag: Vec<bool> = Vec::with_capacity(edges.len());
    for (i, oe) in edges.iter().enumerate() {
        let (from_side, to_side) = edge_sides[i];
        let default_end = anchor_for_side(&geoms[oe.dst_idx], top_lefts[oe.dst_idx], to_side);
        let aligned = if fixed_anchors(oe.src_idx) || fixed_anchors(oe.dst_idx) {
            None
        } else {
            smart_align_coord(
                &geoms[oe.src_idx],
                top_lefts[oe.src_idx],
                &geoms[oe.dst_idx],
                top_lefts[oe.dst_idx],
                from_side,
                to_side,
            )
        };
        let face_horizontal = matches!(to_side, Side::Left | Side::Right);
        let coord = match aligned {
            Some(c) => c,
            None => {
                if face_horizontal {
                    default_end.y
                } else {
                    default_end.x
                }
            }
        };
        to_natural.push(coord);
        to_aligned_flag.push(aligned.is_some());
    }

    for ((entity_idx, side), face_edges) in dst_face_groups.iter() {
        if face_edges.len() < 2 {
            continue;
        }
        let face_horizontal = matches!(side, Side::Left | Side::Right);
        // Detect collision: any two natural coords within EPS?
        let mut collided = false;
        for i in 0..face_edges.len() {
            for j in (i + 1)..face_edges.len() {
                if (to_natural[face_edges[i]] - to_natural[face_edges[j]]).abs() < COLLISION_EPS_PT
                {
                    collided = true;
                    break;
                }
            }
            if collided {
                break;
            }
        }
        if !collided {
            continue;
        }
        let bbox_min = top_lefts[*entity_idx];
        let bbox_max = bbox_min.add(geoms[*entity_idx].size);
        let (face_min, face_max) = if face_horizontal {
            (bbox_min.y, bbox_max.y)
        } else {
            (bbox_min.x, bbox_max.x)
        };
        // Useable portion of the face — inset slightly from the bbox
        // corners. For ellipse-shaped entities (usecase / cloud /
        // database) the corners are far from the actual boundary, and
        // even for rectangles distributing face_edges right up to the
        // corner looks cramped.
        let inset = (face_max - face_min) * FACE_INSET_FRAC;
        let face_min_use = face_min + inset;
        let face_max_use = face_max - inset;
        // Split into "fixed" (smart-aligned) and "flexible" (midpoint).
        let fixed: Vec<usize> = face_edges
            .iter()
            .copied()
            .filter(|&i| to_aligned_flag[i])
            .collect();
        let flexible: Vec<usize> = face_edges
            .iter()
            .copied()
            .filter(|&i| !to_aligned_flag[i])
            .collect();
        let reserved: Vec<f64> = fixed.iter().map(|&i| to_natural[i]).collect();
        // For each flexible edge, place it where its source naturally
        // wants to enter — but force MIN_SEPARATION_PT clearance from
        // every reserved (smart-aligned) and already-chosen anchor.
        // Without the minimum-separation guarantee, sibling arrows
        // land within a few pt of each other and the heads still pile
        // visibly.
        let mut chosen: Vec<f64> = Vec::new();
        for &edge_idx in flexible.iter() {
            let src_center = box_center(
                &geoms[edges[edge_idx].src_idx],
                top_lefts[edges[edge_idx].src_idx],
            );
            let ideal_raw = if face_horizontal {
                src_center.y
            } else {
                src_center.x
            };
            let ideal = ideal_raw.clamp(face_min_use, face_max_use);
            let mut coord = ideal;
            // One pass of snap-away from each conflict. For 1 fixed +
            // few flexible (the common case), one pass is enough; the
            // initial ideal already biases toward the source's side.
            for &r in reserved.iter().chain(chosen.iter()) {
                if (coord - r).abs() < MIN_SEPARATION_PT {
                    if ideal >= r {
                        coord = (r + MIN_SEPARATION_PT).min(face_max_use);
                    } else {
                        coord = (r - MIN_SEPARATION_PT).max(face_min_use);
                    }
                }
            }
            to_overrides[edge_idx] = Some(coord);
            chosen.push(coord);
        }
    }
    // No source-side redistribution.
    for (i, oe) in edges.iter().enumerate() {
        if fixed_anchors(oe.dst_idx) {
            to_overrides[i] = None;
        }
    }
    let from_overrides: Vec<Option<f64>> = vec![None; edges.len()];

    // Two passes: the first resolves every edge's route (ortho routes
    // stay as raw polylines, not yet rounded), then
    // `ortho::separate_overlapping` (§3.5.2) fans apart parallel trunk
    // segments across *all* routes at once — it needs every polyline
    // up front, so it can't run inside the per-edge loop. The second
    // pass rounds the (possibly now-separated) ortho polylines and
    // emits every edge in original order.
    enum PendingRoute {
        Final(Vec<(Point, Point, Point)>),
        Ortho(Vec<Point>),
    }
    struct PendingEdge<'a> {
        oe: &'a RouteEdge,
        start: Point,
        from_side: Side,
        to_side: Side,
        from_emit_override: Option<f64>,
        to_emit_override: Option<f64>,
        route: PendingRoute,
    }
    let mut pending: Vec<PendingEdge> = Vec::with_capacity(edges.len());

    for (edge_idx, oe) in edges.iter().enumerate() {
        let from = oe.src_idx;
        let to = oe.dst_idx;
        let (from_side, to_side) = edge_sides[edge_idx];
        let mainly_vertical = matches!(from_side, Side::Top | Side::Bot);

        let default_start =
            node_anchor(&constraints[from], &geoms[from], top_lefts[from], from_side);
        let default_end = node_anchor(&constraints[to], &geoms[to], top_lefts[to], to_side);

        // Smart alignment — when both ends are unconstrained AND on the
        // same axis with overlapping perpendicular extents, place both
        // anchors at the same coord inside the overlap. The distribution
        // overrides take precedence: once we've assigned a sibling-spread
        // coord to either end, smart-align is no longer applicable.
        let aligned_coord = if !fixed_anchors(from)
            && !fixed_anchors(to)
            && from_overrides[edge_idx].is_none()
            && to_overrides[edge_idx].is_none()
        {
            smart_align_coord(
                &geoms[from],
                top_lefts[from],
                &geoms[to],
                top_lefts[to],
                from_side,
                to_side,
            )
        } else {
            None
        };

        let (mut from_emit_override, mut to_emit_override) =
            (from_overrides[edge_idx], to_overrides[edge_idx]);
        let (start, end) = if let Some(coord) = aligned_coord {
            from_emit_override = Some(coord);
            to_emit_override = Some(coord);
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
            let start = match (from_overrides[edge_idx], from_side) {
                (Some(c), Side::Left | Side::Right) => Point::new(default_start.x, c),
                (Some(c), Side::Top | Side::Bot) => Point::new(c, default_start.y),
                (None, _) => default_start,
            };
            let end = match (to_overrides[edge_idx], to_side) {
                (Some(c), Side::Left | Side::Right) => Point::new(default_end.x, c),
                (Some(c), Side::Top | Side::Bot) => Point::new(c, default_end.y),
                (None, _) => default_end,
            };
            (start, end)
        };

        // Entity obstacles: every entity bbox except this edge's two
        // endpoints. M3 ranks clusters via Sugiyama and tighten pulls
        // them apart, so cross-cluster edges no longer need explicit
        // cluster-bbox obstacles to detour — the rank ordering keeps
        // the natural path from clipping through a sibling cluster.
        // try_manhattan_route's detour-bend remains the safety net for
        // residual obstacle-clipping cases.
        let obstacles: Vec<pathplan::Box> = (0..geoms.len())
            .filter(|i| *i != from && *i != to)
            .map(|i| pathplan::Box::new(node_boxes[i].0, node_boxes[i].1))
            .collect();
        let route_opts = pathplan::RouteOpts {
            obstacle_padding: ROUTE_PADDING_PT,
            src_tangent: side_tangent(from_side),
            dst_tangent: side_tangent(to_side).neg(),
        };

        // The caller can keep curved-shape connectors on spline routing
        // even when the graph's default is orthogonal.
        let edge_prefers_spline =
            constraints[from].prefers_spline || constraints[to].prefers_spline;

        // Orthogonal routes include caller-supplied foreign frame obstacles.
        // Keep raw polylines until the batch trunk-separation pass below.
        let ortho_polyline: Option<Vec<Point>> =
            if options.mode != RoutingMode::Spline && !edge_prefers_spline {
                let sp = options.spacing;
                let mut ortho_obstacles = obstacles.clone();
                ortho_obstacles.extend(foreign_obstacles(from, to));
                let opts = ortho::RouteOpts {
                    clearance: sp.edge_node,
                    bend_penalty: 4.0 * sp.edge_node,
                    stub_len: sp.edge_node,
                };
                ortho::route(
                    start,
                    ortho::Dir::from_tangent(side_tangent(from_side)),
                    end,
                    ortho::Dir::from_tangent(side_tangent(to_side).neg()),
                    &ortho_obstacles,
                    &opts,
                )
                .map(|pts| ortho::simplify(&pts, 1.0))
            } else {
                None
            };

        // Routing priority (cuca-edge-routing-redesign.md §2.1),
        // superseded by `ortho_polyline` when ortho mode is active:
        //   1. Straight line of sight — single direct cubic bezier
        //      from source anchor to dest anchor (PlantUML / dot
        //      `splines=true` style). The control handles sit at 1/3
        //      and 2/3 along the chord so the visible curve is a
        //      straight line; decorated heads rotate to match.
        //   2. Manhattan Z — for blocked diagonals, fall back to a
        //      down-across-down (or right-along-right) right-angle route.
        //   3. Pathplan bezier — for routes that need to detour around
        //      multiple obstacles.
        //   4. Forced straight cubic — last resort.
        let line_of_sight = ortho_polyline.is_none() && line_of_sight_clear(start, end, &obstacles);
        let route = if let Some(pts) = ortho_polyline {
            PendingRoute::Ortho(pts)
        } else if line_of_sight {
            PendingRoute::Final(vec![cubic_from_straight(start, end)])
        } else if let Some(segs) = try_manhattan_route(start, end, &obstacles, mainly_vertical) {
            PendingRoute::Final(segs)
        } else {
            PendingRoute::Final(
                match pathplan::route_edge(start, end, &obstacles, route_opts) {
                    Ok(cubics) => cubics
                        .into_iter()
                        .map(|c| c.into_painter_segment())
                        .collect(),
                    Err(_) => straight_fallback(start, end, EDGE_FORCE_MAX_PT),
                },
            )
        };

        // For direct cubics, codegen owns the chord tangent — the head
        // should rotate with it. Setting explicit anchor overrides
        // signals the painter to skip the axis-snap that would
        // otherwise force the head perpendicular to the destination
        // face. Manhattan / pathplan / ortho routes keep midpoint
        // anchors unless smart-align or distribution already set them,
        // since their final segment is axis-aligned by construction.
        if line_of_sight {
            if from_emit_override.is_none() {
                from_emit_override = Some(match from_side {
                    Side::Top | Side::Bot => start.x,
                    Side::Left | Side::Right => start.y,
                });
            }
            if to_emit_override.is_none() {
                to_emit_override = Some(match to_side {
                    Side::Top | Side::Bot => end.x,
                    Side::Left | Side::Right => end.y,
                });
            }
        }

        let route = if from == to && options.exterior_self_loops {
            let right = top_lefts[from].x + geoms[from].size.x + 24.0;
            let above = top_lefts[from].y - 24.0;
            let points = [
                start,
                Point::new(right, start.y),
                Point::new(right, above),
                Point::new(end.x, above),
                end,
            ];
            PendingRoute::Final(
                points
                    .windows(2)
                    .map(|p| cubic_from_straight(p[0], p[1]))
                    .collect(),
            )
        } else {
            route
        };
        pending.push(PendingEdge {
            oe,
            start,
            from_side,
            to_side,
            from_emit_override,
            to_emit_override,
            route,
        });
    }

    // Batch pass: fan apart parallel trunk segments across every
    // ortho-routed edge, then round each polyline (post-separation)
    // into the painter's cubic segment list, and emit every edge in
    // its original order.
    let mut ortho_indices: Vec<usize> = Vec::new();
    let mut ortho_polylines: Vec<Vec<Point>> = Vec::new();
    for (i, pe) in pending.iter().enumerate() {
        if let PendingRoute::Ortho(pts) = &pe.route {
            ortho_indices.push(i);
            ortho_polylines.push(pts.clone());
        }
    }
    if !ortho_polylines.is_empty() {
        let sp = options.spacing;
        ortho::separate_overlapping(&mut ortho_polylines, sp.ortho_min_gap);
    }
    let mut ortho_result_iter = ortho_indices.into_iter().zip(ortho_polylines);

    let sp = options.spacing;
    let mut label_boxes = node_boxes.clone();
    for pe in &pending {
        let (segments, label_pos) = match &pe.route {
            PendingRoute::Final(segs) => (segs.clone(), None),
            PendingRoute::Ortho(_) => {
                let (_, separated) = ortho_result_iter.next().expect("one entry per ortho edge");
                let arc = if options.mode == RoutingMode::Polyline {
                    0.0
                } else {
                    sp.ortho_arc
                };
                // Longest-trunk midpoint (§3.8): the straight
                // start→end chord midpoint the painter uses by
                // default can land far from a bent orthogonal path,
                // so ortho edges carrying a label get an explicit
                // position instead. Only worth computing when there
                // actually is a label to place.
                let label_pos = pe
                    .oe
                    .has_label
                    .then(|| ortho::longest_trunk_midpoint(&separated))
                    .flatten();
                (ortho::to_rounded_cubics(&separated, arc), label_pos)
            }
        };
        let label_pos = match options.labels {
            LabelPlacement::AvoidOverlaps => pe.oe.label_size.map(|size| {
                super::labels::place_label(pe.start, &segments, size, &mut label_boxes)
            }),
            LabelPlacement::Trunk => label_pos,
        };
        result.push(RoutedEdge {
            segments,
            sides: (pe.from_side, pe.to_side),
            from_override: pe.from_emit_override,
            to_override: pe.to_emit_override,
            label_pos,
        });
    }
    result
}

fn node_anchor(c: &AnchorConstraints, geom: &NodeGeom, top_left: Point, side: Side) -> Point {
    let mut p = anchor_for_side(geom, top_left, side);
    if side == Side::Left {
        p.x += c.left_inset;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_stays_on_route_after_anchor_alignment() {
        for is_lr in [false, true] {
            let orient = |x, y| {
                if is_lr {
                    Point::new(y, x)
                } else {
                    Point::new(x, y)
                }
            };
            let size = orient(100.0, 40.0);
            let geoms = [
                NodeGeom {
                    size,
                    mid_x: size.x / 2.0,
                },
                NodeGeom {
                    size,
                    mid_x: size.x / 2.0,
                },
            ];
            let edges = [RouteEdge {
                src_idx: 0,
                dst_idx: 1,
                has_label: true,
                label_size: Some(Point::new(10.0, 10.0)),
            }];
            let routed = route(
                RoutingInput {
                    geoms: &geoms,
                    top_lefts: &[orient(0.0, 0.0), orient(20.0, 100.0)],
                    edges: &edges,
                    constraints: &[AnchorConstraints::default(); 2],
                    is_lr,
                    foreign_obstacles: &|_, _| vec![],
                },
                RoutingOptions {
                    mode: RoutingMode::Spline,
                    spacing: Spacing::default(),
                    exterior_self_loops: true,
                    labels: LabelPlacement::AvoidOverlaps,
                },
            );
            let edge = &routed[0];
            assert_eq!(edge.from_override, Some(60.0));
            let label = edge.label_pos.unwrap();
            let expected = orient(60.0, 70.0);
            assert!((label.x - expected.x).abs() < 1e-6, "{label:?}");
            assert!((label.y - expected.y).abs() < 1e-6, "{label:?}");
        }
    }
}
