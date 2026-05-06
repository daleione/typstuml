//! Cleanup pass after Brandes-Kopf: straighten dummy-connector chains
//! where it doesn't bump them into a real box, and offset crossing edges
//! when straightening would put them through one.

use super::EPSILON;
use crate::layout::dag::NodeHandle;
use crate::layout::geometry::{in_range, segment_rect_intersection, Point};
use crate::layout::graph::VisualGraph;

use super::simple::align_to_left;

/// X range a node may move within without colliding with its rank
/// neighbours.
fn movement_range(vg: &VisualGraph, node: NodeHandle) -> (f64, f64) {
    let level = vg.dag.level(node);
    let row = vg.dag.row(level);
    let idx = row.iter().position(|x| *x == node).unwrap();
    let left = if idx > 0 {
        vg.pos(row[idx - 1]).right(true)
    } else {
        f64::NEG_INFINITY
    };
    let right = if idx + 1 < row.len() {
        vg.pos(row[idx + 1]).left(true)
    } else {
        f64::INFINITY
    };
    (left, right)
}

/// Move connector dummy nodes to the midpoint of their pred/succ when
/// doing so doesn't punch through another box on the same rank.
fn straighten_edges(vg: &mut VisualGraph) {
    let mut to_move: Vec<NodeHandle> = Vec::new();

    for row_idx in 1..vg.dag.num_levels().saturating_sub(1) {
        let row = vg.dag.row(row_idx);
        'out: for elem in row.iter() {
            if !vg.is_connector(*elem) {
                continue;
            }
            let pred = match vg.dag.single_pred(*elem) {
                Some(p) => p,
                None => continue,
            };
            let succ = match vg.dag.single_succ(*elem) {
                Some(s) => s,
                None => continue,
            };
            if vg.is_connector(pred) || vg.is_connector(succ) {
                continue;
            }

            let seg = (vg.pos(pred).center(), vg.pos(succ).center());
            for blocker in row {
                if segment_rect_intersection(seg, vg.pos(*blocker).bbox(false)) {
                    continue 'out;
                }
            }
            to_move.push(*elem);
        }
    }

    for elem in to_move {
        let p1 = vg.pos(vg.dag.single_pred(elem).unwrap()).center();
        let p2 = vg.pos(vg.dag.single_succ(elem).unwrap()).center();
        let target_x = (p1.x + p2.x) / 2.;
        if in_range(movement_range(vg, elem), target_x) {
            vg.pos_mut(elem).set_x(target_x);
        }
    }
}

/// Pull disconnected nodes (no preds, no succs) up against whichever
/// neighbour they have, so they don't float in the middle.
fn handle_disconnected(vg: &mut VisualGraph) {
    for row_idx in 0..vg.dag.num_levels() {
        let row = vg.dag.row(row_idx).clone();
        for elem in &row {
            if !vg.dag.successors(*elem).is_empty()
                || !vg.dag.predecessors(*elem).is_empty()
            {
                continue;
            }
            let (left, right) = movement_range(vg, *elem);
            if left.is_finite() {
                vg.pos_mut(*elem).align_to_left(left + EPSILON);
            } else if right.is_finite() {
                vg.pos_mut(*elem).align_to_right(right - EPSILON);
            }
        }
    }
}

/// Self-edge connectors get tucked next to the source so the loop arc is
/// short and visible.
fn align_self_edges(vg: &mut VisualGraph) {
    for row_idx in 0..vg.dag.num_levels() {
        let row = vg.dag.row(row_idx).clone();
        for (i, curr) in row.iter().enumerate() {
            if !vg.is_connector(*curr) {
                continue;
            }
            let preds = vg.dag.predecessors(*curr);
            let mut before = false;
            let mut after = false;
            for pred in preds {
                if let Some(idx) = row.iter().position(|x| x == pred) {
                    before |= idx < i;
                    after |= idx > i;
                }
            }
            if before {
                let prev_right = vg.pos(row[i - 1]).right(true);
                vg.pos_mut(*curr).align_to_left(prev_right);
            } else if after {
                let next_left = vg.pos(row[i + 1]).left(true);
                vg.pos_mut(*curr).align_to_right(next_left);
            }
        }
    }
}

/// Offsets tried when nudging a connector to clear a crossed edge: small
/// pulls first, then larger swings in alternating directions.
const CROSSING_OFFSETS: &[Point] = &[
    Point { x: 0., y: 15. },
    Point { x: 0., y: 25. },
    Point { x: 0., y: 35. },
    Point { x: 0., y: 45. },
    Point { x: 0., y: 55. },
    Point { x: 0., y: 65. },
    Point { x: 0., y: 75. },
    Point { x: 0., y: 85. },
    Point { x: 0., y: 95. },
    Point { x: 0., y: -10. },
    Point { x: 0., y: 20. },
    Point { x: 0., y: -20. },
    Point { x: 0., y: 30. },
    Point { x: 0., y: -30. },
    Point { x: 0., y: 40. },
    Point { x: 0., y: -40. },
    Point { x: 0., y: 50. },
    Point { x: 0., y: -50. },
    Point { x: 0., y: 90. },
    Point { x: 0., y: -90. },
];

fn intersects_any(segs: &[(Point, Point)], rects: &[(Point, Point)]) -> bool {
    segs.iter()
        .any(|s| rects.iter().any(|r| segment_rect_intersection(*s, *r)))
}

fn adjust_crossing_edges(vg: &mut VisualGraph) {
    let len = vg.dag.num_levels();
    let mut to_move: Vec<(NodeHandle, Point)> = Vec::new();

    'out: for row_idx in 0..len {
        let row = vg.dag.row(row_idx);
        let mut neighbours = Vec::new();
        if row_idx > 1 {
            neighbours.extend(vg.dag.row(row_idx - 1).iter().copied());
        }
        if row_idx + 1 < len {
            neighbours.extend(vg.dag.row(row_idx + 1).iter().copied());
        }

        for i in 0..row.len() {
            let curr = row[i];
            if !vg.is_connector(curr) {
                continue;
            }
            let pred = match vg.dag.single_pred(curr) {
                Some(p) => p,
                None => continue,
            };
            let succ = match vg.dag.single_succ(curr) {
                Some(s) => s,
                None => continue,
            };
            let p0 = vg.pos(pred).center();
            let p1 = vg.pos(curr).center();
            let p2 = vg.pos(succ).center();
            let seg0 = (p0, p1);
            let seg1 = (p1, p2);

            let mut bounds = Vec::new();
            let mut all_bounds = Vec::new();
            if i > 0 {
                let bb = vg.pos(row[i - 1]).bbox(false);
                bounds.push(bb);
                all_bounds.push(bb);
            }
            if i + 1 < row.len() {
                let bb = vg.pos(row[i + 1]).bbox(false);
                bounds.push(bb);
                all_bounds.push(bb);
            }
            for n in &neighbours {
                if *n != pred && *n != succ {
                    all_bounds.push(vg.pos(*n).bbox(false));
                }
            }

            if !intersects_any(&[seg0, seg1], &bounds) {
                continue;
            }
            for offset in CROSSING_OFFSETS {
                let s0 = (seg0.0, seg0.1.add(*offset));
                let s1 = (seg1.0.add(*offset), seg1.1);
                if !intersects_any(&[s0, s1], &all_bounds) {
                    to_move.push((curr, *offset));
                    continue 'out;
                }
            }
        }
    }

    for (node, offset) in to_move {
        vg.pos_mut(node).translate(offset);
    }
}

pub(crate) fn do_it(vg: &mut VisualGraph) {
    handle_disconnected(vg);
    align_self_edges(vg);
    align_to_left(vg);
    straighten_edges(vg);
    adjust_crossing_edges(vg);
}
