//! Per-node rank-axis compaction. After Brandes-Kopf, each rank's start
//! coordinate is set to the rank's max-bottom — so a wide neighbour pushes
//! the entire next rank away even when narrower nodes wouldn't overlap. We
//! tighten that by sliding each node up the rank-progression axis until it
//! actually touches an earlier-rank node that overlaps it on the
//! perpendicular axis. Same-rank ordering and inter-rank monotonicity are
//! preserved.
//!
//! Operates in TB-internal coords (y is the rank axis), so a single
//! implementation serves both LR and TB diagrams via the placer's
//! transpose wrapper.
//!
//! After per-node sliding, a sibling-alignment pass re-consolidates
//! children of the same parent onto the rightmost compacted position
//! among them — but only when doing so doesn't introduce a new bbox
//! overlap with any other node. This addresses the visual issue where
//! `compact::do_it` slides one sibling free of a previous-rank
//! perp-blocker while another sibling is pinned behind it, leaving
//! same-parent leaves in different rank-axis columns. The safety check
//! ensures the alignment only applies when it's a strict cosmetic
//! improvement.

use crate::layout::dag::NodeHandle;
use crate::layout::geometry::{do_boxes_intersect, Point};
use crate::layout::graph::VisualGraph;

/// Two intervals overlap if neither sits strictly past the other.
fn intervals_overlap(a: (f64, f64), b: (f64, f64)) -> bool {
    a.1 > b.0 && b.1 > a.0
}

pub(crate) fn do_it(vg: &mut VisualGraph) {
    let num_ranks = vg.dag.num_levels();
    for r in 0..num_ranks {
        let row = vg.dag.row(r).clone();
        for &n in &row {
            let n_bbox = vg.pos(n).bbox(true);
            let n_x = (n_bbox.0.x, n_bbox.1.x);

            // Floor: just below the bottom of any earlier-rank node that
            // overlaps `n` on the perpendicular axis. Falls back to 0 when
            // nothing overlaps.
            let mut new_top = 0.0_f64;
            for r_prev in 0..r {
                for &m in vg.dag.row(r_prev) {
                    let m_bbox = vg.pos(m).bbox(true);
                    if intervals_overlap(n_x, (m_bbox.0.x, m_bbox.1.x)) {
                        new_top = new_top.max(m_bbox.1.y);
                    }
                }
            }
            // Don't overshoot direct predecessors on the rank axis — keep
            // edges flowing from low y to high y.
            for &p in vg.dag.predecessors(n) {
                new_top = new_top.max(vg.pos(p).bbox(true).1.y);
            }

            let delta = new_top - n_bbox.0.y;
            if delta != 0. {
                vg.pos_mut(n).translate(Point::new(0., delta));
            }
        }
    }

    align_sibling_leaves(vg);
}

/// Walk ranks bottom-up. For each rank, group same-rank nodes by their
/// single predecessor (skip groups containing non-leaves so we don't
/// have to reconcile descendants). For each leaf-only group of >1
/// members, try aligning everyone to the maximum rank-axis top among
/// them; commit only if no member's new bbox would intersect any other
/// node's bbox. This makes mobile/work end up in the same column on
/// fixtures like `docs/t.puml` without ever creating a new overlap.
fn align_sibling_leaves(vg: &mut VisualGraph) {
    use std::collections::HashMap;

    let n_levels = vg.dag.num_levels();
    for r in (0..n_levels).rev() {
        let row = vg.dag.row(r).clone();
        let mut groups: HashMap<NodeHandle, Vec<NodeHandle>> = HashMap::new();
        for &n in &row {
            if let Some(p) = vg.dag.single_pred(n) {
                groups.entry(p).or_default().push(n);
            }
        }
        for members in groups.values() {
            if members.len() < 2 {
                continue;
            }
            // Skip if any member has descendants — moving them rightward
            // without re-running compact for descendants risks creating
            // overlaps further down the tree.
            if members
                .iter()
                .any(|m| !vg.dag.successors(*m).is_empty())
            {
                continue;
            }
            let max_top = members
                .iter()
                .map(|m| vg.pos(*m).bbox(true).0.y)
                .fold(f64::NEG_INFINITY, f64::max);
            if !alignment_is_safe(vg, members, max_top) {
                continue;
            }
            for &m in members {
                vg.pos_mut(m).align_to_top(max_top);
            }
        }
    }
}

/// Would moving every node in `members` to rank-axis top `target_top`
/// land any of them on top of a non-group node? Same-group siblings
/// share the rank-axis after alignment but have distinct perpendicular
/// positions, so they never collide with each other.
fn alignment_is_safe(
    vg: &VisualGraph,
    members: &[NodeHandle],
    target_top: f64,
) -> bool {
    for &m in members {
        let bbox = vg.pos(m).bbox(true);
        let size_y = bbox.1.y - bbox.0.y;
        let new_bbox = (
            Point::new(bbox.0.x, target_top),
            Point::new(bbox.1.x, target_top + size_y),
        );
        if (new_bbox.1.y - bbox.1.y).abs() < 1e-9 {
            // Already at target_top — its current bbox already coexists
            // with every other node's, so no need to test it.
            continue;
        }
        for n in vg.iter_nodes() {
            if members.contains(&n) {
                continue;
            }
            if do_boxes_intersect(new_bbox, vg.pos(n).bbox(true)) {
                return false;
            }
        }
    }
    true
}
