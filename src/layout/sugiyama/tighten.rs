//! Post-BK cluster bbox derivation.
//!
//! After Brandes-Köpf finishes positioning real nodes, walk every
//! cluster bottom-up: inner bbox = union of direct entity bboxes + child
//! cluster outer bboxes; outer bbox = inner + pad on each side + label
//! band on top. Writes the result back into `HierarchyMap` so codegen
//! can read first-class cluster bboxes instead of reverse-deriving them
//! from inner content.
//!
//! Connector dummies are excluded — they inherit a cluster ID for
//! mincross / cluster_bubble purposes but have zero footprint, so
//! pulling them into the bbox union would distort the cluster.

use crate::layout::dag::NodeHandle;
use crate::layout::geometry::Point;
use crate::layout::graph::VisualGraph;
use crate::layout::sugiyama::hierarchy::ClusterId;

/// Minimum gap enforced between two sibling clusters' outer bboxes
/// along the row axis. Matches the painter's visual rhythm.
const CLUSTER_GAP_PT: f64 = 12.0;

/// Run tighten on every cluster. No-op when the hierarchy is empty.
///
/// Process clusters depth-first from leaves up. At each depth:
///   1. Compute bboxes for every cluster at that depth (from current
///      entity positions and already-resolved child cluster bboxes).
///   2. Resolve sibling overlap among the depth's clusters, grouped
///      by their parent.
/// This ordering means parents always see their children's *final*
/// bboxes, so the parent's bbox tightly wraps the post-shift subtree
/// — no stale ancestor bboxes, no second pass needed.
pub(crate) fn do_it(vg: &mut VisualGraph) {
    if vg.hierarchy.is_empty() {
        return;
    }
    let depths = compute_depths(&vg.hierarchy);
    let max_depth = depths.iter().copied().max().unwrap_or(0);
    for d in (0..=max_depth).rev() {
        let level: Vec<ClusterId> = depths
            .iter()
            .enumerate()
            .filter(|(_, &dd)| dd == d)
            .map(|(i, _)| i)
            .collect();
        if level.is_empty() {
            continue;
        }
        for &c in &level {
            compute_single_bbox(vg, c);
        }
        resolve_level(vg, &level);
        // Each cluster at depth `d` may overlap *direct sibling nodes*
        // in its parent (at depth d-1) — Sugiyama places those nodes
        // on an outer rank above/below the cluster's inner content,
        // but the cluster's label band extends past its content into
        // the outer rank's territory, so a parent-direct node at the
        // adjacent outer rank can clip the cluster's label band.
        // Resolve before the parent's own bbox is computed next
        // iteration up.
        resolve_against_sibling_nodes(vg, &level);
    }

    // Pushing a cluster's interior down to clear stacked label bands
    // (above) moves only that cluster's subtree — a later-rank successor
    // living *outside* the cluster stays where compaction left it and can
    // end up visually above the cluster it should follow (the nested
    // composite back-edge bug). Re-assert the rank monotonicity that
    // compaction guaranteed and the band shifts broke.
    restore_rank_monotonicity(vg);
}

/// After cluster frames are derived, walk nodes in rank order and, for
/// any DAG edge `u → v` whose successor `v` now sits at or above `u`'s
/// halo-bbox bottom, push `v` back down to just below `u` — moving the
/// largest cluster subtree that carries `v` without dragging `u`. This
/// restores the exact `top(v) >= bottom(u)` invariant `compact::do_it`
/// enforces (graph internal halo already encodes the rank gap), so it is
/// a strict no-op for any layout the band shifts left monotonic.
fn restore_rank_monotonicity(vg: &mut VisualGraph) {
    const EPS: f64 = 1e-6;
    let num_ranks = vg.dag.num_levels();
    for r in 0..num_ranks {
        let row = vg.dag.row(r).clone();
        for v in row {
            let mut required_top = f64::NEG_INFINITY;
            let mut blocker = None;
            for &u in vg.dag.predecessors(v) {
                // When the edge crosses into one or more cluster frames
                // (an ancestor cluster of `v` that doesn't contain `u`),
                // reserve each frame's label band + pad above `v` so the
                // frame top clears `u` instead of overlapping it. Handles
                // both top-level composites entered from an outside node
                // and inner composites entered from an outer sibling.
                let (ux0, ux1) = {
                    let b = vg.pos(u).bbox(false);
                    (b.0.x, b.1.x)
                };
                // When the edge crosses into a labelled cluster frame whose
                // title strip sits in `u`'s column, the frame top must clear
                // `u`. Reserve only the distance the frame top currently sits
                // above `v`'s node top, so a frame that already clears `u`
                // (single-level composites) gets no extra push — only an
                // actual band/predecessor overlap moves it.
                let mut frame_top = f64::INFINITY;
                if let Some(vc) = vg.hierarchy.cluster_of(v) {
                    for c in vg.hierarchy.ancestors(vc) {
                        let cl = &vg.hierarchy.clusters[c];
                        if cl.label_band <= 0.0 || vg.hierarchy.is_inside(u, c) {
                            continue;
                        }
                        let x_overlap = cl.x_max.min(ux1) - cl.x_min.max(ux0);
                        if x_overlap > 0.0 {
                            frame_top = frame_top.min(cl.y_min);
                        }
                    }
                }
                let reserve = if frame_top.is_finite() {
                    (vg.pos(v).bbox(true).0.y - frame_top).max(0.0)
                } else {
                    0.0
                };
                let ubot = vg.pos(u).bbox(true).1.y + reserve;
                if ubot > required_top {
                    required_top = ubot;
                    blocker = Some(u);
                }
            }
            let Some(u) = blocker else { continue };
            let vtop = vg.pos(v).bbox(true).0.y;
            let delta = required_top - vtop;
            if delta <= EPS {
                continue;
            }
            shift_node_below(vg, v, u, delta);
        }
    }

    // Member positions moved; rebuild every cluster bbox bottom-up so the
    // frames wrap the now-monotonic subtree.
    let depths = compute_depths(&vg.hierarchy);
    let max_depth = depths.iter().copied().max().unwrap_or(0);
    for d in (0..=max_depth).rev() {
        for c in 0..vg.hierarchy.clusters.len() {
            if depths[c] == d {
                compute_single_bbox(vg, c);
            }
        }
    }
}

/// Shift `v` down by `dy`, moving the outermost cluster subtree that
/// contains `v` but *not* `u`. When `v` shares its whole cluster chain
/// with `u` (true siblings) or is unclustered, `v` moves alone.
fn shift_node_below(vg: &mut VisualGraph, v: NodeHandle, u: NodeHandle, dy: f64) {
    let chain: Vec<ClusterId> = match vg.hierarchy.cluster_of(v) {
        Some(c) => {
            let mut a: Vec<_> = vg.hierarchy.ancestors(c).collect();
            a.reverse(); // outermost first
            a
        }
        None => Vec::new(),
    };
    for c in chain {
        if !vg.hierarchy.is_inside(u, c) {
            shift_cluster_subtree(vg, c, 0.0, dy);
            return;
        }
    }
    vg.pos_mut(v).translate(Point::new(0.0, dy));
}

/// Depth of each cluster: 0 for top-level (no parent), 1 for direct
/// children of top-level, etc.
fn compute_depths(h: &crate::layout::sugiyama::hierarchy::HierarchyMap) -> Vec<usize> {
    let n = h.clusters.len();
    let mut depths = vec![0; n];
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..n {
            if let Some(p) = h.clusters[i].parent {
                let d = depths[p] + 1;
                if depths[i] != d {
                    depths[i] = d;
                    changed = true;
                }
            }
        }
    }
    depths
}

/// Compute one cluster's bbox from its direct entities + direct child
/// cluster bboxes. Leaves it at the f64 sentinel infinities if empty.
fn compute_single_bbox(vg: &mut VisualGraph, c: ClusterId) {
    let nodes = vg.hierarchy.clusters[c].direct_nodes.clone();
    let children = vg.hierarchy.clusters[c].direct_children.clone();
    let pad = vg.hierarchy.clusters[c].pad;
    let band = vg.hierarchy.clusters[c].label_band;
    let min_w = vg.hierarchy.clusters[c].label_min_w;

    let mut x_min = f64::INFINITY;
    let mut x_max = f64::NEG_INFINITY;
    let mut y_min = f64::INFINITY;
    let mut y_max = f64::NEG_INFINITY;
    for h in nodes {
        if vg.is_connector(h) {
            continue;
        }
        let (tl, br) = vg.pos(h).bbox(false);
        x_min = x_min.min(tl.x);
        y_min = y_min.min(tl.y);
        x_max = x_max.max(br.x);
        y_max = y_max.max(br.y);
    }
    for ch in children {
        let child = &vg.hierarchy.clusters[ch];
        if child.x_min.is_finite() {
            x_min = x_min.min(child.x_min);
            y_min = y_min.min(child.y_min);
            x_max = x_max.max(child.x_max);
            y_max = y_max.max(child.y_max);
        }
    }
    let cluster = &mut vg.hierarchy.clusters[c];
    if x_min.is_finite() {
        let outer_w = (x_max - x_min + 2.0 * pad).max(min_w);
        cluster.x_min = x_min - pad;
        cluster.x_max = cluster.x_min + outer_w;
        cluster.y_min = y_min - pad - band;
        cluster.y_max = y_max + pad;
    } else {
        // Empty cluster: reset sentinel infinities.
        cluster.x_min = f64::INFINITY;
        cluster.x_max = f64::NEG_INFINITY;
        cluster.y_min = f64::INFINITY;
        cluster.y_max = f64::NEG_INFINITY;
    }
}

/// Resolve sibling overlap among the clusters at one depth, grouped by
/// shared parent (top-level clusters all share `None`).
fn resolve_level(vg: &mut VisualGraph, level: &[ClusterId]) {
    use std::collections::HashMap;
    let mut by_parent: HashMap<Option<ClusterId>, Vec<ClusterId>> = HashMap::new();
    for &c in level {
        let p = vg.hierarchy.clusters[c].parent;
        by_parent.entry(p).or_default().push(c);
    }
    for (_, siblings) in by_parent {
        if siblings.len() < 2 {
            continue;
        }
        sweep_siblings(vg, &siblings);
    }
}

/// Sibling separation: iterate pairwise until stable, picking the
/// axis per pair from the relative overlap. Direction is decided
/// against the *original* bbox positions snapshot below so the
/// cascade can't put a later cluster ahead of an earlier one in row
/// order (the bug that gave PkgA·PkgB·PkgC → ACB).
fn sweep_siblings(vg: &mut VisualGraph, siblings: &[ClusterId]) {
    let valid: Vec<ClusterId> = siblings
        .iter()
        .copied()
        .filter(|&c| vg.hierarchy.clusters[c].x_min.is_finite())
        .collect();
    if valid.len() < 2 {
        return;
    }

    // Snapshot once: who's "left" and who's "top" by initial position.
    // The cascade may move clusters past one another in current coords,
    // but the intent (BK / barycenter order) is fixed by these
    // snapshots.
    let mut orig_x: std::collections::HashMap<ClusterId, f64> =
        std::collections::HashMap::with_capacity(valid.len());
    let mut orig_y: std::collections::HashMap<ClusterId, f64> =
        std::collections::HashMap::with_capacity(valid.len());
    for &c in &valid {
        let cluster = &vg.hierarchy.clusters[c];
        orig_x.insert(c, cluster.x_min);
        orig_y.insert(c, cluster.y_min);
    }

    const MAX_ITER: usize = 16;
    for _ in 0..MAX_ITER {
        let mut any_shift = false;
        for i in 0..valid.len() {
            for j in (i + 1)..valid.len() {
                let a = valid[i];
                let b = valid[j];
                let aa = &vg.hierarchy.clusters[a];
                let bb = &vg.hierarchy.clusters[b];
                let x_overlap = aa.x_max.min(bb.x_max) - aa.x_min.max(bb.x_min);
                let y_overlap = aa.y_max.min(bb.y_max) - aa.y_min.max(bb.y_min);
                if x_overlap <= 0.0 || y_overlap <= 0.0 {
                    continue;
                }
                // Axis heuristic: when two clusters share a y-band
                // (overlap covers most of the smaller cluster's y
                // span), they sit at the same Sugiyama rank and the
                // natural separation is along x. Otherwise they're at
                // different ranks and should y-separate.
                let y_span_min = (aa.y_max - aa.y_min).min(bb.y_max - bb.y_min);
                let same_y_band = if y_span_min > 0.0 {
                    y_overlap / y_span_min > 0.5
                } else {
                    y_overlap > 0.0
                };
                let (left, right) = if orig_x[&a] <= orig_x[&b] {
                    (a, b)
                } else {
                    (b, a)
                };
                let (top, bot) = if orig_y[&a] <= orig_y[&b] {
                    (a, b)
                } else {
                    (b, a)
                };
                if same_y_band {
                    let dx = vg.hierarchy.clusters[left].x_max + CLUSTER_GAP_PT
                        - vg.hierarchy.clusters[right].x_min;
                    if dx > 0.0 {
                        shift_cluster_subtree(vg, right, dx, 0.0);
                        any_shift = true;
                    }
                } else {
                    let dy = vg.hierarchy.clusters[top].y_max + CLUSTER_GAP_PT
                        - vg.hierarchy.clusters[bot].y_min;
                    if dy > 0.0 {
                        shift_cluster_subtree(vg, bot, 0.0, dy);
                        any_shift = true;
                    }
                }
            }
        }
        if !any_shift {
            break;
        }
    }
}


/// Resolve overlap between each cluster at this depth and its parent's
/// direct sibling nodes. Each cluster's outer bbox already includes
/// its `label_band` (the strip above the inner content where the
/// painter draws the package title), so a parent-direct node sitting
/// at the adjacent outer rank can clip into that band. Push the
/// cluster (and its subtree) along the dominant overlap axis to clear
/// the node by `CLUSTER_GAP_PT`.
fn resolve_against_sibling_nodes(vg: &mut VisualGraph, level: &[ClusterId]) {
    for &c in level {
        let parent = match vg.hierarchy.clusters[c].parent {
            Some(p) => p,
            None => continue,
        };
        if !vg.hierarchy.clusters[c].x_min.is_finite() {
            continue;
        }
        // Only sibling nodes *above* the cluster can clip its label band
        // (the band sits at the cluster's top). A node at the cluster's
        // rank or below is a same-rank neighbour (x-separated) or a
        // later-rank successor that must flow *below* the cluster — never
        // a reason to push the cluster down. Gate by rank so a successor
        // can't shove its own ancestor cluster downward.
        let cluster_min_rank = cluster_min_level(vg, c);
        let sibling_nodes = vg.hierarchy.clusters[parent].direct_nodes.clone();
        let node_bboxes: Vec<(Point, Point, bool)> = sibling_nodes
            .iter()
            .filter(|h| !vg.is_connector(**h))
            .map(|h| {
                let (tl, br) = vg.pos(*h).bbox(false);
                let above = vg.dag.level(*h) < cluster_min_rank;
                (tl, br, above)
            })
            .collect();
        if node_bboxes.is_empty() {
            continue;
        }
        let cb = &vg.hierarchy.clusters[c];
        let (cx_min, cx_max, cy_min, cy_max) = (cb.x_min, cb.x_max, cb.y_min, cb.y_max);
        let mut shift_y = 0.0f64;
        let mut shift_x = 0.0f64;
        for (tl, br, above) in &node_bboxes {
            let x_overlap = cx_max.min(br.x) - cx_min.max(tl.x);
            let y_overlap = cy_max.min(br.y) - cy_min.max(tl.y);
            if x_overlap <= 0.0 || y_overlap <= 0.0 {
                continue;
            }
            // Prefer pushing along the axis with the smaller overlap so
            // we don't move a cluster across an unrelated sibling. For
            // the common nested-package case (TopLevel above Parent),
            // y is the smaller and we push the inner cluster down.
            if y_overlap <= x_overlap {
                // Only a node above the cluster justifies pushing it down.
                if !above {
                    continue;
                }
                let needed = (br.y - cy_min) + CLUSTER_GAP_PT;
                if needed > shift_y {
                    shift_y = needed;
                }
            } else {
                let needed = (br.x - cx_min) + CLUSTER_GAP_PT;
                if needed > shift_x {
                    shift_x = needed;
                }
            }
        }
        if shift_x > 0.0 || shift_y > 0.0 {
            shift_cluster_subtree(vg, c, shift_x, shift_y);
        }
    }
}

/// Minimum DAG rank among the cluster's members (direct + nested).
/// `usize::MAX` when the cluster owns no real node yet.
fn cluster_min_level(vg: &VisualGraph, c: ClusterId) -> usize {
    let mut min = usize::MAX;
    for h in vg.iter_nodes() {
        if vg.is_connector(h) {
            continue;
        }
        if vg.hierarchy.is_inside(h, c) {
            min = min.min(vg.dag.level(h));
        }
    }
    min
}

/// Translate every real entity in `c`'s subtree by `(dx, dy)` and apply
/// the same delta to every descendant cluster's bbox.
fn shift_cluster_subtree(vg: &mut VisualGraph, c: ClusterId, dx: f64, dy: f64) {
    let nodes: Vec<_> = vg.hierarchy.clusters[c].direct_nodes.clone();
    for h in nodes {
        vg.pos_mut(h).translate(Point::new(dx, dy));
    }
    let cluster = &mut vg.hierarchy.clusters[c];
    cluster.x_min += dx;
    cluster.x_max += dx;
    cluster.y_min += dy;
    cluster.y_max += dy;
    let children: Vec<_> = cluster.direct_children.clone();
    for ch in children {
        shift_cluster_subtree(vg, ch, dx, dy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::sugiyama::hierarchy::HierarchyMap;

    #[test]
    fn compute_depths_handles_nested_forest() {
        // root0 → mid0 → leaf0,  root1 → leaf1
        let mut h = HierarchyMap::new();
        let r0 = h.add_cluster(None);
        let m0 = h.add_cluster(Some(r0));
        let _l0 = h.add_cluster(Some(m0));
        let r1 = h.add_cluster(None);
        let _l1 = h.add_cluster(Some(r1));
        let depths = compute_depths(&h);
        assert_eq!(depths, vec![0, 1, 2, 0, 1]);
    }
}
